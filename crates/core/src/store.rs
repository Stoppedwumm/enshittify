//! On-disk project format: a `.cutroom` folder.
//!
//! ```text
//! MyFilm.cutroom/
//!   doc.loro        full Loro snapshot (written on Save)
//!   oplog/          incremental updates since the snapshot (autosave)
//!   cache/          proxies, thumbnails, .peaks — regenerable, never synced
//!   local/          per-machine state (MediaId → path index); never synced
//! ```
//!
//! Crash safety: the app exports `updates(since persisted_vv)` at most every
//! 500 ms and writes it atomically into `oplog/`. Opening imports the snapshot
//! followed by every oplog file; Loro import is idempotent so a file that was
//! also captured by a later snapshot is harmless.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use loro::VersionVector;

use crate::document::Document;

pub const PROJECT_EXT: &str = "cutroom";
const SNAPSHOT: &str = "doc.loro";
const OPLOG: &str = "oplog";
const CACHE: &str = "cache";
const LOCAL: &str = "local";
const UPDATE_EXT: &str = "update";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a cutroom project: {0}")]
    NotAProject(PathBuf),
    #[error("project already exists: {0}")]
    Exists(PathBuf),
    #[error("corrupt document: {0}")]
    Corrupt(String),
}

/// Handle to an open project folder.
#[derive(Debug)]
pub struct ProjectStore {
    root: PathBuf,
    next_oplog: u64,
    persisted: VersionVector,
}

/// Writes `bytes` to `path` atomically (tmp file + fsync + rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        // Persist the rename itself. Directory fsync is unsupported on Windows.
        #[cfg(unix)]
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        #[cfg(not(unix))]
        let _ = dir;
    }
    Ok(())
}

fn oplog_files(dir: &Path) -> std::io::Result<Vec<(u64, PathBuf)>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|e| e.to_str()) != Some(UPDATE_EXT) {
            continue;
        }
        if let Some(n) = p
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
        {
            out.push((n, p));
        }
    }
    out.sort();
    Ok(out)
}

impl ProjectStore {
    /// Normalises a user-chosen path to end in `.cutroom`.
    pub fn normalize_path(p: &Path) -> PathBuf {
        if p.extension().and_then(|e| e.to_str()) == Some(PROJECT_EXT) {
            p.to_path_buf()
        } else {
            let mut s = p.as_os_str().to_owned();
            s.push(".");
            s.push(PROJECT_EXT);
            PathBuf::from(s)
        }
    }

    /// Creates a new project folder and writes the initial snapshot.
    pub fn create(root: &Path, doc: &Document) -> Result<Self, StoreError> {
        let root = Self::normalize_path(root);
        if root.join(SNAPSHOT).exists() {
            return Err(StoreError::Exists(root));
        }
        for d in [OPLOG, CACHE, LOCAL] {
            fs::create_dir_all(root.join(d))?;
        }
        let mut s = ProjectStore {
            root,
            next_oplog: 0,
            persisted: VersionVector::default(),
        };
        s.save(doc)?;
        Ok(s)
    }

    /// Opens a project folder, replaying the oplog on top of the snapshot.
    pub fn open(root: &Path) -> Result<(Self, Document), StoreError> {
        let root = root.to_path_buf();
        let snap_path = root.join(SNAPSHOT);
        if !snap_path.exists() {
            return Err(StoreError::NotAProject(root));
        }
        for d in [OPLOG, CACHE, LOCAL] {
            fs::create_dir_all(root.join(d))?;
        }
        let mut doc = Document::new();
        let bytes = fs::read(&snap_path)?;
        doc.loro()
            .import(&bytes)
            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
        let files = oplog_files(&root.join(OPLOG))?;
        let mut next = 0;
        for (n, p) in &files {
            let b = fs::read(p)?;
            // A torn write can only affect a .tmp file, but be lenient anyway.
            if let Err(e) = doc.loro().import(&b) {
                tracing::warn!(file = %p.display(), "skipping unreadable oplog entry: {e}");
            }
            next = n + 1;
        }
        let persisted = doc.version();
        // Opening must not create an undo step for the loaded state.
        let _ = doc.snapshot();
        Ok((
            ProjectStore {
                root,
                next_oplog: next,
                persisted,
            },
            doc,
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(CACHE)
    }

    pub fn local_dir(&self) -> PathBuf {
        self.root.join(LOCAL)
    }

    /// Display name derived from the folder name.
    pub fn file_stem(&self) -> String {
        self.root
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether `doc` has changes not yet on disk.
    pub fn is_dirty(&self, doc: &Document) -> bool {
        doc.version() != self.persisted
    }

    /// Prepares an incremental oplog write if the document changed. Returns
    /// the target path and bytes; the caller may write it on any thread with
    /// [`write_atomic`].
    pub fn take_incremental(&mut self, doc: &Document) -> Option<(PathBuf, Vec<u8>)> {
        if !self.is_dirty(doc) {
            return None;
        }
        let bytes = doc.export_updates_since(&self.persisted);
        self.persisted = doc.version();
        let path = self
            .root
            .join(OPLOG)
            .join(format!("{:010}.{UPDATE_EXT}", self.next_oplog));
        self.next_oplog += 1;
        Some((path, bytes))
    }

    /// Synchronous incremental flush.
    pub fn flush_incremental(&mut self, doc: &Document) -> Result<bool, StoreError> {
        match self.take_incremental(doc) {
            Some((p, b)) => {
                write_atomic(&p, &b)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Full save: writes a fresh snapshot and compacts the oplog.
    pub fn save(&mut self, doc: &Document) -> Result<(), StoreError> {
        write_atomic(&self.root.join(SNAPSHOT), &doc.export_snapshot())?;
        for (_, p) in oplog_files(&self.root.join(OPLOG))? {
            fs::remove_file(p)?;
        }
        self.persisted = doc.version();
        Ok(())
    }

    /// Save As: copies the current state into a new folder and switches to it.
    pub fn save_as(&mut self, new_root: &Path, doc: &Document) -> Result<(), StoreError> {
        let new = ProjectStore::create(new_root, doc)?;
        *self = new;
        Ok(())
    }
}
