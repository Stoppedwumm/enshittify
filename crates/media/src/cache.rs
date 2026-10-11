//! Byte-bounded LRU cache of decoded frames.

use cutroom_core::MediaId;
use lru::LruCache;
use parking_lot::Mutex;

use crate::frame::VideoFrame;

/// Preview resolution levels. `Full` is used when paused and for export;
/// lower levels while scrubbing / on slow machines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Quality {
    #[default]
    Full,
    Half,
    Quarter,
    /// Thumbnail-size frames for hover-scrub and filmstrips.
    Thumb,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Full, Quality::Half, Quality::Quarter];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Full => "Full",
            Quality::Half => "1/2",
            Quality::Quarter => "1/4",
            Quality::Thumb => "Thumb",
        }
    }

    pub fn decode_options(self, w: u32, h: u32) -> crate::DecodeOptions {
        match self {
            Quality::Full => crate::DecodeOptions::FULL,
            Quality::Half => crate::DecodeOptions::bounded(w.div_ceil(2), h.div_ceil(2)),
            Quality::Quarter => crate::DecodeOptions::bounded(w.div_ceil(4), h.div_ceil(4)),
            Quality::Thumb => crate::DecodeOptions::bounded(320, 180),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub media: MediaId,
    pub index: i64,
    pub quality: Quality,
}

pub struct FrameCache {
    inner: Mutex<Inner>,
}

struct Inner {
    lru: LruCache<FrameKey, VideoFrame>,
    bytes: usize,
    budget: usize,
}

impl FrameCache {
    pub fn new(budget_bytes: usize) -> Self {
        FrameCache {
            inner: Mutex::new(Inner {
                lru: LruCache::unbounded(),
                bytes: 0,
                budget: budget_bytes.max(1),
            }),
        }
    }

    pub fn get(&self, k: &FrameKey) -> Option<VideoFrame> {
        self.inner.lock().lru.get(k).cloned()
    }

    pub fn contains(&self, k: &FrameKey) -> bool {
        self.inner.lock().lru.contains(k)
    }

    pub fn insert(&self, k: FrameKey, f: VideoFrame) {
        let mut g = self.inner.lock();
        g.bytes += f.byte_len();
        if let Some(old) = g.lru.put(k, f) {
            g.bytes -= old.byte_len();
        }
        while g.bytes > g.budget && g.lru.len() > 1 {
            if let Some((_, old)) = g.lru.pop_lru() {
                g.bytes -= old.byte_len();
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.inner.lock().bytes
    }

    pub fn len(&self) -> usize {
        self.inner.lock().lru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.lru.clear();
        g.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn frame(i: i64, n: usize) -> VideoFrame {
        VideoFrame {
            width: 1,
            height: 1,
            index: i,
            rgba: Arc::from(vec![0u8; n]),
        }
    }

    #[test]
    fn evicts_by_bytes() {
        let c = FrameCache::new(100);
        let m = MediaId::of_bytes(b"m");
        let k = |i| FrameKey {
            media: m,
            index: i,
            quality: Quality::Full,
        };
        for i in 0..5 {
            c.insert(k(i), frame(i, 40));
        }
        assert_eq!(c.len(), 2);
        assert!(c.bytes() <= 100);
        assert!(c.contains(&k(4)) && c.contains(&k(3)));
        assert!(!c.contains(&k(0)));
    }
}
