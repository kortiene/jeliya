//! Browser transport for the #158 spike: `GET /api/session` for the token,
//! then one WebSocket to `/ws?token=…`, requests correlated by numeric id.
//!
//! Deliberately minimal. No reconnect, no backoff, no queueing — those are
//! #168's bounded-kernel semantics and inventing them here would create exactly
//! the "spike code promoted by assumption" the acceptance criteria forbid. The
//! socket dying is surfaced as a visible error, not papered over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use futures::channel::oneshot;
use futures::stream::{SplitSink, StreamExt};
use futures::SinkExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use serde::de::DeserializeOwned;

use crate::proto::{DaemonError, Frame, Request, RoomEventPush, SessionToken};

#[derive(Debug)]
pub enum CallError {
    Daemon(DaemonError),
    Transport(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Daemon(e) => write!(f, "{e}"),
            CallError::Transport(m) => write!(f, "transport: {m}"),
        }
    }
}

type Pending = Rc<RefCell<HashMap<u64, oneshot::Sender<Result<serde_json::Value, DaemonError>>>>>;

#[derive(Clone)]
pub struct Client {
    write: Rc<RefCell<SplitSink<WebSocket, Message>>>,
    pending: Pending,
    next_id: Rc<RefCell<u64>>,
}

/// Same-origin page URL -> daemon origin. The daemon serves this page, so the
/// control socket is its own origin; deriving it tracks the ephemeral port for
/// free rather than hardcoding one.
fn origin() -> Result<(String, String), String> {
    let loc = web_sys::window().ok_or("no window")?.location();
    let host = loc.host().map_err(|_| "no host")?;
    let proto = loc.protocol().map_err(|_| "no protocol")?;
    let ws_scheme = if proto == "https:" { "wss" } else { "ws" };
    Ok((format!("{proto}//{host}"), format!("{ws_scheme}://{host}")))
}

impl Client {
    /// Fetch a session token, open the socket, and start the read loop.
    ///
    /// `on_push` receives every `room.event` payload in arrival order.
    pub async fn connect(
        mut on_push: impl FnMut(RoomEventPush) + 'static,
    ) -> Result<Self, String> {
        let (http_origin, ws_origin) = origin()?;

        // Same-origin GET: the browser sets `Sec-Fetch-Site: same-origin`, which
        // is what the daemon accepts for a page it served itself.
        let session: SessionToken = gloo_net::http::Request::get(&format!("{http_origin}/api/session"))
            .send()
            .await
            .map_err(|e| format!("session request failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("session response was not a token: {e}"))?;

        let ws = WebSocket::open(&format!("{ws_origin}/ws?token={}", session.token))
            .map_err(|e| format!("websocket open failed: {e}"))?;
        // The token is never logged: it grants full control of the daemon.
        let (write, mut read) = ws.split();

        let pending: Pending = Rc::new(RefCell::new(HashMap::new()));
        let read_pending = pending.clone();

        wasm_bindgen_futures::spawn_local(async move {
            while let Some(msg) = read.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Bytes(_)) => continue,
                    Err(_) => break,
                };
                let Ok(frame) = serde_json::from_str::<Frame>(&text) else {
                    continue;
                };
                if frame.push.as_deref() == Some("room.event") {
                    if let Some(data) = frame.data {
                        if let Ok(push) = serde_json::from_value::<RoomEventPush>(data) {
                            on_push(push);
                        }
                    }
                    continue;
                }
                let Some(id) = frame.id else { continue };
                let Some(tx) = read_pending.borrow_mut().remove(&id) else {
                    continue;
                };
                let outcome = if frame.ok == Some(true) {
                    Ok(frame.result.unwrap_or(serde_json::Value::Null))
                } else {
                    Err(frame.error.unwrap_or(DaemonError {
                        code: "internal".into(),
                        message: "daemon returned an error with no body".into(),
                        hint: None,
                    }))
                };
                let _ = tx.send(outcome);
            }
            // Socket closed: fail everything still waiting rather than hang.
            for (_, tx) in read_pending.borrow_mut().drain() {
                let _ = tx.send(Err(DaemonError {
                    code: "connection_lost".into(),
                    message: "connection to daemon lost".into(),
                    hint: None,
                }));
            }
        });

        Ok(Client {
            write: Rc::new(RefCell::new(write)),
            pending,
            next_id: Rc::new(RefCell::new(1)),
        })
    }

    pub async fn call<R: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<R, CallError> {
        let id = {
            let mut n = self.next_id.borrow_mut();
            let id = *n;
            *n += 1;
            id
        };
        let frame = serde_json::to_string(&Request { id, method, params })
            .map_err(|e| CallError::Transport(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        self.pending.borrow_mut().insert(id, tx);

        self.write
            .borrow_mut()
            .send(Message::Text(frame))
            .await
            .map_err(|e| CallError::Transport(e.to_string()))?;

        match rx.await {
            Ok(Ok(value)) => serde_json::from_value(value)
                .map_err(|e| CallError::Transport(format!("unexpected result shape: {e}"))),
            Ok(Err(err)) => Err(CallError::Daemon(err)),
            Err(_) => Err(CallError::Transport("request dropped".into())),
        }
    }
}
