//! Cutroom core: frame-accurate time, identifiers, the Loro-backed project
//! document, commands, undo and on-disk persistence.

pub mod command;
pub mod document;
pub mod ids;
pub mod local_index;
pub mod model;
pub mod normalize;
pub mod store;
pub mod time;
pub mod validate;

pub use command::{Command, CommandError};
pub use document::Document;
pub use ids::{ClipId, DocId, MarkerId, MediaId, PeerId, SeqId, TrackId, TransitionId};
pub use model::DocSnapshot;
pub use time::{FLICKS_PER_SECOND, Ticks, Timebase, Timecode};

#[cfg(test)]
mod tests;
