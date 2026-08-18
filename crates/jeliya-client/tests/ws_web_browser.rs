// Real-browser integration tests for the WsWeb adapter (#171 §9.2, AC-6).
//
// These tests run in headless Chromium via wasm-bindgen-test. They cross the
// browser↔daemon boundary: an actual jeliyad instance serves the WebSocket;
// the WASM adapter completes the full dial sequence (GET /api/health, open WS,
// validate hello, reach Ready) and dispatches a real operation.
//
// Graceful skip when the daemon coordinates are absent: if the harness script
// did not set JELIYAD_WEB_TEST_PORT before compiling, `option_env!` resolves to
// None and each test returns early (wasm-bindgen-test counts an early return as
// pass, not fail). The CI job that supplies the env vars is non-required
// initially, matching the jeliya-ui-web precedent (#176 §9.3).
//
// To run locally:
//   JELIYAD_BIN=./target/debug/jeliyad bash scripts/run-ws-web-browser-tests.sh

#[cfg(target_arch = "wasm32")]
mod tests {
    use futures::StreamExt;
    use jeliya_client::{
        connect_ws_web, media, ClientEvent, ClientHandle, Dedup, Endpoint, ExplicitResolver,
        KernelConfig, State, WsWebConfig,
    };
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    // Compile-time constants injected by build.rs from the harness script env.
    // If absent, both resolve to None and each test exits early (graceful skip).
    const DAEMON_PORT: Option<&str> = option_env!("JELIYAD_WEB_TEST_PORT");
    const DAEMON_TOKEN: Option<&str> = option_env!("JELIYAD_WEB_TEST_TOKEN");
    const DAEMON2_PORT: Option<&str> = option_env!("JELIYAD2_WEB_TEST_PORT");
    const DAEMON2_TOKEN: Option<&str> = option_env!("JELIYAD2_WEB_TEST_TOKEN");

    /// Yield to the event loop for `ms` so the adapter's spawn_local pump can
    /// drain socket traffic between poll attempts (a busy loop on the one
    /// thread would starve the very IO it is waiting for).
    async fn sleep_ms(ms: u32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            web_sys::window()
                .expect("window")
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32)
                .expect("set_timeout");
        });
        wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .expect("timer promise");
    }

    fn make_config() -> Option<WsWebConfig> {
        let port = DAEMON_PORT?;
        let token = DAEMON_TOKEN?;
        Some(WsWebConfig {
            endpoint: Endpoint::Explicit {
                http_base: format!("http://127.0.0.1:{port}"),
                ws_url: format!("ws://127.0.0.1:{port}/ws"),
            },
            // ExplicitResolver: pre-supplies the bearer token read from the
            // daemon's portfile. Avoids a cross-origin /api/session fetch while
            // still exercising the full health-check → dial → hello path.
            session: Box::new(ExplicitResolver::new("token", token.to_string())),
            kernel: KernelConfig::default(),
            // 8-second hello deadline: generous enough for CI without hanging
            // indefinitely when the daemon is unreachable.
            hello_timeout_ms: 8_000,
        })
    }

    /// Reach Ready against the live daemon or give up (the earlier connect
    /// test owns the failure diagnosis).
    async fn ready_handle() -> Option<(ClientHandle, jeliya_client::EventSubscription)> {
        let config = make_config()?;
        let handle = connect_ws_web(config);
        let mut sub = handle.subscribe();
        handle.start();
        if !wait_for_state(&mut sub, State::Ready).await {
            return None;
        }
        Some((handle, sub))
    }

    // Poll the EventSubscription until the target state or a terminal state.
    // Returns true if `wanted` was reached, false if `State::Failed` was
    // observed first (unreachable daemon, protocol mismatch, gate refused, …).
    async fn wait_for_state(sub: &mut jeliya_client::EventSubscription, wanted: State) -> bool {
        while let Some(event) = sub.next().await {
            match event {
                ClientEvent::StateChanged { to, .. } if to == wanted => return true,
                ClientEvent::StateChanged {
                    to: State::Failed, ..
                } => return false,
                _ => {}
            }
        }
        false
    }

    // §9.2 scenario 1: Dial → hello → Ready.
    //
    // Proves the complete dial sequence across the real system boundary:
    //   1. GET /api/health (protocol validation, sg extraction)
    //   2. WebSocket open with credential in query string
    //   3. First-frame hello validated (protocol + sg must match)
    //   4. Input::Connected feeds the kernel → State::Ready
    #[wasm_bindgen_test]
    async fn initial_connect_reaches_ready() {
        let Some(config) = make_config() else {
            return; // no daemon configured — graceful skip
        };
        let handle = connect_ws_web(config);
        let mut sub = handle.subscribe();
        handle.start();
        assert!(
            wait_for_state(&mut sub, State::Ready).await,
            "WsWeb adapter must reach State::Ready after a valid hello; \
             State::Failed means the daemon was unreachable or the protocol \
             version did not match — check that jeliyad is running on \
             JELIYAD_WEB_TEST_PORT with a compatible protocol version"
        );
    }

    // §9.2 scenario 2: Ready → dispatch room.list → call resolves.
    //
    // Proves a full request round-trip over the live WebSocket. The daemon may
    // respond with Ok (rooms list) or a typed ApiError (e.g. SubjectAbsent when
    // no pairing has happened). Either is a protocol-correct reply that proves
    // the encode→transmit→receive→decode→settle path works end-to-end.
    #[wasm_bindgen_test]
    async fn room_list_call_resolves_when_ready() {
        let Some(config) = make_config() else {
            return;
        };
        let handle = connect_ws_web(config);
        let mut sub = handle.subscribe();
        handle.start();
        if !wait_for_state(&mut sub, State::Ready).await {
            return; // daemon unreachable — already caught by the prior test
        }
        // Dispatch room.list. The call must resolve (Ok or ApiError) rather than
        // hanging — this proves encode_request → ws.send → ws.onmessage →
        // decode_client_frame → kernel settle all work end-to-end.
        let _ = handle
            .call::<jeliya_api::RoomList>(jeliya_api::RoomList {}, Dedup::None)
            .await;
        // Any resolution is acceptable; a hung future would time out the test
        // runner and fail the suite, which is the right signal.
    }

    // §9.2 scenario 5: stop from Ready → State::Stopped; event stream closes.
    //
    // Proves the kernel's stop path drives the adapter to teardown the WebSocket
    // and deliver the terminal state observable. After stop() the EventSubscription
    // stream must close (no more items).
    #[wasm_bindgen_test]
    async fn stop_from_ready_reaches_stopped_and_closes_stream() {
        let Some(config) = make_config() else {
            return;
        };
        let handle = connect_ws_web(config);
        let mut sub = handle.subscribe();
        handle.start();
        if !wait_for_state(&mut sub, State::Ready).await {
            return;
        }
        handle.stop().await;
        assert_eq!(
            handle.state(),
            State::Stopped,
            "adapter must be in Stopped state after stop()"
        );
        // The kernel's stop contract emits the terminal Stopping/Stopped
        // transitions BEFORE CloseBus (core.rs:511-513), so those events are
        // legitimately queued. What must not happen is anything after Stopped:
        // drain to the close and assert the stream ENDS at Stopped.
        let mut last_to = None;
        while let Some(ClientEvent::StateChanged { to, .. }) = sub.next().await {
            last_to = Some(to);
        }
        assert_eq!(
            last_to,
            Some(State::Stopped),
            "the event stream must end at Stopped — an event after it (or a \
             non-StateChanged tail) indicates a leaked post-teardown emission"
        );
    }

    // Byte-stream media against the LIVE daemon (#233 client remainder / the
    // adapter stream-media slice). Unlike every suite above, these two tests
    // move real file bytes over the socket: `file.share` uploads a
    // deterministic pattern from a registered ByteSource (OPEN → CREDIT →
    // DATA* → END → terminal reply, framed as JBS2 Binary messages), and
    // `file.read` downloads them back into a registered ByteSink (OPEN →
    // client CREDIT → DATA* → END → terminal reply) with the collected bytes
    // verified against the exact pattern — a real digest-level round trip,
    // never the mock.
    //
    // Bootstrap mirrors the conformance corpus's files success case
    // (conformance/v2/files.json): subject.ensure → room.create →
    // room.activate, then the streamed share.

    /// The deterministic upload pattern (not `i mod 251` — that rig is the
    /// in-memory driver's; a live round trip deserves an independent pattern).
    fn share_pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + 13) % 256) as u8).collect()
    }

    async fn ensure_subject_and_live_room(handle: &ClientHandle) -> jeliya_api::RoomId {
        // The subject is global to the daemon; ensure is idempotent.
        let _ = handle
            .call::<jeliya_api::SubjectEnsure>(jeliya_api::SubjectEnsure {}, Dedup::None)
            .await
            .expect("subject.ensure resolves against the live daemon");
        let room = handle
            .call::<jeliya_api::RoomCreate>(
                jeliya_api::RoomCreate {
                    name: String::from("ws-web-stream-media"),
                },
                Dedup::None,
            )
            .await
            .expect("room.create resolves");
        handle
            .call::<jeliya_api::RoomActivate>(
                jeliya_api::RoomActivate {
                    room_id: room.room_id.clone(),
                },
                Dedup::None,
            )
            .await
            .expect("room.activate resolves");
        room.room_id
    }

    #[wasm_bindgen_test]
    async fn live_file_share_uploads_real_bytes_over_the_socket() {
        let Some((handle, _sub)) = ready_handle().await else {
            return;
        };
        let room_id = ensure_subject_and_live_room(&handle).await;

        let pattern = share_pattern(2048);
        let op_id = jeliya_api::OpId::new("ws-web-live-share-1");
        handle
            .register_stream_media(op_id.clone(), media::shared_bytes(pattern.clone()))
            .expect("the web adapter registers stream media");

        let out = handle
            .call_stream::<jeliya_api::FileShare>(
                jeliya_api::FileShare {
                    room_id: room_id.clone(),
                    name: String::from("live-share.bin"),
                    declared_bytes: pattern.len() as u64,
                    declared_content_type: String::from("application/octet-stream"),
                },
                Dedup::Key(op_id),
            )
            .await
            .expect("the streamed file.share resolves against the live daemon");
        assert_eq!(out.bytes, 2048, "every pattern byte was accepted");
        assert!(!out.digest.is_empty(), "the daemon reports its digest");
    }

    // The two-daemon byte-stream matrix (the honest live `file.read`):
    // `file.read` streams only LOCALLY-HELD bytes, and a daemon never fetches
    // from itself (the supervisor's fetch excludes its own device), so a
    // single daemon can never serve a read-back of its own upload — by
    // design, exactly as the corpus records (`resource:fetched_file` is
    // "unestablishable single-subject"). The real download needs a second,
    // invited daemon: A receives the streamed share; B joins the room, p2p-
    // fetches the bytes from provider A, and streams them back out over its
    // own socket. The collected bytes must equal the uploaded pattern
    // exactly — a digest-level round trip across two live daemons and three
    // byte transfers (browser→A upload, A→B p2p fetch, B→browser download).
    #[wasm_bindgen_test]
    async fn live_two_daemon_share_fetch_read_round_trip() {
        let (Some(port2), Some(token2)) = (DAEMON2_PORT, DAEMON2_TOKEN) else {
            return; // no second daemon configured — graceful skip
        };
        let Some((ha, _sub_a)) = ready_handle().await else {
            return;
        };
        let hb = connect_ws_web(WsWebConfig {
            endpoint: Endpoint::Explicit {
                http_base: format!("http://127.0.0.1:{port2}"),
                ws_url: format!("ws://127.0.0.1:{port2}/ws"),
            },
            session: Box::new(ExplicitResolver::new("token", token2.to_string())),
            kernel: KernelConfig::default(),
            hello_timeout_ms: 8_000,
        });
        let mut sub_b = hb.subscribe();
        hb.start();
        if !wait_for_state(&mut sub_b, State::Ready).await {
            return; // second daemon unreachable — its own connect test's domain
        }

        // A: subject + live room; B: subject; A mints, B redeems, B activates.
        let room_id = ensure_subject_and_live_room(&ha).await;
        let subject_b = hb
            .call::<jeliya_api::SubjectEnsure>(jeliya_api::SubjectEnsure {}, Dedup::None)
            .await
            .expect("B subject.ensure")
            .subject_id;
        let minted = ha
            .call::<jeliya_api::InviteMint>(
                jeliya_api::InviteMint {
                    room_id: room_id.clone(),
                    subject_id: subject_b,
                    role: jeliya_api::Role::Member,
                    // `time`'s `now` is unsupported on wasm32-unknown-unknown;
                    // the browser clock (ms since the Unix epoch) is the
                    // platform's own source for an expiry.
                    expires_at: jeliya_api::Timestamp::new(
                        time::OffsetDateTime::from_unix_timestamp_nanos(
                            (js_sys::Date::now() as i128 + 3_600_000) * 1_000_000,
                        )
                        .expect("unix timestamp"),
                    ),
                },
                Dedup::None,
            )
            .await
            .expect("A invite.mint");
        hb.call::<jeliya_api::InviteRedeem>(
            jeliya_api::InviteRedeem {
                capability: minted.capability,
            },
            Dedup::None,
        )
        .await
        .expect("B invite.redeem");
        hb.call::<jeliya_api::RoomActivate>(
            jeliya_api::RoomActivate {
                room_id: room_id.clone(),
            },
            Dedup::None,
        )
        .await
        .expect("B room.activate");

        // A receives the streamed share.
        let pattern = share_pattern(1536);
        let share_op = jeliya_api::OpId::new("ws-web-two-daemon-share-1");
        ha.register_stream_media(share_op.clone(), media::shared_bytes(pattern.clone()))
            .expect("register the share source");
        let shared = ha
            .call_stream::<jeliya_api::FileShare>(
                jeliya_api::FileShare {
                    room_id: room_id.clone(),
                    name: String::from("live-roundtrip.bin"),
                    declared_bytes: pattern.len() as u64,
                    declared_content_type: String::from("application/octet-stream"),
                },
                Dedup::Key(share_op),
            )
            .await
            .expect("the streamed share resolves on A");
        assert_eq!(shared.bytes, pattern.len() as u64);

        // B fetches from provider A — bounded wait: the room session must
        // sync the signed file_shared event and connect the daemons first.
        let mut fetched = false;
        for _ in 0..60 {
            match hb
                .call::<jeliya_api::FileFetch>(
                    jeliya_api::FileFetch {
                        room_id: room_id.clone(),
                        file_id: shared.file_id.clone(),
                    },
                    Dedup::None,
                )
                .await
            {
                Ok(out) => {
                    assert_eq!(out.bytes, pattern.len() as u64);
                    fetched = true;
                    break;
                }
                Err(_) => {
                    sleep_ms(2_000).await;
                }
            }
        }
        assert!(fetched, "B must fetch the file from provider A");

        // B streams the bytes back out; the sink must collect the exact pattern.
        let (sink, sink_media) = media::collected_bytes();
        let read_op = jeliya_api::OpId::new("ws-web-two-daemon-read-1");
        hb.register_stream_media(read_op.clone(), sink_media)
            .expect("register the read sink");
        let header = hb
            .call_stream::<jeliya_api::FileRead>(
                jeliya_api::FileRead {
                    room_id,
                    file_id: shared.file_id,
                },
                Dedup::Key(read_op),
            )
            .await
            .expect("the streamed file.read resolves on B");
        assert_eq!(header.bytes, pattern.len() as u64);
        let collected = sink.take();
        assert_eq!(
            collected, pattern,
            "the bytes the browser collected from B are exactly the bytes it              uploaded to A — a real round trip over two live daemons, not the mock"
        );
    }
}
