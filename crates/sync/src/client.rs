//! Relay client.
//!
//! Runs a tokio runtime on a background thread and talks to the UI through
//! channels; the Loro document itself stays on the UI thread (imports happen
//! there, between commands, so a remote batch can never interleave with a
//! local transaction).

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMsg;

use crate::protocol::{HEARTBEAT_SECS, Msg, PROTOCOL_VERSION, Presence};

#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    Connected,
    /// Import these bytes (Welcome payload or a peer's update).
    Remote(Vec<u8>),
    /// Export `updates(since = server_vv)` and send them with
    /// [`SyncClient::send_update`] (offline edits).
    PushSince(Vec<u8>),
    Presence(Presence),
    PresenceGone(u64),
    Acked(u64),
    Disconnected(String),
}

enum Out {
    Msg(Msg),
    /// Reconnect with this version vector.
    Hello(Vec<u8>),
}

/// Handle to a background connection. Dropping it disconnects.
pub struct SyncClient {
    tx: mpsc::UnboundedSender<Out>,
    events: Receiver<ClientEvent>,
    seq: u64,
    _thread: std::thread::JoinHandle<()>,
}

impl SyncClient {
    /// Connects to `url` (e.g. `ws://127.0.0.1:7878/ws`) for `doc_id`.
    /// `vv` is the encoded version vector the client already has.
    pub fn connect(
        url: String,
        doc_id: String,
        peer_id: u64,
        vv: Vec<u8>,
        token: Option<String>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = unbounded();
        let thread = std::thread::Builder::new()
            .name("sync-client".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(run(url, doc_id, peer_id, vv, token, rx, ev_tx, wake));
            })
            .expect("spawn sync thread");
        SyncClient {
            tx,
            events: ev_rx,
            seq: 0,
            _thread: thread,
        }
    }

    pub fn events(&self) -> &Receiver<ClientEvent> {
        &self.events
    }

    /// Sends one local transaction's update bytes; returns its sequence number.
    pub fn send_update(&mut self, bytes: Vec<u8>) -> u64 {
        self.seq += 1;
        let _ = self.tx.send(Out::Msg(Msg::Update {
            seq: self.seq,
            bytes,
        }));
        self.seq
    }

    pub fn send_presence(&self, p: Presence) {
        let _ = self.tx.send(Out::Msg(Msg::Presence(p)));
    }

    /// Forces a reconnect handshake with a fresh version vector.
    pub fn rehello(&self, vv: Vec<u8>) {
        let _ = self.tx.send(Out::Hello(vv));
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    url: String,
    doc_id: String,
    peer_id: u64,
    mut vv: Vec<u8>,
    token: Option<String>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    ev: Sender<ClientEvent>,
    wake: impl Fn() + Send + Sync + 'static,
) {
    let emit = |e: ClientEvent| {
        let _ = ev.send(e);
        wake();
    };
    let mut backoff = Duration::from_millis(250);
    // Messages produced while disconnected; updates are replayed via
    // the Welcome/PushSince handshake, so only keep the latest presence.
    loop {
        let conn = tokio_tungstenite::connect_async(url.as_str()).await;
        let (mut sink, mut stream) = match conn {
            Ok((ws, _)) => ws.split(),
            Err(e) => {
                emit(ClientEvent::Disconnected(e.to_string()));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    m = rx.recv() => match m {
                        None => return,
                        Some(Out::Hello(v)) => vv = v,
                        Some(Out::Msg(_)) => {}
                    }
                }
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
        };
        backoff = Duration::from_millis(250);
        let hello = Msg::Hello {
            protocol: PROTOCOL_VERSION,
            doc_id: doc_id.clone(),
            peer_id,
            version_vector: vv.clone(),
            token: token.clone(),
        };
        if sink
            .send(WsMsg::Binary(hello.encode().into()))
            .await
            .is_err()
        {
            continue;
        }
        emit(ClientEvent::Connected);
        let mut hb = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
        let reason = loop {
            tokio::select! {
                _ = hb.tick() => {
                    if sink.send(WsMsg::Binary(Msg::Heartbeat.encode().into())).await.is_err() {
                        break "heartbeat send failed".to_string();
                    }
                }
                out = rx.recv() => match out {
                    None => {
                        let _ = sink.send(WsMsg::Close(None)).await;
                        return;
                    }
                    Some(Out::Hello(v)) => {
                        vv = v;
                        break "rehello".to_string();
                    }
                    Some(Out::Msg(m)) => {
                        if sink.send(WsMsg::Binary(m.encode().into())).await.is_err() {
                            break "send failed".to_string();
                        }
                    }
                },
                inc = stream.next() => match inc {
                    Some(Ok(WsMsg::Binary(b))) => match Msg::decode(&b) {
                        Ok(Msg::Welcome { updates, server_version_vector }) => {
                            if !updates.is_empty() {
                                emit(ClientEvent::Remote(updates));
                            }
                            emit(ClientEvent::PushSince(server_version_vector));
                        }
                        Ok(Msg::Update { bytes, .. }) => emit(ClientEvent::Remote(bytes)),
                        Ok(Msg::Ack { seq }) => emit(ClientEvent::Acked(seq)),
                        Ok(Msg::Presence(p)) => emit(ClientEvent::Presence(p)),
                        Ok(Msg::PresenceGone { peer_id }) => emit(ClientEvent::PresenceGone(peer_id)),
                        Ok(Msg::Error { message, .. }) => break message,
                        Ok(_) => {}
                        Err(e) => tracing::warn!("bad message: {e}"),
                    },
                    Some(Ok(WsMsg::Close(_))) | None => break "closed".to_string(),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break e.to_string(),
                },
            }
        };
        emit(ClientEvent::Disconnected(reason));
        tokio::time::sleep(backoff).await;
    }
}
