//! The native WebSocket state machine: dial → generation/hello agreement →
//! serve → reconnect → stop, framed through the protocol-v2 codec (#164).
//!
//! One `run_dial` future per `Action::Dial` performs, in order: a fresh resolve
//! (every attempt), the authenticated upgrade, the **three** agreement checks
//! (resolver health, the daemon's upgrade gate, and a matching `hello`
//! generation) that make "connected before protocol agreement" impossible, then
//! the per-connection read/write loops. Every failure maps to exactly one
//! token- or generation-fenced core input; nothing here re-implements the
//! resync logic — a loss simply produces the correct `Interrupted` and the core
//! (and #169's reconciler) do the rest.

use std::sync::{Arc, Mutex, Weak};

use futures::{SinkExt, StreamExt};
use jeliya_api::RequestId;
use jeliya_codec::{
    decode_client_frame, decode_stream_identity, decode_stream_kind, decode_stream_record_view,
    ClientFrame, CodecBounds, StreamRecordBodyView,
};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};

use crate::backend::RawJson;
use crate::kernel::core::Input;
use crate::kernel::transport::{Inbound, StreamRecordMeta, WireReply};
use crate::kernel::Runtime;

use super::media::map_abort_reason;
use super::runtime::{ConnRegistry, Deadlines};
use super::source::{Dial, DialResolveError, TargetSource};

/// Run one dial attempt to completion, injecting exactly one lifecycle input.
pub(crate) async fn run_dial(
    token: u64,
    runtime: Weak<Runtime>,
    source: Arc<dyn TargetSource>,
    conn: Arc<Mutex<ConnRegistry>>,
    deadlines: Deadlines,
    write_buffer: usize,
) {
    // 1. Resolve — runs on EVERY attempt, so a restart's new port/token/PID
    //    heals transparently and a changed generation fails closed.
    let dial = match source.resolve().await {
        Ok(dial) => dial,
        Err(DialResolveError::Terminal(_)) => {
            // Wrong daemon / attack shape: fail closed, no auto-retry.
            inject(&runtime, Input::GateRefused { token });
            return;
        }
        Err(DialResolveError::Transient(_)) => {
            inject(&runtime, Input::DialFailed { token });
            return;
        }
    };

    // The expected generation gate travels on the (verified, token-free) URL
    // the supervisor built (`?v=&sg=`); the `hello` must match it exactly.
    let Some((expected_protocol, expected_sg)) = parse_gate(&dial.url) else {
        inject(&runtime, Input::DialFailed { token });
        return;
    };

    // 2. Build the authenticated upgrade request (the ONLY place the bearer is
    //    exposed) and dial, bounded by the connect deadline.
    let request = match build_request(&dial) {
        Ok(request) => request,
        // A request-build failure is a dial-URL/header bug: fail closed.
        Err(()) => {
            inject(&runtime, Input::GateRefused { token });
            return;
        }
    };
    // The transport must accept every CONFORMING frame: tungstenite's small
    // defaults (16 MiB frame / 64 MiB message) would kill the connection
    // before the negotiated post-hello bounds — never larger than the codec
    // ceiling — can enforce the real limit. The daemon configures its own
    // acceptor to the served `max_frame_bytes`; mirror that ceiling here.
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(CodecBounds::default().max_frame_bytes);
    config.max_frame_size = Some(CodecBounds::default().max_frame_bytes);
    let ws = match timeout(
        deadlines.connect,
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    {
        Ok(Ok((ws, _response))) => ws,
        Ok(Err(error)) => {
            inject(&runtime, classify_handshake(token, &error));
            return;
        }
        Err(_elapsed) => {
            inject(&runtime, Input::DialFailed { token });
            return;
        }
    };

    // 3. The hello gate — never "connected" before a matching hello. Read the
    //    first Text message, bounded by the hello deadline.
    let mut ws = ws;
    let hello_bytes = match timeout(deadlines.hello, ws.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        Ok(Some(Ok(Message::Close(frame)))) => {
            // An application close during the handshake (`4001`/`4006`) is a
            // terminal gate refusal; anything else is transient.
            let code = frame.map(|frame| u16::from(frame.code));
            inject(&runtime, close_input(token, code));
            return;
        }
        // A non-Text first frame is not a hello → fail closed.
        Ok(Some(Ok(_))) => {
            inject(&runtime, Input::GateRefused { token });
            return;
        }
        // Dropped before hello, a read error, or a timeout → transient.
        Ok(Some(Err(_))) | Ok(None) | Err(_) => {
            inject(&runtime, Input::DialFailed { token });
            return;
        }
    };

    let hello = match decode_client_frame(hello_bytes.as_str().as_bytes(), &CodecBounds::default())
    {
        Ok(ClientFrame::Hello(hello))
            if hello.protocol == expected_protocol && hello.storage_generation == expected_sg =>
        {
            hello
        }
        // A hello with the wrong generation, or a non-hello first frame, is the
        // third independent agreement check refusing: fail closed.
        Ok(_) => {
            inject(&runtime, Input::GateRefused { token });
            return;
        }
        Err(_) => {
            inject(&runtime, Input::DialFailed { token });
            return;
        }
    };

    // 4. Serve. Bound inbound frames by the codec ceiling AND the served
    //    `max_frame_bytes` from the hello (never larger than advertised).
    let bounds = CodecBounds {
        max_frame_bytes: (CodecBounds::default().max_frame_bytes as u64)
            .min(hello.limits.max_frame_bytes) as usize,
        ..CodecBounds::default()
    };
    let (sink, stream) = ws.split();
    let (write_tx, write_rx) = tokio::sync::mpsc::channel::<Message>(write_buffer);

    // The daemon closes a connection that sends nothing within the served
    // `idle_timeout_ms` (4004) and resets that deadline ONLY for inbound
    // client frames — so even a push-busy but request-idle client must
    // produce activity. Ping at half the served timeout (bounded below so a
    // pathological served value cannot spin); the task dies with the
    // connection through the same registry, and its sends simply fail once
    // the writer is gone.
    let keepalive_task = if hello.limits.idle_timeout_ms > 0 {
        let period =
            std::time::Duration::from_millis((hello.limits.idle_timeout_ms / 2).max(1_000));
        Some(tokio::spawn(keepalive_loop(write_tx.clone(), period)))
    } else {
        None
    };

    // Install the writer BEFORE injecting Connected, so the flush of any
    // replay-held/queued calls the core issues on `Connected` reaches it.
    {
        let mut registry = conn.lock().expect("conn registry poisoned");
        registry.tear_down();
        registry.writer = Some(write_tx);
    }

    // Injecting Connected bumps the generation (or is dropped if the dial was
    // retired by a stop). Read the generation across the injection to learn
    // which happened and to tag this connection's inbound frames.
    let Some(previous) = runtime.upgrade().map(|runtime| runtime.generation()) else {
        return;
    };
    inject(
        &runtime,
        Input::Connected {
            token,
            incarnation: hello.incarnation.clone(),
        },
    );
    let Some(generation) = runtime.upgrade().map(|runtime| runtime.generation()) else {
        return;
    };
    if generation == previous {
        // Connected was dropped (stop/retired dial): tear down, close the
        // socket best-effort, and let this attempt end.
        conn.lock().expect("conn registry poisoned").tear_down();
        let mut sink = sink;
        let _ = sink.close().await;
        return;
    }

    // Only now, with a live generation, spawn the per-connection tasks.
    let write_task = tokio::spawn(write_loop(
        sink,
        write_rx,
        generation,
        runtime.clone(),
        conn.clone(),
    ));
    let read_task = tokio::spawn(read_loop(
        stream,
        generation,
        bounds,
        runtime.clone(),
        conn.clone(),
    ));
    {
        let mut registry = conn.lock().expect("conn registry poisoned");
        registry.generation = generation;
        registry.read_task = Some(read_task);
        registry.write_task = Some(write_task);
        registry.keepalive_task = keepalive_task;
    }
}

/// Send one Ping per `period` until the write channel is gone (the connection
/// was torn down — the read side already reported the loss). Ping/Pong rides
/// tungstenite's own framing; the daemon counts any inbound frame as
/// activity, which is what keeps an otherwise idle connection under its
/// served `idle_timeout_ms` alive.
async fn keepalive_loop(tx: tokio::sync::mpsc::Sender<Message>, period: std::time::Duration) {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires immediately; skip it — the handshake itself was
    // just activity.
    interval.tick().await;
    loop {
        interval.tick().await;
        if tx.send(Message::Ping(Bytes::new())).await.is_err() {
            return;
        }
    }
}

/// Drain the write channel to the socket; a write error is a connection loss.
async fn write_loop(
    mut sink: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    mut write_rx: tokio::sync::mpsc::Receiver<Message>,
    generation: u64,
    runtime: Weak<Runtime>,
    conn: Arc<Mutex<ConnRegistry>>,
) {
    while let Some(message) = write_rx.recv().await {
        if sink.send(message).await.is_err() {
            clear_writer_if_current(&conn, generation);
            inject(&runtime, Input::Interrupted { generation });
            return;
        }
    }
    // The channel closed because the writer was dropped (the read task saw the
    // loss and cleared it): nothing to report — it already injected.
}

/// Read inbound frames, decode them through the client-direction codec, and
/// feed the kernel — every frame tagged with THIS connection's generation, so a
/// straggler from a replaced connection is fenced by the core (§K7).
async fn read_loop(
    mut stream: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    generation: u64,
    bounds: CodecBounds,
    runtime: Weak<Runtime>,
    conn: Arc<Mutex<ConnRegistry>>,
) {
    while let Some(item) = stream.next().await {
        match item {
            Ok(Message::Text(text)) => {
                match decode_client_frame(text.as_str().as_bytes(), &bounds) {
                    Ok(ClientFrame::Reply { id, result }) => {
                        let Ok(request_id) = RequestId::new(id) else {
                            // An out-of-range id correlates to nothing.
                            inject(&runtime, Input::Inbound(Inbound::Malformed));
                            continue;
                        };
                        // A stream's terminal reply prunes its media state
                        // (task, quarantine buffer) — cheap when the id binds
                        // no stream (§S10 retire-on-reply).
                        conn.lock()
                            .expect("conn registry poisoned")
                            .media
                            .prune(request_id);
                        let result = match result {
                            Ok(text) => WireReply::Ok(RawJson::from_string(text)),
                            Err(api) => WireReply::Err(api),
                        };
                        inject(
                            &runtime,
                            Input::Inbound(Inbound::Reply {
                                generation,
                                id: request_id,
                                result,
                            }),
                        );
                    }
                    Ok(ClientFrame::Push(push)) => {
                        inject(&runtime, Input::Inbound(Inbound::Push { generation, push }));
                    }
                    // A SECOND hello is a protocol violation → interrupt + close.
                    Ok(ClientFrame::Hello(_)) => {
                        clear_writer_if_current(&conn, generation);
                        inject(&runtime, Input::Interrupted { generation });
                        return;
                    }
                    // A frame that parses to no envelope, or is over the ceiling,
                    // strands nothing (§K4).
                    Ok(ClientFrame::Malformed(_)) | Err(_) => {
                        inject(&runtime, Input::Inbound(Inbound::Malformed));
                    }
                }
            }
            // Binary = a byte-stream record (§S3/§S10): staged decode per
            // the codec's documented order — identity, registry binding
            // lookup, kind, full structural view — then the byte-free meta
            // reaches the core. A codec-fatal or unbindable record strands
            // nothing: `Inbound::Malformed`, exactly as for text (§K4).
            Ok(Message::Binary(bytes)) => {
                match decode_inbound_stream_record(&conn, generation, &bounds, &runtime, &bytes) {
                    Ok(record) => inject(
                        &runtime,
                        Input::Inbound(Inbound::Record { generation, record }),
                    ),
                    Err(()) => inject(&runtime, Input::Inbound(Inbound::Malformed)),
                }
            }
            // Ping/Pong are handled by tungstenite; ignore them here.
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) | Err(_) => {
                clear_writer_if_current(&conn, generation);
                inject(&runtime, Input::Interrupted { generation });
                return;
            }
        }
    }
    // Stream ended (EOF): a connection loss.
    clear_writer_if_current(&conn, generation);
    inject(&runtime, Input::Interrupted { generation });
}

/// Feed one input to the core through the shared shell, if it still exists.
fn inject(runtime: &Weak<Runtime>, input: Input) {
    if let Some(runtime) = runtime.upgrade() {
        runtime.inject(input);
    }
}

/// Clear the live writer only when the registry still names `generation`, so a
/// task tearing down cannot clobber a successor connection's writer. The
/// connection's media dies with it — a stream never survives its connection
/// (§S10), mirroring the core's own drain on `Interrupted`.
fn clear_writer_if_current(conn: &Arc<Mutex<ConnRegistry>>, generation: u64) {
    let mut registry = conn.lock().expect("conn registry poisoned");
    if registry.generation == generation {
        registry.writer = None;
        registry.media.clear();
    }
}

/// Decode one inbound Binary message into the byte-free record meta the core
/// consumes, performing the driver-side media work first (§S3/§S10):
///
/// 1. [`decode_stream_identity`] — codec-fatal (bad magic, short header,
///    over-ceiling) fails closed as `Malformed`;
/// 2. the media registry's binding lookup by request id — a record for a
///    wire id this connection never bound correlates to nothing;
/// 3. [`decode_stream_kind`] and [`decode_stream_record_view`] — the full
///    structural decode, still borrowing the payload;
/// 4. per-kind media work: OPEN spawns the source media task, DATA
///    quarantines its payload (a cap breach is protocol trouble →
///    `Malformed`) **before** the meta is handed onward, so the `WriteSink`
///    the meta provokes always finds its bytes buffered.
///
/// `Err(())` is the `Inbound::Malformed` signal; the generation tag is the
/// caller's (this connection's), so the core fences stragglers itself.
fn decode_inbound_stream_record(
    conn: &Arc<Mutex<ConnRegistry>>,
    _generation: u64,
    bounds: &CodecBounds,
    runtime: &Weak<Runtime>,
    bytes: &Bytes,
) -> Result<StreamRecordMeta, ()> {
    let identity = decode_stream_identity(bytes, bounds).map_err(|_| ())?;
    let wire_id = identity.request_id();
    let stream_id = identity.stream_id().get();
    let mut registry = conn.lock().expect("conn registry poisoned");
    if !registry.media.is_bound(wire_id) {
        return Err(());
    }
    decode_stream_kind(bytes, bounds).map_err(|_| ())?;
    let view = decode_stream_record_view(bytes, bounds).map_err(|_| ())?;
    match view.body {
        StreamRecordBodyView::Open { total } => {
            // The producer's media task starts at admission: it owns the
            // source and the grant channel until the terminal reply prunes
            // it or the connection dies.
            let writer = registry.writer.clone();
            if let Some(writer) = writer {
                registry.media.spawn_source_task(
                    wire_id,
                    stream_id,
                    &writer,
                    runtime.clone(),
                    *bounds,
                );
            }
            Ok(StreamRecordMeta::Open {
                id: wire_id,
                stream_id,
                total,
            })
        }
        StreamRecordBodyView::Data { offset, payload } => {
            if !registry.media.deliver_data(wire_id, offset, payload) {
                return Err(());
            }
            Ok(StreamRecordMeta::Data {
                id: wire_id,
                stream_id,
                offset,
                len: payload.len() as u64,
            })
        }
        StreamRecordBodyView::Credit {
            accepted_through,
            send_through,
        } => Ok(StreamRecordMeta::Credit {
            id: wire_id,
            stream_id,
            accepted_through,
            send_through,
        }),
        StreamRecordBodyView::End { total } => Ok(StreamRecordMeta::End {
            id: wire_id,
            stream_id,
            offset: total,
        }),
        StreamRecordBodyView::Abort {
            accepted_through,
            reason,
        } => Ok(StreamRecordMeta::Abort {
            id: wire_id,
            stream_id,
            high_water: accepted_through,
            reason: map_abort_reason(reason),
        }),
        StreamRecordBodyView::Ack { accepted_through } => Ok(StreamRecordMeta::Ack {
            id: wire_id,
            stream_id,
            high_water: accepted_through,
        }),
    }
}

/// Extract the `v`/`sg` generation gate the supervisor placed on the dial URL.
fn parse_gate(url: &url::Url) -> Option<(u64, u64)> {
    let mut protocol = None;
    let mut storage_generation = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "v" => protocol = value.parse().ok(),
            "sg" => storage_generation = value.parse().ok(),
            _ => {}
        }
    }
    Some((protocol?, storage_generation?))
}

/// Build the authenticated upgrade request. This is the ONE place the bearer is
/// exposed — as an `Authorization: Bearer` header, never a URL, log, or prop.
/// `Origin` is deliberately omitted on native (the daemon accepts its absence).
fn build_request(dial: &Dial) -> Result<http::Request<()>, ()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = dial.url.as_str().into_client_request().map_err(|_| ())?;
    let value =
        http::HeaderValue::from_str(&format!("Bearer {}", dial.bearer.expose())).map_err(|_| ())?;
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, value);
    Ok(request)
}

/// Classify a handshake failure by HTTP status (spec §7.2). `403`/`426` fail
/// closed (a loopback forbidden-origin should not happen, and a generation
/// mismatch is the reset path); `401` (token rotated on restart), `503`
/// (capacity), and a connect refusal/reset all heal on the next re-resolve.
fn classify_handshake(token: u64, error: &WsError) -> Input {
    let status = match error {
        WsError::Http(response) => Some(response.status().as_u16()),
        _ => None,
    };
    match status {
        Some(403) | Some(426) => Input::GateRefused { token },
        _ => Input::DialFailed { token },
    }
}

/// Map an application close code seen during the handshake to a core input:
/// `4001`/`4006` fail closed, anything else is transient.
fn close_input(token: u64, code: Option<u16>) -> Input {
    match code {
        Some(4001) | Some(4006) => Input::GateRefused { token },
        _ => Input::DialFailed { token },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gate_reads_v_and_sg() {
        let url = url::Url::parse("ws://127.0.0.1:7420/ws?v=2&sg=5").expect("url");
        assert_eq!(parse_gate(&url), Some((2, 5)));
    }

    #[test]
    fn parse_gate_missing_axis_is_none() {
        let url = url::Url::parse("ws://127.0.0.1:7420/ws?v=2").expect("url");
        assert_eq!(parse_gate(&url), None);
    }

    #[test]
    fn build_request_sets_a_redacted_bearer_header() {
        let dial = Dial {
            url: url::Url::parse("ws://127.0.0.1:7420/ws?v=2&sg=2").expect("url"),
            bearer: jeliya_supervisor::Redacted::new(String::from("sixtyfourhexsecret")),
        };
        let request = build_request(&dial).expect("builds");
        let auth = request
            .headers()
            .get(http::header::AUTHORIZATION)
            .expect("authorization header");
        assert_eq!(auth.to_str().expect("ascii"), "Bearer sixtyfourhexsecret");
        // The token is never placed in the URL.
        assert!(!request.uri().to_string().contains("sixtyfourhexsecret"));
    }

    #[test]
    fn handshake_status_table_fails_closed_on_403_and_426() {
        // A synthetic 426 response mirrors a generation-gate refusal.
        let response = http::Response::builder()
            .status(426)
            .body(None::<Vec<u8>>)
            .expect("response");
        let error = WsError::Http(Box::new(response));
        assert!(matches!(
            classify_handshake(7, &error),
            Input::GateRefused { token: 7 }
        ));
    }

    #[test]
    fn close_code_table_distinguishes_terminal_from_transient() {
        assert!(matches!(
            close_input(1, Some(4001)),
            Input::GateRefused { .. }
        ));
        assert!(matches!(
            close_input(1, Some(1000)),
            Input::DialFailed { .. }
        ));
        assert!(matches!(close_input(1, None), Input::DialFailed { .. }));
    }

    #[test]
    fn close_code_4006_is_terminal() {
        assert!(
            matches!(close_input(2, Some(4006)), Input::GateRefused { token: 2 }),
            "close 4006 is an application gate refusal and must fail closed"
        );
    }

    #[test]
    fn handshake_401_is_transient() {
        let response = http::Response::builder()
            .status(401)
            .body(None::<Vec<u8>>)
            .expect("response");
        let error = WsError::Http(Box::new(response));
        assert!(
            matches!(
                classify_handshake(5, &error),
                Input::DialFailed { token: 5 }
            ),
            "401 Unauthorized means the token rotated; the next re-resolve heals it"
        );
    }

    #[test]
    fn handshake_503_is_transient() {
        let response = http::Response::builder()
            .status(503)
            .body(None::<Vec<u8>>)
            .expect("response");
        let error = WsError::Http(Box::new(response));
        assert!(
            matches!(classify_handshake(5, &error), Input::DialFailed { .. }),
            "503 Service Unavailable is transient capacity; backoff and retry"
        );
    }

    #[test]
    fn handshake_403_is_terminal() {
        let response = http::Response::builder()
            .status(403)
            .body(None::<Vec<u8>>)
            .expect("response");
        let error = WsError::Http(Box::new(response));
        assert!(
            matches!(classify_handshake(7, &error), Input::GateRefused { token: 7 }),
            "403 Forbidden is a loopback-origin violation; fail closed (should not happen on loopback)"
        );
    }

    #[test]
    fn handshake_non_http_error_is_transient() {
        use tokio_tungstenite::tungstenite::error::ProtocolError;
        let error = WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake);
        assert!(
            matches!(classify_handshake(9, &error), Input::DialFailed { .. }),
            "a non-HTTP WS error (connect refused, reset) is transient"
        );
    }

    #[test]
    fn parse_gate_extra_params_are_ignored() {
        let url = url::Url::parse("ws://127.0.0.1:7420/ws?v=2&sg=5&extra=y&foo=bar").expect("url");
        assert_eq!(
            parse_gate(&url),
            Some((2, 5)),
            "extra query params must not confuse v/sg extraction"
        );
    }

    #[test]
    fn parse_gate_non_numeric_v_is_none() {
        let url = url::Url::parse("ws://127.0.0.1:7420/ws?v=foo&sg=1").expect("url");
        assert_eq!(
            parse_gate(&url),
            None,
            "a non-numeric v makes the gate unparse-able → None"
        );
    }

    #[test]
    fn parse_gate_non_numeric_sg_is_none() {
        let url = url::Url::parse("ws://127.0.0.1:7420/ws?v=2&sg=bar").expect("url");
        assert_eq!(parse_gate(&url), None);
    }
}
