//! Two clients editing through a real relay converge, including edits made
//! while one client is offline.

use std::time::{Duration, Instant};

use cutroom_core::command::{AddMedia, InitProject, RenameProject};
use cutroom_core::model::{MediaEntry, MediaKind, Meta};
use cutroom_core::{Document, MediaId, Ticks};
use cutroom_sync::client::{ClientEvent, SyncClient};

struct Peer {
    doc: Document,
    client: SyncClient,
    sent: loro::VersionVector,
}

impl Peer {
    fn new(url: &str, peer: u64) -> Self {
        let doc = Document::with_peer(peer);
        let client = SyncClient::connect(
            url.into(),
            "test-doc".into(),
            peer,
            doc.version().encode(),
            None,
            || {},
        );
        Peer {
            doc,
            client,
            sent: Default::default(),
        }
    }

    /// Pumps events and pushes local changes; mirrors what the app does.
    fn pump(&mut self) {
        while let Ok(ev) = self.client.events().try_recv() {
            match ev {
                ClientEvent::Remote(b) => self.doc.import(&b).unwrap(),
                ClientEvent::PushSince(vv) => {
                    let vv = loro::VersionVector::decode(&vv).unwrap();
                    let b = self.doc.export_updates_since(&vv);
                    self.client.send_update(b);
                    self.sent = self.doc.version();
                }
                _ => {}
            }
        }
        if self.doc.version() != self.sent {
            let b = self.doc.export_updates_since(&self.sent);
            self.client.send_update(b);
            self.sent = self.doc.version();
        }
    }
}

fn converge(peers: &mut [&mut Peer]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        for p in peers.iter_mut() {
            p.pump();
        }
        let s0 = peers[0].doc.state_json();
        if peers.iter().all(|p| p.doc.state_json() == s0) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn two_clients_converge_through_relay() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relay = cutroom_relay::Relay::new(None).unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let r2 = relay.clone();
    rt.spawn(async move { axum::serve(listener, cutroom_relay::router(r2)).await });
    let url = format!("ws://{addr}/ws");

    let mut a = Peer::new(&url, 1);
    a.doc.execute(&InitProject::new(Meta::default())).unwrap();
    let mut b = Peer::new(&url, 2);
    assert!(converge(&mut [&mut a, &mut b]));
    assert!(b.doc.snapshot().main().is_some());

    // Live edit A → B, timed.
    let t0 = Instant::now();
    a.doc.execute(&RenameProject("Live".into())).unwrap();
    assert!(converge(&mut [&mut a, &mut b]));
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "{:?}",
        t0.elapsed()
    );
    assert_eq!(b.doc.snapshot().meta.name, "Live");

    // B goes offline and edits; A edits too; B reconnects and both merge.
    drop(b.client);
    let mut b_doc = b.doc;
    b_doc
        .execute(&AddMedia(MediaEntry {
            id: MediaId::of_bytes(b"offline"),
            name: "offline.mov".into(),
            kind: MediaKind::Video,
            duration: Ticks::from_secs(1),
            fps: None,
            width: 1,
            height: 1,
            audio_channels: 0,
            audio_sample_rate: 0,
            size_bytes: 1,
            proxy_hash: None,
        }))
        .unwrap();
    a.doc.execute(&RenameProject("Merged".into())).unwrap();
    let mut b = Peer {
        client: SyncClient::connect(
            url.clone(),
            "test-doc".into(),
            2,
            b_doc.version().encode(),
            None,
            || {},
        ),
        sent: b_doc.version(),
        doc: b_doc,
    };
    assert!(converge(&mut [&mut a, &mut b]));
    let s = a.doc.snapshot();
    assert_eq!(s.meta.name, "Merged");
    assert!(s.media.contains_key(&MediaId::of_bytes(b"offline")));

    // The relay's own copy matches too.
    let mut server = Document::with_peer(99);
    server
        .import(&relay.room_snapshot("test-doc").unwrap())
        .unwrap();
    assert_eq!(server.state_json(), a.doc.state_json());
}

#[test]
fn journal_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("j.sqlite");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let expected;
    {
        let relay = cutroom_relay::Relay::new(Some(&db)).unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let r2 = relay.clone();
        let h = rt.spawn(async move { axum::serve(listener, cutroom_relay::router(r2)).await });
        let url = format!("ws://{addr}/ws");
        let mut a = Peer::new(&url, 1);
        a.doc.execute(&InitProject::new(Meta::default())).unwrap();
        a.doc.execute(&RenameProject("Durable".into())).unwrap();
        let mut probe = Peer::new(&url, 3);
        assert!(converge(&mut [&mut a, &mut probe]));
        expected = a.doc.state_json();
        h.abort();
    }
    let relay = cutroom_relay::Relay::new(Some(&db)).unwrap();
    // Opening the room replays the journal.
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let r2 = relay.clone();
    rt.spawn(async move { axum::serve(listener, cutroom_relay::router(r2)).await });
    let mut c = Peer::new(&format!("ws://{addr}/ws"), 4);
    let deadline = Instant::now() + Duration::from_secs(10);
    while c.doc.state_json() != expected && Instant::now() < deadline {
        c.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(c.doc.state_json(), expected);
    assert!(cutroom_relay::valid_doc_id("abc-DEF_1"));
    assert!(!cutroom_relay::valid_doc_id("../etc"));
}
