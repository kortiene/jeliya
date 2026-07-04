//! HTTP + WebSocket multiplexing for `bantabad` on one loopback port.
//!
//! A single `127.0.0.1:<port>` serves both the control channel (`/ws`, the
//! `docs/PROTOCOL.md` WebSocket) and, when built with the `embed-ui` feature
//! (or pointed at a directory with `--ui-dir`), the static web UI. Serving the
//! SPA from the daemon's own loopback origin makes the page and the WebSocket
//! same-origin loopback: no mixed-content block (Safari/iOS included), no Local
//! Network Access prompt, and the cross-origin `Origin` guard is unchanged.

use std::convert::Infallible;
use std::path::PathBuf;

use futures_util::{SinkExt, StreamExt};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, ORIGIN};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::{is_local_origin, AppState};

/// UI assets embedded at build time from `ui/dist`. Only compiled for the
/// `embed-ui` (packaged) build; a plain `cargo build` daemon bundles no UI.
#[cfg(feature = "embed-ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct UiAssets;

/// Where the daemon serves the web UI from.
#[derive(Clone)]
pub enum UiSource {
    /// Assets compiled into the binary (`embed-ui` build).
    #[cfg(feature = "embed-ui")]
    Embedded,
    /// A filesystem directory (an explicit `--ui-dir <path>`), which overrides
    /// any embedded assets and decouples UI iteration from a daemon rebuild.
    Dir(PathBuf),
    /// No UI: the daemon answers HTTP with a short status page; `/ws` still
    /// works. Only reachable in a non-`embed-ui` build with no `--ui-dir`.
    #[cfg_attr(feature = "embed-ui", allow(dead_code))]
    None,
}

impl UiSource {
    /// `--ui-dir` wins; otherwise the embedded assets if this is an `embed-ui`
    /// build; otherwise nothing to serve.
    pub fn resolve(ui_dir: Option<PathBuf>) -> Self {
        if let Some(dir) = ui_dir {
            return UiSource::Dir(dir);
        }
        #[cfg(feature = "embed-ui")]
        {
            UiSource::Embedded
        }
        #[cfg(not(feature = "embed-ui"))]
        {
            UiSource::None
        }
    }

    pub fn is_serving(&self) -> bool {
        !matches!(self, UiSource::None)
    }

    /// Load a request-relative asset path, returning its bytes and content type.
    fn load(&self, rel: &str) -> Option<(Bytes, &'static str)> {
        match self {
            #[cfg(feature = "embed-ui")]
            UiSource::Embedded => {
                UiAssets::get(rel).map(|file| (Bytes::from(file.data.into_owned()), guess_mime(rel)))
            }
            UiSource::Dir(base) => std::fs::read(base.join(rel))
                .ok()
                .map(|bytes| (Bytes::from(bytes), guess_mime(rel))),
            UiSource::None => None,
        }
    }
}

/// Serve one accepted TCP connection with hyper: `/ws` upgrades to a WebSocket
/// (behind the same `Origin` guard as before), everything else is static UI.
pub async fn handle_conn(stream: TcpStream, state: AppState, ui: UiSource) {
    let io = TokioIo::new(stream);
    let service = service_fn(move |req: Request<Incoming>| {
        let state = state.clone();
        let ui = ui.clone();
        async move { Ok::<_, Infallible>(route(req, state, ui)) }
    });
    // `with_upgrades` is required for the WebSocket upgrade on `/ws`. A
    // connection-level error just means the client went away; nothing to do.
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await;
}

/// Route a single request: `/ws` → WebSocket upgrade; anything else → static UI.
fn route(mut req: Request<Incoming>, state: AppState, ui: UiSource) -> Response<Full<Bytes>> {
    if req.uri().path() == "/ws" {
        return ws_upgrade(&mut req, state);
    }
    serve_static(req.uri().path(), &ui)
}

/// The WebSocket handshake gate, preserving the pre-hyper behavior exactly:
/// only a same-machine (or non-browser) `Origin` may upgrade; a remote page
/// mounting a cross-site WebSocket hijack is refused with 403.
fn ws_upgrade(req: &mut Request<Incoming>, state: AppState) -> Response<Full<Bytes>> {
    // Cross-Site WebSocket Hijacking guard: reject any request whose `Origin`
    // is a real remote site. Non-browser clients send no `Origin` (allowed);
    // the same-origin served UI sends a loopback `Origin` (allowed).
    if let Some(origin) = req.headers().get(ORIGIN) {
        let allowed = origin.to_str().map(is_local_origin).unwrap_or(false);
        if !allowed {
            return text(
                StatusCode::FORBIDDEN,
                "forbidden: cross-origin WebSocket connections are refused",
            );
        }
    }
    if !hyper_tungstenite::is_upgrade_request(&*req) {
        return text(StatusCode::BAD_REQUEST, "expected a websocket upgrade; connect to /ws");
    }
    match hyper_tungstenite::upgrade(req, None) {
        Ok((response, websocket)) => {
            tokio::spawn(async move {
                if let Ok(ws) = websocket.await {
                    serve_ws(ws, state).await;
                }
            });
            response
        }
        Err(_) => text(StatusCode::BAD_REQUEST, "malformed websocket upgrade"),
    }
}

/// Serve a static UI asset. `/` maps to `index.html`; an unknown *route-like*
/// path (no file extension) falls back to `index.html` so the SPA boots; an
/// unknown asset path 404s.
fn serve_static(path: &str, ui: &UiSource) -> Response<Full<Bytes>> {
    if let UiSource::None = ui {
        return text(
            StatusCode::OK,
            "bantabad is running. No web UI is bundled in this build — start the dev UI \
             (npm run dev), pass --ui-dir <path>, or use an embed-ui build. The control \
             channel is at /ws.",
        );
    }
    let Some(rel) = safe_rel(path) else {
        return text(StatusCode::BAD_REQUEST, "bad path");
    };
    let rel = if rel.is_empty() { "index.html".to_owned() } else { rel };

    if let Some((bytes, mime)) = ui.load(&rel) {
        return asset(bytes, mime);
    }
    if !last_segment_has_ext(&rel) {
        if let Some((bytes, mime)) = ui.load("index.html") {
            return asset(bytes, mime);
        }
    }
    text(StatusCode::NOT_FOUND, "not found")
}

/// One WebSocket client: JSON text frames dispatched to `rpc::handle_frame`,
/// interleaved with broadcast pushes. Generic over the upgraded stream type so
/// the exact same loop drives the hyper-upgraded socket.
pub async fn serve_ws<S>(ws: WebSocketStream<S>, state: AppState)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut messages) = ws.split();
    let mut push_rx = state.push_tx.subscribe();

    loop {
        tokio::select! {
            msg = messages.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let reply = crate::rpc::handle_frame(text.as_str(), &state).await;
                    if sink.send(Message::text(reply)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sink.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {} // binary/pong frames: ignored
            },
            push = push_rx.recv() => match push {
                Ok(frame) => {
                    if sink.send(Message::text(frame)).await.is_err() {
                        break;
                    }
                }
                // A lagged subscriber just misses pushes; the request/response
                // surface (room.timeline / peers.status) re-syncs it.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

/// Reject path traversal and produce a clean relative asset key.
fn safe_rel(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            other => out.push(other),
        }
    }
    Some(out.join("/"))
}

fn last_segment_has_ext(rel: &str) -> bool {
    rel.rsplit('/').next().is_some_and(|s| s.contains('.'))
}

fn guess_mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn asset(bytes: Bytes, mime: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime)
        .body(Full::new(bytes))
        .expect("static response is well-formed")
}

fn text(status: StatusCode, body: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from_static(body.as_bytes())))
        .expect("text response is well-formed")
}
