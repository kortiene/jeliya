//! HTTP + WebSocket multiplexing for `jeliyad` on one loopback port.
//!
//! A single `127.0.0.1:<port>` serves both the control channel (`/ws`, the
//! `docs/PROTOCOL.md` WebSocket) and, when built with the `embed-ui` feature
//! (or pointed at a directory with `--ui-dir`), the static web UI. Serving the
//! SPA from the daemon's own loopback origin makes the page and the WebSocket
//! same-origin loopback: no mixed-content block (Safari/iOS included), no Local
//! Network Access prompt, and the cross-origin `Origin` guard is unchanged.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE,
    HOST, ORIGIN, VARY,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{json, Value};

use hyper_util::rt::TokioIo;
use jeliya_core::error::CoreError;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::{host_header_is_loopback, is_local_origin, AppState};

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
            UiSource::Embedded => UiAssets::get(rel)
                .map(|file| (Bytes::from(file.data.into_owned()), guess_mime(rel))),
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
        async move { Ok::<_, Infallible>(route(req, state, ui).await) }
    });
    // `with_upgrades` is required for the WebSocket upgrade on `/ws`. A
    // connection-level error just means the client went away; nothing to do.
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .with_upgrades()
        .await;
}

/// Route a single request: `/ws` → WebSocket upgrade; `/api/session` → the
/// browser UI's token handshake; `/api/health` → unauthenticated liveness for
/// adoption checks; `/api/files/*` → token-gated file endpoints; anything
/// else → static UI.
async fn route(mut req: Request<Incoming>, state: AppState, ui: UiSource) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_owned();

    // DNS-rebinding gate: the control surface answers only requests addressed
    // to the loopback host itself. A hostile page can point its own domain at
    // 127.0.0.1, but its requests still say `Host: evil.example`.
    if (path == "/ws" || path.starts_with("/api/")) && !host_is_loopback(&req) {
        return text(
            StatusCode::FORBIDDEN,
            "forbidden: the daemon only answers requests addressed to the loopback host",
        );
    }

    if path == "/ws" {
        return ws_upgrade(&mut req, state);
    }
    if path == "/api/health" {
        if req.method() != Method::GET {
            return text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        return health(&state);
    }
    // CORS preflight for the cross-origin dev UI (localhost:5173 → daemon
    // port). A non-simple request (the upload's Content-Type, a Bearer header)
    // preflights with OPTIONS before the real request; answer it or the browser
    // never sends the POST/GET.
    if req.method() == Method::OPTIONS && path.starts_with("/api/") {
        return preflight(&req);
    }
    if path == "/api/session" {
        // v2's ticket issuance: `POST /api/session` proves possession of the
        // daemon token (`Authorization: Bearer <token>`) and returns a
        // short-TTL, single-use connect ticket the client redeems once as
        // `?ct=` — never the long-lived bearer itself, which must not be
        // placed in a URL. The pairing-code browser flow
        // (`POST /api/session` with a `pairing_code` body) and
        // `/api/session/ticket` are a documented follow-up to #166; the v1
        // browser GET handshake is retained meanwhile for the served UI.
        match *req.method() {
            Method::POST => {
                if !token_ok(&req, &state) {
                    return unauthorized(local_origin(&req));
                }
                let (ticket, expires_at) = mint_connect_ticket(&state);
                return json_response(
                    StatusCode::OK,
                    json!({ "ticket": ticket, "expires_at": expires_at }),
                );
            }
            Method::GET => return session(&req, &state),
            _ => return text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        }
    }
    if path == "/api/files/share" {
        if req.method() != Method::POST {
            return text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        let cors = local_origin(&req);
        if !token_ok(&req, &state) {
            return unauthorized(cors);
        }
        return apply_cors(cors, share_upload(req, state).await);
    }
    if path == "/api/files/local" {
        if req.method() != Method::GET {
            return text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        let cors = local_origin(&req);
        if !token_ok(&req, &state) {
            return unauthorized(cors);
        }
        return apply_cors(cors, local_file(req, state).await);
    }
    if path.starts_with("/api/") {
        return text(StatusCode::NOT_FOUND, "not found");
    }
    serve_static(&path, &ui)
}

/// Liveness + the v2 discovery object (docs/protocol-v2.md Layer 0): a parent
/// deciding whether to adopt a running daemon checks the answering process's
/// `pid` and `port`, and a v2 client reads the `storage_generation` it must
/// present at the upgrade gate. `data_dir` is removed — an unauthenticated
/// endpoint must not leak an absolute filesystem path, and the adoption check
/// needs only pid/port. Deliberately unauthenticated (loopback Host only) and
/// secret-free.
fn health(state: &AppState) -> Response<Full<Bytes>> {
    let info = version_info(state);
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "pid": std::process::id(),
            "port": state.port,
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": info.protocol,
            "min_protocol": info.min_protocol,
            "storage_generation": info.storage_generation,
            "limits": info.limits,
        }),
    )
}

/// The shared `{protocol, min_protocol, storage_generation, limits}` object —
/// Layer 0 health, the ready line, and the portfile carry an identical one so
/// the three producers can never drift apart.
pub(crate) fn version_info(state: &AppState) -> jeliya_api::VersionInfo {
    jeliya_api::VersionInfo {
        protocol: jeliya_core::engine::PROTOCOL_VERSION,
        min_protocol: jeliya_core::engine::MIN_PROTOCOL_VERSION,
        storage_generation: jeliya_core::engine::STORAGE_GENERATION,
        limits: state.engine.limits(),
    }
}

/// The connect-ticket TTL (matches the served `browser_session_ttl_ms` policy
/// bound for a fresh ticket).
const CONNECT_TICKET_TTL_MS: u64 = 60_000;

/// A process-unique counter for ephemeral session principals: an omitted
/// `client_id` yields a fresh principal per connection, so the counter (not
/// the wall clock, never a guessable sequence a client could reuse) is what
/// keeps two ephemeral connections' ledgers isolated.
static EPHEMERAL_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The next ephemeral-connection nonce.
fn conn_nonce() -> u64 {
    EPHEMERAL_CONN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Mint a single-use connect ticket and record its expiry. The ticket is a
/// fresh 256-bit opaque value, never derived from the daemon token.
fn mint_connect_ticket(state: &AppState) -> (String, u64) {
    let mut bytes = [0u8; 32];
    let _ = getrandom::fill(&mut bytes);
    let ticket = hex::encode(bytes);
    let expires_at = jeliya_core::now_ms() + CONNECT_TICKET_TTL_MS;
    let mut tickets = state.connect_tickets.lock().expect("tickets poisoned");
    // Bound the outstanding set by dropping expired entries on each mint.
    tickets.retain(|_, exp| *exp > jeliya_core::now_ms());
    tickets.insert(ticket.clone(), expires_at);
    (ticket, expires_at)
}

/// Redeem a connect ticket, burning it. Returns `true` iff the ticket was
/// outstanding, unexpired, and is now consumed.
fn redeem_connect_ticket(state: &AppState, ticket: &str) -> bool {
    let mut tickets = state.connect_tickets.lock().expect("tickets poisoned");
    match tickets.remove(ticket) {
        Some(exp) => exp > jeliya_core::now_ms(),
        None => false,
    }
}

/// Hand the WS auth token to the browser UI. Two browser shapes reach here:
///
/// - **Production** (SPA served from the daemon's own loopback origin): a
///   same-origin `GET` fetch, which per the Fetch spec carries NO `Origin`
///   header — so an Origin-only check would 403 the packaged UI and lock it out
///   of `/ws` entirely. We instead accept the browser-set `Sec-Fetch-Site:
///   same-origin` (page JS cannot forge a `Sec-Fetch-*` header — it is on the
///   forbidden list), which is present only on a genuine same-origin request.
/// - **Dev** (Vite on `localhost:5173` → daemon port): a cross-origin fetch
///   that carries a loopback `Origin`; mirror it back as CORS so the JS can
///   read the token.
///
/// Note the honest limit (see docs/PROTOCOL.md "Process supervision"): neither
/// header is a boundary against a *non-browser* local process, which can forge
/// both. On a single-user machine that process could read the 0600 portfile
/// anyway; multi-user machines are out of scope. Native clients never call this
/// — they read the portfile.
fn session(req: &Request<Incoming>, state: &AppState) -> Response<Full<Bytes>> {
    let headers = req.headers();
    let origin = headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| is_local_origin(origin))
        .map(str::to_owned);
    let same_origin_browser = headers.get(ORIGIN).is_none()
        && headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .map(|site| site == "same-origin" || site == "none")
            .unwrap_or(false);
    if origin.is_none() && !same_origin_browser {
        return text(
            StatusCode::FORBIDDEN,
            "forbidden: the session token is served only to the local browser UI; \
             native clients read the portfile (daemon.json in the data dir)",
        );
    }
    let mut response = json_response(
        StatusCode::OK,
        json!({ "token": state.auth_token.as_str() }),
    );
    let out = response.headers_mut();
    // CORS mirror only for the cross-origin dev case; a same-origin page needs
    // none and gets none.
    if let Some(origin) = origin {
        if let Ok(value) = HeaderValue::from_str(&origin) {
            out.insert("access-control-allow-origin", value);
        }
        out.insert(VARY, HeaderValue::from_static("Origin"));
    }
    out.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Answer a CORS preflight from the loopback dev origin: echo the origin and
/// the methods/headers the file + session endpoints use. A non-loopback origin
/// gets a bare 204 with no allow headers (the real request will still be
/// gated).
fn preflight(req: &Request<Incoming>) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(StatusCode::NO_CONTENT);
    if let Some(origin) = local_origin(req) {
        builder = builder
            .header("access-control-allow-origin", origin)
            .header("access-control-allow-methods", "GET, POST, OPTIONS")
            .header(
                "access-control-allow-headers",
                "Content-Type, Authorization",
            )
            .header("access-control-max-age", "600")
            .header(VARY, "Origin");
    }
    builder
        .body(Full::new(Bytes::new()))
        .expect("preflight response is well-formed")
}

/// Whether the request's `Host` header addresses loopback. Absent or
/// unparsable `Host` is refused: every legitimate client (browsers, curl,
/// tungstenite, Node ws) sends one.
fn host_is_loopback(req: &Request<Incoming>) -> bool {
    req.headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .map(host_header_is_loopback)
        .unwrap_or(false)
}

/// The token presented by a request: `?token=` query param (works for
/// browser WebSocket and plain navigation) or `Authorization: Bearer` (works
/// for native HTTP clients).
fn presented_token(req: &Request<Incoming>) -> Option<String> {
    let query = parse_query(req.uri().query().unwrap_or(""));
    if let Some(token) = query.get("token").filter(|token| !token.is_empty()) {
        return Some(token.clone());
    }
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|token| token.trim().to_owned())
}

fn token_ok(req: &Request<Incoming>, state: &AppState) -> bool {
    presented_token(req)
        .map(|token| constant_time_eq(token.as_bytes(), state.auth_token.as_bytes()))
        .unwrap_or(false)
}

/// Constant-time byte comparison so the token gate leaks no prefix-timing
/// signal. The length check short-circuits, which only reveals the (public)
/// token length.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn unauthorized(cors: Option<String>) -> Response<Full<Bytes>> {
    let response = json_error(
        StatusCode::UNAUTHORIZED,
        &CoreError::invalid("missing or invalid daemon token").with_hint(
            "the browser UI gets a token from /api/session; native clients read \
             daemon.json in the data dir",
        ),
    );
    apply_cors(cors, response)
}

/// The request's `Origin`, kept only when it is loopback. Captured before the
/// request body is consumed so the response can still mirror it as CORS.
fn local_origin(req: &Request<Incoming>) -> Option<String> {
    req.headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| is_local_origin(origin))
        .map(str::to_owned)
}

/// Mirror a loopback `Origin` back as CORS headers so the dev-mode UI
/// (`localhost:5173` against a daemon port) can read responses. Remote
/// origins get nothing (their requests stay opaque).
fn apply_cors(
    origin: Option<String>,
    mut response: Response<Full<Bytes>>,
) -> Response<Full<Bytes>> {
    if let Some(origin) = origin {
        if let Ok(value) = HeaderValue::from_str(&origin) {
            let headers = response.headers_mut();
            headers.insert("access-control-allow-origin", value);
            headers.insert(VARY, HeaderValue::from_static("Origin"));
        }
    }
    response
}

/// The WebSocket handshake: the protocol-v2 generation gate runs **before**
/// any upgrade, frame parse, or dispatch — the only point provably before
/// mutation. A remote `Origin` is refused (cross-site WebSocket hijacking);
/// `v` must name the supported generation and `sg` the storage generation; a
/// per-start credential authenticates the connection. A v1 client (no `v`)
/// is refused `426 protocol_unsupported` here, never reaching the engine.
fn ws_upgrade(req: &mut Request<Incoming>, state: AppState) -> Response<Full<Bytes>> {
    let headers = req.headers();
    let host = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let query = parse_query(req.uri().query().unwrap_or(""));
    let v = query.get("v").and_then(|s| s.parse::<u64>().ok());
    let sg = query.get("sg").and_then(|s| s.parse::<u64>().ok());
    let cid = query.get("cid").cloned();
    // The credential is the daemon bearer (`?token=` / `Authorization:
    // Bearer`) OR a single-use connect ticket (`?ct=`), which is burned on
    // redemption. Both map to the same authenticated principal.
    let credential = match query.get("ct") {
        Some(ct) if !ct.is_empty() => {
            if redeem_connect_ticket(&state, ct) {
                Some(state.auth_token.as_str().to_owned())
            } else {
                None // a spent/unknown/expired ticket is not a credential
            }
        }
        _ => presented_token(req),
    };

    let max_connections = state.engine.limits().max_connections;
    let live = state.connections.load(std::sync::atomic::Ordering::Relaxed);
    let decision = jeliya_codec::gate(&jeliya_codec::GateParams {
        host,
        origin,
        v,
        sg,
        credential,
        at_capacity: live >= max_connections,
        max_connections,
        cid,
        daemon_sg: jeliya_core::engine::STORAGE_GENERATION,
        expected_credential: state.auth_token.as_str().to_owned(),
    });
    let principal = match decision {
        Ok(jeliya_codec::GateDecision::Admit(p)) => p,
        Ok(jeliya_codec::GateDecision::Refuse(rejection)) => {
            return gate_refusal(rejection);
        }
        Err(err) => {
            return text(
                StatusCode::BAD_REQUEST,
                Box::leak(format!("gate error: {err}").into_boxed_str()),
            );
        }
    };

    if !hyper_tungstenite::is_upgrade_request(&*req) {
        return text(
            StatusCode::BAD_REQUEST,
            "expected a websocket upgrade; connect to /ws",
        );
    }
    let max_frame_bytes = state.runtime_limits.max_frame_bytes();
    let max_encoded_message_bytes = max_frame_bytes
        .checked_add(14)
        .expect("validated frame bound leaves WebSocket header headroom");
    let websocket_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        // Jeliya checks the complete-message payload itself. Align both
        // tungstenite guards with the served bound so its 64/16 MiB defaults
        // cannot reject a conforming 128 MiB message below the application.
        .max_message_size(Some(max_frame_bytes))
        .max_frame_size(Some(max_frame_bytes))
        // Each message is flushed eagerly; the runtime's own queue owns the
        // explicit DATA byte permits and writer acknowledgement. Tungstenite's
        // write buffer counts the encoded frame, so retain the maximum RFC
        // 6455 header in addition to the application payload bound.
        .write_buffer_size(0)
        .max_write_buffer_size(max_encoded_message_bytes);
    match hyper_tungstenite::upgrade(req, Some(websocket_config)) {
        Ok((response, websocket)) => {
            tokio::spawn(async move {
                if let Ok(ws) = websocket.await {
                    // Count the live connection for the `max_connections`
                    // gate; decrement on any exit path.
                    state
                        .connections
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    serve_ws(ws, state.clone(), principal).await;
                    state
                        .connections
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
            response
        }
        Err(_) => text(StatusCode::BAD_REQUEST, "malformed websocket upgrade"),
    }
}

/// The pre-upgrade gate refusal: the codec's bare error body plus its HTTP
/// status (403/401/426/503). No JSON envelope — the upgrade never happened.
fn gate_refusal(rejection: jeliya_codec::GateRejection) -> Response<Full<Bytes>> {
    let status = StatusCode::from_u16(rejection.status).unwrap_or(StatusCode::FORBIDDEN);
    let body = serde_json::to_string(&rejection.body)
        .unwrap_or_else(|_| "{\"code\":\"forbidden_origin\"}".to_owned());
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("gate refusal is well-formed")
}

/// Serve a verified local file copy by `(room_id, file_id)`. The browser never
/// supplies a filesystem path; the core maps protocol ids to a previously
/// verified local copy.
async fn local_file(req: Request<Incoming>, state: AppState) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query().unwrap_or(""));
    let Some(room_id) = query.get("room_id").filter(|v| !v.trim().is_empty()) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            &CoreError::invalid("missing room_id for local file"),
        );
    };
    let Some(file_id) = query.get("file_id").filter(|v| !v.trim().is_empty()) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            &CoreError::invalid("missing file_id for local file"),
        );
    };
    let file_req = jeliya_api::FileRead {
        room_id: jeliya_api::RoomId::new(room_id),
        file_id: jeliya_api::FileId::new(file_id),
    };
    let file = match state.engine.local_file(&file_req).await {
        Ok(file) => file,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    let bytes = match std::fs::read(&file.path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return json_error(
                StatusCode::NOT_FOUND,
                &CoreError::internal(format!("could not read local file copy: {err}")),
            )
        }
    };
    if bytes.len() as u64 != file.bytes {
        return json_error(
            StatusCode::CONFLICT,
            &CoreError::internal("local file copy changed before it could be served"),
        );
    }
    let display_name =
        upload_display_name(Some(&file.name)).unwrap_or_else(|_| "download".to_owned());
    // This blob came from a remote room peer, and `file.mime` is that peer's
    // self-declared type. Never let the browser render it inline in the
    // daemon's own loopback origin: a peer-supplied `text/html` / `image/svg+xml`
    // opened as a top-level document would run script with the daemon's origin
    // and could exfiltrate the auth token. Force a download (attachment),
    // forbid MIME sniffing, and hand the browser only a safe, inert type — the
    // real type travels in the filename the user saved.
    let content_type = HeaderValue::from_str(&safe_download_mime(&file.declared_content_type))
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let content_disposition =
        HeaderValue::from_str(&content_disposition_value(&display_name, "attachment"))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"download\""));
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, file.bytes.to_string())
        .header(CONTENT_DISPOSITION, content_disposition)
        .header("x-content-type-options", "nosniff")
        .header(
            "content-security-policy",
            "default-src 'none'; sandbox; frame-ancestors 'none'",
        )
        .body(Full::new(Bytes::from(bytes)))
        .expect("local file response is well-formed")
}

/// Map a peer-declared MIME to one that cannot execute as a document if a
/// browser ignores `Content-Disposition: attachment`. Known-inert types (plain
/// images, audio, video, PDF, text) pass through so a saved file still opens in
/// the right app; anything that a browser could run as script/markup collapses
/// to `application/octet-stream`.
fn safe_download_mime(mime: &str) -> String {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let inert = matches!(
        base.as_str(),
        "application/pdf"
            | "text/plain"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
            | "audio/webm"
            | "video/mp4"
            | "video/webm"
            | "video/ogg"
    ) || (base.starts_with("image/") && base != "image/svg+xml");
    if inert {
        base
    } else {
        "application/octet-stream".to_owned()
    }
}

/// Browser-backed file sharing. The browser cannot reveal a real local path for
/// a selected file, so it POSTs the file bytes to this local-only endpoint. The
/// daemon stages those bytes under its data dir, then uses the normal confined
/// `file.share` path so protocol authorship and blob import remain centralized.
async fn share_upload(req: Request<Incoming>, state: AppState) -> Response<Full<Bytes>> {
    let upload_limit = state.engine.limits().max_shared_file_bytes;
    if let Some(origin) = req.headers().get(ORIGIN) {
        let allowed = origin.to_str().map(is_local_origin).unwrap_or(false);
        if !allowed {
            return json_error(
                StatusCode::FORBIDDEN,
                &CoreError::invalid("cross-origin file uploads are refused")
                    .with_hint("open Jeliya from the local daemon UI"),
            );
        }
    }

    if let Some(content_length) = req.headers().get(CONTENT_LENGTH) {
        match content_length
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(n) if n <= upload_limit => {}
            Some(n) => {
                return json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    &CoreError::invalid(format!(
                        "upload is {n} bytes; the share limit is {upload_limit} bytes"
                    )),
                )
            }
            None => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    &CoreError::invalid("invalid Content-Length for file upload"),
                )
            }
        }
    }

    let query = parse_query(req.uri().query().unwrap_or(""));
    let Some(room_id) = query.get("room_id").filter(|v| !v.trim().is_empty()) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            &CoreError::invalid("missing room_id for file upload"),
        );
    };
    let display_name = match upload_display_name(query.get("name").map(String::as_str)) {
        Ok(name) => name,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
    let mime = query
        .get("mime")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| {
            req.headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or(value).trim().to_owned())
                .filter(|value| !value.is_empty())
        });

    let body = match read_limited(req.into_body(), upload_limit).await {
        Ok(bytes) => bytes,
        Err(err) => return json_error(StatusCode::PAYLOAD_TOO_LARGE, &err),
    };
    let stage_dir = state.data_dir.join("uploads");
    if let Err(err) = std::fs::create_dir_all(&stage_dir) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &CoreError::internal(format!("could not create upload staging dir: {err}")),
        );
    }
    let stage_path = stage_dir.join(unique_stage_name(&display_name));
    if let Err(err) = std::fs::write(&stage_path, &body) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &CoreError::internal(format!("could not stage upload: {err}")),
        );
    }

    let share = jeliya_api::FileShare {
        room_id: jeliya_api::RoomId::new(room_id),
        name: display_name,
        declared_bytes: body.len() as u64,
        declared_content_type: mime.unwrap_or_else(|| "application/octet-stream".to_owned()),
    };
    let result = state.engine.share_staged_file(&share, &stage_path).await;
    let _ = std::fs::remove_file(&stage_path);
    match result {
        Ok(value) => json_ok(value),
        // The typed refusal is already the record's error object; it is served
        // verbatim rather than flattened into a prose HTTP error, so the
        // legacy staging edge answers the same taxonomy the WS surface does.
        //
        // The `ok` discriminator stays, because it is the envelope and not the
        // taxonomy: `json_ok` pairs every success with `ok: true` and every
        // other refusal on this endpoint carries `ok: false`, so dropping it
        // here would leave one response shape a consumer decoding the envelope
        // before the status could not classify. Only the nested object changed
        // generation — `message`/`hint` are gone from v2 deliberately and are
        // not re-added.
        Err(err) => match serde_json::to_value(&err) {
            Ok(body) => json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": body}),
            ),
            Err(_) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &CoreError::internal("could not encode the typed refusal"),
            ),
        },
    }
}

async fn read_limited(mut body: Incoming, max: u64) -> Result<Bytes, CoreError> {
    let mut out = Vec::new();
    let mut total = 0_u64;
    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|e| CoreError::invalid(format!("could not read upload body: {e}")))?;
        if let Ok(data) = frame.into_data() {
            total += data.len() as u64;
            if total > max {
                return Err(CoreError::invalid(format!(
                    "upload is larger than the share limit of {max} bytes"
                )));
            }
            out.extend_from_slice(&data);
        }
    }
    Ok(Bytes::from(out))
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn upload_display_name(raw: Option<&str>) -> Result<String, CoreError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(CoreError::invalid("missing file name for upload"));
    };
    let base = Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(raw)
        .trim();
    let cleaned: String = base
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim_matches(|ch| ch == '.' || ch == ' ')
        .chars()
        .take(180)
        .collect();
    if cleaned.is_empty() {
        return Err(CoreError::invalid("file name is empty after sanitizing"));
    }
    Ok(cleaned)
}

fn disposition_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '\r' | '\n' => '_',
            ch if ch == ' ' || ch.is_ascii_graphic() => ch,
            _ => '_',
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "download".to_owned()
    } else {
        cleaned
    }
}

fn content_disposition_value(name: &str, disposition: &str) -> String {
    format!(
        "{disposition}; filename=\"{}\"; filename*=UTF-8''{}",
        disposition_filename(name),
        rfc5987_filename(name)
    )
}

fn rfc5987_filename(name: &str) -> String {
    let mut out = String::new();
    for byte in name.as_bytes() {
        match *byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'!'
            | b'#'
            | b'$'
            | b'&'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~' => out.push(*byte as char),
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn unique_stage_name(display_name: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{}-{now}-{display_name}", std::process::id())
}

fn json_ok(result: impl serde::Serialize) -> Response<Full<Bytes>> {
    json_response(StatusCode::OK, json!({ "ok": true, "result": result }))
}

fn json_error(status: StatusCode, err: &CoreError) -> Response<Full<Bytes>> {
    json_response(
        status,
        json!({
            "ok": false,
            "error": {
                "code": err.kind.code(),
                "message": err.message,
                "hint": err.hint,
            },
        }),
    )
}

fn json_response(status: StatusCode, body: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("json response is well-formed")
}

/// Serve a static UI asset. `/` maps to `index.html`; an unknown *route-like*
/// path (no file extension) falls back to `index.html` so the SPA boots; an
/// unknown asset path 404s.
fn serve_static(path: &str, ui: &UiSource) -> Response<Full<Bytes>> {
    if let UiSource::None = ui {
        return text(
            StatusCode::OK,
            "jeliyad is running. No web UI is bundled in this build — start the dev UI \
             (npm run dev), pass --ui-dir <path>, or use an embed-ui build. The control \
             channel is at /ws.",
        );
    }
    let Some(rel) = safe_rel(path) else {
        return text(StatusCode::BAD_REQUEST, "bad path");
    };
    let rel = if rel.is_empty() {
        "index.html".to_owned()
    } else {
        rel
    };

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

/// One v2 WebSocket connection. The daemon's first message is exactly one
/// `hello`; thereafter each inbound Text message is decoded by the codec into a
/// typed call, executed by the engine, and its typed reply encoded back —
/// interleaved with typed pushes. A complete message over the limit closes
/// `4005`; a message whose `id` cannot be recovered closes `4007`; any other
/// malformed message gets a correlated error reply so one bad request never strands the
/// others in flight. A lagged push receiver is told to resync (the one
/// resync path), never silently continued.
pub async fn serve_ws<S>(
    ws: WebSocketStream<S>,
    state: AppState,
    principal: jeliya_codec::SessionPrincipal,
) where
    // `Send + 'static` so the single-writer task and the spawned request tasks
    // can own the socket and engine handles across threads.
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, mut messages) = ws.split();
    let mut push_rx = state.engine.subscribe_pushes();
    let bounds = jeliya_codec::CodecBounds {
        max_frame_bytes: state.runtime_limits.max_frame_bytes(),
        ..jeliya_codec::CodecBounds::default()
    };
    let served = state.engine.limits();

    // The authenticated session principal rendered as one ledger key:
    // `(credential, client_id)`. An omitted `client_id` yields a fresh
    // ephemeral principal per connection (no cross-reconnect replay), the
    // documented choice a short-lived CLI makes. Rendered once here; every
    // request on this connection shares it. Explicit `cid`s and generated
    // ephemeral keys live in DISJOINT namespaces (distinct tag bytes): a
    // client declaring `cid=ephemeral:0` must not land in the same ledger
    // principal as a connection that omitted `cid`, or two supposedly
    // isolated principals could replay/conflict each other's operations.
    let principal_key = match &principal.client_id {
        Some(cid) => format!("{}\u{1}explicit\u{1}{}", principal.credential, cid),
        None => format!(
            "{}\u{1}generated\u{1}{}",
            principal.credential,
            conn_nonce()
        ),
    };

    // One writer owns the sink. Its control queue is independent from the
    // byte-permitted DATA queue and is selected first between bounded DATA
    // records, so JSON, terminal control, Pong, and Close cannot sit behind a
    // file-sized message-count backlog.
    let (outbound, writer_queues) = crate::outbound::Outbound::new(
        state.runtime_limits.control_queue_capacity_messages(),
        state.runtime_limits.data_queue_capacity_messages(),
        state.runtime_limits.control_queue_capacity_bytes(),
        state.runtime_limits.data_queue_capacity_bytes(),
        state.runtime_limits.max_frame_bytes(),
    );
    let (writer_done_tx, mut writer_done_rx) = tokio::sync::watch::channel(false);
    let writer_timeout = state.runtime_limits.transfer_stall();
    let mut writer = tokio::spawn(async move {
        let _ = writer_queues.run(sink, writer_timeout).await;
        let _ = writer_done_tx.send(true);
    });

    let streams = crate::file_read::StreamRegistry::new();
    let upload_budget = crate::file_share::UploadIngressBudget::new(state.runtime_limits);
    let requests = crate::file_read::RequestTracker::new(served.max_inflight_requests);
    let stream_ids = std::sync::Arc::new(std::sync::Mutex::new(
        crate::transfer::StreamIdGenerator::new(),
    ));

    // This connection's room subscriptions: `stream.subscribe` adds a room
    // with the position the client's stream begins at; `stream.unsubscribe`
    // removes it; pushes are gated on it (no push before subscribe, per the
    // record's "no global broadcast" rule). Shared with the spawned request
    // tasks behind a mutex (only ever held for a map op, never across an
    // engine await).
    let subscriptions = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::<
        String,
        SubscriptionState,
    >::new()));

    // The `hello` frame: exactly one, first, carrying the generation, the
    // storage generation, the served limits, and the local subject. An
    // unreadable subject store is `not_ready` — refuse the connection rather
    // than inviting `subject.ensure` against state that cannot be served.
    let subject = match state.engine.subject_state() {
        Ok(s) => s,
        Err(_) => {
            let _ = outbound
                .close(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 4003.into(),
                    reason: "not_ready".into(),
                })
                .await;
            drop(outbound);
            let _ = writer.await;
            return;
        }
    };
    let hello = jeliya_api::Hello {
        protocol: jeliya_core::engine::PROTOCOL_VERSION,
        storage_generation: jeliya_core::engine::STORAGE_GENERATION,
        limits: state.engine.limits(),
        subject,
        resume: jeliya_api::Resume::Fresh,
    };
    match serde_json::to_vec(&hello) {
        Ok(bytes) => {
            if outbound.text(bytes).await != crate::outbound::WriteReceipt::Sent {
                drop(outbound);
                let _ = writer.await;
                return;
            }
        }
        Err(_) => {
            drop(outbound);
            let _ = writer.await;
            return;
        }
    }

    let (closer, mut close_requested_rx, mut close_completed_rx) =
        crate::file_read::ConnectionCloser::new(streams.clone(), outbound.clone());

    // Track in-flight request tasks so the loop never serializes on a slow
    // operation; a finished task's result is reaped without blocking.
    let mut inflight: tokio::task::JoinSet<bool> = tokio::task::JoinSet::new();

    // The served idle timeout: a connection with no inbound activity for
    // `idle_timeout_ms` is closed with 4004. Reset on any inbound frame.
    let idle_ms = state.engine.limits().idle_timeout_ms;
    let idle_deadline = tokio::time::sleep(tokio::time::Duration::from_millis(idle_ms));
    tokio::pin!(idle_deadline);
    let mut force_writer_abort = false;

    loop {
        if closer.is_requested() {
            break;
        }
        tokio::select! {
            biased;
            changed = close_requested_rx.changed() => {
                if changed.is_err() || *close_requested_rx.borrow() {
                    break;
                }
            }
            changed = writer_done_rx.changed() => {
                if changed.is_err() || *writer_done_rx.borrow() {
                    force_writer_abort = true;
                    break;
                }
            }
            msg = messages.next() => match msg {
                Some(Ok(message)) => {
                    // The Close owner publishes `claimed` before it may need
                    // to wait on a stream sequencing lock. Linearize reader
                    // admission here, after the socket future resolves: a
                    // message that became ready in that interval must not be
                    // parsed or allowed to spawn a mutating request.
                    if closer.is_requested() {
                        break;
                    }
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_millis(idle_ms));
                    // Complete-message size wins before Text/Binary class or
                    // any JSON/header/magic/binding inspection.
                    if complete_data_message_len(&message).is_some_and(|len| len > bounds.max_frame_bytes) {
                        closer.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: 4005.into(),
                            reason: "frame_too_large".into(),
                        });
                        break;
                    }
                    match message {
                        Message::Text(text) => {
                            if !dispatch_text(
                                text.as_bytes(),
                                &state,
                                &bounds,
                                &subscriptions,
                                &outbound,
                                &requests,
                                &streams,
                                &stream_ids,
                                &closer,
                                &mut inflight,
                                &principal_key,
                                &upload_budget,
                            ).await {
                                break;
                            }
                        }
                        Message::Binary(bytes) => match streams.route_binary_message(bytes, &bounds).await {
                            crate::file_read::BinaryRoute::Delivered => {}
                            crate::file_read::BinaryRoute::CloseTooLarge => {
                                closer.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                    code: 4005.into(),
                                    reason: "frame_too_large".into(),
                                });
                                break;
                            }
                            crate::file_read::BinaryRoute::CloseMalformed => {
                                closer.malformed();
                                break;
                            }
                        },
                        Message::Ping(payload) => {
                            if !outbound.pong(payload).await {
                                break;
                            }
                        }
                        Message::Close(_) => {
                            force_writer_abort = true;
                            break;
                        }
                        Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
                Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(
                    tokio_tungstenite::tungstenite::error::CapacityError::MessageTooLong { .. },
                ))) => {
                    // The transport rejected an oversized reassembled message
                    // before exposing its class/content; preserve the same
                    // application close as the explicit complete-message gate.
                    closer.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4005.into(),
                        reason: "frame_too_large".into(),
                    });
                    break;
                }
                Some(Err(_)) | None => {
                    force_writer_abort = true;
                    break;
                }
            },
            () = &mut idle_deadline => {
                if streams.is_active() {
                    // A correctly credit-paused transfer remains governed by
                    // its stall and absolute timers, never ordinary idle.
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_millis(idle_ms));
                } else {
                    closer.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4004.into(),
                        reason: "idle_timeout".into(),
                    });
                    break;
                }
            }
            push = push_rx.recv() => match push {
                Ok(push) => {
                    // Gate delivery on this connection's subscriptions: a push
                    // for a room the connection has not subscribed to is not
                    // delivered (the record's no-global-broadcast rule), and a
                    // push is never a membership oracle.
                    let room = push_room_id(&push).map(str::to_owned);
                    let subscribed = {
                        let mut subs = subscriptions.lock().await;
                        room.as_ref()
                            .and_then(|room| subs.get_mut(room))
                            .map(|subscription| {
                                // The position joins the subscription state so
                                // unsubscribe/resubscribe cannot retain a stale
                                // high-water mark and skip recovery events.
                                if let Some(pos) = push_pos(&push) {
                                    subscription.last_delivered = Some(pos);
                                }
                            })
                            .is_some()
                    };
                    if subscribed {
                        let bytes = jeliya_codec::push_to_bytes(&push);
                        if outbound.text(bytes).await != crate::outbound::WriteReceipt::Sent {
                            break;
                        }
                    }
                }
                // A lagged receiver fell behind the push fan-out. v2 never
                // silently continues: emit an explicit gap for EACH subscribed
                // room from its last-delivered position, so the client can
                // resync each affected room rather than getting one unusable
                // global frame.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let rooms: Vec<(String, u64)> = {
                        let subs = subscriptions.lock().await;
                        subs.iter()
                            .map(|(room, subscription)| {
                                (
                                    room.clone(),
                                    subscription
                                        .last_delivered
                                        .unwrap_or(subscription.from_pos),
                                )
                            })
                            .collect()
                    };
                    for (room, from_pos) in rooms {
                        let gap = jeliya_api::Push::Gap {
                            room_id: jeliya_api::RoomId::new(room),
                            from_pos,
                            to: jeliya_api::GapTo::Open,
                            reason: jeliya_api::GapReason::Backpressure,
                        };
                        let bytes = jeliya_codec::push_to_bytes(&gap);
                        if outbound.text(bytes).await != crate::outbound::WriteReceipt::Sent {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Reap finished request tasks without blocking the loop.
            Some(_) = inflight.join_next() => {
                // Completion of a transfer counts as connection activity;
                // restart ordinary idle after its binding/request retires.
                idle_deadline.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_millis(idle_ms));
            }
        }
    }

    // Teardown: stop accepting new work, drain in-flight replies, then drop
    // the writer task.
    streams.invalidate_connection();
    outbound.invalidate_connection();
    inflight.abort_all();
    while inflight.join_next().await.is_some() {}
    let close_requested = closer.is_requested();
    if close_requested && !*close_completed_rx.borrow() {
        let wait_for_close = async {
            while !*close_completed_rx.borrow() {
                if close_completed_rx.changed().await.is_err() {
                    break;
                }
            }
        };
        let _ =
            tokio::time::timeout(tokio::time::Duration::from_millis(1_100), wait_for_close).await;
    }
    drop(closer);
    drop(outbound);
    if force_writer_abort && !close_requested {
        writer.abort();
        let _ = writer.await;
    } else if tokio::time::timeout(tokio::time::Duration::from_secs(1), &mut writer)
        .await
        .is_err()
    {
        // A peer can keep TCP open while refusing to drain its receive
        // window. Give a queued Close a short flush grace, then cancel the
        // sole sink owner so connection teardown cannot hang indefinitely.
        writer.abort();
        let _ = writer.await;
    }
}

fn complete_data_message_len(message: &Message) -> Option<usize> {
    match message {
        Message::Text(text) => Some(text.len()),
        Message::Binary(bytes) => Some(bytes.len()),
        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => None,
    }
}

/// The room a push is scoped to, for subscription gating. `transfer` frames
/// are principal-scoped (not room-gated) and `gap` may name no room yet.
fn push_room_id(push: &jeliya_api::Push) -> Option<&str> {
    match push {
        jeliya_api::Push::Event { room_id, .. } => Some(room_id.as_str()),
        jeliya_api::Push::Gap { room_id, .. } => Some(room_id.as_str()),
        jeliya_api::Push::Peer { room_id, .. } => Some(room_id.as_str()),
        jeliya_api::Push::Transfer { .. } => None,
    }
}

/// The position an event push carries (for last-delivered tracking). Peer,
/// gap, and transfer pushes carry no event position.
fn push_pos(push: &jeliya_api::Push) -> Option<u64> {
    match push {
        jeliya_api::Push::Event { event, .. } => Some(event.pos),
        _ => None,
    }
}

/// The served `max_subscriptions_per_connection`.
const MAX_SUBSCRIPTIONS: u64 = 64;

/// Per-room delivery baseline for one connection.
///
/// Keeping the resolved cursor and delivered high-water mark in the same map
/// makes unsubscribe remove both atomically; a later subscribe cannot inherit
/// a stale position from the prior subscription.
#[derive(Debug, Clone, Copy)]
struct SubscriptionState {
    from_pos: u64,
    last_delivered: Option<u64>,
}

/// `stream.subscribe` — add the room to this connection's subscription set.
/// Naturally idempotent; exceeding the served limit is
/// `subscription_limit_reached`, never a silent drop.
async fn handle_stream_subscribe(
    out_tx: &crate::outbound::Outbound,
    state: &AppState,
    request: &jeliya_codec::Request,
    subscriptions: &std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, SubscriptionState>>,
    >,
) -> bool {
    // Extract owned values up front so no `&Request` (non-Sync) is held
    // across an await.
    let id = request.id;
    let req = match request
        .call
        .input_any()
        .downcast_ref::<jeliya_api::StreamSubscribe>()
    {
        Some(r) => r.clone(),
        None => return send_api_err(out_tx, id, jeliya_api::ApiError::MalformedFrame).await,
    };
    let room_key = req.room_id.to_string();
    let existing = {
        let subs = subscriptions.lock().await;
        subs.get(&room_key).map(|state| state.from_pos)
    };
    if let Some(from_pos) = existing {
        return send_typed(
            out_tx,
            id,
            &jeliya_api::StreamSubscribeOut {
                room_id: req.room_id,
                from_pos,
            },
        )
        .await;
    }
    if subscriptions.lock().await.len() as u64 >= MAX_SUBSCRIPTIONS {
        return send_api_err(
            out_tx,
            id,
            jeliya_api::ApiError::SubscriptionLimitReached {
                limit: MAX_SUBSCRIPTIONS,
            },
        )
        .await;
    }
    // Authorize the room before recording any subscription: an unknown,
    // malformed, or inaccessible room is `room_not_available`, never a dormant
    // subscription that later springs to life.
    let from_pos = match &req.from {
        jeliya_api::Cursor::Start => match room_head_pos(state, &req.room_id).await {
            Ok(pos) => pos,
            Err(err) => return send_api_err(out_tx, id, err).await,
        },
        jeliya_api::Cursor::At { pos } => {
            // An explicit position still requires the room to be visible.
            match authorize_room(state, &req.room_id).await {
                Ok(()) => *pos,
                Err(err) => return send_api_err(out_tx, id, err).await,
            }
        }
    };
    // Another in-flight subscribe can race this one. The first insertion wins
    // and defines the idempotent result; the second returns that same baseline.
    let from_pos = {
        let mut subs = subscriptions.lock().await;
        if let Some(existing) = subs.get(&room_key) {
            existing.from_pos
        } else if subs.len() as u64 >= MAX_SUBSCRIPTIONS {
            drop(subs);
            return send_api_err(
                out_tx,
                id,
                jeliya_api::ApiError::SubscriptionLimitReached {
                    limit: MAX_SUBSCRIPTIONS,
                },
            )
            .await;
        } else {
            subs.insert(
                room_key,
                SubscriptionState {
                    from_pos,
                    last_delivered: None,
                },
            );
            from_pos
        }
    };
    send_typed(
        out_tx,
        id,
        &jeliya_api::StreamSubscribeOut {
            room_id: req.room_id,
            from_pos,
        },
    )
    .await
}

/// `stream.unsubscribe` — remove the room from this connection's set.
async fn handle_stream_unsubscribe(
    out_tx: &crate::outbound::Outbound,
    state: &AppState,
    request: &jeliya_codec::Request,
    subscriptions: &std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, SubscriptionState>>,
    >,
) -> bool {
    let id = request.id;
    let req = match request
        .call
        .input_any()
        .downcast_ref::<jeliya_api::StreamUnsubscribe>()
    {
        Some(r) => r.clone(),
        None => return send_api_err(out_tx, id, jeliya_api::ApiError::MalformedFrame).await,
    };
    // The room-access stages, before the connection-local semantics.
    //
    // This operation is resolved from the connection's own subscription map
    // rather than through the engine, so the stages the engine applies to
    // everything it dispatches have to be applied here by hand. They are not
    // optional for it: the record makes step 4 "every operation whose `in`
    // carries `room_id`" and step 5 the same set minus `room.archive` and
    // `room.list`, and it enumerates its carve-outs exhaustively —
    // `subject.ensure` at step 2, `invite.redeem` at 4, 5 and 6. There is no
    // `stream.*` exception, `stream.unsubscribe`'s `in` is `{room_id}`, and
    // `request_room` already declares it room-scoped for core dispatch.
    //
    // An earlier revision applied step 2 alone and argued the rest away as
    // connection state. That left `subscribe R` → `leave R` → `unsubscribe R`
    // answering `ok {unsubscribed: true}` to a caller whose membership the room
    // had already ended — an answer neither the record nor the fixtures
    // support, since nothing tears the map entry down when standing ends.
    //
    // `authorize_room` is the same call the two siblings in this file make, and
    // it yields the stages in the record's order: `subject_absent`,
    // `room_not_available`, then `membership_ended`. `subscription_unknown`
    // stays what it always was — the step-7 answer for an active member holding
    // no such subscription. None of it is an oracle: `room_not_available` is
    // the record's own conflation of unknown-and-inaccessible.
    if let Err(err) = authorize_room(state, &req.room_id).await {
        return send_api_err(out_tx, id, err).await;
    }
    let mut subs = subscriptions.lock().await;
    if subs.remove(&req.room_id.to_string()).is_none() {
        return send_api_err(
            out_tx,
            id,
            jeliya_api::ApiError::SubscriptionUnknown {
                room_id: req.room_id.clone(),
            },
        )
        .await;
    }
    send_typed(
        out_tx,
        id,
        &jeliya_api::StreamUnsubscribeOut {
            room_id: req.room_id,
            unsubscribed: true,
        },
    )
    .await
}

/// `stream.resync` — the authoritative recovery: events since `from_pos`, or
/// `resync_required` naming a position to discard back to.
async fn handle_stream_resync(
    out_tx: &crate::outbound::Outbound,
    state: &AppState,
    request: &jeliya_codec::Request,
) -> bool {
    let id = request.id;
    let req = match request
        .call
        .input_any()
        .downcast_ref::<jeliya_api::StreamResync>()
    {
        Some(r) => r.clone(),
        None => return send_api_err(out_tx, id, jeliya_api::ApiError::MalformedFrame).await,
    };
    // A client naming a position AHEAD of the room's head holds state the
    // daemon does not: that is the discard-and-re-read case, never an empty
    // success (which would leave the client permanently convinced it is
    // ahead). The head is the last committed position; `from_pos > head` is
    // `resync_required` naming the real head. `from_pos == head` is caught
    // up and answers an empty set below.
    let head = match room_head_pos(state, &req.room_id).await {
        Ok(head) => head,
        Err(err) => return send_api_err(out_tx, id, err).await,
    };
    if req.from_pos > head {
        return send_api_err(
            out_tx,
            id,
            jeliya_api::ApiError::ResyncRequired {
                room_id: req.room_id.clone(),
                from_pos: head,
            },
        )
        .await;
    }
    // Read the committed events after from_pos via the engine's typed
    // timeline, starting the page AT from_pos (positions are exclusive on the
    // low side, so `at {pos: from_pos + 1}` reads strictly after it) rather
    // than reading the head and filtering — a room longer than one page must
    // not hide its tail.
    let call = jeliya_core::typed::TypedCall::RoomTimeline(jeliya_api::RoomTimeline {
        room_id: req.room_id.clone(),
        page: jeliya_api::Page {
            cursor: jeliya_api::Cursor::At {
                pos: req.from_pos.saturating_add(1),
            },
            direction: jeliya_api::Direction::Forward,
            limit: 1024,
        },
    });
    let executed = state.engine.execute(call).await;
    let events: Vec<jeliya_api::Event> = match executed.reply {
        Ok(jeliya_core::typed::TypedReply::RoomTimeline(out)) => out.events,
        Ok(_) => Vec::new(),
        Err(err) => return send_api_err(out_tx, id, err).await,
    };
    let next_pos = events.last().map(|e| e.pos).unwrap_or(req.from_pos);
    let truncated = if events.len() >= 1024 {
        jeliya_api::Truncated::More {
            cursor: jeliya_api::Cursor::At { pos: next_pos },
        }
    } else {
        jeliya_api::Truncated::Complete
    };
    send_typed(
        out_tx,
        request.id,
        &jeliya_api::StreamResyncOut {
            room_id: req.room_id.clone(),
            events,
            next_pos,
            truncated,
        },
    )
    .await
}

/// Authorize that the caller can see the room, returning `room_not_available`
/// when it cannot. A `room.members` read enforces the access boundary.
async fn authorize_room(
    state: &AppState,
    room_id: &jeliya_api::RoomId,
) -> Result<(), jeliya_api::ApiError> {
    let call = jeliya_core::typed::TypedCall::RoomMembers(jeliya_api::RoomMembers {
        room_id: room_id.clone(),
    });
    match state.engine.execute(call).await.reply {
        Ok(_) => Ok(()),
        Err(err) => Err(err),
    }
}

/// The room's last committed position (the concrete lower bound a `start`
/// subscription resolves to), or the access error if the room is not visible.
async fn room_head_pos(
    state: &AppState,
    room_id: &jeliya_api::RoomId,
) -> Result<u64, jeliya_api::ApiError> {
    authorize_room(state, room_id).await?;
    let call = jeliya_core::typed::TypedCall::RoomTimeline(jeliya_api::RoomTimeline {
        room_id: room_id.clone(),
        page: jeliya_api::Page {
            cursor: jeliya_api::Cursor::Start,
            direction: jeliya_api::Direction::Backward,
            limit: 1,
        },
    });
    match state.engine.execute(call).await.reply {
        Ok(jeliya_core::typed::TypedReply::RoomTimeline(out)) => Ok(stream_start_position(
            out.events.last().map(|event| event.pos),
        )),
        Ok(_) => Ok(0),
        Err(err) => Err(err),
    }
}

/// Resolve a `start` cursor to the last position the room already holds.
///
/// The first future event is one greater than this baseline. Returning that
/// next position here would make lag recovery read strictly after it and skip
/// the first missed event.
fn stream_start_position(last_committed: Option<u64>) -> u64 {
    last_committed.unwrap_or(0)
}

/// Encode a typed output as a reply frame and send it.
async fn send_typed<O>(out_tx: &crate::outbound::Outbound, id: u64, out: &O) -> bool
where
    O: serde::Serialize,
{
    let reply = jeliya_codec::Reply {
        id,
        ok: true,
        out: serde_json::to_value(out).ok(),
        err: None,
    };
    out_tx.text(reply.to_bytes()).await == crate::outbound::WriteReceipt::Sent
}

/// Encode a typed API error as a reply frame and send it.
async fn send_api_err(
    out_tx: &crate::outbound::Outbound,
    id: u64,
    err: jeliya_api::ApiError,
) -> bool {
    let reply = jeliya_codec::Reply {
        id,
        ok: false,
        out: None,
        err: Some(err),
    };
    out_tx.text(reply.to_bytes()).await == crate::outbound::WriteReceipt::Sent
}

/// Decode one inbound frame and route it. Connection-terminating frames (over
/// the limit → `4005`, unrecoverable id → `4007`) are decided synchronously;
/// well-formed requests are executed on a spawned task so a slow operation
/// never head-of-line blocks the push fan-out or other requests. Returns
/// `false` when the connection must close.
#[allow(clippy::too_many_arguments)]
async fn dispatch_text(
    bytes: &[u8],
    state: &AppState,
    bounds: &jeliya_codec::CodecBounds,
    subscriptions: &std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, SubscriptionState>>,
    >,
    out_tx: &crate::outbound::Outbound,
    requests: &crate::file_read::RequestTracker,
    streams: &crate::file_read::StreamRegistry,
    stream_ids: &std::sync::Arc<std::sync::Mutex<crate::transfer::StreamIdGenerator>>,
    closer: &crate::file_read::ConnectionCloser,
    inflight: &mut tokio::task::JoinSet<bool>,
    principal_key: &str,
    upload_budget: &crate::file_share::UploadIngressBudget,
) -> bool {
    // This is the final request-admission linearization point. If it wins
    // before a concurrent Close claim, the request was already in flight and
    // teardown will cancel it; if the Close claim wins, no operation starts.
    if closer.is_requested() {
        return false;
    }
    use jeliya_codec::CodecError;
    let frame = match jeliya_codec::decode(bytes, bounds) {
        Ok(frame) => frame,
        Err(CodecError::FrameTooLarge { .. }) => {
            closer.request(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 4005.into(),
                reason: "frame_too_large".into(),
            });
            return false;
        }
        Err(CodecError::UnrecoverableId(_)) => {
            closer.malformed();
            return false;
        }
        Err(CodecError::Malformed { id, error }) => {
            if requests.is_outstanding(id) {
                closer.malformed();
                return false;
            }
            let reply = jeliya_codec::Reply::from_result::<jeliya_api::RoomList>(id, Err(error));
            return out_tx.text(reply.to_bytes()).await == crate::outbound::WriteReceipt::Sent;
        }
        Err(CodecError::GateRefused(_)) => {
            return false;
        }
    };

    let jeliya_codec::Frame::Request(request) = frame else {
        // An inbound non-request frame has no place client-to-daemon.
        return true;
    };

    let request_permit = match requests.acquire(request.id) {
        Ok(permit) => permit,
        Err(crate::file_read::RequestAdmissionError::Duplicate) => {
            // Reusing an outstanding correlation id makes either reply
            // ambiguous, so there is no trustworthy request to answer.
            closer.malformed();
            return false;
        }
        Err(crate::file_read::RequestAdmissionError::Exhausted(error)) => {
            return send_api_err(out_tx, request.id, error).await;
        }
    };

    // The three stream operations are connection-scoped: they read and mutate
    // THIS connection's subscription set, never the supervisor. They are fast
    // (map ops plus one engine read), so run them inline.
    match request.call.op {
        "stream.subscribe" => {
            let _request_permit = request_permit;
            return handle_stream_subscribe(out_tx, state, &request, subscriptions).await;
        }
        "stream.unsubscribe" => {
            let _request_permit = request_permit;
            return handle_stream_unsubscribe(out_tx, state, &request, subscriptions).await;
        }
        "stream.resync" => {
            let _request_permit = request_permit;
            return handle_stream_resync(out_tx, state, &request).await;
        }
        _ => {}
    }

    // Stream requests remain outstanding inside their actors through the
    // terminal Text writer acknowledgement. Upload owners are detached from
    // the connection task set so accepted-END finalization and ledger
    // publication survive socket teardown; registry invalidation remains the
    // pre-END transport-loss signal.
    if request.call.op == "file.share" {
        let Some(file_share) = request
            .call
            .input_any()
            .downcast_ref::<jeliya_api::FileShare>()
            .cloned()
        else {
            return send_api_err(out_tx, request.id, jeliya_api::ApiError::MalformedFrame).await;
        };
        let engine = state.engine.clone();
        let outbound = out_tx.clone();
        let registry = streams.clone();
        let stream_ids = stream_ids.clone();
        let transfer_pool = state.transfer_pool.clone();
        let limits = state.runtime_limits;
        let closer = closer.clone();
        let cancellations = state.upload_cancellations.clone();
        let upload_budget = upload_budget.clone();
        let principal_key = principal_key.to_owned();
        let op_id = request.op_id.clone();
        let id = request.id;
        tokio::spawn(async move {
            let _ = crate::file_share::run_file_share(
                engine,
                file_share,
                op_id,
                principal_key,
                id,
                request_permit,
                outbound,
                registry,
                stream_ids,
                transfer_pool,
                limits,
                closer,
                cancellations,
                upload_budget,
            )
            .await;
        });
        return true;
    }

    if request.call.op == "file.read" {
        let Some(file_read) = request
            .call
            .input_any()
            .downcast_ref::<jeliya_api::FileRead>()
            .cloned()
        else {
            return send_api_err(out_tx, request.id, jeliya_api::ApiError::MalformedFrame).await;
        };
        let engine = state.engine.clone();
        let outbound = out_tx.clone();
        let registry = streams.clone();
        let stream_ids = stream_ids.clone();
        let transfer_pool = state.transfer_pool.clone();
        let limits = state.runtime_limits;
        let closer = closer.clone();
        let id = request.id;
        inflight.spawn(async move {
            crate::file_read::run_file_read(
                engine,
                file_read,
                id,
                request_permit,
                outbound,
                registry,
                stream_ids,
                transfer_pool,
                limits,
                closer,
            )
            .await
        });
        return true;
    }

    // Upload cancellation is transport-owned and principal-scoped. Its own
    // envelope op_id is intentionally ignored; only transfer_op_id selects a
    // cancellable upload. Structural decoding has already succeeded, and the
    // subject precondition remains ahead of the live-transfer lookup.
    if request.call.op == "transfer.cancel" {
        let Some(cancel) = request
            .call
            .input_any()
            .downcast_ref::<jeliya_api::TransferCancel>()
            .cloned()
        else {
            return send_api_err(out_tx, request.id, jeliya_api::ApiError::MalformedFrame).await;
        };
        let id = request.id;
        let engine = state.engine.clone();
        let cancellations = state.upload_cancellations.clone();
        let principal_key = principal_key.to_owned();
        let out_tx = out_tx.clone();
        inflight.spawn(async move {
            let _request_permit = request_permit;
            let result = match engine.validate_transfer_cancel() {
                Ok(()) => cancellations.cancel(&principal_key, &cancel).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(out) => send_typed(&out_tx, id, &out).await,
                Err(error) => send_api_err(&out_tx, id, error).await,
            }
        });
        return true;
    }

    let Some(call) = jeliya_core::typed::resolve_call(request.call.op, request.call.input_any())
    else {
        let reply = jeliya_codec::Reply::from_result::<jeliya_api::RoomList>(
            request.id,
            Err(jeliya_api::ApiError::MalformedFrame),
        );
        return out_tx.text(reply.to_bytes()).await == crate::outbound::WriteReceipt::Sent;
    };

    // Execute on a spawned task so a slow op (file.fetch, pipe.connect) does
    // not block the loop. `daemon.stop` is sequenced by the engine; its reply
    // is flushed by the writer before teardown. The envelope `op_id` and this
    // connection's principal key drive the dedup ledger.
    let id = request.id;
    let op_id = request.op_id.clone();
    let principal_key = principal_key.to_owned();
    let engine = state.engine.clone();
    let out_tx = out_tx.clone();
    inflight.spawn(async move {
        let _request_permit = request_permit;
        let executed = engine.execute_with(call, op_id, &principal_key).await;
        let reply = match executed.reply {
            Ok(out) => jeliya_codec::Reply {
                id,
                ok: true,
                out: serde_json::to_value(out).ok(),
                err: None,
            },
            Err(err) => jeliya_codec::Reply {
                id,
                ok: false,
                out: None,
                err: Some(err),
            },
        };
        out_tx.text(reply.to_bytes()).await == crate::outbound::WriteReceipt::Sent
    });
    true
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
        "webmanifest" => "application/manifest+json; charset=utf-8",
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::{FutureExt, SinkExt, StreamExt};
    use hyper::{header::CONTENT_TYPE, StatusCode};
    use tempfile::TempDir;
    use tokio_tungstenite::tungstenite::{protocol::Role, Message};
    use tokio_tungstenite::WebSocketStream;

    use super::{
        complete_data_message_len, constant_time_eq, gate_refusal, safe_download_mime, serve_ws,
        stream_start_position,
    };

    const SOCKET_FRAME_BYTES: usize = 4_096;

    fn test_state(max_frame_bytes: u64) -> (TempDir, crate::AppState) {
        let dir = TempDir::new().expect("server tempdir");
        let (shutdown_tx, _shutdown_rx) = tokio::sync::mpsc::channel(1);
        let engine = jeliya_core::engine::Engine::new(
            dir.path().to_path_buf(),
            true,
            jeliya_core::engine::EngineConfig {
                port: 0,
                version: "test".into(),
                shutdown_tx,
            },
        )
        .expect("test engine");
        let mut limits = engine.limits();
        limits.max_frame_bytes = max_frame_bytes;
        let runtime_limits =
            crate::transfer::RuntimeLimits::from_served(&limits).expect("test runtime limits");
        let transfer_pool = crate::transfer::TransferPool::from_runtime(&runtime_limits);
        let upload_cancellations =
            crate::file_share::UploadCancellationRegistry::with_engine(&engine);
        let state = crate::AppState {
            data_dir: dir.path().to_path_buf(),
            engine,
            auth_token: Arc::new("test-token".into()),
            port: 0,
            connect_tickets: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            connections: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            runtime_limits,
            transfer_pool,
            upload_cancellations,
        };
        (dir, state)
    }

    fn configure_transfer_pool(
        state: &mut crate::AppState,
        max_concurrent_transfers: u64,
        max_transfer_bytes_inflight: u64,
    ) {
        let mut limits = state.engine.limits();
        limits.max_frame_bytes = SOCKET_FRAME_BYTES as u64;
        limits.max_concurrent_transfers = max_concurrent_transfers;
        limits.max_transfer_bytes_inflight = max_transfer_bytes_inflight;
        state.runtime_limits =
            crate::transfer::RuntimeLimits::from_served(&limits).expect("test runtime limits");
        state.transfer_pool = crate::transfer::TransferPool::from_runtime(&state.runtime_limits);
    }

    fn configure_transfer_timers(
        state: &mut crate::AppState,
        transfer_stall_ms: u64,
        transfer_connect_allowance_ms: u64,
        transfer_floor_bits_per_second: u64,
    ) {
        let mut limits = state.engine.limits();
        limits.max_frame_bytes = SOCKET_FRAME_BYTES as u64;
        limits.transfer_stall_ms = transfer_stall_ms;
        limits.transfer_connect_allowance_ms = transfer_connect_allowance_ms;
        limits.transfer_floor_bits_per_second = transfer_floor_bits_per_second;
        state.runtime_limits =
            crate::transfer::RuntimeLimits::from_served(&limits).expect("test runtime limits");
        state.transfer_pool = crate::transfer::TransferPool::from_runtime(&state.runtime_limits);
    }

    async fn socket_pair(
        state: crate::AppState,
    ) -> (
        WebSocketStream<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        socket_pair_as(state, "serve-test").await
    }

    async fn socket_pair_as(
        state: crate::AppState,
        client_id: &str,
    ) -> (
        WebSocketStream<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<()>,
    ) {
        let (server_io, client_io) = tokio::io::duplex(1 << 20);
        let server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let client_id = client_id.to_owned();
        let task = tokio::spawn(serve_ws(
            server,
            state,
            jeliya_codec::SessionPrincipal {
                credential: "test-token".into(),
                client_id: Some(client_id),
            },
        ));
        (client, task)
    }

    async fn file_state(
        payload: &[u8],
        max_frame_bytes: u64,
    ) -> (TempDir, crate::AppState, jeliya_api::FileRead) {
        let (dir, state) = test_state(max_frame_bytes);
        state
            .engine
            .execute(jeliya_core::typed::TypedCall::SubjectEnsure(
                jeliya_api::SubjectEnsure {},
            ))
            .await
            .reply
            .expect("subject.ensure");
        let created = state
            .engine
            .execute(jeliya_core::typed::TypedCall::RoomCreate(
                jeliya_api::RoomCreate {
                    name: "socket file read".into(),
                },
            ))
            .await
            .reply
            .expect("room.create");
        let jeliya_core::typed::TypedReply::RoomCreate(created) = created else {
            panic!("wrong room.create reply");
        };
        state
            .engine
            .execute(jeliya_core::typed::TypedCall::RoomActivate(
                jeliya_api::RoomActivate {
                    room_id: created.room_id.clone(),
                },
            ))
            .await
            .reply
            .expect("room.activate");
        let staged = dir.path().join("socket-source.bin");
        std::fs::write(&staged, payload).expect("write socket source");
        let shared = state
            .engine
            .share_staged_file(
                &jeliya_api::FileShare {
                    room_id: created.room_id.clone(),
                    name: "socket-source.bin".into(),
                    declared_bytes: payload.len() as u64,
                    declared_content_type: "application/octet-stream".into(),
                },
                &staged,
            )
            .await
            .expect("host-staged share");
        let state_path = dir.path().join("state.json");
        let mut local_state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).expect("read local state"))
                .expect("decode local state");
        local_state["rooms"][created.room_id.as_str()]["fetched_files"][shared.file_id.as_str()] = serde_json::json!({
            "path": staged,
            "bytes": payload.len(),
            "fetched_at_ms": 0,
        });
        std::fs::write(
            state_path,
            serde_json::to_vec_pretty(&local_state).expect("encode local state"),
        )
        .expect("write local state");
        let request = jeliya_api::FileRead {
            room_id: created.room_id,
            file_id: shared.file_id,
        };
        (dir, state, request)
    }

    fn stream_wire(kind: u8, request_id: u64, stream_id: u128, offset: u64, value: u64) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(jeliya_codec::STREAM_HEADER_BYTES);
        bytes.extend_from_slice(b"JBS2");
        bytes.push(kind);
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&request_id.to_be_bytes());
        bytes.extend_from_slice(&stream_id.to_be_bytes());
        bytes.extend_from_slice(&offset.to_be_bytes());
        bytes.extend_from_slice(&value.to_be_bytes());
        bytes
    }

    fn stream_data_wire(request_id: u64, stream_id: u128, offset: u64, payload: &[u8]) -> Vec<u8> {
        let mut bytes = stream_wire(0x02, request_id, stream_id, offset, 0);
        bytes.extend_from_slice(payload);
        bytes
    }

    async fn send_file_read(
        client: &mut WebSocketStream<tokio::io::DuplexStream>,
        id: u64,
        request: &jeliya_api::FileRead,
    ) {
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": id,
                    "op": "file.read",
                    "in": request,
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send file.read");
    }

    async fn complete_open_file_read(
        client: &mut WebSocketStream<tokio::io::DuplexStream>,
        request_id: u64,
        stream_id: u128,
        expected: &[u8],
        bounds: &jeliya_codec::CodecBounds,
    ) -> jeliya_codec::Reply {
        let total = u64::try_from(expected.len()).unwrap();
        client
            .send(Message::Binary(
                stream_wire(0x03, request_id, stream_id, 0, total).into(),
            ))
            .await
            .expect("grant file.read credit");

        let mut accepted = 0_u64;
        loop {
            let Message::Binary(record) = client.next().await.unwrap().unwrap() else {
                panic!("file.read success reply preceded END");
            };
            let record = jeliya_codec::decode_stream_record(&record, bounds).unwrap();
            match record.body {
                jeliya_codec::StreamRecordBody::Data { offset, payload } => {
                    assert_eq!(offset, accepted);
                    let start = usize::try_from(accepted).unwrap();
                    assert_eq!(payload, expected[start..start + payload.len()]);
                    accepted += u64::try_from(payload.len()).unwrap();
                    client
                        .send(Message::Binary(
                            stream_wire(0x03, request_id, stream_id, accepted, total).into(),
                        ))
                        .await
                        .expect("acknowledge file.read DATA");
                }
                jeliya_codec::StreamRecordBody::End { total: observed } => {
                    assert_eq!(observed, total);
                    assert_eq!(accepted, total);
                    break;
                }
                other => panic!("unexpected file.read record: {other:?}"),
            }
        }

        let Message::Text(reply) = client.next().await.unwrap().unwrap() else {
            panic!("file.read terminal success must be Text");
        };
        let reply: jeliya_codec::Reply = serde_json::from_str(&reply).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.id, request_id);
        reply
    }

    async fn next_socket_message(client: &mut WebSocketStream<tokio::io::DuplexStream>) -> Message {
        tokio::time::timeout(std::time::Duration::from_secs(10), client.next())
            .await
            .expect("WebSocket stream made progress")
            .expect("WebSocket connection remained open")
            .expect("WebSocket message was readable")
    }

    async fn upload_through_declared_boundary(
        client: &mut WebSocketStream<tokio::io::DuplexStream>,
        request_id: u64,
        identity: jeliya_codec::StreamIdentity,
        declared: u64,
        chunk_bytes: usize,
        bounds: &jeliya_codec::CodecBounds,
    ) {
        assert!(chunk_bytes > 0);
        let chunk_bytes_u64 = u64::try_from(chunk_bytes).unwrap();
        let payload = vec![0x5a; chunk_bytes];
        let mut offset = 0_u64;
        while offset < declared {
            let remaining = declared - offset;
            let payload_len = usize::try_from(remaining.min(chunk_bytes_u64)).unwrap();
            client
                .send(Message::Binary(
                    stream_data_wire(
                        request_id,
                        identity.stream_id().get(),
                        offset,
                        &payload[..payload_len],
                    )
                    .into(),
                ))
                .await
                .expect("send one bounded upload DATA record");
            offset += u64::try_from(payload_len).unwrap();

            let credit = tokio::time::timeout(std::time::Duration::from_secs(30), client.next())
                .await
                .expect("bounded upload must continue making sink progress")
                .expect("bounded upload connection remains open")
                .expect("bounded upload CREDIT is readable");
            let Message::Binary(credit) = credit else {
                panic!("accepted upload DATA must advance Binary CREDIT");
            };
            let credit = jeliya_codec::decode_stream_record(&credit, bounds).unwrap();
            assert_eq!(credit.identity, identity);
            let send_through = if offset == declared {
                declared.checked_add(1).unwrap()
            } else {
                offset.checked_add(chunk_bytes_u64).unwrap().min(declared)
            };
            assert_eq!(
                credit.body,
                jeliya_codec::StreamRecordBody::Credit {
                    accepted_through: offset,
                    send_through,
                }
            );
        }
    }

    async fn open_one_byte_file_share(
        client: &mut WebSocketStream<tokio::io::DuplexStream>,
        request_id: u64,
        room_id: &str,
        name: &str,
        bounds: &jeliya_codec::CodecBounds,
    ) -> jeliya_codec::StreamIdentity {
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": request_id,
                    "op": "file.share",
                    "in": {
                        "room_id": room_id,
                        "name": name,
                        "declared_bytes": 1,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send one-byte file.share");
        let Message::Binary(open) = next_socket_message(client).await else {
            panic!("admitted file.share must begin with Binary OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, bounds).expect("decode upload OPEN");
        assert_eq!(open.body, jeliya_codec::StreamRecordBody::Open { total: 1 });

        let Message::Binary(credit) = next_socket_message(client).await else {
            panic!("upload OPEN must be followed by Binary CREDIT");
        };
        let credit =
            jeliya_codec::decode_stream_record(&credit, bounds).expect("decode initial CREDIT");
        assert_eq!(credit.identity, open.identity);
        assert_eq!(
            credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            }
        );
        open.identity
    }

    #[test]
    fn gate_refusal_is_bare_json_with_the_exact_media_type() {
        let response = gate_refusal(jeliya_codec::GateRejection {
            body: jeliya_api::ApiError::ProtocolUnsupported {
                supported: vec![2],
                client: jeliya_api::DeclaredVersion::Declared { v: 1 },
            },
            status: 426,
        });

        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    }

    #[test]
    fn stream_start_uses_the_last_held_position_not_the_next_position() {
        assert_eq!(stream_start_position(Some(41)), 41);
        assert_eq!(stream_start_position(None), 0);
    }

    #[test]
    fn constant_time_eq_matches_only_identical_bytes() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd")); // length mismatch
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn safe_download_mime_neutralizes_active_types() {
        // Executable-as-document types collapse to octet-stream.
        for dangerous in [
            "text/html",
            "text/html; charset=utf-8",
            "image/svg+xml",
            "application/xhtml+xml",
            "application/javascript",
            "text/javascript",
            "",
            "application/x-shellscript",
        ] {
            assert_eq!(
                safe_download_mime(dangerous),
                "application/octet-stream",
                "{dangerous} must be neutralized"
            );
        }
        // Known-inert types pass through (normalized/lowercased, params dropped).
        assert_eq!(safe_download_mime("image/png"), "image/png");
        assert_eq!(safe_download_mime("IMAGE/PNG"), "image/png");
        assert_eq!(safe_download_mime("application/pdf"), "application/pdf");
        assert_eq!(safe_download_mime("video/mp4"), "video/mp4");
        assert_eq!(
            safe_download_mime("text/plain; charset=utf-8"),
            "text/plain"
        );
    }

    #[test]
    fn complete_message_size_is_class_agnostic_and_excludes_controls() {
        assert_eq!(
            complete_data_message_len(&Message::Text("123".into())),
            Some(3)
        );
        assert_eq!(
            complete_data_message_len(&Message::Binary(vec![0; 4].into())),
            Some(4)
        );
        assert_eq!(
            complete_data_message_len(&Message::Ping(vec![0; 8].into())),
            None
        );
    }

    #[tokio::test]
    async fn websocket_enforces_text_json_binary_stream_and_size_before_parse() {
        let (_dir, state) = test_state(SOCKET_FRAME_BYTES as u64);
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));

        client
            .send(Message::Text(
                r#"{"id":1,"op":"subject.ensure","in":{}}"#.into(),
            ))
            .await
            .unwrap();
        let reply = client.next().await.expect("reply").expect("valid reply");
        let Message::Text(reply) = reply else {
            panic!("JSON reply must be Text");
        };
        let reply: jeliya_codec::Reply = serde_json::from_str(&reply).unwrap();
        assert!(reply.ok);

        // Valid JSON in Binary is never offered to the JSON decoder. It is
        // shorter than JBS2's header and therefore closes unbound with 4007.
        client
            .send(Message::Binary(
                br#"{"id":2,"op":"room.list","in":{}}"#.to_vec().into(),
            ))
            .await
            .unwrap();
        let close = client.next().await.expect("close").expect("close frame");
        let Message::Close(Some(close)) = close else {
            panic!("Binary JSON must close");
        };
        assert_eq!(u16::from(close.code), 4007);
        let _ = server.await;

        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        // Oversized and bad-magic: the complete-message bound wins without
        // attempting header, magic, identity, or class-specific parsing.
        client
            .send(Message::Binary(vec![0; SOCKET_FRAME_BYTES + 1].into()))
            .await
            .unwrap();
        let close = client.next().await.expect("close").expect("close frame");
        let Message::Close(Some(close)) = close else {
            panic!("oversized Binary must close");
        };
        assert_eq!(u16::from(close.code), 4005);
        let _ = server.await;

        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text("x".repeat(SOCKET_FRAME_BYTES + 1).into()))
            .await
            .unwrap();
        let close = client.next().await.expect("close").expect("close frame");
        let Message::Close(Some(close)) = close else {
            panic!("oversized malformed Text must close");
        };
        assert_eq!(u16::from(close.code), 4005);
        let _ = server.await;

        let (mut client, server) = socket_pair(state).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        let mut stream_as_text = Vec::new();
        stream_as_text.extend_from_slice(b"JBS2");
        stream_as_text.resize(48, 0);
        stream_as_text[31] = 1; // structurally nonzero stream id, still UTF-8
        client
            .send(Message::Text(
                String::from_utf8(stream_as_text).unwrap().into(),
            ))
            .await
            .unwrap();
        let close = client.next().await.expect("close").expect("close frame");
        let Message::Close(Some(close)) = close else {
            panic!("Text stream record must close");
        };
        assert_eq!(u16::from(close.code), 4007);
        let _ = server.await;
    }

    #[tokio::test]
    async fn websocket_file_read_disconnects_cleanly_and_restarts_from_zero() {
        let payload: Vec<u8> = (0..300).map(|value| (value % 251) as u8).collect();
        let (_dir, state, request) = file_state(&payload, SOCKET_FRAME_BYTES as u64).await;

        let (mut first, first_server) = socket_pair(state.clone()).await;
        assert!(matches!(first.next().await, Some(Ok(Message::Text(_)))));
        send_file_read(&mut first, 7, &request).await;
        let Message::Binary(open) = first.next().await.unwrap().unwrap() else {
            panic!("file.read must begin with Binary OPEN");
        };
        let open = jeliya_codec::decode_stream_record(
            &open,
            &jeliya_codec::CodecBounds {
                max_frame_bytes: SOCKET_FRAME_BYTES,
                ..jeliya_codec::CodecBounds::default()
            },
        )
        .expect("decode first OPEN");
        assert_eq!(
            open.body,
            jeliya_codec::StreamRecordBody::Open { total: 300 }
        );

        // The socket reader remains usable for ordinary Text requests while
        // the download is correctly paused before initial CREDIT.
        first
            .send(Message::Text(r#"{"id":8,"op":"room.list","in":{}}"#.into()))
            .await
            .unwrap();
        let Message::Text(ordinary) = first.next().await.unwrap().unwrap() else {
            panic!("ordinary reply must interleave as Text");
        };
        let ordinary: jeliya_codec::Reply = serde_json::from_str(&ordinary).unwrap();
        assert_eq!(ordinary.id, 8);
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), first_server)
            .await
            .expect("disconnect teardown must finish")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        let (mut second, second_server) = socket_pair(state.clone()).await;
        assert!(matches!(second.next().await, Some(Ok(Message::Text(_)))));
        send_file_read(&mut second, 7, &request).await;
        let Message::Binary(open_again) = second.next().await.unwrap().unwrap() else {
            panic!("retry must begin with a fresh OPEN");
        };
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let open_again = jeliya_codec::decode_stream_record(&open_again, &bounds).unwrap();
        assert!(matches!(
            open_again.body,
            jeliya_codec::StreamRecordBody::Open { total: 300 }
        ));
        assert_ne!(
            open_again.identity.stream_id(),
            open.identity.stream_id(),
            "reconnect never resumes or reuses the prior stream"
        );
        let stream_id = open_again.identity.stream_id().get();
        second
            .send(Message::Binary(
                stream_wire(0x03, 7, stream_id, 0, payload.len() as u64).into(),
            ))
            .await
            .unwrap();

        let mut accepted = 0_u64;
        loop {
            match second.next().await.unwrap().unwrap() {
                Message::Binary(record) => {
                    let record = jeliya_codec::decode_stream_record(&record, &bounds).unwrap();
                    match record.body {
                        jeliya_codec::StreamRecordBody::Data {
                            offset,
                            payload: data,
                        } => {
                            assert_eq!(offset, accepted);
                            assert_eq!(
                                data,
                                payload[accepted as usize..accepted as usize + data.len()]
                            );
                            accepted += data.len() as u64;
                            second
                                .send(Message::Binary(
                                    stream_wire(0x03, 7, stream_id, accepted, payload.len() as u64)
                                        .into(),
                                ))
                                .await
                                .unwrap();
                        }
                        jeliya_codec::StreamRecordBody::End { total } => {
                            assert_eq!(total, payload.len() as u64);
                            assert_eq!(accepted, total);
                            break;
                        }
                        other => panic!("unexpected producer record: {other:?}"),
                    }
                }
                other => panic!("success reply preceded END: {other:?}"),
            }
        }
        let Message::Text(reply) = second.next().await.unwrap().unwrap() else {
            panic!("terminal success must be Text");
        };
        let reply: jeliya_codec::Reply = serde_json::from_str(&reply).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.id, 7);

        second
            .send(Message::Text(
                serde_json::json!({
                    "id": 9,
                    "op": "file.share",
                    "in": {
                        "room_id": request.room_id,
                        "name": "empty.bin",
                        "declared_bytes": 0,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(share_open) = second.next().await.unwrap().unwrap() else {
            panic!("file.share must begin with Binary OPEN");
        };
        let share_open = jeliya_codec::decode_stream_record(&share_open, &bounds).unwrap();
        assert_eq!(
            share_open.body,
            jeliya_codec::StreamRecordBody::Open { total: 0 }
        );
        let Message::Binary(share_credit) = second.next().await.unwrap().unwrap() else {
            panic!("OPEN must be followed by Binary CREDIT");
        };
        let share_credit = jeliya_codec::decode_stream_record(&share_credit, &bounds).unwrap();
        assert_eq!(share_credit.identity, share_open.identity);
        assert_eq!(
            share_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            }
        );
        second
            .send(Message::Binary(
                stream_wire(0x04, 9, share_open.identity.stream_id().get(), 0, 0).into(),
            ))
            .await
            .unwrap();
        let Message::Text(share_reply) = second.next().await.unwrap().unwrap() else {
            panic!("file.share success must follow END as Text");
        };
        let share_reply: jeliya_codec::Reply = serde_json::from_str(&share_reply).unwrap();
        assert!(share_reply.ok);
        assert_eq!(share_reply.id, 9);
        second.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), second_server)
            .await
            .expect("completed connection teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
    }

    #[tokio::test]
    async fn websocket_file_share_joins_replays_and_authors_once() {
        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let room_id = existing.room_id;
        let request = serde_json::json!({
            "op_id": "op-socket-upload-join-1",
            "op": "file.share",
            "in": {
                "room_id": room_id,
                "name": "joined.bin",
                "declared_bytes": 3,
                "declared_content_type": "application/octet-stream",
            }
        });
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };

        let (mut owner, owner_server) = socket_pair(state.clone()).await;
        let (mut joiner, joiner_server) = socket_pair(state.clone()).await;
        assert!(matches!(owner.next().await, Some(Ok(Message::Text(_)))));
        assert!(matches!(joiner.next().await, Some(Ok(Message::Text(_)))));

        let mut owner_request = request.clone();
        owner_request["id"] = 31.into();
        owner
            .send(Message::Text(owner_request.to_string().into()))
            .await
            .unwrap();
        let Message::Binary(open) = owner.next().await.unwrap().unwrap() else {
            panic!("fresh upload owner must receive OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        assert_eq!(open.body, jeliya_codec::StreamRecordBody::Open { total: 3 });
        let identity = open.identity;
        let Message::Binary(initial_credit) = owner.next().await.unwrap().unwrap() else {
            panic!("OPEN must be followed by CREDIT");
        };
        let initial_credit = jeliya_codec::decode_stream_record(&initial_credit, &bounds).unwrap();
        assert_eq!(initial_credit.identity, identity);
        assert_eq!(
            initial_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 3,
            }
        );

        let mut joined_request = request.clone();
        joined_request["id"] = 32.into();
        joiner
            .send(Message::Text(joined_request.to_string().into()))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), joiner.next())
                .await
                .is_err(),
            "faithful concurrent replay must neither open nor reply before its owner"
        );

        owner
            .send(Message::Binary(
                stream_data_wire(31, identity.stream_id().get(), 0, &[1, 2]).into(),
            ))
            .await
            .unwrap();
        owner
            .send(Message::Binary(
                stream_data_wire(31, identity.stream_id().get(), 2, &[3]).into(),
            ))
            .await
            .unwrap();
        let Message::Binary(sentinel_credit) = owner.next().await.unwrap().unwrap() else {
            panic!("accepted declaration must expose the sentinel CREDIT");
        };
        let sentinel_credit =
            jeliya_codec::decode_stream_record(&sentinel_credit, &bounds).unwrap();
        assert_eq!(sentinel_credit.identity, identity);
        assert_eq!(
            sentinel_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 3,
                send_through: 4,
            }
        );
        owner
            .send(Message::Binary(
                stream_wire(0x04, 31, identity.stream_id().get(), 3, 0).into(),
            ))
            .await
            .unwrap();

        let Message::Text(owner_reply) = owner.next().await.unwrap().unwrap() else {
            panic!("owner terminal must be Text");
        };
        let owner_reply: jeliya_codec::Reply = serde_json::from_str(&owner_reply).unwrap();
        assert!(owner_reply.ok);
        let Message::Text(joined_reply) = joiner.next().await.unwrap().unwrap() else {
            panic!("faithful join terminal must be Text only");
        };
        let joined_reply: jeliya_codec::Reply = serde_json::from_str(&joined_reply).unwrap();
        assert!(joined_reply.ok);
        assert_eq!(joined_reply.out, owner_reply.out);

        let mut replay_request = request.clone();
        replay_request["id"] = 33.into();
        joiner
            .send(Message::Text(replay_request.to_string().into()))
            .await
            .unwrap();
        let Message::Text(replayed) = joiner.next().await.unwrap().unwrap() else {
            panic!("completed replay must not open a second stream");
        };
        let replayed: jeliya_codec::Reply = serde_json::from_str(&replayed).unwrap();
        assert_eq!(replayed.out, owner_reply.out);

        let mut divergent = request;
        divergent["id"] = 34.into();
        divergent["in"]["name"] = "different.bin".into();
        joiner
            .send(Message::Text(divergent.to_string().into()))
            .await
            .unwrap();
        let Message::Text(conflict) = joiner.next().await.unwrap().unwrap() else {
            panic!("divergent replay refusal must be Text");
        };
        let conflict: jeliya_codec::Reply = serde_json::from_str(&conflict).unwrap();
        assert!(matches!(
            conflict.err,
            Some(jeliya_api::ApiError::OpIdConflict { .. })
        ));

        let listed = state
            .engine
            .execute(jeliya_core::typed::TypedCall::FileList(
                jeliya_api::FileList {
                    room_id: room_id.clone(),
                    page: jeliya_api::Page {
                        cursor: jeliya_api::Cursor::Start,
                        direction: jeliya_api::Direction::Forward,
                        limit: 100,
                    },
                },
            ))
            .await
            .reply
            .expect("file.list after streamed share");
        let jeliya_core::typed::TypedReply::FileList(listed) = listed else {
            panic!("wrong file.list reply");
        };
        assert_eq!(
            listed
                .files
                .iter()
                .filter(|file| file.name == "joined.bin")
                .count(),
            1,
            "join and replay must not import or author again"
        );

        joiner
            .send(Message::Text(
                serde_json::json!({
                    "id": 35,
                    "op": "transfer.cancel",
                    "in": { "transfer_op_id": "op-socket-upload-join-1" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(completed_cancel) = joiner.next().await.unwrap().unwrap() else {
            panic!("completed transfer cancel refusal must be Text");
        };
        let completed_cancel: jeliya_codec::Reply =
            serde_json::from_str(&completed_cancel).unwrap();
        assert!(matches!(
            completed_cancel.err,
            Some(jeliya_api::ApiError::TransferUnknown { .. })
        ));

        owner.close(None).await.unwrap();
        joiner.close(None).await.unwrap();
        for server in [owner_server, joiner_server] {
            tokio::time::timeout(std::time::Duration::from_secs(1), server)
                .await
                .expect("connection teardown")
                .expect("serve task");
        }
        assert_eq!(state.transfer_pool.usage(), (0, 0));
    }

    #[tokio::test]
    async fn websocket_file_share_succeeds_at_served_maximum_with_sentinel_credit() {
        const EXPECTED_MAX_SHARED_BYTES: u64 = 100 * 1024 * 1024;
        const DATA_PAYLOAD_BYTES: usize = 65_536;
        const DATA_PAYLOAD_BYTES_U64: u64 = 65_536;
        const REQUEST_ID: u64 = 36;

        // A complete DATA record is bounded to 64 KiB by RuntimeLimits. Keep
        // exactly one such payload resident and wait for advancing CREDIT
        // after each record, so the test never allocates or queues the whole
        // 100 MiB upload.
        let max_frame_bytes = jeliya_codec::STREAM_HEADER_BYTES + DATA_PAYLOAD_BYTES;
        let (dir, state) = test_state(u64::try_from(max_frame_bytes).unwrap());
        let declared = state.engine.limits().max_shared_file_bytes;
        assert_eq!(declared, EXPECTED_MAX_SHARED_BYTES);
        assert_eq!(
            state.runtime_limits.max_data_payload_bytes(),
            DATA_PAYLOAD_BYTES
        );

        state
            .engine
            .execute(jeliya_core::typed::TypedCall::SubjectEnsure(
                jeliya_api::SubjectEnsure {},
            ))
            .await
            .reply
            .expect("subject.ensure");
        let created = state
            .engine
            .execute(jeliya_core::typed::TypedCall::RoomCreate(
                jeliya_api::RoomCreate {
                    name: "maximum socket upload".into(),
                },
            ))
            .await
            .reply
            .expect("room.create");
        let jeliya_core::typed::TypedReply::RoomCreate(created) = created else {
            panic!("wrong room.create reply");
        };
        state
            .engine
            .execute(jeliya_core::typed::TypedCall::RoomActivate(
                jeliya_api::RoomActivate {
                    room_id: created.room_id.clone(),
                },
            ))
            .await
            .reply
            .expect("room.activate");

        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": REQUEST_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": created.room_id,
                        "name": "served-maximum.bin",
                        "declared_bytes": declared,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send maximum file.share");

        let Message::Binary(open) = client.next().await.unwrap().unwrap() else {
            panic!("maximum upload must begin with Binary OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        assert_eq!(
            open.body,
            jeliya_codec::StreamRecordBody::Open { total: declared }
        );
        let identity = open.identity;

        let Message::Binary(initial_credit) = client.next().await.unwrap().unwrap() else {
            panic!("OPEN must be followed by Binary CREDIT");
        };
        let initial_credit = jeliya_codec::decode_stream_record(&initial_credit, &bounds).unwrap();
        assert_eq!(initial_credit.identity, identity);
        assert_eq!(
            initial_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: DATA_PAYLOAD_BYTES_U64,
            }
        );

        upload_through_declared_boundary(
            &mut client,
            REQUEST_ID,
            identity,
            declared,
            DATA_PAYLOAD_BYTES,
            &bounds,
        )
        .await;

        client
            .send(Message::Binary(
                stream_wire(0x04, REQUEST_ID, identity.stream_id().get(), declared, 0).into(),
            ))
            .await
            .expect("send END at the served maximum");
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(120), client.next())
            .await
            .expect("maximum upload finalization completes")
            .expect("maximum upload connection remains open")
            .expect("maximum upload terminal is readable");
        let Message::Text(terminal) = terminal else {
            panic!("maximum upload terminal must be Text");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert!(terminal.ok, "maximum upload failed: {:?}", terminal.err);
        let output: jeliya_api::FileShareOut =
            serde_json::from_value(terminal.out.expect("maximum upload output")).unwrap();
        assert_eq!(output.bytes, declared);
        assert!(!output.digest.is_empty());

        let listed = state
            .engine
            .execute(jeliya_core::typed::TypedCall::FileList(
                jeliya_api::FileList {
                    room_id: created.room_id.clone(),
                    page: jeliya_api::Page {
                        cursor: jeliya_api::Cursor::Start,
                        direction: jeliya_api::Direction::Forward,
                        limit: 100,
                    },
                },
            ))
            .await
            .reply
            .expect("file.list after maximum upload");
        let jeliya_core::typed::TypedReply::FileList(listed) = listed else {
            panic!("wrong file.list reply");
        };
        assert_eq!(listed.files.len(), 1, "exactly one file_shared event");
        assert_eq!(listed.files[0].file_id, output.file_id);
        assert_eq!(listed.files[0].name, "served-maximum.bin");

        let protocol_staging = dir.path().join("protocol-v2-stream-staging");
        assert!(protocol_staging.is_dir());
        assert_eq!(
            std::fs::read_dir(protocol_staging).unwrap().count(),
            0,
            "successful maximum upload leaves no protocol staging residue"
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_file_share_declared_max_plus_one_refuses_before_open() {
        const REQUEST_ID: u64 = 131;
        const SURVIVAL_ID: u64 = 132;

        let (dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let limit = state.engine.limits().max_shared_file_bytes;
        let declared = limit.checked_add(1).unwrap();
        let protocol_staging = dir.path().join("protocol-v2-stream-staging");
        assert!(!protocol_staging.exists());

        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": REQUEST_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "declared-too-large.bin",
                        "declared_bytes": declared,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

        let Message::Text(refused) = next_socket_message(&mut client).await else {
            panic!("stage_declared refusal must be Text without OPEN");
        };
        let refused: jeliya_codec::Reply = serde_json::from_str(&refused).unwrap();
        assert_eq!(refused.id, REQUEST_ID);
        assert_eq!(
            refused.err,
            Some(jeliya_api::ApiError::FileTooLarge {
                declared_bytes: declared,
                limit_bytes: limit,
                enforced_at: jeliya_api::EnforcedAt::StageDeclared,
            })
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert!(
            !protocol_staging.exists(),
            "declared policy runs before reservation and staging"
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SURVIVAL_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(survival) = next_socket_message(&mut client).await else {
            panic!("stage_declared refusal must leave the connection usable");
        };
        let survival: jeliya_codec::Reply = serde_json::from_str(&survival).unwrap();
        assert_eq!(survival.id, SURVIVAL_ID);
        assert!(survival.ok);

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_file_share_declared_sentinel_data_aborts_then_replies_mismatch() {
        const REQUEST_ID: u64 = 133;
        const SURVIVAL_ID: u64 = 134;
        const DECLARED: u64 = 3;

        let (dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": REQUEST_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "declared-sentinel-probe.bin",
                        "declared_bytes": DECLARED,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(open) = next_socket_message(&mut client).await else {
            panic!("ordinary upload must OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        assert_eq!(
            open.body,
            jeliya_codec::StreamRecordBody::Open { total: DECLARED }
        );
        let identity = open.identity;
        let Message::Binary(initial_credit) = next_socket_message(&mut client).await else {
            panic!("ordinary upload OPEN must be followed by CREDIT");
        };
        let initial_credit = jeliya_codec::decode_stream_record(&initial_credit, &bounds).unwrap();
        assert_eq!(initial_credit.identity, identity);
        assert_eq!(
            initial_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: DECLARED,
            }
        );

        client
            .send(Message::Binary(
                stream_data_wire(REQUEST_ID, identity.stream_id().get(), 0, b"abc").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(sentinel_credit) = next_socket_message(&mut client).await else {
            panic!("accepted declaration must expose sentinel CREDIT");
        };
        let sentinel_credit =
            jeliya_codec::decode_stream_record(&sentinel_credit, &bounds).unwrap();
        assert_eq!(sentinel_credit.identity, identity);
        assert_eq!(
            sentinel_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: DECLARED,
                send_through: DECLARED + 1,
            }
        );

        client
            .send(Message::Binary(
                stream_data_wire(REQUEST_ID, identity.stream_id().get(), DECLARED, b"x").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(abort) = next_socket_message(&mut client).await else {
            panic!("one-past declaration DATA must receive daemon ABORT");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: DECLARED,
                reason: jeliya_codec::BinaryAbortReason::OperationError,
            }
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        client
            .send(Message::Binary(
                stream_wire(0x06, REQUEST_ID, identity.stream_id().get(), DECLARED, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(terminal) = next_socket_message(&mut client).await else {
            panic!("declared mismatch terminal must follow client ACK as Text");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.id, REQUEST_ID);
        assert_eq!(
            terminal.err,
            Some(jeliya_api::ApiError::DeclaredSizeMismatch {
                declared_bytes: DECLARED,
                observed_bytes: DECLARED + 1,
            })
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("protocol-v2-stream-staging"))
                .unwrap()
                .count(),
            0
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SURVIVAL_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(survival) = next_socket_message(&mut client).await else {
            panic!("declared mismatch must leave the connection usable");
        };
        let survival: jeliya_codec::Reply = serde_json::from_str(&survival).unwrap();
        assert_eq!(survival.id, SURVIVAL_ID);
        assert!(survival.ok);

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_file_share_maximum_sentinel_data_aborts_then_replies_stage_stream() {
        const EXPECTED_MAX_SHARED_BYTES: u64 = 100 * 1024 * 1024;
        const DATA_PAYLOAD_BYTES: usize = 65_536;
        const DATA_PAYLOAD_BYTES_U64: u64 = 65_536;
        const REQUEST_ID: u64 = 135;
        const SURVIVAL_ID: u64 = 136;

        let max_frame_bytes = jeliya_codec::STREAM_HEADER_BYTES + DATA_PAYLOAD_BYTES;
        let (dir, state, existing) =
            file_state(b"seed", u64::try_from(max_frame_bytes).unwrap()).await;
        let declared = state.engine.limits().max_shared_file_bytes;
        assert_eq!(declared, EXPECTED_MAX_SHARED_BYTES);
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": REQUEST_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "maximum-sentinel-probe.bin",
                        "declared_bytes": declared,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(open) = next_socket_message(&mut client).await else {
            panic!("maximum sentinel probe must OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        assert_eq!(
            open.body,
            jeliya_codec::StreamRecordBody::Open { total: declared }
        );
        let identity = open.identity;
        let Message::Binary(initial_credit) = next_socket_message(&mut client).await else {
            panic!("maximum OPEN must be followed by CREDIT");
        };
        let initial_credit = jeliya_codec::decode_stream_record(&initial_credit, &bounds).unwrap();
        assert_eq!(initial_credit.identity, identity);
        assert_eq!(
            initial_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: DATA_PAYLOAD_BYTES_U64,
            }
        );
        upload_through_declared_boundary(
            &mut client,
            REQUEST_ID,
            identity,
            declared,
            DATA_PAYLOAD_BYTES,
            &bounds,
        )
        .await;

        client
            .send(Message::Binary(
                stream_data_wire(REQUEST_ID, identity.stream_id().get(), declared, b"x").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(abort) = next_socket_message(&mut client).await else {
            panic!("maximum+1 DATA must receive daemon ABORT");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: declared,
                reason: jeliya_codec::BinaryAbortReason::OperationError,
            }
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        client
            .send(Message::Binary(
                stream_wire(0x06, REQUEST_ID, identity.stream_id().get(), declared, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(terminal) = next_socket_message(&mut client).await else {
            panic!("stage_stream terminal must follow client ACK as Text");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.id, REQUEST_ID);
        assert_eq!(
            terminal.err,
            Some(jeliya_api::ApiError::FileTooLarge {
                declared_bytes: declared,
                limit_bytes: declared,
                enforced_at: jeliya_api::EnforcedAt::StageStream,
            })
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("protocol-v2-stream-staging"))
                .unwrap()
                .count(),
            0,
            "maximum sentinel is observed but never staged"
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SURVIVAL_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(survival) = next_socket_message(&mut client).await else {
            panic!("stage_stream refusal must leave the connection usable");
        };
        let survival: jeliya_codec::Reply = serde_json::from_str(&survival).unwrap();
        assert_eq!(survival.id, SURVIVAL_ID);
        assert!(survival.ok);

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_duplicate_end_closes_4007_but_preserves_one_replayable_finalization() {
        const OWNER_ID: u64 = 37;
        const REPLAY_ID: u64 = 38;

        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let room_id = existing.room_id;
        let request = serde_json::json!({
            "op_id": "op-duplicate-end-finalization",
            "op": "file.share",
            "in": {
                "room_id": room_id,
                "name": "duplicate-end.bin",
                "declared_bytes": 0,
                "declared_content_type": "application/octet-stream",
            }
        });
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };

        let (mut owner, owner_server) = socket_pair(state.clone()).await;
        assert!(matches!(owner.next().await, Some(Ok(Message::Text(_)))));
        let mut owner_request = request.clone();
        owner_request["id"] = OWNER_ID.into();
        owner
            .send(Message::Text(owner_request.to_string().into()))
            .await
            .unwrap();
        let Message::Binary(open) = owner.next().await.unwrap().unwrap() else {
            panic!("duplicate-END upload must open");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        let identity = open.identity;
        assert_eq!(open.body, jeliya_codec::StreamRecordBody::Open { total: 0 });
        let Message::Binary(credit) = owner.next().await.unwrap().unwrap() else {
            panic!("duplicate-END upload must receive sentinel CREDIT");
        };
        assert_eq!(
            jeliya_codec::decode_stream_record(&credit, &bounds)
                .unwrap()
                .body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            }
        );

        let end =
            Message::Binary(stream_wire(0x04, OWNER_ID, identity.stream_id().get(), 0, 0).into());
        owner.feed(end.clone()).await.unwrap();
        owner.feed(end).await.unwrap();
        owner.flush().await.unwrap();

        let close = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match owner.next().await {
                    Some(Ok(Message::Close(frame))) => break frame,
                    Some(Ok(Message::Text(reply))) => {
                        let reply: jeliya_codec::Reply = serde_json::from_str(&reply).unwrap();
                        assert!(reply.ok, "accepted END result must not be rewritten");
                    }
                    Some(Ok(other)) => panic!("unexpected post-END message: {other:?}"),
                    Some(Err(error)) => panic!("connection failed before Close: {error}"),
                    None => panic!("connection ended without 4007 Close"),
                }
            }
        })
        .await
        .expect("duplicate END must close promptly");
        let close = close.expect("malformed close carries a frame");
        assert_eq!(close.code, 4007.into());
        tokio::time::timeout(std::time::Duration::from_secs(2), owner_server)
            .await
            .expect("duplicate-END connection teardown")
            .expect("serve task");

        let (mut replay, replay_server) = socket_pair(state.clone()).await;
        assert!(matches!(replay.next().await, Some(Ok(Message::Text(_)))));
        let mut replay_request = request;
        replay_request["id"] = REPLAY_ID.into();
        replay
            .send(Message::Text(replay_request.to_string().into()))
            .await
            .unwrap();
        let Message::Text(replayed) =
            tokio::time::timeout(std::time::Duration::from_secs(5), replay.next())
                .await
                .expect("detached finalization publishes replay")
                .unwrap()
                .unwrap()
        else {
            panic!("faithful replay must return Text without a second OPEN");
        };
        let replayed: jeliya_codec::Reply = serde_json::from_str(&replayed).unwrap();
        assert!(replayed.ok);

        let listed = state
            .engine
            .execute(jeliya_core::typed::TypedCall::FileList(
                jeliya_api::FileList {
                    room_id,
                    page: jeliya_api::Page {
                        cursor: jeliya_api::Cursor::Start,
                        direction: jeliya_api::Direction::Forward,
                        limit: 100,
                    },
                },
            ))
            .await
            .reply
            .expect("file.list after duplicate END");
        let jeliya_core::typed::TypedReply::FileList(listed) = listed else {
            panic!("wrong file.list reply");
        };
        assert_eq!(
            listed
                .files
                .iter()
                .filter(|file| file.name == "duplicate-end.bin")
                .count(),
            1,
            "duplicate END must not author a second event"
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        replay.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), replay_server)
            .await
            .expect("replay connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_shared_transfer_count_exhaustion_crosses_upload_to_download() {
        const UPLOAD_ID: u64 = 91;
        const REFUSED_READ_ID: u64 = 92;
        const ADMITTED_READ_ID: u64 = 93;

        let download = b"read";
        let (_dir, mut state, read_request) = file_state(download, SOCKET_FRAME_BYTES as u64).await;
        configure_transfer_pool(&mut state, 1, 8);
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": UPLOAD_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": read_request.room_id,
                        "name": "count-holder.bin",
                        "declared_bytes": 1,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(upload_open) = client.next().await.unwrap().unwrap() else {
            panic!("first upload must receive OPEN");
        };
        let upload_open = jeliya_codec::decode_stream_record(&upload_open, &bounds).unwrap();
        assert_eq!(
            upload_open.body,
            jeliya_codec::StreamRecordBody::Open { total: 1 }
        );
        let upload_identity = upload_open.identity;
        let Message::Binary(upload_credit) = client.next().await.unwrap().unwrap() else {
            panic!("upload OPEN must be followed by CREDIT");
        };
        let upload_credit = jeliya_codec::decode_stream_record(&upload_credit, &bounds).unwrap();
        assert_eq!(
            upload_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            }
        );
        assert_eq!(state.transfer_pool.usage(), (1, 1));

        send_file_read(&mut client, REFUSED_READ_ID, &read_request).await;
        let Message::Text(refused) = client.next().await.unwrap().unwrap() else {
            panic!("count exhaustion must reply as Text without a second OPEN");
        };
        let refused: jeliya_codec::Reply = serde_json::from_str(&refused).unwrap();
        assert_eq!(
            refused.err,
            Some(jeliya_api::ApiError::ResourceExhausted {
                resource: "max_concurrent_transfers".into(),
                limit: 1,
            })
        );
        assert_eq!(state.transfer_pool.usage(), (1, 1));

        // The refused download cannot disturb the upload that owns the slot.
        client
            .send(Message::Binary(
                stream_data_wire(UPLOAD_ID, upload_identity.stream_id().get(), 0, b"x").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(sentinel) = client.next().await.unwrap().unwrap() else {
            panic!("the active upload must remain usable");
        };
        let sentinel = jeliya_codec::decode_stream_record(&sentinel, &bounds).unwrap();
        assert_eq!(
            sentinel.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            }
        );
        client
            .send(Message::Binary(
                stream_wire(0x04, UPLOAD_ID, upload_identity.stream_id().get(), 1, 0).into(),
            ))
            .await
            .unwrap();
        let Message::Text(upload_reply) = client.next().await.unwrap().unwrap() else {
            panic!("active upload terminal must be Text");
        };
        let upload_reply: jeliya_codec::Reply = serde_json::from_str(&upload_reply).unwrap();
        assert!(upload_reply.ok);
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        // Releasing the upload slot makes the same-direction-independent
        // download admission succeed on this connection.
        send_file_read(&mut client, ADMITTED_READ_ID, &read_request).await;
        let Message::Binary(read_open) = client.next().await.unwrap().unwrap() else {
            panic!("released count capacity must admit file.read OPEN");
        };
        let read_open = jeliya_codec::decode_stream_record(&read_open, &bounds).unwrap();
        assert_eq!(
            read_open.body,
            jeliya_codec::StreamRecordBody::Open {
                total: download.len() as u64,
            }
        );
        complete_open_file_read(
            &mut client,
            ADMITTED_READ_ID,
            read_open.identity.stream_id().get(),
            download,
            &bounds,
        )
        .await;
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_shared_transfer_byte_exhaustion_crosses_download_to_upload() {
        const READ_ID: u64 = 94;
        const REFUSED_UPLOAD_ID: u64 = 95;
        const ADMITTED_UPLOAD_ID: u64 = 96;

        let download = b"read";
        let (dir, mut state, read_request) = file_state(download, SOCKET_FRAME_BYTES as u64).await;
        configure_transfer_pool(&mut state, 2, u64::try_from(download.len()).unwrap());
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));

        send_file_read(&mut client, READ_ID, &read_request).await;
        let Message::Binary(read_open) = client.next().await.unwrap().unwrap() else {
            panic!("first download must receive OPEN");
        };
        let read_open = jeliya_codec::decode_stream_record(&read_open, &bounds).unwrap();
        assert_eq!(
            read_open.body,
            jeliya_codec::StreamRecordBody::Open {
                total: download.len() as u64,
            }
        );
        let read_identity = read_open.identity;
        assert_eq!(
            state.transfer_pool.usage(),
            (1, u64::try_from(download.len()).unwrap())
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": REFUSED_UPLOAD_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": read_request.room_id,
                        "name": "byte-refused.bin",
                        "declared_bytes": 1,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(refused) = client.next().await.unwrap().unwrap() else {
            panic!("byte exhaustion must reply as Text without an upload OPEN");
        };
        let refused: jeliya_codec::Reply = serde_json::from_str(&refused).unwrap();
        assert_eq!(
            refused.err,
            Some(jeliya_api::ApiError::ResourceExhausted {
                resource: "max_transfer_bytes_inflight".into(),
                limit: u64::try_from(download.len()).unwrap(),
            })
        );
        assert!(
            !dir.path().join("protocol-v2-stream-staging").exists(),
            "byte refusal precedes upload staging creation"
        );

        // The refused upload cannot disturb the download holding the bytes.
        complete_open_file_read(
            &mut client,
            READ_ID,
            read_identity.stream_id().get(),
            download,
            &bounds,
        )
        .await;
        assert_eq!(state.transfer_pool.usage(), (0, 0));

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": ADMITTED_UPLOAD_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": read_request.room_id,
                        "name": "byte-admitted.bin",
                        "declared_bytes": 1,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(upload_open) = client.next().await.unwrap().unwrap() else {
            panic!("released byte capacity must admit file.share OPEN");
        };
        let upload_open = jeliya_codec::decode_stream_record(&upload_open, &bounds).unwrap();
        assert_eq!(
            upload_open.body,
            jeliya_codec::StreamRecordBody::Open { total: 1 }
        );
        let upload_identity = upload_open.identity;
        let Message::Binary(upload_credit) = client.next().await.unwrap().unwrap() else {
            panic!("admitted upload OPEN must be followed by CREDIT");
        };
        let upload_credit = jeliya_codec::decode_stream_record(&upload_credit, &bounds).unwrap();
        assert_eq!(
            upload_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: 1,
            }
        );
        client
            .send(Message::Binary(
                stream_data_wire(
                    ADMITTED_UPLOAD_ID,
                    upload_identity.stream_id().get(),
                    0,
                    b"y",
                )
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(sentinel) = client.next().await.unwrap().unwrap() else {
            panic!("admitted upload must reach sentinel CREDIT");
        };
        let sentinel = jeliya_codec::decode_stream_record(&sentinel, &bounds).unwrap();
        assert_eq!(
            sentinel.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            }
        );
        client
            .send(Message::Binary(
                stream_wire(
                    0x04,
                    ADMITTED_UPLOAD_ID,
                    upload_identity.stream_id().get(),
                    1,
                    0,
                )
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(upload_reply) = client.next().await.unwrap().unwrap() else {
            panic!("admitted upload terminal must be Text");
        };
        let upload_reply: jeliya_codec::Reply = serde_json::from_str(&upload_reply).unwrap();
        assert!(upload_reply.ok);
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert_eq!(
            std::fs::read_dir(dir.path().join("protocol-v2-stream-staging"))
                .unwrap()
                .count(),
            0
        );

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_full_tiny_record_credit_window_cannot_starve_controls_or_abort() {
        const UPLOAD_ID: u64 = 118;
        const ORDINARY_ID: u64 = 119;

        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let window = state.runtime_limits.max_data_payload_bytes();
        assert!(window > 4, "regression must exceed the former fixed lane");
        let declared = u64::try_from(window).unwrap();
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": UPLOAD_ID,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "tiny-window.bin",
                        "declared_bytes": declared,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(open) = next_socket_message(&mut client).await else {
            panic!("upload must begin with OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        assert_eq!(
            open.body,
            jeliya_codec::StreamRecordBody::Open { total: declared }
        );
        let identity = open.identity;
        let Message::Binary(credit) = next_socket_message(&mut client).await else {
            panic!("OPEN must be followed by CREDIT");
        };
        let credit = jeliya_codec::decode_stream_record(&credit, &bounds).unwrap();
        assert_eq!(credit.identity, identity);
        assert_eq!(
            credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 0,
                send_through: declared,
            }
        );

        // Fill the complete legal window with the maximum possible record
        // count without reading staging progress. The next Ping, ordinary
        // Text request, and producer ABORT must still reach their independent
        // control paths through the real WebSocket reader.
        for offset in 0..window {
            client
                .send(Message::Binary(
                    stream_data_wire(
                        UPLOAD_ID,
                        identity.stream_id().get(),
                        u64::try_from(offset).unwrap(),
                        &[0x5a],
                    )
                    .into(),
                ))
                .await
                .unwrap();
        }
        let ping_payload = b"after-full-upload-window".to_vec();
        client
            .send(Message::Ping(ping_payload.clone().into()))
            .await
            .unwrap();
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": ORDINARY_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        client
            .send(Message::Binary(
                stream_wire(0x05, UPLOAD_ID, identity.stream_id().get(), 0, 0x01).into(),
            ))
            .await
            .unwrap();

        let mut saw_pong = false;
        let mut ordinary_reply = false;
        let mut acknowledged = None;
        let mut upload_reply = None;
        while !saw_pong || !ordinary_reply || acknowledged.is_none() || upload_reply.is_none() {
            match next_socket_message(&mut client).await {
                Message::Pong(payload) => {
                    assert_eq!(payload.as_ref(), ping_payload.as_slice());
                    saw_pong = true;
                }
                Message::Binary(bytes) => {
                    let record = jeliya_codec::decode_stream_record(&bytes, &bounds).unwrap();
                    assert_eq!(record.identity, identity);
                    match record.body {
                        jeliya_codec::StreamRecordBody::Credit { .. } => {}
                        jeliya_codec::StreamRecordBody::Ack { accepted_through } => {
                            assert!(acknowledged.replace(accepted_through).is_none());
                        }
                        other => panic!("unexpected upload control after client ABORT: {other:?}"),
                    }
                }
                Message::Text(text) => {
                    let reply: jeliya_codec::Reply = serde_json::from_str(&text).unwrap();
                    if reply.id == ORDINARY_ID {
                        assert!(reply.ok);
                        ordinary_reply = true;
                    } else if reply.id == UPLOAD_ID {
                        assert!(!reply.ok);
                        assert!(upload_reply.replace(reply).is_none());
                    } else {
                        panic!("unexpected reply id {}", reply.id);
                    }
                }
                other => panic!("unexpected message while draining upload controls: {other:?}"),
            }
        }

        let acknowledged = acknowledged.unwrap();
        let upload_reply = upload_reply.unwrap();
        assert!(matches!(
            upload_reply.err,
            Some(jeliya_api::ApiError::StreamAborted {
                transferred_bytes,
                total: jeliya_api::ByteTotal::Known { bytes },
                reason: jeliya_api::StreamAbortReason::Cancelled,
            }) if transferred_bytes == acknowledged && bytes == declared
        ));

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
    }

    #[tokio::test]
    async fn websocket_interleaves_multiple_uploads_download_requests_controls_and_pushes() {
        const SUBSCRIBE_ID: u64 = 121;
        const UPLOAD_ONE_ID: u64 = 122;
        const UPLOAD_TWO_ID: u64 = 123;
        const READ_ID: u64 = 124;
        const ORDINARY_ID: u64 = 125;
        const UPLOAD_ONE_NAME: &str = "interleaved-one.bin";
        const UPLOAD_TWO_NAME: &str = "interleaved-two.bin";

        let download = b"download";
        let (dir, mut state, read_request) = file_state(download, SOCKET_FRAME_BYTES as u64).await;
        let mut interleaved_limits = state.engine.limits();
        interleaved_limits.max_frame_bytes = SOCKET_FRAME_BYTES as u64;
        interleaved_limits.transfer_connect_allowance_ms = 60_000;
        interleaved_limits.transfer_stall_ms = 60_000;
        state.runtime_limits =
            crate::transfer::RuntimeLimits::from_served(&interleaved_limits).unwrap();
        state.transfer_pool = crate::transfer::TransferPool::from_runtime(&state.runtime_limits);
        let push_loop = state.engine.start_push_loop();
        let room_id = read_request.room_id.clone();
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SUBSCRIBE_ID,
                    "op": "stream.subscribe",
                    "in": jeliya_api::StreamSubscribe {
                        room_id: room_id.clone(),
                        from: jeliya_api::Cursor::Start,
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        loop {
            let Message::Text(text) = next_socket_message(&mut client).await else {
                panic!("stream.subscribe must receive a Text reply");
            };
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value.get("id").and_then(serde_json::Value::as_u64) == Some(SUBSCRIBE_ID) {
                let reply: jeliya_codec::Reply = serde_json::from_value(value).unwrap();
                assert!(reply.ok);
                break;
            }
            let _: jeliya_api::Push = serde_json::from_value(value).unwrap();
        }

        for (id, name) in [
            (UPLOAD_ONE_ID, UPLOAD_ONE_NAME),
            (UPLOAD_TWO_ID, UPLOAD_TWO_NAME),
        ] {
            client
                .send(Message::Text(
                    serde_json::json!({
                        "id": id,
                        "op": "file.share",
                        "in": {
                            "room_id": room_id,
                            "name": name,
                            "declared_bytes": 1,
                            "declared_content_type": "application/octet-stream",
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
        send_file_read(&mut client, READ_ID, &read_request).await;

        let mut upload_one = None;
        let mut upload_two = None;
        let mut read = None;
        let mut upload_one_credit = false;
        let mut upload_two_credit = false;
        while upload_one.is_none()
            || upload_two.is_none()
            || read.is_none()
            || !upload_one_credit
            || !upload_two_credit
        {
            match next_socket_message(&mut client).await {
                Message::Binary(bytes) => {
                    let record = jeliya_codec::decode_stream_record(&bytes, &bounds).unwrap();
                    match (record.identity.request_id().get(), record.body) {
                        (UPLOAD_ONE_ID, jeliya_codec::StreamRecordBody::Open { total: 1 }) => {
                            assert!(upload_one.replace(record.identity).is_none());
                        }
                        (
                            UPLOAD_ONE_ID,
                            jeliya_codec::StreamRecordBody::Credit {
                                accepted_through: 0,
                                send_through: 1,
                            },
                        ) => {
                            assert_eq!(upload_one, Some(record.identity));
                            upload_one_credit = true;
                        }
                        (UPLOAD_TWO_ID, jeliya_codec::StreamRecordBody::Open { total: 1 }) => {
                            assert!(upload_two.replace(record.identity).is_none());
                        }
                        (
                            UPLOAD_TWO_ID,
                            jeliya_codec::StreamRecordBody::Credit {
                                accepted_through: 0,
                                send_through: 1,
                            },
                        ) => {
                            assert_eq!(upload_two, Some(record.identity));
                            upload_two_credit = true;
                        }
                        (READ_ID, jeliya_codec::StreamRecordBody::Open { total }) => {
                            assert_eq!(total, u64::try_from(download.len()).unwrap());
                            assert!(read.replace(record.identity).is_none());
                        }
                        (request_id, body) => {
                            panic!("unexpected opening record for request {request_id}: {body:?}")
                        }
                    }
                }
                Message::Text(text) => {
                    // A delayed push for the host-staged download source is
                    // harmless; replies from these fresh stream requests are
                    // forbidden before their terminal records.
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert!(value.get("id").is_none(), "stream replied before OPEN");
                    let _: jeliya_api::Push = serde_json::from_value(value).unwrap();
                }
                other => panic!("unexpected opening message: {other:?}"),
            }
        }
        let upload_one = upload_one.unwrap();
        let upload_two = upload_two.unwrap();
        let read = read.unwrap();
        assert_ne!(upload_one, upload_two);
        assert_ne!(upload_one, read);
        assert_ne!(upload_two, read);
        assert_eq!(
            state.transfer_pool.usage(),
            (3, u64::try_from(download.len()).unwrap() + 2)
        );

        // Control and ordinary Text work must remain schedulable while all
        // three byte streams are paused and active.
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": ORDINARY_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let ping_payload = b"interleaved-control".to_vec();
        client
            .send(Message::Ping(ping_payload.clone().into()))
            .await
            .unwrap();
        let mut ordinary_reply = false;
        let mut pong = false;
        while !ordinary_reply || !pong {
            match next_socket_message(&mut client).await {
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value.get("id").and_then(serde_json::Value::as_u64) == Some(ORDINARY_ID) {
                        let reply: jeliya_codec::Reply = serde_json::from_value(value).unwrap();
                        assert!(reply.ok);
                        ordinary_reply = true;
                    } else {
                        let _: jeliya_api::Push = serde_json::from_value(value).unwrap();
                    }
                }
                Message::Pong(payload) => {
                    assert_eq!(payload.as_ref(), ping_payload.as_slice());
                    pong = true;
                }
                other => panic!("paused streams emitted unexpected work: {other:?}"),
            }
        }

        // Complete the first upload while the second upload and download stay
        // active. Its subscribed file_shared push and terminal reply may
        // arrive in either order.
        client
            .send(Message::Binary(
                stream_data_wire(UPLOAD_ONE_ID, upload_one.stream_id().get(), 0, b"a").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(first_sentinel) = next_socket_message(&mut client).await else {
            panic!("first upload must advance CREDIT");
        };
        let first_sentinel = jeliya_codec::decode_stream_record(&first_sentinel, &bounds).unwrap();
        assert_eq!(first_sentinel.identity, upload_one);
        assert_eq!(
            first_sentinel.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            }
        );
        client
            .send(Message::Binary(
                stream_wire(0x04, UPLOAD_ONE_ID, upload_one.stream_id().get(), 1, 0).into(),
            ))
            .await
            .unwrap();

        let mut first_reply = false;
        let mut first_push = false;
        while !first_reply || !first_push {
            let message = next_socket_message(&mut client).await;
            let Message::Text(text) = message else {
                panic!("first upload completion must use Text reply/push frames, got {message:?}");
            };
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value.get("id").and_then(serde_json::Value::as_u64) == Some(UPLOAD_ONE_ID) {
                let reply: jeliya_codec::Reply = serde_json::from_value(value).unwrap();
                assert!(reply.ok);
                first_reply = true;
                continue;
            }
            let push: jeliya_api::Push = serde_json::from_value(value).unwrap();
            if let jeliya_api::Push::Event {
                room_id: pushed,
                event,
            } = push
            {
                assert_eq!(pushed, room_id);
                if let jeliya_api::EventKindContent::FileShared { name, .. } = event.kind {
                    if name == UPLOAD_ONE_NAME {
                        first_push = true;
                    }
                }
            }
        }
        assert_eq!(
            state.transfer_pool.usage(),
            (2, u64::try_from(download.len()).unwrap() + 1),
            "the other upload and download remain admitted across the push"
        );

        // Now progress the remaining upload and download together. Route every
        // Binary message by the complete identity; their relative order is
        // intentionally unconstrained.
        client
            .send(Message::Binary(
                stream_data_wire(UPLOAD_TWO_ID, upload_two.stream_id().get(), 0, b"b").into(),
            ))
            .await
            .unwrap();
        client
            .send(Message::Binary(
                stream_wire(
                    0x03,
                    READ_ID,
                    read.stream_id().get(),
                    0,
                    u64::try_from(download.len()).unwrap(),
                )
                .into(),
            ))
            .await
            .unwrap();

        let mut second_end_sent = false;
        let mut second_reply = false;
        let mut second_push = false;
        let mut read_accepted = 0_u64;
        let mut read_end = false;
        let mut read_reply = false;
        while !second_reply || !second_push || !read_end || !read_reply {
            match next_socket_message(&mut client).await {
                Message::Binary(bytes) => {
                    let record = jeliya_codec::decode_stream_record(&bytes, &bounds).unwrap();
                    let request_id = record.identity.request_id().get();
                    if request_id == UPLOAD_TWO_ID {
                        assert_eq!(record.identity, upload_two);
                        assert_eq!(
                            record.body,
                            jeliya_codec::StreamRecordBody::Credit {
                                accepted_through: 1,
                                send_through: 2,
                            }
                        );
                        assert!(!second_end_sent);
                        client
                            .send(Message::Binary(
                                stream_wire(
                                    0x04,
                                    UPLOAD_TWO_ID,
                                    upload_two.stream_id().get(),
                                    1,
                                    0,
                                )
                                .into(),
                            ))
                            .await
                            .unwrap();
                        second_end_sent = true;
                    } else if request_id == READ_ID {
                        assert_eq!(record.identity, read);
                        match record.body {
                            jeliya_codec::StreamRecordBody::Data { offset, payload } => {
                                assert_eq!(offset, read_accepted);
                                let start = usize::try_from(read_accepted).unwrap();
                                assert_eq!(payload, download[start..start + payload.len()]);
                                read_accepted += u64::try_from(payload.len()).unwrap();
                                client
                                    .send(Message::Binary(
                                        stream_wire(
                                            0x03,
                                            READ_ID,
                                            read.stream_id().get(),
                                            read_accepted,
                                            u64::try_from(download.len()).unwrap(),
                                        )
                                        .into(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                            jeliya_codec::StreamRecordBody::End { total } => {
                                assert_eq!(total, u64::try_from(download.len()).unwrap());
                                assert_eq!(read_accepted, total);
                                read_end = true;
                            }
                            other => panic!("unexpected download record: {other:?}"),
                        }
                    } else {
                        panic!("Binary record crossed to request {request_id}");
                    }
                }
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) {
                        let reply: jeliya_codec::Reply = serde_json::from_value(value).unwrap();
                        assert!(reply.ok);
                        match id {
                            UPLOAD_TWO_ID => second_reply = true,
                            READ_ID => read_reply = true,
                            _ => panic!("unexpected terminal reply id {id}"),
                        }
                    } else {
                        let push: jeliya_api::Push = serde_json::from_value(value).unwrap();
                        if let jeliya_api::Push::Event {
                            room_id: pushed,
                            event,
                        } = push
                        {
                            assert_eq!(pushed, room_id);
                            if let jeliya_api::EventKindContent::FileShared { name, .. } =
                                event.kind
                            {
                                if name == UPLOAD_TWO_NAME {
                                    second_push = true;
                                }
                            }
                        }
                    }
                }
                other => panic!("unexpected interleaved message: {other:?}"),
            }
        }
        assert!(second_end_sent);
        assert_eq!(read_accepted, u64::try_from(download.len()).unwrap());
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert_eq!(
            std::fs::read_dir(dir.path().join("protocol-v2-stream-staging"))
                .unwrap()
                .count(),
            0
        );

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
        push_loop.stop();
    }

    #[tokio::test]
    async fn websocket_file_share_cancel_is_principal_scoped_and_idempotent() {
        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let room_id = existing.room_id;
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let transfer_op_id = "op-socket-upload-cancel-1";

        let (mut owner, owner_server) = socket_pair(state.clone()).await;
        let (mut joiner, joiner_server) = socket_pair(state.clone()).await;
        let (mut canceller, canceller_server) = socket_pair(state.clone()).await;
        let (mut stranger, stranger_server) = socket_pair_as(state.clone(), "other-client").await;
        for client in [&mut owner, &mut joiner, &mut canceller, &mut stranger] {
            assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        }

        owner
            .send(Message::Text(
                serde_json::json!({
                    "id": 41,
                    "op_id": transfer_op_id,
                    "op": "file.share",
                    "in": {
                        "room_id": room_id,
                        "name": "cancelled.bin",
                        "declared_bytes": 3,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(open) = owner.next().await.unwrap().unwrap() else {
            panic!("upload owner must receive OPEN");
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        let identity = open.identity;
        assert!(matches!(
            owner.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));

        joiner
            .send(Message::Text(
                serde_json::json!({
                    "id": 44,
                    "op_id": transfer_op_id,
                    "op": "file.share",
                    "in": {
                        "room_id": room_id,
                        "name": "cancelled.bin",
                        "declared_bytes": 3,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), joiner.next())
                .await
                .is_err(),
            "faithful join must receive neither OPEN nor an early reply"
        );

        stranger
            .send(Message::Text(
                serde_json::json!({
                    "id": 51,
                    "op": "transfer.cancel",
                    "in": { "transfer_op_id": transfer_op_id }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(unknown) = stranger.next().await.unwrap().unwrap() else {
            panic!("wrong-principal cancel refusal must be Text");
        };
        let unknown: jeliya_codec::Reply = serde_json::from_str(&unknown).unwrap();
        assert!(matches!(
            unknown.err,
            Some(jeliya_api::ApiError::TransferUnknown { .. })
        ));

        canceller
            .send(Message::Text(
                serde_json::json!({
                    "id": 42,
                    "op_id": "ignored-cancel-envelope-id",
                    "op": "transfer.cancel",
                    "in": { "transfer_op_id": transfer_op_id }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(cancelled) = canceller.next().await.unwrap().unwrap() else {
            panic!("cancel outcome must be Text");
        };
        let cancelled: jeliya_codec::Reply = serde_json::from_str(&cancelled).unwrap();
        let cancelled: jeliya_api::TransferCancelOut =
            serde_json::from_value(cancelled.out.expect("cancelled outcome")).unwrap();
        assert_eq!(cancelled.outcome, jeliya_api::CancelOutcome::Cancelled);
        assert_eq!(cancelled.transferred_bytes, 0);
        assert_eq!(cancelled.total, jeliya_api::ByteTotal::Known { bytes: 3 });
        assert_eq!(
            state.transfer_pool.usage(),
            (0, 0),
            "cancel reply cannot outrun local reservation release"
        );

        let Message::Binary(abort) = owner.next().await.unwrap().unwrap() else {
            panic!("winning cancellation must send daemon ABORT");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: 0,
                reason: jeliya_codec::BinaryAbortReason::Cancelled,
            }
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), joiner.next())
                .await
                .is_err(),
            "ledger publication must wait for the owner's explicit ACK obligation"
        );
        owner
            .send(Message::Binary(
                stream_wire(0x06, 41, identity.stream_id().get(), 0, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(original) = owner.next().await.unwrap().unwrap() else {
            panic!("original upload terminal must follow ACK as Text");
        };
        let original: jeliya_codec::Reply = serde_json::from_str(&original).unwrap();
        assert_eq!(
            original.err,
            Some(jeliya_api::ApiError::StreamAborted {
                transferred_bytes: 0,
                total: jeliya_api::ByteTotal::Known { bytes: 3 },
                reason: jeliya_api::StreamAbortReason::Cancelled,
            })
        );
        let Message::Text(joined) = joiner.next().await.unwrap().unwrap() else {
            panic!("faithful join must receive the selected terminal Text result");
        };
        let joined: jeliya_codec::Reply = serde_json::from_str(&joined).unwrap();
        assert_eq!(joined.err, original.err);

        canceller
            .send(Message::Text(
                serde_json::json!({
                    "id": 43,
                    "op": "transfer.cancel",
                    "in": { "transfer_op_id": transfer_op_id }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(already) = canceller.next().await.unwrap().unwrap() else {
            panic!("repeat cancel outcome must be Text");
        };
        let already: jeliya_codec::Reply = serde_json::from_str(&already).unwrap();
        let already: jeliya_api::TransferCancelOut =
            serde_json::from_value(already.out.expect("already-cancelled outcome")).unwrap();
        assert_eq!(already.outcome, jeliya_api::CancelOutcome::AlreadyCancelled);
        assert_eq!(already.transferred_bytes, cancelled.transferred_bytes);
        assert_eq!(already.total, cancelled.total);

        owner.close(None).await.unwrap();
        joiner.close(None).await.unwrap();
        canceller.close(None).await.unwrap();
        stranger.close(None).await.unwrap();
        for server in [
            owner_server,
            joiner_server,
            canceller_server,
            stranger_server,
        ] {
            tokio::time::timeout(std::time::Duration::from_secs(1), server)
                .await
                .expect("connection teardown")
                .expect("serve task");
        }
    }

    #[tokio::test]
    async fn websocket_file_share_crossed_abort_is_correlated_and_connection_survives() {
        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        client
            .send(Message::Text(
                serde_json::json!({
                    "id": 55,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "crossed-abort.bin",
                        "declared_bytes": 0,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(open) = client.next().await.unwrap().unwrap() else {
            panic!("upload must open");
        };
        let identity = jeliya_codec::decode_stream_record(&open, &bounds)
            .unwrap()
            .identity;
        assert!(matches!(
            client.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));

        // Empty DATA is an exact-bound request-local fault. Queue a valid
        // producer ABORT immediately behind it so the daemon must preserve its
        // selected protocol_error while discharging both ACK obligations.
        client
            .send(Message::Binary(
                stream_wire(0x02, 55, identity.stream_id().get(), 0, 0).into(),
            ))
            .await
            .unwrap();
        client
            .send(Message::Binary(
                stream_wire(0x05, 55, identity.stream_id().get(), 0, 0x02).into(),
            ))
            .await
            .unwrap();

        let Message::Binary(daemon_abort) = client.next().await.unwrap().unwrap() else {
            panic!("bound fault must send daemon ABORT");
        };
        let daemon_abort = jeliya_codec::decode_stream_record(&daemon_abort, &bounds).unwrap();
        assert_eq!(daemon_abort.identity, identity);
        assert_eq!(
            daemon_abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: 0,
                reason: jeliya_codec::BinaryAbortReason::ProtocolError,
            }
        );
        let Message::Binary(client_abort_ack) = client.next().await.unwrap().unwrap() else {
            panic!("daemon must ACK the crossed producer ABORT");
        };
        let client_abort_ack =
            jeliya_codec::decode_stream_record(&client_abort_ack, &bounds).unwrap();
        assert_eq!(client_abort_ack.identity, identity);
        assert_eq!(
            client_abort_ack.body,
            jeliya_codec::StreamRecordBody::Ack {
                accepted_through: 0,
            }
        );
        client
            .send(Message::Binary(
                stream_wire(0x06, 55, identity.stream_id().get(), 0, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(terminal) = client.next().await.unwrap().unwrap() else {
            panic!("daemon terminal must follow the crossed ACK exchange");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.err, Some(jeliya_api::ApiError::MalformedFrame));

        client
            .send(Message::Text(
                r#"{"id":56,"op":"subject.ensure","in":{}}"#.into(),
            ))
            .await
            .unwrap();
        let Message::Text(unrelated) = client.next().await.unwrap().unwrap() else {
            panic!("request-local upload fault must leave the connection usable");
        };
        let unrelated: jeliya_codec::Reply = serde_json::from_str(&unrelated).unwrap();
        assert!(unrelated.ok);

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": 57,
                    "op": "file.share",
                    "in": {
                        "room_id": existing.room_id,
                        "name": "client-abort.bin",
                        "declared_bytes": 0,
                        "declared_content_type": "application/octet-stream",
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Binary(second_open) = client.next().await.unwrap().unwrap() else {
            panic!("unrelated upload must still open");
        };
        let second_identity = jeliya_codec::decode_stream_record(&second_open, &bounds)
            .unwrap()
            .identity;
        assert!(matches!(
            client.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        client
            .send(Message::Binary(
                stream_wire(0x05, 57, second_identity.stream_id().get(), 0, 0x02).into(),
            ))
            .await
            .unwrap();
        let Message::Binary(ack) = client.next().await.unwrap().unwrap() else {
            panic!("producer ABORT must be ACKed");
        };
        let ack = jeliya_codec::decode_stream_record(&ack, &bounds).unwrap();
        assert_eq!(ack.identity, second_identity);
        assert_eq!(
            ack.body,
            jeliya_codec::StreamRecordBody::Ack {
                accepted_through: 0,
            }
        );
        let Message::Text(aborted) = client.next().await.unwrap().unwrap() else {
            panic!("client ABORT must finish with Text stream_aborted");
        };
        let aborted: jeliya_codec::Reply = serde_json::from_str(&aborted).unwrap();
        assert_eq!(
            aborted.err,
            Some(jeliya_api::ApiError::StreamAborted {
                transferred_bytes: 0,
                total: jeliya_api::ByteTotal::Known { bytes: 0 },
                reason: jeliya_api::StreamAbortReason::SourceFailed,
            })
        );

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("connection teardown")
            .expect("serve task");
    }

    #[tokio::test]
    async fn websocket_file_share_daemon_abort_ack_timeout_replies_then_closes_4007() {
        const REQUEST_ID: u64 = 71;
        const STALL_MS: u64 = 200;

        let (dir, mut state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        configure_transfer_timers(&mut state, STALL_MS, 5_000, 8_000);
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        let identity = open_one_byte_file_share(
            &mut client,
            REQUEST_ID,
            existing.room_id.as_str(),
            "ack-timeout.bin",
            &bounds,
        )
        .await;

        // Freeze only after staging and OPEN/CREDIT have completed. The
        // malformed record then selects protocol_error without allowing a
        // paused clock to race asynchronous staging setup.
        tokio::time::pause();
        client
            .send(Message::Binary(
                stream_wire(0x02, REQUEST_ID, identity.stream_id().get(), 0, 0).into(),
            ))
            .await
            .unwrap();
        let Message::Binary(abort) = next_socket_message(&mut client).await else {
            panic!("bound malformed DATA must receive daemon ABORT");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: 0,
                reason: jeliya_codec::BinaryAbortReason::ProtocolError,
            }
        );

        // The local terminal decision releases admission and discards the
        // private stage before waiting for the producer's exact ACK.
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        let staging = dir.path().join("protocol-v2-stream-staging");
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        assert!(
            client.next().now_or_never().is_none(),
            "terminal Text must wait for the ACK boundary"
        );
        tokio::time::advance(std::time::Duration::from_millis(STALL_MS - 1)).await;
        tokio::task::yield_now().await;
        assert!(
            client.next().now_or_never().is_none(),
            "ACK wait must remain open through transfer_stall_ms - 1"
        );
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        let Message::Text(terminal) = next_socket_message(&mut client).await else {
            panic!("ACK timeout must publish the selected terminal Text result");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.id, REQUEST_ID);
        assert_eq!(terminal.err, Some(jeliya_api::ApiError::MalformedFrame));

        let Message::Close(Some(close)) = next_socket_message(&mut client).await else {
            panic!("ACK timeout terminal must be followed by a Close frame");
        };
        assert_eq!(u16::from(close.code), 4007);
        assert_eq!(close.reason, "malformed_frame");
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("ACK-timeout connection teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn websocket_file_share_no_progress_stalls_and_survives_exact_ack() {
        const REQUEST_ID: u64 = 72;
        const SURVIVAL_ID: u64 = 73;
        const STALL_MS: u64 = 100;

        let (dir, mut state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        configure_transfer_timers(&mut state, STALL_MS, 5_000, 8_000);
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        let identity = open_one_byte_file_share(
            &mut client,
            REQUEST_ID,
            existing.room_id.as_str(),
            "stalled-upload.bin",
            &bounds,
        )
        .await;

        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_millis(STALL_MS)).await;
        tokio::task::yield_now().await;
        let Message::Binary(abort) = next_socket_message(&mut client).await else {
            panic!("zero accepted progress must receive daemon ABORT");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: 0,
                reason: jeliya_codec::BinaryAbortReason::OperationError,
            }
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        let staging = dir.path().join("protocol-v2-stream-staging");
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        client
            .send(Message::Binary(
                stream_wire(0x06, REQUEST_ID, identity.stream_id().get(), 0, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(terminal) = next_socket_message(&mut client).await else {
            panic!("exact ACK must release the stalled terminal Text reply");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.id, REQUEST_ID);
        assert_eq!(
            terminal.err,
            Some(jeliya_api::ApiError::TransferStalled {
                transferred_bytes: 0,
                total: jeliya_api::ByteTotal::Known { bytes: 1 },
            })
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SURVIVAL_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(survival) = next_socket_message(&mut client).await else {
            panic!("an exact daemon-ABORT ACK must preserve the connection");
        };
        let survival: jeliya_codec::Reply = serde_json::from_str(&survival).unwrap();
        assert_eq!(survival.id, SURVIVAL_ID);
        assert!(survival.ok);

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("stalled upload connection teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn websocket_file_share_progress_resets_stall_but_not_absolute_deadline() {
        const REQUEST_ID: u64 = 74;
        const SURVIVAL_ID: u64 = 75;
        const STALL_MS: u64 = 1_000;
        const DEADLINE_BUDGET_MS: u64 = 1_600;

        let (dir, mut state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        // One byte costs exactly 1 ms at 8,000 bits/s, so the admitted
        // absolute budget is 1,599 + 1 = 1,600 ms. Accepting at about 800 ms
        // moves the stall boundary to about 1,800 ms without moving the
        // absolute deadline.
        configure_transfer_timers(&mut state, STALL_MS, 1_599, 8_000);
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        let identity = open_one_byte_file_share(
            &mut client,
            REQUEST_ID,
            existing.room_id.as_str(),
            "deadline-upload.bin",
            &bounds,
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        client
            .send(Message::Binary(
                stream_data_wire(REQUEST_ID, identity.stream_id().get(), 0, b"x").into(),
            ))
            .await
            .unwrap();
        let Message::Binary(sentinel_credit) = next_socket_message(&mut client).await else {
            panic!("durably accepted progress must advance CREDIT");
        };
        let sentinel_credit =
            jeliya_codec::decode_stream_record(&sentinel_credit, &bounds).unwrap();
        assert_eq!(sentinel_credit.identity, identity);
        assert_eq!(
            sentinel_credit.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            }
        );

        let Message::Binary(abort) = next_socket_message(&mut client).await else {
            panic!("absolute deadline must receive daemon ABORT after progress");
        };
        let abort = jeliya_codec::decode_stream_record(&abort, &bounds).unwrap();
        assert_eq!(abort.identity, identity);
        assert_eq!(
            abort.body,
            jeliya_codec::StreamRecordBody::Abort {
                accepted_through: 1,
                reason: jeliya_codec::BinaryAbortReason::OperationError,
            }
        );
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        let staging = dir.path().join("protocol-v2-stream-staging");
        assert_eq!(std::fs::read_dir(&staging).unwrap().count(), 0);

        client
            .send(Message::Binary(
                stream_wire(0x06, REQUEST_ID, identity.stream_id().get(), 1, 0x05).into(),
            ))
            .await
            .unwrap();
        let Message::Text(terminal) = next_socket_message(&mut client).await else {
            panic!("exact ACK must release the deadline terminal Text reply");
        };
        let terminal: jeliya_codec::Reply = serde_json::from_str(&terminal).unwrap();
        assert_eq!(terminal.id, REQUEST_ID);
        assert_eq!(
            terminal.err,
            Some(jeliya_api::ApiError::TransferDeadlineExceeded {
                transferred_bytes: 1,
                total: jeliya_api::ByteTotal::Known { bytes: 1 },
                budget_ms: DEADLINE_BUDGET_MS,
            })
        );

        client
            .send(Message::Text(
                serde_json::json!({
                    "id": SURVIVAL_ID,
                    "op": "room.list",
                    "in": {},
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let Message::Text(survival) = next_socket_message(&mut client).await else {
            panic!("deadline terminal with exact ACK must preserve the connection");
        };
        let survival: jeliya_codec::Reply = serde_json::from_str(&survival).unwrap();
        assert_eq!(survival.id, SURVIVAL_ID);
        assert!(survival.ok);

        client.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("deadline upload connection teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
        assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn websocket_file_share_disconnect_replays_pre_and_post_end_results() {
        let (_dir, state, existing) = file_state(b"seed", SOCKET_FRAME_BYTES as u64).await;
        let room_id = existing.room_id;
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let pre_body = serde_json::json!({
            "op_id": "op-socket-upload-pre-end-loss-1",
            "op": "file.share",
            "in": {
                "room_id": room_id,
                "name": "pre-end-lost.bin",
                "declared_bytes": 1,
                "declared_content_type": "application/octet-stream",
            }
        });

        let (mut first, first_server) = socket_pair(state.clone()).await;
        assert!(matches!(first.next().await, Some(Ok(Message::Text(_)))));
        let mut first_request = pre_body.clone();
        first_request["id"] = 61.into();
        first
            .send(Message::Text(first_request.to_string().into()))
            .await
            .unwrap();
        let Message::Binary(open) = first.next().await.unwrap().unwrap() else {
            panic!("pre-END upload must open");
        };
        let identity = jeliya_codec::decode_stream_record(&open, &bounds)
            .unwrap()
            .identity;
        assert!(matches!(
            first.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        first
            .send(Message::Binary(
                stream_data_wire(61, identity.stream_id().get(), 0, &[7]).into(),
            ))
            .await
            .unwrap();
        let Message::Binary(accepted) = first.next().await.unwrap().unwrap() else {
            panic!("accepted DATA must advance sentinel CREDIT");
        };
        let accepted = jeliya_codec::decode_stream_record(&accepted, &bounds).unwrap();
        assert_eq!(
            accepted.body,
            jeliya_codec::StreamRecordBody::Credit {
                accepted_through: 1,
                send_through: 2,
            }
        );
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), first_server)
            .await
            .expect("pre-END disconnect teardown")
            .expect("serve task");

        let (mut replay, replay_server) = socket_pair(state.clone()).await;
        assert!(matches!(replay.next().await, Some(Ok(Message::Text(_)))));
        let mut replay_request = pre_body;
        replay_request["id"] = 62.into();
        replay
            .send(Message::Text(replay_request.to_string().into()))
            .await
            .unwrap();
        let Message::Text(pre_lost) = replay.next().await.unwrap().unwrap() else {
            panic!("pre-END replay must return Text without OPEN");
        };
        let pre_lost: jeliya_codec::Reply = serde_json::from_str(&pre_lost).unwrap();
        assert_eq!(
            pre_lost.err,
            Some(jeliya_api::ApiError::StreamAborted {
                transferred_bytes: 1,
                total: jeliya_api::ByteTotal::Known { bytes: 1 },
                reason: jeliya_api::StreamAbortReason::TransportLost,
            })
        );

        let post_body = serde_json::json!({
            "op_id": "op-socket-upload-post-end-loss-1",
            "op": "file.share",
            "in": {
                "room_id": room_id,
                "name": "post-end-committed.bin",
                "declared_bytes": 0,
                "declared_content_type": "application/octet-stream",
            }
        });
        let mut post_request = post_body.clone();
        post_request["id"] = 63.into();
        replay
            .send(Message::Text(post_request.to_string().into()))
            .await
            .unwrap();
        let Message::Binary(post_open) = replay.next().await.unwrap().unwrap() else {
            panic!("post-END owner must open");
        };
        let post_identity = jeliya_codec::decode_stream_record(&post_open, &bounds)
            .unwrap()
            .identity;
        assert!(matches!(
            replay.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        replay
            .send(Message::Binary(
                stream_wire(0x04, 63, post_identity.stream_id().get(), 0, 0).into(),
            ))
            .await
            .unwrap();
        // Drop immediately after the END write: the actor must honor the
        // already-routed terminal before connection invalidation can synthesize
        // transport_lost.
        drop(replay);
        tokio::time::timeout(std::time::Duration::from_secs(1), replay_server)
            .await
            .expect("post-END disconnect teardown")
            .expect("serve task");

        let (mut final_replay, final_server) = socket_pair(state.clone()).await;
        assert!(matches!(
            final_replay.next().await,
            Some(Ok(Message::Text(_)))
        ));
        let mut final_request = post_body;
        final_request["id"] = 64.into();
        final_replay
            .send(Message::Text(final_request.to_string().into()))
            .await
            .unwrap();
        let Message::Text(committed) = final_replay.next().await.unwrap().unwrap() else {
            panic!("post-END replay must return the committed Text result");
        };
        let committed: jeliya_codec::Reply = serde_json::from_str(&committed).unwrap();
        assert!(committed.ok);

        let listed = state
            .engine
            .execute(jeliya_core::typed::TypedCall::FileList(
                jeliya_api::FileList {
                    room_id: room_id.clone(),
                    page: jeliya_api::Page {
                        cursor: jeliya_api::Cursor::Start,
                        direction: jeliya_api::Direction::Forward,
                        limit: 100,
                    },
                },
            ))
            .await
            .reply
            .expect("file.list after disconnect races");
        let jeliya_core::typed::TypedReply::FileList(listed) = listed else {
            panic!("wrong file.list reply");
        };
        assert_eq!(
            listed
                .files
                .iter()
                .filter(|file| file.name == "pre-end-lost.bin")
                .count(),
            0
        );
        assert_eq!(
            listed
                .files
                .iter()
                .filter(|file| file.name == "post-end-committed.bin")
                .count(),
            1
        );
        final_replay.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), final_server)
            .await
            .expect("final replay teardown")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
    }

    #[tokio::test]
    async fn active_credit_pause_is_not_closed_by_ordinary_idle_timeout() {
        let (_dir, mut state, request) = file_state(&[1], SOCKET_FRAME_BYTES as u64).await;
        let mut limits = state.engine.limits();
        limits.max_frame_bytes = SOCKET_FRAME_BYTES as u64;
        limits.transfer_connect_allowance_ms = 700_000;
        limits.transfer_stall_ms = 700_000;
        state.runtime_limits = crate::transfer::RuntimeLimits::from_served(&limits).unwrap();
        state.transfer_pool = crate::transfer::TransferPool::from_runtime(&state.runtime_limits);

        tokio::time::pause();
        let (mut client, server) = socket_pair(state.clone()).await;
        assert!(matches!(client.next().await, Some(Ok(Message::Text(_)))));
        send_file_read(&mut client, 11, &request).await;
        let Message::Binary(open) = client.next().await.unwrap().unwrap() else {
            panic!("expected OPEN");
        };
        let bounds = jeliya_codec::CodecBounds {
            max_frame_bytes: SOCKET_FRAME_BYTES,
            ..jeliya_codec::CodecBounds::default()
        };
        let open = jeliya_codec::decode_stream_record(&open, &bounds).unwrap();
        client
            .send(Message::Binary(
                stream_wire(0x03, 11, open.identity.stream_id().get(), 0, 0).into(),
            ))
            .await
            .unwrap();

        tokio::time::advance(std::time::Duration::from_millis(600_000)).await;
        tokio::task::yield_now().await;
        client
            .send(Message::Text(
                r#"{"id":12,"op":"room.list","in":{}}"#.into(),
            ))
            .await
            .unwrap();
        let Message::Text(reply) = client.next().await.unwrap().unwrap() else {
            panic!("active transfer lost to ordinary idle timeout");
        };
        let reply: jeliya_codec::Reply = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply.id, 12);
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("disconnect cleanup")
            .expect("serve task");
        assert_eq!(state.transfer_pool.usage(), (0, 0));
    }
}
