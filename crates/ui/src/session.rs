//! An open project: document, on-disk store, local media index, media
//! library state, background import and crash-safe autosave.
//!
//! Nothing here touches egui, so it is unit-testable.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use cutroom_audio::timeline::PathResolver;
use cutroom_core::command::{AddMedia, InitProject, RenameProject};
use cutroom_core::local_index::{LocalIndex, LocalMedia};
use cutroom_core::model::{DocSnapshot, Meta};
use cutroom_core::store::{ProjectStore, write_atomic};
use cutroom_core::{Command, CommandError, Document, MediaId, SeqId};
use cutroom_media::server::FrameServer;
use cutroom_media::{MediaBackend, MediaInfo};
use loro::VersionVector;
use parking_lot::RwLock;

/// Autosave cadence. Acceptance 9 requires < 2 s of loss after `kill -9`.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub enum ItemStatus {
    /// Hashing/probing in the background.
    Importing,
    Ready,
    /// In the project but not on this machine.
    Offline,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct LibraryItem {
    pub id: MediaId,
    pub path: Option<PathBuf>,
    pub info: Option<MediaInfo>,
    pub status: ItemStatus,
    pub thumb: Option<PathBuf>,
    pub strip: Option<PathBuf>,
    /// Monotonic order of arrival (for "Date added" sorting).
    pub order: u64,
}

#[derive(Debug)]
pub enum ImportEvent {
    Probed {
        path: PathBuf,
        id: MediaId,
        info: MediaInfo,
        /// Re-attaching a file already in the project (on open).
        existing: bool,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
    Thumbs {
        id: MediaId,
        thumb: Option<PathBuf>,
        strip: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    Autosaved,
    Unsaved,
}

pub struct Session {
    pub doc: Document,
    pub store: ProjectStore,
    pub index: LocalIndex,
    pub library: BTreeMap<MediaId, LibraryItem>,
    /// Files being hashed/probed, by path.
    pub pending: Vec<PathBuf>,
    paths: Arc<RwLock<HashMap<MediaId, PathBuf>>>,
    events_tx: Sender<ImportEvent>,
    events_rx: Receiver<ImportEvent>,
    writer: Sender<(PathBuf, Vec<u8>)>,
    last_flush: Instant,
    saved_vv: VersionVector,
    next_order: u64,
}

fn spawn_writer() -> Sender<(PathBuf, Vec<u8>)> {
    let (tx, rx) = unbounded::<(PathBuf, Vec<u8>)>();
    std::thread::Builder::new()
        .name("autosave".into())
        .spawn(move || {
            for (p, b) in rx {
                if let Err(e) = write_atomic(&p, &b) {
                    tracing::error!("autosave to {} failed: {e}", p.display());
                }
            }
        })
        .expect("spawn autosave writer");
    tx
}

impl Session {
    fn from_parts(doc: Document, store: ProjectStore) -> anyhow::Result<Self> {
        let index = LocalIndex::open(&store.local_dir().join("index.sqlite"))?;
        let (events_tx, events_rx) = unbounded();
        let saved_vv = doc.version();
        Ok(Session {
            doc,
            store,
            index,
            library: BTreeMap::new(),
            pending: Vec::new(),
            paths: Arc::default(),
            events_tx,
            events_rx,
            writer: spawn_writer(),
            last_flush: Instant::now(),
            saved_vv,
            next_order: 0,
        })
    }

    /// Creates a new project folder at `path` (".cutroom" is appended).
    pub fn create(path: &Path, meta: Meta) -> anyhow::Result<Self> {
        let mut doc = Document::new();
        let name = ProjectStore::normalize_path(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| meta.name.clone());
        doc.execute(&InitProject::new(Meta { name, ..meta }))?;
        let store = ProjectStore::create(path, &doc)?;
        // The initial structure is not something the user can undo.
        let mut doc2 = ProjectStore::open(store.root())?.1;
        let _ = doc2.snapshot();
        Self::from_parts(doc2, store)
    }

    /// Opens an existing project and re-attaches local media in the background.
    pub fn open(path: &Path, backend: &Arc<dyn MediaBackend>) -> anyhow::Result<Self> {
        let (store, mut doc) = ProjectStore::open(path)?;
        let snap = doc.snapshot();
        let mut s = Self::from_parts(doc, store)?;
        for (id, entry) in &snap.media {
            let local = s.index.get(*id)?;
            let order = s.bump_order();
            s.library.insert(
                *id,
                LibraryItem {
                    id: *id,
                    path: None,
                    info: None,
                    status: ItemStatus::Offline,
                    thumb: None,
                    strip: None,
                    order,
                },
            );
            if let Some(lm) = local
                && lm.path.is_file()
                && std::fs::metadata(&lm.path).map(|m| m.len()).ok() == Some(entry.size_bytes)
            {
                s.library.get_mut(id).expect("inserted").status = ItemStatus::Importing;
                s.spawn_probe(lm.path, Some(*id), backend.clone());
            }
        }
        Ok(s)
    }

    /// A project that also joins a collaboration room (empty until synced).
    pub fn create_empty(path: &Path) -> anyhow::Result<Self> {
        let doc = Document::new();
        let store = ProjectStore::create(path, &doc)?;
        Self::from_parts(doc, store)
    }

    fn bump_order(&mut self) -> u64 {
        self.next_order += 1;
        self.next_order
    }

    pub fn snapshot(&mut self) -> Arc<DocSnapshot> {
        self.doc.snapshot()
    }

    pub fn seq(&mut self) -> Option<SeqId> {
        self.doc.snapshot().main_sequence
    }

    pub fn execute(&mut self, cmd: &dyn Command) -> Result<(), CommandError> {
        self.doc.execute(cmd)
    }

    pub fn name(&mut self) -> String {
        self.doc.snapshot().meta.name.clone()
    }

    pub fn rename(&mut self, name: &str) -> Result<(), CommandError> {
        self.doc.execute(&RenameProject(name.to_string()))
    }

    /// Called every UI frame; flushes an incremental update at most every
    /// [`AUTOSAVE_INTERVAL`] on a background thread.
    pub fn autosave_tick(&mut self) {
        if self.last_flush.elapsed() < AUTOSAVE_INTERVAL {
            return;
        }
        self.last_flush = Instant::now();
        if let Some(job) = self.store.take_incremental(&self.doc) {
            let _ = self.writer.send(job);
        }
    }

    /// Full save: snapshot + oplog compaction (synchronous).
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.store.save(&self.doc)?;
        self.saved_vv = self.doc.version();
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> anyhow::Result<()> {
        let old_local = self.store.local_dir();
        let old_cache = self.store.cache_dir();
        self.store.save_as(path, &self.doc)?;
        // Carry the machine-local index and caches along.
        let _ = std::fs::copy(
            old_local.join("index.sqlite"),
            self.store.local_dir().join("index.sqlite"),
        );
        if let Ok(rd) = std::fs::read_dir(old_cache.join("thumbs")) {
            let dst = self.store.cache_dir().join("thumbs");
            let _ = std::fs::create_dir_all(&dst);
            for e in rd.flatten() {
                let _ = std::fs::copy(e.path(), dst.join(e.file_name()));
            }
        }
        self.index = LocalIndex::open(&self.store.local_dir().join("index.sqlite"))?;
        self.saved_vv = self.doc.version();
        Ok(())
    }

    pub fn save_state(&self) -> SaveState {
        if self.doc.version() == self.saved_vv {
            SaveState::Saved
        } else if !self.store.is_dirty(&self.doc) {
            SaveState::Autosaved
        } else {
            SaveState::Unsaved
        }
    }

    /// Resolver for decoder threads (audio mixer, export).
    pub fn path_resolver(&self) -> PathResolver {
        let paths = self.paths.clone();
        Arc::new(move |id| paths.read().get(&id).cloned())
    }

    pub fn local_path(&self, id: MediaId) -> Option<PathBuf> {
        self.paths.read().get(&id).cloned()
    }

    /// Starts importing files (hash + probe in the background).
    pub fn import(&mut self, files: Vec<PathBuf>, backend: &Arc<dyn MediaBackend>) {
        for f in files {
            if self.pending.contains(&f) {
                continue;
            }
            self.pending.push(f.clone());
            self.spawn_probe(f, None, backend.clone());
        }
    }

    fn spawn_probe(
        &self,
        path: PathBuf,
        expected: Option<MediaId>,
        backend: Arc<dyn MediaBackend>,
    ) {
        let tx = self.events_tx.clone();
        rayon::spawn(move || {
            let res = (|| -> anyhow::Result<(MediaId, MediaInfo)> {
                let id = cutroom_media::hash::hash_file(&path)?;
                if let Some(e) = expected
                    && e != id
                {
                    // Never accept a file whose content differs.
                    anyhow::bail!("file content changed (hash mismatch)");
                }
                Ok((id, backend.probe(&path)?))
            })();
            let _ = tx.send(match res {
                Ok((id, info)) => ImportEvent::Probed {
                    path,
                    id,
                    info,
                    existing: expected.is_some(),
                },
                Err(e) => ImportEvent::Failed {
                    path,
                    error: e.to_string(),
                },
            });
        });
    }

    fn spawn_thumbs(
        &self,
        id: MediaId,
        path: PathBuf,
        info: MediaInfo,
        backend: Arc<dyn MediaBackend>,
    ) {
        let tx = self.events_tx.clone();
        let cache = self.store.cache_dir();
        rayon::spawn(move || {
            let (thumb, strip) = match cutroom_media::thumbs::ensure_thumbs(
                backend.as_ref(),
                &cache,
                id,
                &path,
                &info,
            ) {
                Ok((t, s)) => (Some(t), s),
                Err(e) => {
                    tracing::debug!("no thumbnail for {}: {e}", path.display());
                    (None, None)
                }
            };
            let _ = tx.send(ImportEvent::Thumbs { id, thumb, strip });
        });
    }

    /// Relinks an offline item to a local file. Accepted only if the hash
    /// matches; the check runs in the background.
    pub fn relink(&mut self, id: MediaId, file: PathBuf, backend: &Arc<dyn MediaBackend>) {
        if let Some(it) = self.library.get_mut(&id) {
            it.status = ItemStatus::Importing;
        }
        self.spawn_probe(file, Some(id), backend.clone());
    }

    /// Drains background results. Returns the media ids that became ready.
    pub fn poll_imports(
        &mut self,
        backend: &Arc<dyn MediaBackend>,
        frames: &FrameServer,
    ) -> Vec<MediaId> {
        let mut ready = Vec::new();
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                ImportEvent::Probed {
                    path,
                    id,
                    info,
                    existing,
                } => {
                    self.pending.retain(|p| *p != path);
                    let in_doc = self.doc.snapshot().media.contains_key(&id);
                    if !in_doc && !existing {
                        let name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if let Err(e) = self.doc.execute(&AddMedia(info.to_entry(id, &name))) {
                            tracing::warn!("cannot add media: {e}");
                            continue;
                        }
                    }
                    let mtime = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let _ = self.index.set_path(&LocalMedia {
                        id,
                        path: path.clone(),
                        size: info.size_bytes,
                        mtime,
                    });
                    self.paths.write().insert(id, path.clone());
                    frames.register(id, path.clone(), info.clone());
                    let order = match self.library.get(&id) {
                        Some(it) => it.order,
                        None => self.bump_order(),
                    };
                    self.library.insert(
                        id,
                        LibraryItem {
                            id,
                            path: Some(path.clone()),
                            info: Some(info.clone()),
                            status: ItemStatus::Ready,
                            thumb: None,
                            strip: None,
                            order,
                        },
                    );
                    if info.has_video() {
                        self.spawn_thumbs(id, path, info, backend.clone());
                    }
                    ready.push(id);
                }
                ImportEvent::Failed { path, error } => {
                    tracing::warn!("import of {} failed: {error}", path.display());
                    self.pending.retain(|p| *p != path);
                    if let Some(it) = self
                        .library
                        .values_mut()
                        .find(|i| i.status == ItemStatus::Importing && i.path.is_none())
                    {
                        it.status = ItemStatus::Failed(error);
                    }
                }
                ImportEvent::Thumbs { id, thumb, strip } => {
                    if let Some(it) = self.library.get_mut(&id) {
                        it.thumb = thumb;
                        it.strip = strip;
                    }
                }
            }
        }
        // Media that arrived from collaborators shows up offline.
        let snap = self.doc.snapshot();
        for id in snap.media.keys() {
            if !self.library.contains_key(id) {
                let order = self.bump_order();
                self.library.insert(
                    *id,
                    LibraryItem {
                        id: *id,
                        path: None,
                        info: None,
                        status: ItemStatus::Offline,
                        thumb: None,
                        strip: None,
                        order,
                    },
                );
            }
        }
        self.library.retain(|id, _| snap.media.contains_key(id));
        ready
    }

    /// Project version vector (for sync handshakes).
    pub fn version(&self) -> VersionVector {
        self.doc.version()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_autosave_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Session::create(&dir.path().join("Film"), Meta::default()).unwrap();
        assert_eq!(s.name(), "Film");
        assert!(!s.doc.can_undo(), "fresh project has nothing to undo");
        assert_eq!(s.save_state(), SaveState::Saved);
        s.rename("Renamed").unwrap();
        assert_eq!(s.save_state(), SaveState::Unsaved);
        s.last_flush = Instant::now() - AUTOSAVE_INTERVAL;
        s.autosave_tick();
        assert_eq!(s.save_state(), SaveState::Autosaved);
        // Wait for the writer thread, then reopen as after a crash.
        let root = s.store.root().to_path_buf();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let n = std::fs::read_dir(root.join("oplog")).unwrap().count();
            if n > 0 || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        let (_, mut d) = ProjectStore::open(&root).unwrap();
        assert_eq!(d.snapshot().meta.name, "Renamed");
        s.save().unwrap();
        assert_eq!(s.save_state(), SaveState::Saved);
    }

    #[test]
    fn import_registers_media_and_reattaches_on_open() {
        let Ok(cli) = cutroom_media::cli::CliBackend::locate() else {
            return;
        };
        let backend: Arc<dyn MediaBackend> = Arc::new(cli.clone());
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("tone.wav");
        let ok = std::process::Command::new(cli.ffmpeg_path())
            .args(["-nostdin", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1")
            .arg(&media)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let frames = FrameServer::new(backend.clone(), 1 << 20);
        let mut s = Session::create(&dir.path().join("P"), Meta::default()).unwrap();
        s.import(vec![media.clone()], &backend);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = vec![];
        while ready.is_empty() && Instant::now() < deadline {
            ready = s.poll_imports(&backend, &frames);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ready.len(), 1);
        let id = ready[0];
        assert_eq!(s.local_path(id).as_deref(), Some(media.as_path()));
        assert_eq!(s.snapshot().media.len(), 1);
        s.save().unwrap();
        let root = s.store.root().to_path_buf();
        drop(s);

        let mut s2 = Session::open(&root, &backend).unwrap();
        assert_eq!(s2.library[&id].status, ItemStatus::Importing);
        let deadline = Instant::now() + Duration::from_secs(10);
        while s2.library[&id].status != ItemStatus::Ready && Instant::now() < deadline {
            s2.poll_imports(&backend, &frames);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(s2.library[&id].status, ItemStatus::Ready);
        // Re-attaching must not create document changes.
        assert_eq!(s2.save_state(), SaveState::Saved);
    }
}
