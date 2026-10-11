//! Non-blocking frame access for the UI and renderer.
//!
//! A [`FrameServer`] owns one worker thread per `(media, quality)` source.
//! The UI calls [`FrameServer::request`] every frame: it returns the exact
//! frame when cached, otherwise the most recently decoded frame of that
//! source (so the preview never blocks) and queues the request. Workers:
//!
//! * decode **forward** when the target is a short distance ahead of the
//!   decoder position (cheaper than a keyframe seek for typical GOPs),
//!   otherwise **seek** (the backend jumps to the preceding keyframe);
//! * keep decoding a look-ahead window past the latest request while idle,
//!   so steady playback is served from cache;
//! * call a wake-up callback so the UI repaints when a frame lands.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use cutroom_core::MediaId;
use parking_lot::{Condvar, Mutex};

use crate::backend::MediaBackend;
use crate::cache::{FrameCache, FrameKey, Quality};
use crate::frame::VideoFrame;
use crate::info::MediaInfo;

/// Max distance (frames) for which decoding forward beats seeking.
const FORWARD_DECODE_LIMIT: i64 = 48;
/// Frames decoded ahead of the latest request.
const LOOKAHEAD: i64 = 12;

type Waker = Arc<dyn Fn() + Send + Sync>;
type LastFrame = Arc<Mutex<Option<VideoFrame>>>;

struct Source {
    path: PathBuf,
    info: MediaInfo,
}

struct Worker {
    tx: Sender<i64>,
    last: LastFrame,
}

struct Shared {
    cache: FrameCache,
    landed: Mutex<u64>,
    cv: Condvar,
    waker: Mutex<Option<Waker>>,
}

pub struct FrameServer {
    backend: Arc<dyn MediaBackend>,
    shared: Arc<Shared>,
    sources: Mutex<HashMap<MediaId, Arc<Source>>>,
    workers: Mutex<HashMap<(MediaId, Quality), Worker>>,
}

impl FrameServer {
    pub fn new(backend: Arc<dyn MediaBackend>, cache_bytes: usize) -> Self {
        FrameServer {
            backend,
            shared: Arc::new(Shared {
                cache: FrameCache::new(cache_bytes),
                landed: Mutex::new(0),
                cv: Condvar::new(),
                waker: Mutex::new(None),
            }),
            sources: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
        }
    }

    pub fn backend(&self) -> &Arc<dyn MediaBackend> {
        &self.backend
    }

    /// Called (from a worker thread) whenever a frame is decoded.
    pub fn set_waker(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.shared.waker.lock() = Some(Arc::new(f));
    }

    /// Makes a media file decodable. Re-registering replaces the path and
    /// restarts its workers.
    pub fn register(&self, media: MediaId, path: PathBuf, info: MediaInfo) {
        self.sources
            .lock()
            .insert(media, Arc::new(Source { path, info }));
        self.workers.lock().retain(|(m, _), _| *m != media);
    }

    pub fn is_registered(&self, media: MediaId) -> bool {
        self.sources.lock().contains_key(&media)
    }

    pub fn info(&self, media: MediaId) -> Option<MediaInfo> {
        self.sources.lock().get(&media).map(|s| s.info.clone())
    }

    fn worker(&self, media: MediaId, q: Quality) -> Option<(Sender<i64>, LastFrame)> {
        let mut workers = self.workers.lock();
        if let Some(w) = workers.get(&(media, q)) {
            return Some((w.tx.clone(), w.last.clone()));
        }
        let src = self.sources.lock().get(&media)?.clone();
        let (tx, rx) = unbounded();
        let last = Arc::new(Mutex::new(None));
        let backend = self.backend.clone();
        let shared = self.shared.clone();
        let last2 = last.clone();
        std::thread::Builder::new()
            .name(format!("decode-{}-{:?}", media.short(), q))
            .spawn(move || run_worker(backend, src, media, q, rx, shared, last2))
            .ok()?;
        workers.insert(
            (media, q),
            Worker {
                tx: tx.clone(),
                last: last.clone(),
            },
        );
        Some((tx, last))
    }

    /// Non-blocking: exact frame if cached, else the most recent frame of
    /// that source (possibly `None` right after opening).
    pub fn request(&self, media: MediaId, index: i64, q: Quality) -> Option<VideoFrame> {
        let index = self.clamp_index(media, index);
        let key = FrameKey {
            media,
            index,
            quality: q,
        };
        if let Some(f) = self.shared.cache.get(&key) {
            // Keep the look-ahead rolling during playback.
            if let Some((tx, _)) = self.worker(media, q) {
                let _ = tx.send(index);
            }
            return Some(f);
        }
        let (tx, last) = self.worker(media, q)?;
        let _ = tx.send(index);
        last.lock().clone()
    }

    /// Blocking variant for export and tests.
    pub fn get_blocking(
        &self,
        media: MediaId,
        index: i64,
        q: Quality,
        timeout: Duration,
    ) -> Option<VideoFrame> {
        let index = self.clamp_index(media, index);
        let key = FrameKey {
            media,
            index,
            quality: q,
        };
        let deadline = Instant::now() + timeout;
        let (tx, _) = self.worker(media, q)?;
        let _ = tx.send(index);
        let mut landed = self.shared.landed.lock();
        loop {
            if let Some(f) = self.shared.cache.get(&key) {
                return Some(f);
            }
            if self.shared.cv.wait_until(&mut landed, deadline).timed_out() {
                return self.shared.cache.get(&key);
            }
        }
    }

    fn clamp_index(&self, media: MediaId, index: i64) -> i64 {
        match self.sources.lock().get(&media) {
            Some(s) if s.info.kind == cutroom_core::model::MediaKind::Image => 0,
            Some(s) => index.clamp(0, (s.info.frames() - 1).max(0)),
            None => index.max(0),
        }
    }

    pub fn cache(&self) -> &FrameCache {
        &self.shared.cache
    }
}

fn run_worker(
    backend: Arc<dyn MediaBackend>,
    src: Arc<Source>,
    media: MediaId,
    q: Quality,
    rx: Receiver<i64>,
    shared: Arc<Shared>,
    last: LastFrame,
) {
    let opts = q.decode_options(src.info.width, src.info.height);
    let mut dec = match backend.open_video(&src.path, &src.info, opts) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(media = %media.short(), "cannot open decoder: {e}");
            return;
        }
    };
    let still = src.info.kind == cutroom_core::model::MediaKind::Image;
    let max_index = if still {
        0
    } else {
        (src.info.frames() - 1).max(0)
    };
    let key = |index| FrameKey {
        media,
        index,
        quality: q,
    };
    // Position the decoder will return next, if known.
    let mut pos: Option<i64> = None;
    let mut target: Option<i64> = None;
    let mut at_eof = false;

    loop {
        // Wait for work; when idle with no target, block.
        let msg = if target.is_none() {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        } else {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return,
            }
        };
        if let Some(mut m) = msg {
            // Coalesce to the newest request.
            while let Ok(n) = rx.try_recv() {
                m = n;
            }
            target = Some(m.clamp(0, max_index));
        }
        let Some(t) = target else { continue };

        // Next frame we need: the target itself, else the first missing
        // frame in the look-ahead window.
        let want = if !shared.cache.contains(&key(t)) {
            Some(t)
        } else {
            (t + 1..=(t + LOOKAHEAD).min(max_index)).find(|i| !shared.cache.contains(&key(*i)))
        };
        let Some(want) = want else {
            target = None; // window full; idle until the next request
            continue;
        };
        if at_eof && pos.is_some_and(|p| want >= p) {
            target = None;
            continue;
        }

        let need_seek = match pos {
            Some(p) => want < p || want - p > FORWARD_DECODE_LIMIT,
            None => true,
        };
        if need_seek {
            if let Err(e) = dec.seek(want) {
                tracing::warn!("seek failed: {e}");
                target = None;
                continue;
            }
            pos = Some(want);
            at_eof = false;
        }
        match dec.decode_next() {
            Ok(Some(f)) => {
                pos = Some(f.index + 1);
                let is_target = f.index == t;
                shared.cache.insert(key(f.index), f.clone());
                if is_target || last.lock().is_none() {
                    *last.lock() = Some(f);
                }
                *shared.landed.lock() += 1;
                shared.cv.notify_all();
                if is_target && let Some(w) = shared.waker.lock().clone() {
                    w();
                }
            }
            Ok(None) => {
                at_eof = true;
                pos = Some(pos.unwrap_or(0));
                // Nothing more to decode for this window.
                if !shared.cache.contains(&key(t)) {
                    target = None;
                    *shared.landed.lock() += 1;
                    shared.cv.notify_all();
                }
            }
            Err(e) => {
                tracing::warn!("decode failed: {e}");
                pos = None;
                target = None;
            }
        }
    }
}
