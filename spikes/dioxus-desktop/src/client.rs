//! The native transport half of issue #159: a Rust WebSocket client that
//! authenticates to `jeliyad` and keeps the token out of the WebView.
//!
//! Two properties matter more than the code:
//!
//! - the token travels in an `Authorization: Bearer` HEADER, never in the URL.
//!   `?token=` works and is what the browser UI must use (a browser cannot set
//!   headers on a WebSocket), but a URL is the single most leak-prone place to
//!   put a secret — it lands in logs, in error messages, and in anything that
//!   renders a connection string. A native client has no such excuse;
//! - the token is read only from the 0600 portfile. `/api/session` exists to
//!   hand a token to the browser and this client never calls it.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug)]
pub enum ClientError {
    Connect(String),
    Protocol(String),
    /// The daemon answered, but with a different protocol generation than this
    /// build speaks. A hard stop, never a warning — see `check_protocol`.
    ProtocolMismatch { expected: u32, found: u32 },
    Rpc { code: String, message: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(why) => write!(f, "could not connect: {why}"),
            Self::Protocol(why) => write!(f, "protocol error: {why}"),
            Self::ProtocolMismatch { expected, found } => write!(
                f,
                "this build speaks protocol {expected}; the daemon speaks {found}"
            ),
            Self::Rpc { code, message } => write!(f, "{code}: {message}"),
        }
    }
}

/// The protocol generation this spike was built against. A daemon that speaks
/// anything else is refused rather than half-driven: the whole point of the
/// clean-slate program is one generation at a time.
pub const SPEAKS_PROTOCOL: u32 = 1;

#[derive(Debug, Deserialize)]
struct Reply {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: String,
    message: String,
}

type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// An authenticated connection to the daemon.
///
/// Deliberately NOT `Clone`, and deliberately holding no copy of the token
/// after the handshake: once connected there is nothing left to leak, and a
/// non-cloneable handle cannot be smuggled into a component tree.
pub struct Connection {
    socket: Socket,
    next_id: u64,
}

impl Connection {
    /// Dial `ws_url` and authenticate with `token`.
    ///
    /// `token` is borrowed, used to build one header, and never stored.
    pub async fn connect(ws_url: &str, token: &str) -> Result<Self, ClientError> {
        if !ws_url.starts_with("ws://127.0.0.1:") {
            // The portfile is trusted, but a loopback assertion is cheap and
            // this is the one place a bad value would become a network dial.
            return Err(ClientError::Connect(format!(
                "refusing a non-loopback daemon URL: {ws_url}"
            )));
        }

        let mut request = ws_url
            .into_client_request()
            .map_err(|e| ClientError::Connect(e.to_string()))?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .map_err(|_| ClientError::Connect("token is not a valid header".into()))?,
        );

        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| ClientError::Connect(e.to_string()))?;

        Ok(Self { socket, next_id: 1 })
    }

    /// One request/response round trip.
    ///
    /// The daemon interleaves pushes with replies, so this skips any frame that
    /// is not the reply we are waiting for. A real client would route those to
    /// a subscriber; a spike only has to not deadlock on them.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;

        let frame = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;

        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|e| ClientError::Protocol(e.to_string()))?;
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(reply) = serde_json::from_str::<Reply>(&text) else {
                // A push, not a reply. Skip it.
                continue;
            };
            if reply.id != id {
                continue;
            }
            return if reply.ok {
                Ok(reply.result.unwrap_or(Value::Null))
            } else {
                let error = reply.error.ok_or_else(|| {
                    ClientError::Protocol("a failed reply carried no error".into())
                })?;
                Err(ClientError::Rpc {
                    code: error.code,
                    message: error.message,
                })
            };
        }
        Err(ClientError::Protocol("the socket closed mid-call".into()))
    }

    /// `daemon.status`, with the protocol generation range-checked.
    ///
    /// Called as the first frame after connecting. A mismatch is fatal: pairing
    /// a new shell with an old daemon is exactly the failure the resolution
    /// order's `PATH` fallback would cause, which is why this spike does not
    /// implement that fallback either.
    pub async fn check_protocol(&mut self) -> Result<Value, ClientError> {
        let status = self.call("daemon.status", json!({})).await?;
        let found = status
            .get("protocol")
            .and_then(Value::as_u64)
            .ok_or_else(|| ClientError::Protocol("daemon.status carried no protocol".into()))?
            as u32;
        if found != SPEAKS_PROTOCOL {
            return Err(ClientError::ProtocolMismatch {
                expected: SPEAKS_PROTOCOL,
                found,
            });
        }
        Ok(status)
    }
}
