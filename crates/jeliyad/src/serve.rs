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

use futures_util::{SinkExt, StreamExt};
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
use jeliya_core::supervisor::FILE_UPLOAD_MAX_BYTES;
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
    match hyper_tungstenite::upgrade(req, None) {
        Ok((response, websocket)) => {
            tokio::spawn(async move {
                if let Ok(ws) = websocket.await {
                    // Count the live connection for the `max_connections`
                    // gate; decrement on any exit path.
                    state.connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    serve_ws(ws, state.clone(), principal).await;
                    state.connections.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
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
    let file = match state.supervisor.local_file(room_id, file_id).await {
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
    let content_type = HeaderValue::from_str(&safe_download_mime(&file.mime))
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
            Some(n) if n <= FILE_UPLOAD_MAX_BYTES => {}
            Some(n) => {
                return json_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    &CoreError::invalid(format!(
                        "upload is {n} bytes; the share limit is {FILE_UPLOAD_MAX_BYTES} bytes"
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

    let body = match read_limited(req.into_body(), FILE_UPLOAD_MAX_BYTES).await {
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

    let path = stage_path.to_string_lossy().to_string();
    let result = state
        .supervisor
        .share_file(room_id, &path, Some(&display_name), mime.as_deref())
        .await;
    let _ = std::fs::remove_file(&stage_path);
    match result {
        Ok(value) => json_ok(value),
        Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
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

fn json_ok(result: Value) -> Response<Full<Bytes>> {
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

/// One v2 WebSocket connection. The daemon's first frame is exactly one
/// `hello`; thereafter each inbound text frame is decoded by the codec into a
/// typed call, executed by the engine, and its typed reply encoded back —
/// interleaved with typed pushes. A frame over the limit closes `4005`; a
/// frame whose `id` cannot be recovered closes `4007`; any other malformed
/// frame gets a correlated error reply so one bad request never strands the
/// others in flight. A lagged push receiver is told to resync (the one
/// resync path), never silently continued.
pub async fn serve_ws<S>(
    ws: WebSocketStream<S>,
    state: AppState,
    _principal: jeliya_codec::SessionPrincipal,
) where
    // `Send + 'static` so the single-writer task and the spawned request tasks
    // can own the socket and engine handles across threads.
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut messages) = ws.split();
    let mut push_rx = state.engine.subscribe_pushes();
    let bounds = jeliya_codec::CodecBounds::default();

    // All outbound frames flow through ONE writer task over an mpsc channel,
    // so a slow request's execution never head-of-line blocks a push, a ping,
    // or another request's reply. Replies may complete out of order — the
    // record allows that and the client correlates by `id`.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // This connection's room subscriptions: `stream.subscribe` adds a room
    // with the position the client's stream begins at; `stream.unsubscribe`
    // removes it; pushes are gated on it (no push before subscribe, per the
    // record's "no global broadcast" rule). Shared with the spawned request
    // tasks behind a mutex (only ever held for a map op, never across an
    // engine await).
    let subscriptions = std::sync::Arc::new(tokio::sync::Mutex::new(
        std::collections::HashMap::<String, u64>::new(),
    ));
    // The last position actually delivered to this connection per room, so a
    // backpressure lag can name a real resync point for each subscribed room.
    let mut last_delivered: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();

    // The `hello` frame: exactly one, first, carrying the generation, the
    // storage generation, the served limits, and the local subject.
    let hello = jeliya_api::Hello {
        protocol: jeliya_core::engine::PROTOCOL_VERSION,
        storage_generation: jeliya_core::engine::STORAGE_GENERATION,
        limits: state.engine.limits(),
        subject: state.engine.subject_state(),
        resume: jeliya_api::Resume::Fresh,
    };
    match serde_json::to_vec(&hello) {
        Ok(bytes) => {
            if out_tx.send(Message::Binary(bytes.into())).await.is_err() {
                writer.abort();
                return;
            }
        }
        Err(_) => {
            writer.abort();
            return;
        }
    }

    // Track in-flight request tasks so the loop never serializes on a slow
    // operation; a finished task's result is reaped without blocking.
    let mut inflight: tokio::task::JoinSet<bool> = tokio::task::JoinSet::new();

    // The served idle timeout: a connection with no inbound activity for
    // `idle_timeout_ms` is closed with 4004. Reset on any inbound frame.
    let idle_ms = state.engine.limits().idle_timeout_ms;
    let idle_deadline = tokio::time::sleep(tokio::time::Duration::from_millis(idle_ms));
    tokio::pin!(idle_deadline);

    loop {
        tokio::select! {
            msg = messages.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_millis(idle_ms));
                    if !dispatch_inbound(text.as_bytes(), &state, &bounds, &subscriptions, &out_tx, &mut inflight).await {
                        break;
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_millis(idle_ms));
                    if !dispatch_inbound(&bytes, &state, &bounds, &subscriptions, &out_tx, &mut inflight).await {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if out_tx.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {} // pong frames: ignored
            },
            () = &mut idle_deadline => {
                // No inbound activity for the served idle window: close 4004.
                let _ = out_tx
                    .send(Message::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: 4004.into(),
                            reason: "idle_timeout".into(),
                        },
                    )))
                    .await;
                break;
            }
            push = push_rx.recv() => match push {
                Ok(push) => {
                    // Gate delivery on this connection's subscriptions: a push
                    // for a room the connection has not subscribed to is not
                    // delivered (the record's no-global-broadcast rule), and a
                    // push is never a membership oracle.
                    let room = push_room_id(&push).map(str::to_owned);
                    let subscribed = {
                        let subs = subscriptions.lock().await;
                        room.as_ref().map(|r| subs.contains_key(r)).unwrap_or(false)
                    };
                    if subscribed {
                        // Track the last-delivered position per room so a later
                        // lag can name a real resync point for that room.
                        if let (Some(r), Some(pos)) = (room.as_ref(), push_pos(&push)) {
                            last_delivered.insert(r.clone(), pos);
                        }
                        let bytes = jeliya_codec::push_to_bytes(&push);
                        if out_tx.send(Message::Binary(bytes.into())).await.is_err() {
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
                    let rooms: Vec<String> = {
                        let subs = subscriptions.lock().await;
                        subs.keys().cloned().collect()
                    };
                    for room in rooms {
                        let from_pos = last_delivered.get(&room).copied().unwrap_or(0);
                        let gap = jeliya_api::Push::Gap {
                            room_id: jeliya_api::RoomId::new(room),
                            from_pos,
                            to: jeliya_api::GapTo::Open,
                            reason: jeliya_api::GapReason::Backpressure,
                        };
                        let bytes = jeliya_codec::push_to_bytes(&gap);
                        if out_tx.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Reap finished request tasks without blocking the loop.
            Some(_) = inflight.join_next() => {}
        }
    }

    // Teardown: stop accepting new work, drain in-flight replies, then drop
    // the writer task.
    inflight.abort_all();
    drop(out_tx);
    let _ = writer.await;
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

/// `stream.subscribe` — add the room to this connection's subscription set.
/// Naturally idempotent; exceeding the served limit is
/// `subscription_limit_reached`, never a silent drop.
async fn handle_stream_subscribe(
    out_tx: &tokio::sync::mpsc::Sender<Message>,
    state: &AppState,
    request: &jeliya_codec::Request,
    subscriptions: &std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, u64>>>,
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
    let mut subs = subscriptions.lock().await;
    if !subs.contains_key(&room_key) && subs.len() as u64 >= MAX_SUBSCRIPTIONS {
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
    subs.insert(room_key, from_pos);
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
    out_tx: &tokio::sync::mpsc::Sender<Message>,
    request: &jeliya_codec::Request,
    subscriptions: &std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, u64>>>,
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
    out_tx: &tokio::sync::mpsc::Sender<Message>,
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
async fn authorize_room(state: &AppState, room_id: &jeliya_api::RoomId) -> Result<(), jeliya_api::ApiError> {
    let call = jeliya_core::typed::TypedCall::RoomMembers(jeliya_api::RoomMembers {
        room_id: room_id.clone(),
    });
    match state.engine.execute(call).await.reply {
        Ok(_) => Ok(()),
        Err(err) => Err(err),
    }
}

/// The room's current head position (the next position a `start` cursor
/// resolves to), or the access error if the room is not visible.
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
        Ok(jeliya_core::typed::TypedReply::RoomTimeline(out)) => {
            Ok(out.events.last().map(|e| e.pos + 1).unwrap_or(0))
        }
        Ok(_) => Ok(0),
        Err(err) => Err(err),
    }
}

/// Encode a typed output as a reply frame and send it.
async fn send_typed<O>(
    out_tx: &tokio::sync::mpsc::Sender<Message>,
    id: u64,
    out: &O,
) -> bool
where
    O: serde::Serialize,
{
    let reply = jeliya_codec::Reply {
        id,
        ok: true,
        out: serde_json::to_value(out).ok(),
        err: None,
    };
    out_tx.send(Message::Binary(reply.to_bytes().into()))
        .await
        .is_ok()
}

/// Encode a typed API error as a reply frame and send it.
async fn send_api_err(
    out_tx: &tokio::sync::mpsc::Sender<Message>,
    id: u64,
    err: jeliya_api::ApiError,
) -> bool {
    let reply = jeliya_codec::Reply {
        id,
        ok: false,
        out: None,
        err: Some(err),
    };
    out_tx.send(Message::Binary(reply.to_bytes().into()))
        .await
        .is_ok()
}

/// Decode one inbound frame and route it. Connection-terminating frames (over
/// the limit → `4005`, unrecoverable id → `4007`) are decided synchronously;
/// well-formed requests are executed on a spawned task so a slow operation
/// never head-of-line blocks the push fan-out or other requests. Returns
/// `false` when the connection must close.
async fn dispatch_inbound(
    bytes: &[u8],
    state: &AppState,
    bounds: &jeliya_codec::CodecBounds,
    subscriptions: &std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, u64>>>,
    out_tx: &tokio::sync::mpsc::Sender<Message>,
    inflight: &mut tokio::task::JoinSet<bool>,
) -> bool {
    use jeliya_codec::CodecError;
    let frame = match jeliya_codec::decode(bytes, bounds) {
        Ok(frame) => frame,
        Err(CodecError::FrameTooLarge { .. }) => {
            let _ = out_tx
                .send(Message::Close(Some(
                    tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4005.into(),
                        reason: "frame_too_large".into(),
                    },
                )))
                .await;
            return false;
        }
        Err(CodecError::UnrecoverableId(_)) => {
            let _ = out_tx
                .send(Message::Close(Some(
                    tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: 4007.into(),
                        reason: "malformed_frame".into(),
                    },
                )))
                .await;
            return false;
        }
        Err(CodecError::Malformed { id, error }) => {
            let reply = jeliya_codec::Reply::from_result::<jeliya_api::RoomList>(id, Err(error));
            return out_tx
                .send(Message::Binary(reply.to_bytes().into()))
                .await
                .is_ok();
        }
        Err(CodecError::GateRefused(_)) => {
            return false;
        }
    };

    let jeliya_codec::Frame::Request(request) = frame else {
        // An inbound non-request frame has no place client-to-daemon.
        return true;
    };

    // The three stream operations are connection-scoped: they read and mutate
    // THIS connection's subscription set, never the supervisor. They are fast
    // (map ops plus one engine read), so run them inline.
    match request.call.op {
        "stream.subscribe" => {
            return handle_stream_subscribe(out_tx, state, &request, subscriptions).await;
        }
        "stream.unsubscribe" => {
            return handle_stream_unsubscribe(out_tx, &request, subscriptions).await;
        }
        "stream.resync" => {
            return handle_stream_resync(out_tx, state, &request).await;
        }
        _ => {}
    }

    let Some(call) = jeliya_core::typed::resolve_call(request.call.op, request.call.input_any())
    else {
        let reply = jeliya_codec::Reply::from_result::<jeliya_api::RoomList>(
            request.id,
            Err(jeliya_api::ApiError::MalformedFrame),
        );
        return out_tx
            .send(Message::Binary(reply.to_bytes().into()))
            .await
            .is_ok();
    };

    // Execute on a spawned task so a slow op (file.fetch, pipe.connect) does
    // not block the loop. `daemon.stop` is sequenced by the engine; its reply
    // is flushed by the writer before teardown.
    let id = request.id;
    let engine = state.engine.clone();
    let out_tx = out_tx.clone();
    inflight.spawn(async move {
        let executed = engine.execute(call).await;
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
        out_tx
            .send(Message::Binary(reply.to_bytes().into()))
            .await
            .is_ok()
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
    use super::{constant_time_eq, safe_download_mime};

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
}
