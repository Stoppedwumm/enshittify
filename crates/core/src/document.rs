//! The project document: a Loro CRDT plus a per-user undo manager.
//!
//! The document layout follows `docs/schema.md`. All top-level entries are Loro
//! *root* containers and every nested container is created with
//! `ensure_mergeable_*`, so two peers that concurrently create "the same"
//! child (e.g. a clip's `transform` map) converge on one container instead of
//! one silently overwriting the other.

use std::sync::Arc;

use loro::{Frontiers, LoroDoc, LoroMap, LoroValue, UndoManager, VersionVector};

use crate::command::{Command, CommandError, Tx};
use crate::ids::{PeerId, SeqId};
use crate::model::{self, DocSnapshot, as_map};

pub const ROOT_META: &str = "meta";
pub const ROOT_MEDIA: &str = "media";
pub const ROOT_BINS: &str = "bins";
pub const ROOT_SEQUENCES: &str = "sequences";
pub const ROOT_TITLE_TEXT: &str = "title_text";

/// Origin tag used when importing remote updates (excluded from undo).
pub const REMOTE_ORIGIN: &str = "remote";

pub struct Document {
    doc: LoroDoc,
    undo: UndoManager,
    cached: Option<(Frontiers, Arc<DocSnapshot>)>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("peer", &self.doc.peer_id())
            .finish_non_exhaustive()
    }
}

impl Document {
    /// A blank document with a random peer id. Call
    /// [`crate::command::InitProject`] to populate it.
    pub fn new() -> Self {
        Self::from_loro(LoroDoc::new())
    }

    /// A blank document with a fixed peer id (tests, deterministic sims).
    pub fn with_peer(peer: PeerId) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer).expect("valid peer id");
        Self::from_loro(doc)
    }

    fn from_loro(doc: LoroDoc) -> Self {
        doc.set_record_timestamp(true);
        let mut undo = UndoManager::new(&doc);
        // One transaction = one undo step; never merge.
        undo.set_merge_interval(0);
        undo.set_max_undo_steps(1000);
        Document {
            doc,
            undo,
            cached: None,
        }
    }

    pub fn loro(&self) -> &LoroDoc {
        &self.doc
    }

    pub fn peer_id(&self) -> PeerId {
        self.doc.peer_id()
    }

    /// Executes a command as one Loro transaction (= one undo step = one
    /// network batch).
    ///
    /// Commands validate against the pre-command snapshot before writing. If a
    /// command nevertheless fails after emitting ops, the partial transaction
    /// is committed and immediately reverted so the document is unchanged.
    pub fn execute(&mut self, cmd: &dyn Command) -> Result<(), CommandError> {
        let before = self.doc.state_frontiers();
        let snapshot = self.snapshot();
        let result = {
            let mut tx = Tx::new(&self.doc, &snapshot);
            cmd.apply(&mut tx)
        };
        match result {
            Ok(()) => {
                self.doc.set_next_commit_message(cmd.name());
                self.doc.commit();
                Ok(())
            }
            Err(e) => {
                self.doc.commit();
                if self.doc.state_frontiers() != before {
                    tracing::warn!(cmd = cmd.name(), "command failed after writing; reverting");
                    let _ = self.doc.revert_to(&before);
                    self.doc.commit();
                }
                Err(e)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Undoes the local user's last transaction. Never touches other peers' ops.
    pub fn undo(&mut self) -> bool {
        self.undo.undo().unwrap_or(false)
    }

    pub fn redo(&mut self) -> bool {
        self.undo.redo().unwrap_or(false)
    }

    /// Imports a remote update or snapshot. Idempotent.
    pub fn import(&mut self, bytes: &[u8]) -> Result<(), loro::LoroError> {
        self.doc.import_with(bytes, REMOTE_ORIGIN).map(|_| ())
    }

    pub fn export_snapshot(&self) -> Vec<u8> {
        self.doc
            .export(loro::ExportMode::Snapshot)
            .expect("snapshot export")
    }

    pub fn export_updates_since(&self, vv: &VersionVector) -> Vec<u8> {
        self.doc
            .export(loro::ExportMode::updates(vv))
            .expect("updates export")
    }

    pub fn version(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    pub fn frontiers(&self) -> Frontiers {
        self.doc.state_frontiers()
    }

    /// Returns the materialised snapshot, rebuilding it only when the
    /// document changed.
    pub fn snapshot(&mut self) -> Arc<DocSnapshot> {
        let f = self.doc.state_frontiers();
        if let Some((cf, snap)) = &self.cached
            && *cf == f
        {
            return snap.clone();
        }
        let snap = Arc::new(build_snapshot(&self.doc));
        self.cached = Some((f, snap.clone()));
        snap
    }

    /// Canonical JSON of the document state; equal for converged replicas.
    pub fn state_json(&self) -> String {
        canonical_json(&self.doc.get_deep_value())
    }

    pub fn meta_map(&self) -> LoroMap {
        self.doc.get_map(ROOT_META)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn build_snapshot(doc: &LoroDoc) -> DocSnapshot {
    let root = doc.get_deep_value();
    let LoroValue::Map(root) = root else {
        return DocSnapshot::default();
    };
    let meta = as_map(root.get(ROOT_META))
        .map(model::parse_meta)
        .unwrap_or_default();
    let mut media = std::collections::BTreeMap::new();
    if let Some(mm) = as_map(root.get(ROOT_MEDIA)) {
        for (k, v) in mm.iter() {
            if let Some(e) = as_map(Some(v)).and_then(|m| model::parse_media(k, m)) {
                media.insert(e.id, e);
            }
        }
    }
    let mut sequences = std::collections::BTreeMap::new();
    if let Some(sm) = as_map(root.get(ROOT_SEQUENCES)) {
        for (k, v) in sm.iter() {
            if let Some(s) = as_map(Some(v)).and_then(|m| model::parse_sequence(k, m)) {
                sequences.insert(s.id, s);
            }
        }
    }
    let main_sequence = as_map(root.get(ROOT_META))
        .and_then(|m| model::get_str(m, "main_seq"))
        .and_then(|s| s.parse::<SeqId>().ok())
        .filter(|id| sequences.contains_key(id))
        .or_else(|| sequences.keys().next().copied());
    DocSnapshot {
        meta,
        media,
        sequences,
        main_sequence,
    }
}

/// Deterministic JSON (sorted keys) for comparing replicas.
pub fn canonical_json(v: &LoroValue) -> String {
    fn conv(v: &LoroValue) -> serde_json::Value {
        match v {
            LoroValue::Null => serde_json::Value::Null,
            LoroValue::Bool(b) => (*b).into(),
            LoroValue::Double(d) => serde_json::Number::from_f64(*d)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            LoroValue::I64(i) => (*i).into(),
            LoroValue::Binary(b) => serde_json::Value::String(blake3::hash(b).to_hex().to_string()),
            LoroValue::String(s) => serde_json::Value::String(s.to_string()),
            LoroValue::List(l) => serde_json::Value::Array(l.iter().map(conv).collect()),
            LoroValue::Map(m) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    m.iter().map(|(k, v)| (k.clone(), conv(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            LoroValue::Container(c) => serde_json::Value::String(c.to_string()),
        }
    }
    serde_json::to_string(&conv(v)).expect("json")
}
