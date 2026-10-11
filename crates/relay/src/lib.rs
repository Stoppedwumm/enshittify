//! `cutroom-relay`: authoritative ordering, persistence and fan-out of Loro
//! updates.
//!
//! * One room per `doc_id`, holding an in-memory `LoroDoc`.
//! * Every accepted update is appended to a SQLite journal before it is
//!   acknowledged and fanned out.
//! * Every 1000 updates (or 60 s with changes) the room writes a snapshot
//!   checkpoint and truncates the journal.
//! * Presence is relayed but never stored.
//!
//! M0 scope: no authentication or roles yet (JWT + role checks land with the
//! MVP), so bind it to localhost or a trusted LAN.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use cutroom_sync::protocol::{ErrorCode, Msg, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use loro::{ExportMode, LoroDoc, VersionVector};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use tokio::sync::broadcast;

const CHECKPOINT_UPDATES: u64 = 1000;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(60);

/// Room ids are used as SQLite keys only, but are still restricted so a
/// client can never smuggle path-like or oversized identifiers.
pub fn valid_doc_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

struct Journal {
    conn: Connection,
}

impl Journal {
    fn open(path: Option<&Path>) -> anyhow::Result<Self> {
        let conn = match path {
            Some(p) => Connection::open(p)?,
            None => Connection::open_in_memory()?,
        };
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS updates (
                doc_id TEXT NOT NULL, seq INTEGER NOT NULL, bytes BLOB NOT NULL,
                PRIMARY KEY (doc_id, seq));
             CREATE TABLE IF NOT EXISTS checkpoints (
                doc_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, snapshot BLOB NOT NULL);",
        )?;
        Ok(Journal { conn })
    }

    /// Loads checkpoint + journal tail into a fresh doc; returns (doc, last_seq).
    fn load(&self, doc_id: &str) -> anyhow::Result<(LoroDoc, u64)> {
        let doc = LoroDoc::new();
        let mut seq = 0u64;
        if let Ok((s, snap)) = self.conn.query_row(
            "SELECT seq, snapshot FROM checkpoints WHERE doc_id = ?1",
            params![doc_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)),
        ) {
            doc.import(&snap)?;
            seq = s as u64;
        }
        let mut st = self.conn.prepare(
            "SELECT seq, bytes FROM updates WHERE doc_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = st.query_map(params![doc_id, seq as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (s, b) = row?;
            if let Err(e) = doc.import(&b) {
                tracing::warn!(doc_id, seq = s, "skipping bad journal entry: {e}");
            }
            seq = s as u64;
        }
        Ok((doc, seq))
    }

    fn append(&self, doc_id: &str, seq: u64, bytes: &[u8]) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO updates (doc_id, seq, bytes) VALUES (?1, ?2, ?3)",
            params![doc_id, seq as i64, bytes],
        )?;
        Ok(())
    }

    fn checkpoint(&mut self, doc_id: &str, seq: u64, snapshot: &[u8]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO checkpoints (doc_id, seq, snapshot) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET seq = ?2, snapshot = ?3",
            params![doc_id, seq as i64, snapshot],
        )?;
        tx.execute(
            "DELETE FROM updates WHERE doc_id = ?1 AND seq <= ?2",
            params![doc_id, seq as i64],
        )?;
        tx.commit()?;
        Ok(())
    }
}

struct Room {
    doc: LoroDoc,
    seq: u64,
    since_checkpoint: u64,
    last_checkpoint: Instant,
    /// (sender connection id, encoded message)
    tx: broadcast::Sender<(u64, Arc<Vec<u8>>)>,
}

pub struct Relay {
    journal: Mutex<Journal>,
    rooms: Mutex<HashMap<String, Arc<Mutex<Room>>>>,
    next_conn: Mutex<u64>,
}

impl Relay {
    pub fn new(db: Option<&Path>) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Relay {
            journal: Mutex::new(Journal::open(db)?),
            rooms: Mutex::new(HashMap::new()),
            next_conn: Mutex::new(0),
        }))
    }

    fn room(&self, doc_id: &str) -> anyhow::Result<Arc<Mutex<Room>>> {
        let mut rooms = self.rooms.lock();
        if let Some(r) = rooms.get(doc_id) {
            return Ok(r.clone());
        }
        let (doc, seq) = self.journal.lock().load(doc_id)?;
        let (tx, _) = broadcast::channel(1024);
        let r = Arc::new(Mutex::new(Room {
            doc,
            seq,
            since_checkpoint: 0,
            last_checkpoint: Instant::now(),
            tx,
        }));
        rooms.insert(doc_id.to_string(), r.clone());
        Ok(r)
    }

    /// Imports, journals, and returns the bytes to fan out.
    fn accept_update(&self, doc_id: &str, room: &Mutex<Room>, bytes: &[u8]) -> anyhow::Result<()> {
        let mut r = room.lock();
        let before = r.doc.oplog_vv();
        r.doc.import(bytes)?;
        if r.doc.oplog_vv() == before {
            return Ok(()); // duplicate delivery
        }
        r.seq += 1;
        r.since_checkpoint += 1;
        self.journal.lock().append(doc_id, r.seq, bytes)?;
        if r.since_checkpoint >= CHECKPOINT_UPDATES
            || r.last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
        {
            let snap = r.doc.export(ExportMode::Snapshot)?;
            self.journal.lock().checkpoint(doc_id, r.seq, &snap)?;
            r.since_checkpoint = 0;
            r.last_checkpoint = Instant::now();
        }
        Ok(())
    }

    /// Current room state (tests, admin).
    pub fn room_snapshot(&self, doc_id: &str) -> Option<Vec<u8>> {
        let rooms = self.rooms.lock();
        let r = rooms.get(doc_id)?.lock();
        r.doc.export(ExportMode::Snapshot).ok()
    }
}

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(relay)
}

async fn ws_handler(ws: WebSocketUpgrade, State(relay): State<Arc<Relay>>) -> impl IntoResponse {
    ws.max_message_size(64 << 20)
        .on_upgrade(move |socket| async move {
            if let Err(e) = handle_socket(socket, relay).await {
                tracing::debug!("connection ended: {e}");
            }
        })
}

fn err_msg(code: ErrorCode, m: &str) -> Message {
    Message::Binary(
        Msg::Error {
            code,
            message: m.into(),
        }
        .encode()
        .into(),
    )
}

async fn handle_socket(socket: WebSocket, relay: Arc<Relay>) -> anyhow::Result<()> {
    let (mut sink, mut stream) = socket.split();
    // First message must be Hello.
    let hello = loop {
        match stream.next().await {
            Some(Ok(Message::Binary(b))) => break Msg::decode(&b)?,
            Some(Ok(Message::Close(_))) | None => return Ok(()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
        }
    };
    let Msg::Hello {
        protocol,
        doc_id,
        peer_id,
        version_vector,
        ..
    } = hello
    else {
        sink.send(err_msg(ErrorCode::BadRequest, "expected Hello"))
            .await?;
        return Ok(());
    };
    if protocol != PROTOCOL_VERSION {
        sink.send(err_msg(
            ErrorCode::VersionMismatch,
            "protocol version mismatch",
        ))
        .await?;
        return Ok(());
    }
    if !valid_doc_id(&doc_id) {
        sink.send(err_msg(ErrorCode::BadRequest, "invalid doc id"))
            .await?;
        return Ok(());
    }
    let room = relay.room(&doc_id)?;
    let conn_id = {
        let mut n = relay.next_conn.lock();
        *n += 1;
        *n
    };
    let (welcome, mut rx) = {
        let r = room.lock();
        let client_vv = VersionVector::decode(&version_vector).unwrap_or_default();
        let updates = r.doc.export(ExportMode::updates(&client_vv))?;
        (
            Msg::Welcome {
                updates,
                server_version_vector: r.doc.oplog_vv().encode(),
            },
            r.tx.subscribe(),
        )
    };
    sink.send(Message::Binary(welcome.encode().into())).await?;
    tracing::info!(doc_id, peer_id, conn_id, "client joined");

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                fan = rx.recv() => match fan {
                    Ok((from, bytes)) if from != conn_id => {
                        sink.send(Message::Binary(bytes.to_vec().into())).await?;
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Too slow: force a resync on reconnect.
                        anyhow::bail!("client lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
                inc = stream.next() => match inc {
                    Some(Ok(Message::Binary(b))) => match Msg::decode(&b)? {
                        Msg::Update { seq, bytes } => {
                            relay.accept_update(&doc_id, &room, &bytes)?;
                            let out = Msg::Update { seq: 0, bytes }.encode();
                            let _ = room.lock().tx.send((conn_id, Arc::new(out)));
                            sink.send(Message::Binary(Msg::Ack { seq }.encode().into())).await?;
                        }
                        Msg::Presence(mut p) => {
                            p.peer_id = peer_id; // never trust a claimed peer id
                            let out = Msg::Presence(p).encode();
                            let _ = room.lock().tx.send((conn_id, Arc::new(out)));
                        }
                        Msg::Heartbeat => {}
                        _ => sink.send(err_msg(ErrorCode::BadRequest, "unexpected message")).await?,
                    },
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                },
            }
        }
    }
    .await;
    let gone = Msg::PresenceGone { peer_id }.encode();
    let _ = room.lock().tx.send((conn_id, Arc::new(gone)));
    tracing::info!(doc_id, peer_id, conn_id, "client left");
    result
}

/// Binds and serves until the process exits.
pub async fn serve(addr: SocketAddr, relay: Arc<Relay>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        "cutroom-relay listening on ws://{}/ws",
        listener.local_addr()?
    );
    axum::serve(listener, router(relay)).await?;
    Ok(())
}
