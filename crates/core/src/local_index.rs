//! Per-machine SQLite index: `MediaId → local path` and other metadata that
//! must never enter the shared document.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::ids::MediaId;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
}

pub struct LocalIndex {
    conn: Connection,
}

impl std::fmt::Debug for LocalIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalIndex")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMedia {
    pub id: MediaId,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

impl LocalIndex {
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, IndexError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, IndexError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS media_paths (
                media_id TEXT PRIMARY KEY,
                path     TEXT NOT NULL,
                size     INTEGER NOT NULL,
                mtime    INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
        )?;
        Ok(LocalIndex { conn })
    }

    /// Records where `id` lives on this machine. Callers must have verified
    /// that the file's BLAKE3 hash equals `id`.
    pub fn set_path(&self, m: &LocalMedia) -> Result<(), IndexError> {
        self.conn.execute(
            "INSERT INTO media_paths (media_id, path, size, mtime) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(media_id) DO UPDATE SET path = ?2, size = ?3, mtime = ?4",
            params![
                m.id.to_hex(),
                m.path.to_string_lossy(),
                m.size as i64,
                m.mtime
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: MediaId) -> Result<Option<LocalMedia>, IndexError> {
        Ok(self
            .conn
            .query_row(
                "SELECT path, size, mtime FROM media_paths WHERE media_id = ?1",
                params![id.to_hex()],
                |r| {
                    Ok(LocalMedia {
                        id,
                        path: PathBuf::from(r.get::<_, String>(0)?),
                        size: r.get::<_, i64>(1)? as u64,
                        mtime: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn all(&self) -> Result<Vec<LocalMedia>, IndexError> {
        let mut st = self
            .conn
            .prepare("SELECT media_id, path, size, mtime FROM media_paths")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, path, size, mtime) = row?;
            if let Ok(id) = id.parse() {
                out.push(LocalMedia {
                    id,
                    path: path.into(),
                    size: size as u64,
                    mtime,
                });
            }
        }
        Ok(out)
    }

    pub fn kv_get(&self, k: &str) -> Result<Option<String>, IndexError> {
        Ok(self
            .conn
            .query_row("SELECT v FROM kv WHERE k = ?1", params![k], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, k: &str, v: &str) -> Result<(), IndexError> {
        self.conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = ?2",
            params![k, v],
        )?;
        Ok(())
    }
}
