//! Live probe: the full two-daemon byte-stream matrix against running
//! jeliyad instances. Daemon A receives a streamed `file.share` (real upload
//! over the socket); daemon B — a second, invited daemon — `file.fetch`es the
//! file from A (real p2p provider transfer) and streams it back out via
//! `file.read` (real download over the socket). The collected bytes are
//! asserted byte-for-byte against the uploaded pattern.
//!
//! `file.read` serves only locally-held bytes and a daemon never fetches from
//! itself (supervisor fetch excludes `self_device`), so single-daemon
//! read-back is impossible BY DESIGN — the corpus records the same
//! (`resource:fetched_file` is "unestablishable single-subject"). Two daemons
//! is the honest minimum for a live round trip.
//!
//! Run (both daemons WITHOUT `--loopback` — RealNetwork mode: the invited
//! join dials the minter via discovery, which loopback mode cannot do):
//!   JELIYAD_PROBE_URL=ws://127.0.0.1:P1/ws JELIYAD_PROBE_HTTP=http://127.0.0.1:P1 \
//!   JELIYAD_PROBE_TOKEN=... JELIYAD_PROBE2_URL=ws://127.0.0.1:P2/ws \
//!   JELIYAD_PROBE2_HTTP=http://127.0.0.1:P2 JELIYAD_PROBE2_TOKEN=... \
//!   cargo test -p jeliya-client --features ws-native --test live_native_probe \
//!     -- --nocapture --ignored

use futures::StreamExt;
use jeliya_api::{
    FileFetch, FileRead, FileShare, InviteMint, InviteRedeem, RoomActivate, RoomCreate,
    SubjectEnsure,
};
use jeliya_client::{media, Dedup, State};

/// One live daemon connection: URL pieces + a connected, Ready handle.
struct Endpoint {
    url: String,
    http: String,
    token: String,
    sg: u64,
}

impl Endpoint {
    fn from_env(prefix: &str) -> Self {
        Self {
            url: std::env::var(format!("{prefix}_URL")).expect("url env"),
            http: std::env::var(format!("{prefix}_HTTP")).expect("http env"),
            token: std::env::var(format!("{prefix}_TOKEN")).expect("token env"),
            sg: 0,
        }
    }

    async fn health_sg(&self) -> u64 {
        let health = std::process::Command::new("curl")
            .arg("-s")
            .arg(format!("{}/api/health", self.http))
            .output()
            .expect("curl health")
            .stdout;
        let health: serde_json::Value = serde_json::from_slice(&health).expect("health json");
        health["storage_generation"].as_u64().expect("sg")
    }

    async fn connect(&mut self) -> jeliya_client::ClientHandle {
        self.sg = self.health_sg().await;
        struct Fixed {
            url: String,
            token: String,
            sg: u64,
        }
        impl jeliya_client::TargetSource for Fixed {
            fn resolve(
                &self,
            ) -> futures::future::BoxFuture<
                'static,
                Result<jeliya_client::Dial, jeliya_client::DialResolveError>,
            > {
                let url =
                    url::Url::parse(&format!("{}?v=2&sg={}", self.url, self.sg)).expect("url");
                let dial = jeliya_client::Dial {
                    url,
                    bearer: jeliya_supervisor::Redacted::new(self.token.clone()),
                };
                Box::pin(async move { Ok(dial) })
            }
        }
        let handle = jeliya_client::connect_ws_native(
            Fixed {
                url: self.url.clone(),
                token: self.token.clone(),
                sg: self.sg,
            },
            jeliya_client::NativeClientConfig::default(),
        )
        .expect("connect");
        let mut sub = handle.subscribe();
        handle.start();
        loop {
            let ev = sub.next().await.expect("event");
            if let jeliya_client::ClientEvent::StateChanged { to, .. } = ev {
                eprintln!("  state -> {to:?}");
                if to == State::Ready {
                    break;
                }
                if to == State::Failed {
                    panic!("daemon connection failed");
                }
            }
        }
        handle
    }
}

#[tokio::test]
#[ignore]
async fn two_daemon_share_fetch_read_roundtrip_live() {
    let mut a = Endpoint::from_env("JELIYAD_PROBE");
    let mut b = Endpoint::from_env("JELIYAD_PROBE2");

    eprintln!("connecting daemon A (sharer)…");
    let ha = a.connect().await;
    eprintln!("connecting daemon B (reader)…");
    let hb = b.connect().await;

    // A: subject + live room.
    let _ = ha
        .call::<SubjectEnsure>(SubjectEnsure {}, Dedup::None)
        .await
        .expect("A subject.ensure");
    let room = ha
        .call::<RoomCreate>(
            RoomCreate {
                name: String::from("native-two-daemon-probe"),
            },
            Dedup::None,
        )
        .await
        .expect("A room.create");
    let room_id = room.room_id;
    ha.call::<RoomActivate>(
        RoomActivate {
            room_id: room_id.clone(),
        },
        Dedup::None,
    )
    .await
    .expect("A room.activate");

    // B's subject, then A mints an invite bound to it and B redeems.
    let subject_b = hb
        .call::<SubjectEnsure>(SubjectEnsure {}, Dedup::None)
        .await
        .expect("B subject.ensure")
        .subject_id;
    let minted = ha
        .call::<InviteMint>(
            InviteMint {
                room_id: room_id.clone(),
                subject_id: subject_b,
                role: jeliya_api::Role::Member,
                expires_at: jeliya_api::Timestamp::new(
                    time::OffsetDateTime::now_utc().saturating_add(time::Duration::hours(1)),
                ),
            },
            Dedup::None,
        )
        .await
        .expect("A invite.mint");
    hb.call::<InviteRedeem>(
        InviteRedeem {
            capability: minted.capability,
        },
        Dedup::None,
    )
    .await
    .expect("B invite.redeem");
    eprintln!("B joined the room");
    // B activates its copy of the room (the live session both daemons serve).
    hb.call::<RoomActivate>(
        RoomActivate {
            room_id: room_id.clone(),
        },
        Dedup::None,
    )
    .await
    .expect("B room.activate");

    // The streamed share on A: 64 KiB of pattern (bigger than one DATA chunk).
    let pattern: Vec<u8> = (0..65_536usize)
        .map(|i| ((i * 11 + 5) % 256) as u8)
        .collect();
    let share_op = jeliya_api::OpId::new("native-two-daemon-share-1");
    ha.register_stream_media(share_op.clone(), media::shared_bytes(pattern.clone()))
        .expect("register share source");
    let shared = ha
        .call_stream::<FileShare>(
            FileShare {
                room_id: room_id.clone(),
                name: String::from("roundtrip.bin"),
                declared_bytes: pattern.len() as u64,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(share_op),
        )
        .await
        .expect("the streamed file.share resolves against live daemon A");
    eprintln!("A accepted the share ({} bytes)", shared.bytes);
    assert_eq!(shared.bytes, pattern.len() as u64);

    // B fetches from provider A (bounded wait: the room session must sync the
    // signed file_shared event from A and connect the two daemons; until then
    // the file is unknown/unfetchable on B).
    let mut fetched = false;
    for attempt in 0..120 {
        match hb
            .call::<FileFetch>(
                FileFetch {
                    room_id: room_id.clone(),
                    file_id: shared.file_id.clone(),
                },
                Dedup::None,
            )
            .await
        {
            Ok(out) => {
                eprintln!("B fetched {} bytes from the provider", out.bytes);
                assert_eq!(out.bytes, pattern.len() as u64);
                fetched = true;
                break;
            }
            Err(err) => {
                eprintln!("fetch attempt {attempt}: {err:?}");
                tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;
            }
        }
    }
    assert!(fetched, "B must fetch the file from provider A");

    // B reads the bytes back over its own socket.
    let (sink, sink_media) = media::collected_bytes();
    let read_op = jeliya_api::OpId::new("native-two-daemon-read-1");
    hb.register_stream_media(read_op.clone(), sink_media)
        .expect("register read sink");
    let header = hb
        .call_stream::<FileRead>(
            FileRead {
                room_id: room_id.clone(),
                file_id: shared.file_id.clone(),
            },
            Dedup::Key(read_op),
        )
        .await
        .expect("the streamed file.read resolves against live daemon B");
    let collected = sink.take();
    eprintln!(
        "B streamed {} bytes back (header says {})",
        collected.len(),
        header.bytes
    );
    assert_eq!(header.bytes, pattern.len() as u64);
    assert_eq!(
        collected, pattern,
        "the bytes B read over the socket are exactly the bytes A accepted"
    );
    eprintln!("TWO-DAEMON ROUND TRIP OK ({} bytes)", collected.len());

    ha.stop().await;
    hb.stop().await;
}
