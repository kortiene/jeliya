//! Protocol dispatch: one JSON request frame in, exactly one JSON response
//! frame out, with the envelope and error codes from `docs/PROTOCOL.md`.

use serde::Deserialize;
use serde_json::{json, Value};

use jeliya_core::error::{CoreError, CoreResult};
use jeliya_core::{identity, supervisor::RoomSupervisor};

use crate::AppState;

/// Handle one raw text frame; always returns a serialized response envelope
/// (`{id, ok:true, result}` or `{id, ok:false, error}`).
pub async fn handle_frame(raw: &str, state: &AppState) -> String {
    let parsed: Result<Value, _> = serde_json::from_str(raw);
    let (id, method, params) = match parsed {
        Ok(Value::Object(mut obj)) => {
            let id = obj.remove("id").unwrap_or(Value::Null);
            let method = obj.get("method").and_then(Value::as_str).map(str::to_owned);
            let params = obj.remove("params").unwrap_or_else(|| json!({}));
            match method {
                Some(method) => (id, method, params),
                None => {
                    return envelope_err(
                        id,
                        &CoreError::invalid("request must carry a string \"method\""),
                    )
                }
            }
        }
        _ => {
            return envelope_err(
                Value::Null,
                &CoreError::invalid("request must be a JSON object {id, method, params}"),
            )
        }
    };

    match dispatch(&method, params, state).await {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }).to_string(),
        Err(err) => envelope_err(id, &err),
    }
}

fn envelope_err(id: Value, err: &CoreError) -> String {
    json!({
        "id": id,
        "ok": false,
        "error": {
            "code": err.kind.code(),
            "message": err.message,
            "hint": err.hint,
        },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Params shapes
// ---------------------------------------------------------------------------

fn params<T: for<'de> Deserialize<'de>>(value: Value) -> CoreResult<T> {
    serde_json::from_value(value).map_err(|e| CoreError::invalid(format!("invalid params: {e}")))
}

#[derive(Deserialize)]
struct RoomIdParams {
    room_id: String,
}

#[derive(Deserialize)]
struct CreateRoomParams {
    name: String,
}

#[derive(Deserialize)]
struct OpenRoomParams {
    room_id: String,
    /// Optional dial hints (`"<endpoint_id>@<ip:port>"`) merged into the
    /// room's persisted hint set — loopback mode has no discovery.
    peers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct TimelineParams {
    room_id: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct InviteParams {
    room_id: String,
    identity_id: String,
    role: String,
    expiry: Option<Value>,
}

#[derive(Deserialize)]
struct JoinParams {
    ticket: String,
    name: Option<String>,
    peers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SendParams {
    room_id: String,
    body: String,
}

#[derive(Deserialize)]
struct StatusPostParams {
    room_id: String,
    label: String,
    message: Option<String>,
    progress: Option<u64>,
    artifacts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct FileShareParams {
    room_id: String,
    path: String,
    name: Option<String>,
    mime: Option<String>,
}

#[derive(Deserialize)]
struct FileFetchParams {
    room_id: String,
    file_id: String,
    save_dir: Option<String>,
}

#[derive(Deserialize)]
struct AgentHistoryParams {
    room_id: String,
    identity_id: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct PipeExposeParams {
    room_id: String,
    target: String,
    peer_identity: String,
}

#[derive(Deserialize)]
struct PipeIdParams {
    room_id: String,
    pipe_id: String,
}

/// Accept `"24h"` / `"3600"` (string spec) or a bare number of seconds.
fn expiry_spec(value: Option<Value>) -> CoreResult<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|secs| Some(format!("{secs}s")))
            .ok_or_else(|| CoreError::invalid("expiry must be a positive integer of seconds")),
        Some(other) => Err(CoreError::invalid(format!(
            "expiry must be a string like \"24h\" or a number of seconds, got {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

async fn dispatch(method: &str, raw_params: Value, state: &AppState) -> CoreResult<Value> {
    // The supervisor is a plain `Arc` — no daemon-wide lock is taken here, so a
    // slow request (a `file.fetch` against an offline provider, a `pipe.connect`
    // busy-wait) runs on its own without head-of-line blocking any other client
    // or the push loop. The supervisor guards its own session map internally,
    // only for the span of a map lookup, never across a network await.
    let sup = &state.supervisor;
    match method {
        // ---- Daemon & identity -------------------------------------------
        "daemon.status" => Ok(daemon_status(sup, state)),
        "daemon.shutdown" => {
            // Reply first, then die: the shutdown signal is delayed a beat so
            // this response flushes to the requesting client before teardown.
            let tx = state.shutdown_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                let _ = tx.send("daemon.shutdown RPC".to_owned()).await;
            });
            Ok(json!({ "shutting_down": true }))
        }
        "identity.create" => {
            let profile = identity::create(&state.data_dir)?;
            Ok(json!({
                "identity_id": profile.identity_id,
                "device_id": profile.device_id,
            }))
        }

        // ---- Rooms --------------------------------------------------------
        "room.create" => {
            let p: CreateRoomParams = params(raw_params)?;
            let room_id = sup.create_room(&p.name)?;
            Ok(json!({ "room_id": room_id }))
        }
        "room.list" => Ok(json!({ "rooms": sup.list_rooms().await? })),
        "room.open" => {
            let p: OpenRoomParams = params(raw_params)?;
            sup.open_room(&p.room_id, p.peers.as_deref().unwrap_or(&[]))
                .await
        }
        "room.close" => {
            let p: RoomIdParams = params(raw_params)?;
            sup.close_room(&p.room_id).await?;
            Ok(json!({}))
        }
        "room.leave" => {
            let p: RoomIdParams = params(raw_params)?;
            let event_id = sup.leave_room(&p.room_id).await?;
            Ok(json!({ "event_id": event_id }))
        }
        "room.timeline" => {
            let p: TimelineParams = params(raw_params)?;
            Ok(json!({ "events": sup.timeline(&p.room_id, p.limit).await? }))
        }
        "room.members" => {
            let p: RoomIdParams = params(raw_params)?;
            Ok(json!({ "members": sup.members(&p.room_id).await? }))
        }
        "invite.create" => {
            let p: InviteParams = params(raw_params)?;
            let expiry = expiry_spec(p.expiry)?;
            let ticket = sup
                .create_invite(&p.room_id, &p.identity_id, &p.role, expiry.as_deref())
                .await?;
            Ok(json!({ "ticket": ticket }))
        }
        "room.join" => {
            let p: JoinParams = params(raw_params)?;
            let room_id = sup
                .join_room(
                    &p.ticket,
                    p.name.as_deref(),
                    p.peers.as_deref().unwrap_or(&[]),
                )
                .await?;
            Ok(json!({ "room_id": room_id }))
        }

        // ---- Messages & agent status ---------------------------------------
        "message.send" => {
            let p: SendParams = params(raw_params)?;
            let event_id = sup.send_message(&p.room_id, &p.body).await?;
            Ok(json!({ "event_id": event_id }))
        }
        "status.post" => {
            let p: StatusPostParams = params(raw_params)?;
            let event_id = sup
                .post_status(
                    &p.room_id,
                    &p.label,
                    p.message.as_deref(),
                    p.progress,
                    p.artifacts.as_deref().unwrap_or(&[]),
                )
                .await?;
            Ok(json!({ "event_id": event_id }))
        }

        // ---- Files ----------------------------------------------------------
        "file.share" => {
            let p: FileShareParams = params(raw_params)?;
            sup.share_file(&p.room_id, &p.path, p.name.as_deref(), p.mime.as_deref())
                .await
        }
        "file.list" => {
            let p: RoomIdParams = params(raw_params)?;
            Ok(json!({ "files": sup.list_files(&p.room_id).await? }))
        }
        "file.fetch" => {
            let p: FileFetchParams = params(raw_params)?;
            sup.fetch_file(&p.room_id, &p.file_id, p.save_dir.as_deref())
                .await
        }

        // ---- Pipes ----------------------------------------------------------
        "pipe.expose" => {
            let p: PipeExposeParams = params(raw_params)?;
            sup.pipe_expose(&p.room_id, &p.target, &p.peer_identity)
                .await
        }
        "pipe.list" => {
            let p: RoomIdParams = params(raw_params)?;
            Ok(json!({ "pipes": sup.pipe_list(&p.room_id)? }))
        }
        "pipe.connect" => {
            let p: PipeIdParams = params(raw_params)?;
            let local_addr = sup.pipe_connect(&p.room_id, &p.pipe_id).await?;
            Ok(json!({ "local_addr": local_addr }))
        }
        "pipe.close" => {
            let p: PipeIdParams = params(raw_params)?;
            sup.pipe_close(&p.room_id, &p.pipe_id).await
        }

        // ---- Agents (fleet reads) --------------------------------------------
        "agents.fleet" => sup.agents_fleet().await,
        "agent.history" => {
            let p: AgentHistoryParams = params(raw_params)?;
            sup.agent_history(&p.room_id, &p.identity_id, p.limit)
        }

        // ---- Peers ----------------------------------------------------------
        "peers.status" => {
            let p: RoomIdParams = params(raw_params)?;
            Ok(json!({ "peers": sup.peers_status(&p.room_id).await? }))
        }

        other => Err(CoreError::invalid(format!("unknown method {other:?}"))
            .with_hint("see docs/PROTOCOL.md for the method list")),
    }
}

fn daemon_status(sup: &RoomSupervisor, state: &AppState) -> Value {
    let identity = identity::load_profile(&state.data_dir)
        .ok()
        .flatten()
        .map(|p| json!({ "identity_id": p.identity_id, "device_id": p.device_id }));
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": crate::PROTOCOL_VERSION,
        "pid": std::process::id(),
        "port": state.port,
        "data_dir": state.data_dir.display().to_string(),
        "mode": sup.mode(),
        "identity": identity,
        "endpoint": sup.status_endpoint(),
        "rooms_open": sup.open_rooms(),
    })
}
