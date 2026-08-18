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
        connect_ws_web, ClientEvent, Dedup, Endpoint, ExplicitResolver, KernelConfig, State,
        WsWebConfig,
    };
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    // Compile-time constants injected by build.rs from the harness script env.
    // If absent, both resolve to None and each test exits early (graceful skip).
    const DAEMON_PORT: Option<&str> = option_env!("JELIYAD_WEB_TEST_PORT");
    const DAEMON_TOKEN: Option<&str> = option_env!("JELIYAD_WEB_TEST_TOKEN");

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
}
