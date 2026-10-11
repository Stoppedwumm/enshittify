//! Wire protocol.
//!
//! ```text
//! client → server  Hello { doc_id, peer_id, version_vector, token }
//! server → client  Welcome { updates, server_vv } // each side then sends what the other lacks
//! both directions  Update { seq, bytes }      // Loro update batch (one transaction)
//! server → client  Ack { seq }                // client update `seq` persisted & fanned out
//! both directions  Presence(..)               // ephemeral, ≤ 20 Hz, never stored
//! both directions  Heartbeat                  // every 5 s
//! server → client  Error { code, message }
//! ```

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const HEARTBEAT_SECS: u64 = 5;
pub const PRESENCE_TTL_SECS: u64 = 15;
pub const PRESENCE_MAX_HZ: u32 = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DragGhost {
    pub clip_ids: Vec<String>,
    pub track_id: String,
    pub start_tick: i64,
}

/// Ephemeral collaborator state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Presence {
    pub peer_id: u64,
    pub name: String,
    pub color: [u8; 3],
    pub playhead_tick: i64,
    pub selected_clips: Vec<String>,
    /// Panel identifier, e.g. "timeline", "media".
    pub active_panel: String,
    pub drag: Option<DragGhost>,
    /// Visible timeline range in ticks.
    pub viewport: (i64, i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    VersionMismatch,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    Hello {
        protocol: u32,
        doc_id: String,
        peer_id: u64,
        /// `loro::VersionVector::encode()` of what the client already has.
        version_vector: Vec<u8>,
        token: Option<String>,
    },
    Welcome {
        /// Updates the client is missing.
        updates: Vec<u8>,
        /// The server's version, so the client can push what *it* is missing
        /// (offline edits).
        server_version_vector: Vec<u8>,
    },
    Update {
        seq: u64,
        bytes: Vec<u8>,
    },
    Ack {
        seq: u64,
    },
    Presence(Presence),
    /// A peer left or timed out.
    PresenceGone {
        peer_id: u64,
    },
    Heartbeat,
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("protocol decode error: {0}")]
pub struct DecodeError(#[from] postcard::Error);

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("serialisable")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let msgs = vec![
            Msg::Hello {
                protocol: PROTOCOL_VERSION,
                doc_id: "d".into(),
                peer_id: 7,
                version_vector: vec![1, 2],
                token: None,
            },
            Msg::Update {
                seq: 3,
                bytes: vec![9; 100],
            },
            Msg::Presence(Presence {
                peer_id: 1,
                name: "Ana".into(),
                playhead_tick: 42,
                drag: Some(DragGhost {
                    clip_ids: vec!["c".into()],
                    track_id: "t".into(),
                    start_tick: 5,
                }),
                ..Default::default()
            }),
            Msg::Heartbeat,
        ];
        for m in msgs {
            assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
        }
        assert!(Msg::decode(&[255, 255]).is_err());
    }

    #[test]
    fn presence_is_small() {
        let p = Msg::Presence(Presence {
            name: "Collaborator".into(),
            selected_clips: vec!["01J0000000000000000000000".into(); 3],
            ..Default::default()
        });
        assert!(p.encode().len() < 200);
    }
}
