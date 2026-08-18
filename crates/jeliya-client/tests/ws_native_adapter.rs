//! Deterministic integration tests for the native WebSocket adapter (#172).
//!
//! An in-process fake `jeliyad` — a loopback `tokio` TCP listener with a
//! minimal WebSocket handshake and scripted hello — drives the full
//! dial → hello agreement → serve pipeline **without** external services or
//! real daemons.
//!
//! Tests cover (§10.1):
//! - Terminal resolve → `Failed` without auto-retry.
//! - Stop while connecting → `Stopped` cleanly.
//! - `Connected` only after a matching hello from the fake server.
//! - Wrong-generation hello is treated as a gate refusal (`Failed`).
//! - Resolve fires on every dial attempt, not just the initial one.
//! - Hello deadline fires as transient (retry) when no hello arrives.
//! - Reconnect after server disconnect returns to `Ready`.
//! - A Gap push from the server surfaces as `ClientEvent::Gap` to a subscriber.
//! - HTTP 426 during the upgrade handshake fails closed (terminal).
//!
//! Requires feature `ws-native`; CI runs:
//!   cargo test -p jeliya-client --features ws-native

#![cfg(all(feature = "ws-native", not(target_arch = "wasm32")))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::StreamExt;
use jeliya_client::{
    connect_ws_native, ClientEvent, Dial, DialResolveError, KernelConfig, KernelLimits,
    NativeClientConfig, State, TargetSource, TickDelta,
};
use jeliya_supervisor::Redacted;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

/// Short backoff and few retries so deterministic tests complete quickly.
fn fast_config() -> NativeClientConfig {
    NativeClientConfig {
        kernel: KernelConfig {
            limits: KernelLimits {
                backoff_base: TickDelta::from_ticks(1),
                backoff_cap: TickDelta::from_ticks(5),
                max_reconnect_attempts: 2,
                ..KernelLimits::default()
            },
            jitter_seed: 0,
            stable_principal: false,
            streams: jeliya_client::StreamLimits::default(),
        },
        connect_timeout: Duration::from_millis(100),
        hello_timeout: Duration::from_millis(100),
    }
}

/// Poll the handle until it reaches `expected` or the deadline passes.
async fn wait_for_state(handle: &jeliya_client::ClientHandle, expected: State, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if handle.state() == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for state {expected:?}, current state is {:?}",
            handle.state()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Poll the handle until it leaves `not_expected` or the deadline passes.
/// Build the `Limits` the fake hello carries (same values as codec tests).
fn test_limits() -> jeliya_api::Limits {
    jeliya_api::Limits {
        max_shared_file_bytes: 104_857_600,
        max_message_body_bytes: 65_536,
        max_frame_bytes: 128 * 1024 * 1024,
        max_inflight_requests: 256,
        max_subscriptions_per_connection: 64,
        max_connections: 64,
        max_concurrent_transfers: 8,
        max_transfer_bytes_inflight: 1 << 30,
        transfer_connect_allowance_ms: 10_000,
        transfer_floor_bits_per_second: 1_000_000,
        transfer_stall_ms: 30_000,
        timeline_page_max: 200,
        idle_timeout_ms: 60_000,
        pairing_code_ttl_ms: 300_000,
        pairing_code_max_attempts: 5,
        browser_session_ttl_ms: 3_600_000,
    }
}

/// Serialize a `Hello` for a given `storage_generation`. The `Hello` struct
/// carries `t:"hello"` in JSON (via its serde tag), which `decode_client_frame`
/// uses to classify it.
fn hello_json(storage_generation: u64) -> String {
    serde_json::to_string(&jeliya_api::Hello {
        protocol: 2,
        storage_generation,
        limits: test_limits(),
        subject: jeliya_api::SubjectState::Absent,
        resume: jeliya_api::Resume::Fresh,
        incarnation: jeliya_api::Incarnation::new("ws-native-test-incarnation"),
    })
    .expect("Hello serializes")
}

/// A `TargetSource` that always returns a `Dial` pointing to the given URL.
struct FixedSource {
    url: url::Url,
}

impl TargetSource for FixedSource {
    fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
        let url = self.url.clone();
        Box::pin(futures::future::ready(Ok(Dial {
            url,
            bearer: Redacted::new("test-token-stub".to_owned()),
        })))
    }
}

/// Accept one connection, perform the WS upgrade, send `hello`, then keep the
/// connection open for `hold_ms` milliseconds before dropping it.
async fn serve_one_hello(listener: TcpListener, hello: String, hold_ms: u64) {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let _ = ws.send(Message::text(hello)).await;
    tokio::time::sleep(Duration::from_millis(hold_ms)).await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A source that immediately returns a terminal failure causes the kernel to
/// transition to `Failed` without any backoff retry — "fail closed" (spec §8).
#[test]
fn terminal_resolve_transitions_to_failed_without_retry() {
    let rt = test_runtime();
    rt.block_on(async {
        struct AlwaysTerminal;
        impl TargetSource for AlwaysTerminal {
            fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
                Box::pin(futures::future::ready(Err(DialResolveError::Terminal(
                    "wrong daemon — fail closed".to_owned(),
                ))))
            }
        }

        let handle = connect_ws_native(AlwaysTerminal, fast_config()).expect("creates");
        handle.start();
        wait_for_state(&handle, State::Failed, 1_000).await;
    });
}

/// Stop while the kernel is in the connecting phase tears down cleanly with no
/// lingering tasks (spec §7.7 — no task/socket survives stop).
#[test]
fn stop_while_connecting_completes_cleanly() {
    let rt = test_runtime();
    rt.block_on(async {
        struct AlwaysTransient;
        impl TargetSource for AlwaysTransient {
            fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
                Box::pin(futures::future::ready(Err(DialResolveError::Transient(
                    "not yet".to_owned(),
                ))))
            }
        }

        let handle = connect_ws_native(AlwaysTransient, fast_config()).expect("creates");
        handle.start();
        // Give the resolve a chance to fire (ensures at least one Transient
        // path is exercised before stop).
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.stop().await;
        assert_eq!(handle.state(), State::Stopped);
    });
}

/// The kernel must NOT report `Connected` (→ `Ready`) before it receives a
/// valid hello whose `protocol` and `storage_generation` match the URL gate.
/// When the fake server sends a matching hello, the handle transitions to
/// `Ready` (spec §7.3 — third independent agreement check).
#[test]
fn connected_only_after_valid_matching_hello() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        // Server sends a hello whose generation matches the URL gate.
        let server = tokio::spawn(serve_one_hello(listener, hello_json(sg), 800));

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        wait_for_state(&handle, State::Ready, 2_000).await;

        handle.stop().await;
        server.abort();
    });
}

/// When the fake server sends a hello whose `storage_generation` does not match
/// the value in the dial URL, the adapter must refuse it as a third-agreement
/// failure and inject `GateRefused` → the kernel transitions to `Failed`
/// (spec §7.3 — "wrong-generation hello → terminal").
#[test]
fn wrong_generation_hello_fails_closed() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let expected_sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={expected_sg}"))
            .expect("url");

        // Server sends hello with sg=2 but the client's URL gate expects sg=1.
        let wrong_sg = expected_sg + 1;
        let server = tokio::spawn(serve_one_hello(listener, hello_json(wrong_sg), 500));

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        // The gate refusal is terminal → Failed without retrying into the
        // wrong-generation server.
        wait_for_state(&handle, State::Failed, 2_000).await;

        server.abort();
    });
}

/// The resolver must be called once per dial attempt, not just once at
/// construction time. This is a critical acceptance criterion (spec §11 and
/// §7.2 step 1). With `max_reconnect_attempts: 2` a `Transient` source drives
/// 3 dial attempts (initial + 2 retries), and all three must invoke `resolve`.
#[test]
fn resolve_called_on_every_dial_attempt() {
    let rt = test_runtime();
    rt.block_on(async {
        struct CountedTransient {
            count: Arc<AtomicU64>,
        }
        impl TargetSource for CountedTransient {
            fn resolve(&self) -> BoxFuture<'static, Result<Dial, DialResolveError>> {
                let count = self.count.clone();
                Box::pin(async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(DialResolveError::Transient("not yet".to_owned()))
                })
            }
        }

        let count = Arc::new(AtomicU64::new(0));
        let handle = connect_ws_native(
            CountedTransient {
                count: count.clone(),
            },
            fast_config(),
        )
        .expect("creates");
        handle.start();
        wait_for_state(&handle, State::Failed, 2_000).await;

        // Initial dial + 2 retries (max_reconnect_attempts: 2) = 3 resolves.
        let calls = count.load(Ordering::SeqCst);
        assert!(
            calls >= 3,
            "resolve must be called on every dial attempt; expected ≥3, got {calls}"
        );
    });
}

/// When the server accepts the WebSocket upgrade but never sends the Layer-2
/// `hello`, the adapter's hello deadline fires and injects `DialFailed`
/// (transient) — "connection dropped before hello → transient" (spec §7.3).
/// After retry-budget exhaustion the client reaches `Failed`.
#[test]
fn hello_timeout_fires_as_transient_until_budget_exhausted() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        // Server: accept the WS upgrade, never send a hello — the client's
        // hello deadline must classify this as transient, not terminal.
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let Ok(mut _ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    // Hold the connection open without sending anything.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        // Budget exhausted through repeated transient hello-timeouts.
        wait_for_state(&handle, State::Failed, 3_000).await;

        server.abort();
    });
}

/// After a live connection is lost the adapter re-resolves, reconnects, and the
/// kernel transitions back to `Ready` — verifying the full reconnect path and
/// generation bump (spec §7.6, AC: gap/reconnect routes through #169).
///
/// Uses event subscription rather than polling to avoid a race where the
/// 1 ms reconnect backoff is shorter than the 5 ms poll interval.
#[test]
fn reconnect_after_server_disconnect_returns_to_ready() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        let server = tokio::spawn(async move {
            use futures::SinkExt;
            // First connection: accept, send hello, close after 80 ms.
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::text(hello_json(
                            sg,
                        )))
                        .await;
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    // Drop ws → EOF on the client's read_loop → Interrupted.
                }
            }
            // Second connection: hold open indefinitely (killed by server.abort()).
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    let _ = ws
                        .send(tokio_tungstenite::tungstenite::Message::text(hello_json(
                            sg,
                        )))
                        .await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        });

        let config = NativeClientConfig {
            kernel: KernelConfig {
                limits: KernelLimits {
                    backoff_base: TickDelta::from_ticks(1),
                    backoff_cap: TickDelta::from_ticks(10),
                    max_reconnect_attempts: 5,
                    ..KernelLimits::default()
                },
                jitter_seed: 0,
                stable_principal: false,
                streams: jeliya_client::StreamLimits::default(),
            },
            connect_timeout: Duration::from_millis(300),
            hello_timeout: Duration::from_millis(300),
        };
        let handle = connect_ws_native(FixedSource { url }, config).expect("creates");
        let mut sub = handle.subscribe();
        handle.start();

        // Observe events until we see a second StateChanged { to: Ready } transition.
        // The first transition is the initial connection; the second proves reconnect.
        // If the reconnect is faster than the event channel flushes (the 1 ms backoff
        // can race the fan-out), the coalesced_through_problem flag on a Ready→Ready
        // transition is also accepted as evidence of the disconnect+reconnect cycle.
        let reconnect_confirmed = tokio::time::timeout(Duration::from_millis(5_000), async {
            let mut ready_count = 0u32;
            loop {
                match sub.next().await {
                    Some(ClientEvent::StateChanged {
                        to: State::Ready,
                        coalesced_through_problem,
                        ..
                    }) => {
                        if coalesced_through_problem {
                            return true; // fast reconnect coalesced into one event
                        }
                        ready_count += 1;
                        if ready_count >= 2 {
                            return true;
                        }
                    }
                    Some(_) => {}
                    None => return false,
                }
            }
        })
        .await
        .expect("timed out waiting for second Ready transition after reconnect");
        assert!(
            reconnect_confirmed,
            "client must reach Ready a second time after server disconnect"
        );

        handle.stop().await;
        server.abort();
    });
}

/// A `Gap` push sent by the server after the initial `hello` must surface as a
/// `ClientEvent::Gap` to an active subscriber — verifying the full
/// wire-push → codec decode → kernel inject → event fan-out path (spec §7.4).
#[test]
fn gap_push_surfaces_as_gap_event_to_subscriber() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        let gap_push = jeliya_api::Push::Gap {
            room_id: jeliya_api::RoomId::new("room-e2e"),
            from_pos: 7,
            to: jeliya_api::GapTo::Open,
            reason: jeliya_api::GapReason::Backpressure,
        };
        let push_json = serde_json::to_string(&gap_push).expect("push serializes");

        let server = tokio::spawn(async move {
            use futures::SinkExt;
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let _ = ws
                .send(tokio_tungstenite::tungstenite::Message::text(hello_json(
                    sg,
                )))
                .await;
            // Short pause so `Connected` is processed before the push.
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = ws
                .send(tokio_tungstenite::tungstenite::Message::text(push_json))
                .await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        let mut sub = handle.subscribe();
        handle.start();

        let (gap_room_id, gap_from_pos) =
            tokio::time::timeout(Duration::from_millis(2_000), async {
                loop {
                    match sub.next().await {
                        Some(ClientEvent::Gap {
                            room_id, from_pos, ..
                        }) => {
                            break (room_id, from_pos);
                        }
                        Some(_) => {}
                        None => panic!("subscription ended before Gap arrived"),
                    }
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for Gap event; state is {:?}",
                    handle.state()
                )
            });
        assert_eq!(gap_room_id.as_str(), "room-e2e");
        assert_eq!(gap_from_pos, 7);

        handle.stop().await;
        server.abort();
    });
}

/// An HTTP 426 Upgrade Required response during the WebSocket handshake maps to
/// the terminal `GateRefused` path — the same as a generation-gate refusal —
/// and the client transitions to `Failed` without retrying (spec §7.2
/// handshake table, §8 fail-closed invariant).
#[test]
fn http_426_during_upgrade_fails_closed_as_terminal() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        // Raw TCP server: return a 426 HTTP response to every upgrade request.
        // The adapter's `classify_handshake` maps 426 → `GateRefused` (terminal).
        let server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 426 Upgrade Required\r\n\
                              Content-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        // The 426 is terminal; no retries → Failed immediately.
        wait_for_state(&handle, State::Failed, 2_000).await;

        server.abort();
    });
}

// ---------------------------------------------------------------------------
// Byte-stream media drive (upload/download over the real socket path)
// ---------------------------------------------------------------------------

use jeliya_api::{FileId, OpId, RoomId};
use jeliya_client::media::{collected_bytes, shared_bytes};
use jeliya_client::{CallError, Dedup};
use jeliya_codec::{
    decode_stream_record, encode_stream_record, BinaryAbortReason, CodecBounds, StreamIdentity,
    StreamRecord, StreamRecordBody,
};
use tokio_tungstenite::tungstenite::Message;

/// Frame one JBS2 record for the fake daemon to send (the codec owns the
/// layout; the test never hand-builds a header).
fn frame(identity: StreamIdentity, body: StreamRecordBody) -> Message {
    let bytes = encode_stream_record(&StreamRecord { identity, body }, &CodecBounds::default())
        .expect("fake-daemon record frames");
    Message::Binary(bytes.into())
}

/// Read the stream op's Text request off the socket: `(id, op, in)`.
async fn read_request(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> (u64, String, serde_json::Value) {
    use futures::StreamExt;
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = serde_json::from_str(&text).expect("request json");
                let id = value["id"].as_u64().expect("request id");
                let op = value["op"].as_str().expect("op").to_owned();
                let input = value["in"].clone();
                return (id, op, input);
            }
            // Pings from the client's keepalive ride along; ignore them.
            Some(Ok(Message::Ping(_))) => continue,
            other => panic!("expected the stream op's Text request, got {other:?}"),
        }
    }
}

/// A terminal success reply for `id` carrying `out_json`.
fn reply_text(id: u64, out_json: String) -> Message {
    Message::text(format!("{{\"id\":{id},\"ok\":true,\"out\":{out_json}}}"))
}

/// A `file.share` upload moves the caller's registered bytes to the daemon:
/// OPEN → CREDIT → contiguous DATA covering the source → CREDIT(total) →
/// END → terminal reply, with the daemon verifying the byte content equals
/// the registered source and the offsets stay contiguous.
#[test]
fn file_share_round_trips_bytes_over_the_socket() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        // 200_000 bytes: spans four codec-capped DATA chunks (65_536 each).
        let payload: Vec<u8> = (0..200_000_u32).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();

        let server = tokio::spawn(async move {
            use futures::SinkExt;
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::text(hello_json(sg))).await.expect("hello");

            let (id, op, input) = read_request(&mut ws).await;
            assert_eq!(op, "file.share");
            let total = input["declared_bytes"].as_u64().expect("declared_bytes");
            assert_eq!(total, expected.len() as u64);

            let identity = StreamIdentity::new(id, 1).expect("identity");
            let bounds = CodecBounds::default();
            ws.send(frame(identity, StreamRecordBody::Open { total }))
                .await
                .expect("OPEN");
            ws.send(frame(
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: total,
                },
            ))
            .await
            .expect("CREDIT");

            // Read contiguous DATA until the whole file arrived, then
            // acknowledge everything so the client may finalize.
            let mut received: Vec<u8> = Vec::new();
            while (received.len() as u64) < total {
                let record = match ws.next().await {
                    Some(Ok(Message::Binary(bytes))) => {
                        decode_stream_record(&bytes, &bounds).expect("client record decodes")
                    }
                    Some(Ok(Message::Ping(_))) => continue,
                    other => panic!("expected DATA, got {other:?}"),
                };
                match record.body {
                    StreamRecordBody::Data { offset, payload } => {
                        assert_eq!(
                            offset,
                            received.len() as u64,
                            "the DATA offset/len sequence must be contiguous from zero"
                        );
                        received.extend_from_slice(&payload);
                    }
                    other => panic!("expected DATA before END, got {other:?}"),
                }
            }
            assert_eq!(received, expected, "the served bytes equal the source");

            // Acknowledge the full file: the client's END waits for this.
            ws.send(frame(
                identity,
                StreamRecordBody::Credit {
                    accepted_through: total,
                    send_through: total,
                },
            ))
            .await
            .expect("final CREDIT");

            // The client's END carries its sent total.
            let end = loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(bytes))) => {
                        break decode_stream_record(&bytes, &bounds).expect("END decodes");
                    }
                    Some(Ok(Message::Ping(_))) => continue,
                    other => panic!("expected END, got {other:?}"),
                }
            };
            assert_eq!(end.body, StreamRecordBody::End { total });

            let out = serde_json::json!({
                "room_id": "room-share",
                "file_id": "file-1",
                "event_id": "event-1",
                "pos": 1,
                "bytes": total,
                "digest": "sha256:test",
            });
            ws.send(reply_text(id, out.to_string()))
                .await
                .expect("reply");
            // Hold the connection until the client stops.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        wait_for_state(&handle, State::Ready, 2_000).await;

        let key = OpId::new("share-op-1");
        handle
            .register_stream_media(key.clone(), shared_bytes(payload))
            .expect("register");
        let mut call = handle.call_stream::<jeliya_api::FileShare>(
            jeliya_api::FileShare {
                room_id: RoomId::new("room-share"),
                name: String::from("payload.bin"),
                declared_bytes: 200_000,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(key),
        );
        let out = tokio::time::timeout(Duration::from_millis(5_000), &mut call)
            .await
            .expect("the upload resolves")
            .expect("the upload succeeds");
        assert_eq!(out.bytes, 200_000);
        assert_eq!(out.room_id.as_str(), "room-share");

        handle.stop().await;
        server.abort();
    });
}

/// A `file.read` download moves the daemon's served bytes into the caller's
/// registered sink: OPEN → client CREDIT → DATA covering the file → END →
/// terminal reply, with the collected bytes equalling the served bytes
/// exactly.
#[test]
fn file_read_round_trips_bytes_over_the_socket() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        let served: Vec<u8> = (0..150_000_u32)
            .map(|i| ((i * 7 + 3) % 253) as u8)
            .collect();
        let expected = served.clone();

        let server = tokio::spawn(async move {
            use futures::SinkExt;
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::text(hello_json(sg))).await.expect("hello");

            let (id, op, _) = read_request(&mut ws).await;
            assert_eq!(op, "file.read");
            let total = served.len() as u64;

            let identity = StreamIdentity::new(id, 1).expect("identity");
            let bounds = CodecBounds::default();
            ws.send(frame(identity, StreamRecordBody::Open { total }))
                .await
                .expect("OPEN");

            // The client's opening CREDIT must arrive before any DATA.
            let credit = loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(bytes))) => {
                        break decode_stream_record(&bytes, &bounds).expect("CREDIT decodes");
                    }
                    Some(Ok(Message::Ping(_))) => continue,
                    other => panic!("expected CREDIT, got {other:?}"),
                }
            };
            match credit.body {
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through,
                } if send_through == total => {}
                other => panic!("expected CREDIT(0, {total}), got {other:?}"),
            }

            // Serve the file in codec-capped chunks, then END.
            let mut offset: u64 = 0;
            while offset < total {
                let end = (offset + 65_536).min(total);
                ws.send(frame(
                    identity,
                    StreamRecordBody::Data {
                        offset,
                        payload: served[offset as usize..end as usize].to_vec(),
                    },
                ))
                .await
                .expect("DATA");
                offset = end;
            }
            ws.send(frame(identity, StreamRecordBody::End { total }))
                .await
                .expect("END");

            let out = serde_json::json!({
                "room_id": "room-read",
                "file_id": "file-9",
                "bytes": total,
                "declared_content_type": "application/octet-stream",
            });
            ws.send(reply_text(id, out.to_string()))
                .await
                .expect("reply");
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        wait_for_state(&handle, State::Ready, 2_000).await;

        let (sink, media) = collected_bytes();
        let key = OpId::new("read-op-1");
        handle
            .register_stream_media(key.clone(), media)
            .expect("register");
        let mut call = handle.call_stream::<jeliya_api::FileRead>(
            jeliya_api::FileRead {
                room_id: RoomId::new("room-read"),
                file_id: FileId::new("file-9"),
            },
            Dedup::Key(key),
        );
        let out = tokio::time::timeout(Duration::from_millis(5_000), &mut call)
            .await
            .expect("the download resolves")
            .expect("the download succeeds");
        assert_eq!(out.bytes, 150_000);

        let collected = sink.take();
        assert_eq!(
            collected, expected,
            "the collected bytes equal the served bytes exactly"
        );

        handle.stop().await;
        server.abort();
    });
}

/// A stream dispatched with **no** registered media aborts honestly: the
/// first `ProduceData` grant reports `SourceFailed`, the kernel authors a
/// `source_failed` ABORT the daemon can observe on the wire, and the call
/// settles with a real error (the kernel's `Timeout` classification) —
/// never a hang and never a fake success.
#[test]
fn unregistered_stream_aborts_honestly_with_an_on_wire_abort() {
    let rt = test_runtime();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let sg = 1_u64;
        let url = url::Url::parse(&format!("ws://127.0.0.1:{port}/ws?v=2&sg={sg}")).expect("url");

        let server = tokio::spawn(async move {
            use futures::SinkExt;
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::text(hello_json(sg))).await.expect("hello");

            let (id, op, _) = read_request(&mut ws).await;
            assert_eq!(op, "file.share");
            let identity = StreamIdentity::new(id, 1).expect("identity");
            let bounds = CodecBounds::default();
            ws.send(frame(identity, StreamRecordBody::Open { total: 16 }))
                .await
                .expect("OPEN");
            ws.send(frame(
                identity,
                StreamRecordBody::Credit {
                    accepted_through: 0,
                    send_through: 16,
                },
            ))
            .await
            .expect("CREDIT");

            // The honest abort must cross the wire before any timeout.
            let abort = loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(bytes))) => {
                        break decode_stream_record(&bytes, &bounds).expect("ABORT decodes");
                    }
                    Some(Ok(Message::Ping(_))) => continue,
                    other => panic!("expected ABORT, got {other:?}"),
                }
            };
            assert_eq!(
                abort.body,
                StreamRecordBody::Abort {
                    accepted_through: 0,
                    reason: BinaryAbortReason::SourceFailed,
                },
                "an unregistered producer aborts source_failed at its first grant"
            );
            // The late terminal reply is absorbed by the settled tombstone.
            let err = serde_json::json!({
                "code": "unavailable",
            });
            ws.send(Message::text(format!(
                "{{\"id\":{id},\"ok\":false,\"err\":{err}}}"
            )))
            .await
            .expect("err reply");
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let handle = connect_ws_native(FixedSource { url }, fast_config()).expect("creates");
        handle.start();
        wait_for_state(&handle, State::Ready, 2_000).await;

        // No register_stream_media: the call is dispatched bare.
        let mut call = handle.call_stream::<jeliya_api::FileShare>(
            jeliya_api::FileShare {
                room_id: RoomId::new("room-bare"),
                name: String::from("none.bin"),
                declared_bytes: 16,
                declared_content_type: String::from("application/octet-stream"),
            },
            Dedup::Key(OpId::new("bare-op-1")),
        );
        let error = tokio::time::timeout(Duration::from_millis(5_000), &mut call)
            .await
            .expect("the bare stream settles promptly, never hangs")
            .expect_err("an unregistered stream must fail, not succeed");
        assert!(
            matches!(error, CallError::Timeout),
            "the kernel settles an unregistered stream's abort as Timeout; got {error}"
        );

        handle.stop().await;
        server.abort();
    });
}
