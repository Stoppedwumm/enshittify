//! Cutroom synchronisation.
//!
//! * [`protocol`] — postcard-encoded messages shared by every transport
//!   (relay WebSocket today, iroh QUIC later).
//! * [`presence`] — ephemeral collaborator state with 20 Hz coalescing and
//!   15 s expiry; never written to the document.
//! * [`client`] — a background WebSocket client that exchanges Loro updates
//!   with `cutroom-relay` and hands them to the UI over channels.

pub mod client;
pub mod presence;
pub mod protocol;

pub use protocol::{Msg, PROTOCOL_VERSION};
