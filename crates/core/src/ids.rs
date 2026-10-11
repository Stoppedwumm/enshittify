//! Identifiers.
//!
//! * [`MediaId`] — BLAKE3 hash of a media file's bytes. The shared document
//!   only ever references media by hash; each machine resolves hashes to local
//!   paths on its own.
//! * Timeline entities ([`ClipId`], [`TrackId`], ...) are ULIDs so that peers
//!   can mint ids offline without coordination.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Content address of a media file (BLAKE3, 32 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(pub [u8; 32]);

impl MediaId {
    pub fn from_hash(h: blake3::Hash) -> Self {
        MediaId(*h.as_bytes())
    }

    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self::from_hash(blake3::hash(bytes))
    }

    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    /// First 12 hex chars, for logs and UI.
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }
}

impl fmt::Debug for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaId({})", self.short())
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid id: {0}")]
pub struct IdParseError(pub String);

impl FromStr for MediaId {
    type Err = IdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        blake3::Hash::from_hex(s)
            .map(|h| MediaId(*h.as_bytes()))
            .map_err(|_| IdParseError(s.to_string()))
    }
}

impl Serialize for MediaId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for MediaId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

macro_rules! ulid_id {
    ($(#[$m:meta])* $name:ident, $prefix:literal) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Ulid);

        impl $name {
            pub fn new() -> Self {
                $name(Ulid::generate())
            }
            /// Key used inside the Loro document.
            pub fn key(&self) -> String {
                self.0.to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(s).map($name).map_err(|_| IdParseError(s.to_string()))
            }
        }
    };
}

ulid_id!(/// A clip on a sequence.
    ClipId, "clip:");
ulid_id!(/// A track on a sequence.
    TrackId, "track:");
ulid_id!(/// A sequence (timeline).
    SeqId, "seq:");
ulid_id!(/// A marker on a sequence.
    MarkerId, "marker:");
ulid_id!(/// A transition between two clips.
    TransitionId, "trans:");
ulid_id!(/// A project document.
    DocId, "doc:");

/// Loro peer id of a collaborator's replica.
pub type PeerId = u64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_id_hex_round_trip() {
        let id = MediaId::of_bytes(b"hello");
        let s = id.to_hex();
        assert_eq!(s.len(), 64);
        assert_eq!(s.parse::<MediaId>().unwrap(), id);
        assert!("zz".parse::<MediaId>().is_err());
        let j = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<MediaId>(&j).unwrap(), id);
    }

    #[test]
    fn ulid_ids_parse() {
        let c = ClipId::new();
        assert_eq!(c.key().parse::<ClipId>().unwrap(), c);
    }
}
