//! Endpoint resolution, daemon-status validation, and the injected session
//! resolver seam (§6.1 steps 1–2, §6.4).
//!
//! Every dial obtains **fresh** daemon status (`GET /api/health`) and a
//! **fresh** credential (via the injected [`SessionResolver`]) on every attempt,
//! so a daemon restart or a token rotation heals through the normal reconnect
//! loop. The credential value is never logged — only its presence is meaningful
//! to the driver.

use std::future::Future;
use std::pin::Pin;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortSignal, Request, RequestInit, RequestMode, Response};

/// Where the daemon lives and how its browser endpoints are derived.
pub enum Endpoint {
    /// Production: derive same-origin endpoints from `window.location`
    /// (`http`→`ws`, `https`→`wss`; host verbatim), matching the daemon that
    /// served the SPA. Loopback host, no hardcoded port — it tracks the
    /// daemon's actual port and any port-collision fallback for free.
    SameOrigin,
    /// Dev/tests: an explicit HTTP base (e.g. `http://127.0.0.1:1234`) and WS
    /// URL base (e.g. `ws://127.0.0.1:1234/ws`), for a test-supervised daemon
    /// on another loopback port.
    Explicit {
        /// The HTTP origin the `/api/health` and `/api/session` requests target.
        http_base: String,
        /// The WebSocket URL base the credential query is appended to.
        ws_url: String,
    },
}

impl Endpoint {
    /// Resolve `(http_base, ws_url_base)`. The HTTP base is where
    /// `/api/health` and `/api/session` are fetched; the WS base is the URL the
    /// `?v=&sg=&<credential>` query is appended to.
    pub(crate) fn resolve(&self) -> Result<(String, String), SessionError> {
        match self {
            Endpoint::Explicit { http_base, ws_url } => Ok((http_base.clone(), ws_url.clone())),
            Endpoint::SameOrigin => {
                let location = web_sys::window().ok_or(SessionError::NoWindow)?.location();
                let protocol = location.protocol().map_err(|_| SessionError::Location)?;
                let host = location.host().map_err(|_| SessionError::Location)?;
                let ws_scheme = if protocol == "https:" { "wss:" } else { "ws:" };
                let http_base = format!("{protocol}//{host}");
                let ws_url = format!("{ws_scheme}//{host}/ws");
                Ok((http_base, ws_url))
            }
        }
    }
}

/// A recoverable-or-terminal failure obtaining daemon status or a credential.
/// The driver maps every variant to a token-fenced dial input; the credential
/// itself never appears here.
#[derive(Debug)]
pub enum SessionError {
    /// No browser `Window` (not a browser context).
    NoWindow,
    /// `window.location` could not be read.
    Location,
    /// The request could not be constructed.
    Request,
    /// A network-level failure (offline, refused, aborted).
    Network,
    /// A non-2xx HTTP status. Carries the status for diagnostics.
    Status(u16),
    /// The response body could not be read or parsed.
    Body,
}

/// The validated daemon status the client presents at the gate (§6.1 step 1).
#[derive(serde::Deserialize)]
pub(crate) struct HealthView {
    /// The highest protocol generation the daemon speaks.
    pub(crate) protocol: u64,
    /// The lowest protocol generation the daemon accepts.
    pub(crate) min_protocol: u64,
    /// The storage generation the client must present as `sg`.
    pub(crate) storage_generation: u64,
}

/// A future returned by a [`SessionResolver`]; deliberately **not** `Send`
/// (a browser resolver awaits `!Send` `fetch` work).
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// How the connect credential is obtained and named on the WS URL. Injectable
/// so the #166 pairing/ticket flow (`?ct=`) swaps in later with no change to
/// the transport or the kernel.
pub trait SessionResolver {
    /// Resolve `(query-param-name, credential-value)` against `http_base`, or a
    /// [`SessionError`] to retry. The param is `"token"` today; a ticket
    /// resolver would return `"ct"`.
    fn resolve<'a>(
        &'a self,
        http_base: &'a str,
    ) -> LocalBoxFuture<'a, Result<(&'static str, String), SessionError>>;
}

/// The production default: `GET /api/session` → `{ "token": … }` →
/// `("token", <token>)`. Same-origin, so no CORS.
pub struct GetTokenResolver;

#[derive(serde::Deserialize)]
struct SessionView {
    token: String,
}

impl SessionResolver for GetTokenResolver {
    fn resolve<'a>(
        &'a self,
        http_base: &'a str,
    ) -> LocalBoxFuture<'a, Result<(&'static str, String), SessionError>> {
        let url = format!("{http_base}/api/session");
        Box::pin(async move {
            let text = fetch_text(&url, None).await?;
            let view: SessionView = serde_json::from_str(&text).map_err(|_| SessionError::Body)?;
            Ok(("token", view.token))
        })
    }
}

/// A dev/test resolver that supplies a known credential (against a
/// test-supervised daemon), with no fetch.
pub struct ExplicitResolver {
    param: &'static str,
    credential: String,
}

impl ExplicitResolver {
    /// A resolver that always returns `(param, credential)`. `param` is
    /// `"token"` or `"ct"`.
    pub fn new(param: &'static str, credential: impl Into<String>) -> Self {
        Self {
            param,
            credential: credential.into(),
        }
    }
}

impl SessionResolver for ExplicitResolver {
    fn resolve<'a>(
        &'a self,
        _http_base: &'a str,
    ) -> LocalBoxFuture<'a, Result<(&'static str, String), SessionError>> {
        let param = self.param;
        let credential = self.credential.clone();
        Box::pin(async move { Ok((param, credential)) })
    }
}

/// Fetch and validate `GET {http_base}/api/health` (§6.1 step 1).
pub(crate) async fn fetch_health(
    http_base: &str,
    signal: Option<&AbortSignal>,
) -> Result<HealthView, SessionError> {
    let url = format!("{http_base}/api/health");
    let text = fetch_text(&url, signal).await?;
    serde_json::from_str(&text).map_err(|_| SessionError::Body)
}

/// Issue a `GET` and return the response body text, erroring on any non-2xx
/// status or transport failure. `signal` wires an [`AbortSignal`] so an
/// in-flight fetch can be cancelled on a superseding dial or a stop.
pub(crate) async fn fetch_text(
    url: &str,
    signal: Option<&AbortSignal>,
) -> Result<String, SessionError> {
    let win = web_sys::window().ok_or(SessionError::NoWindow)?;
    let opts = RequestInit::new();
    opts.set_method("GET");
    // Loopback dev endpoints are cross-origin; the daemon mirrors CORS for
    // loopback origins. Same-origin production is unaffected.
    opts.set_mode(RequestMode::Cors);
    if let Some(sig) = signal {
        opts.set_signal(Some(sig));
    }
    let request = Request::new_with_str_and_init(url, &opts).map_err(|_| SessionError::Request)?;
    let response_value: JsValue = JsFuture::from(win.fetch_with_request(&request))
        .await
        .map_err(|_| SessionError::Network)?;
    let response: Response = response_value
        .dyn_into()
        .map_err(|_| SessionError::Network)?;
    if !response.ok() {
        return Err(SessionError::Status(response.status()));
    }
    let text_promise = response.text().map_err(|_| SessionError::Body)?;
    let text_value = JsFuture::from(text_promise)
        .await
        .map_err(|_| SessionError::Body)?;
    text_value.as_string().ok_or(SessionError::Body)
}
