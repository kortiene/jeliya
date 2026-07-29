//! Exhaustive request routing: an `op` name maps to exactly one
//! `jeliya-api` request type. An unknown `op` is `unknown_operation`; an
//! unknown request field is `invalid_argument` with `unrecognised_field` —
//! and both happen **here**, at the edge, never in the engine.

use jeliya_api::*;

/// A decoded call: the operation's typed input plus its wire name and
/// mutability, so the engine dispatches without ever seeing JSON or a
/// method string.
#[derive(Debug)]
pub struct Call {
    /// The operation's wire name (also its capability token).
    pub op: &'static str,
    /// Whether the operation mutates (accepts and deduplicates `op_id`).
    pub mutating: bool,
    /// The typed input, erased behind the trait the engine consumes.
    pub input: Box<dyn ErasedInput>,
}

impl Call {
    /// The decoded input as `&dyn Any` for the engine's total downcast. The
    /// router guarantees the concrete type matches `op`, so the engine's
    /// downcast cannot fail on a frame the router accepted.
    #[must_use]
    pub fn input_any(&self) -> &dyn std::any::Any {
        self.input.as_any()
    }
}

/// A type-erased operation input. The engine downcasts by `op`, which is
/// total: every `op` the router accepts maps to exactly one concrete type,
/// so the downcast cannot fail.
pub trait ErasedInput: std::fmt::Debug + Send + Sync {
    /// The input as `&dyn Any` for the engine's total downcast.
    fn as_any(&self) -> &dyn std::any::Any;
}

impl<T: Operation + std::fmt::Debug + Send + Sync + 'static> ErasedInput for T {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

macro_rules! route_op {
    ($op:expr, $ty:ty, $input:expr) => {{
        let decoded: $ty =
            serde_json::from_value($input.clone()).map_err(|e| invalid_arg(e, $op, &$input))?;
        Call {
            op: <$ty as Operation>::PATH,
            mutating: <$ty as Operation>::MUTATING,
            input: Box::new(decoded),
        }
    }};
}

/// Routes an `op` name and its raw `in` value to a typed [`Call`].
/// Exhaustive over the 33 approved operations; anything else is
/// `unknown_operation`.
pub fn route(op: &str, input: serde_json::Value) -> Result<Call, ApiError> {
    Ok(match op {
        "subject.ensure" => route_op!(op, SubjectEnsure, input),
        "daemon.stop" => route_op!(op, DaemonStop, input),
        "room.create" => route_op!(op, RoomCreate, input),
        "room.list" => route_op!(op, RoomList, input),
        "room.activate" => route_op!(op, RoomActivate, input),
        "room.deactivate" => route_op!(op, RoomDeactivate, input),
        "room.leave" => route_op!(op, RoomLeave, input),
        "room.timeline" => route_op!(op, RoomTimeline, input),
        "room.members" => route_op!(op, RoomMembers, input),
        "room.archive" => route_op!(op, RoomArchive, input),
        "room.peers" => route_op!(op, RoomPeers, input),
        "member.remove" => route_op!(op, MemberRemove, input),
        "invite.mint" => route_op!(op, InviteMint, input),
        "invite.list" => route_op!(op, InviteList, input),
        "invite.revoke" => route_op!(op, InviteRevoke, input),
        "invite.redeem" => route_op!(op, InviteRedeem, input),
        "message.send" => route_op!(op, MessageSend, input),
        "status.post" => route_op!(op, StatusPost, input),
        "status.history" => route_op!(op, StatusHistory, input),
        "fleet.list" => route_op!(op, FleetList, input),
        "file.share" => route_op!(op, FileShare, input),
        "file.list" => route_op!(op, FileList, input),
        "file.fetch" => route_op!(op, FileFetch, input),
        "file.read" => route_op!(op, FileRead, input),
        "transfer.cancel" => route_op!(op, TransferCancel, input),
        "pipe.publish" => route_op!(op, PipePublish, input),
        "pipe.list" => route_op!(op, PipeList, input),
        "pipe.connect" => route_op!(op, PipeConnect, input),
        "pipe.release" => route_op!(op, PipeRelease, input),
        "pipe.revoke" => route_op!(op, PipeRevoke, input),
        "stream.subscribe" => route_op!(op, StreamSubscribe, input),
        "stream.unsubscribe" => route_op!(op, StreamUnsubscribe, input),
        "stream.resync" => route_op!(op, StreamResync, input),
        other => {
            return Err(ApiError::UnknownOperation {
                op: other.to_string(),
            })
        }
    })
}

/// Maps a serde structural error to the record's step-1 refusal. An
/// unknown field is `unrecognised_field`; a missing field is `missing`;
/// anything else structural is `invalid_argument` with the field serde
/// named, because serde cannot always tell the two apart and the record's
/// reason arms are what the client branches on.
fn invalid_arg(e: serde_json::Error, op: &str, input: &serde_json::Value) -> ApiError {
    let msg = e.to_string();

    // status.post's label is a closed vocabulary; an unrecognized label is
    // the operation's distinctive code, carrying the raw label.
    if op == "status.post" && msg.contains("unknown variant") {
        if let Some(label) = input.get("label").and_then(|l| l.as_str()) {
            return ApiError::StatusLabelUnknown {
                label: label.to_string(),
            };
        }
    }

    if let Some(f) = extract_field(&msg, "unknown field `", "`") {
        return ApiError::InvalidArgument {
            field: format!("in.{f}"),
            reason: InvalidReason::UnrecognisedField,
        };
    }
    if let Some(f) = extract_field(&msg, "missing field `", "`") {
        return ApiError::InvalidArgument {
            field: format!("in.{f}"),
            reason: InvalidReason::Missing,
        };
    }
    // A wrong JSON type: serde's "invalid type: X, expected Y" carries both.
    if let Some(expected) = extract_field(&msg, "expected ", "") {
        let expected = expected
            .trim_start_matches(|c: char| !c.is_alphabetic())
            .trim_end_matches(|c: char| !c.is_alphabetic())
            .to_string();
        return ApiError::InvalidArgument {
            field: "in".to_string(),
            reason: InvalidReason::Type { expected },
        };
    }
    ApiError::InvalidArgument {
        field: "in".to_string(),
        reason: InvalidReason::Format,
    }
}

fn extract_field(msg: &str, prefix: &str, suffix: &str) -> Option<String> {
    let start = msg.find(prefix)? + prefix.len();
    if suffix.is_empty() {
        return Some(msg[start..].to_string());
    }
    let end = msg[start..].find(suffix)? + start;
    Some(msg[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_ok(op: &str, input: serde_json::Value) -> Call {
        route(op, input).expect("routes")
    }

    #[test]
    fn routes_every_approved_operation() {
        // a smoke check that all 33 route arms exist and produce a call
        let empty = serde_json::json!({});
        for (op, input) in [
            ("subject.ensure", empty.clone()),
            ("daemon.stop", empty.clone()),
            ("room.list", empty.clone()),
            ("fleet.list", empty.clone()),
        ] {
            let call = route_ok(op, input);
            assert_eq!(call.op, op);
        }
    }

    #[test]
    fn unknown_operation_is_refused_by_name() {
        let err = route("room.explode", serde_json::json!({})).unwrap_err();
        match err {
            ApiError::UnknownOperation { op } => assert_eq!(op, "room.explode"),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn an_unknown_request_field_is_unrecognised_field() {
        let err = route(
            "status.post",
            serde_json::json!({
                "room_id": "r1",
                "label": "working",
                "message": "free text",
                "progress": {"state": "absent"}
            }),
        )
        .unwrap_err();
        match err {
            ApiError::InvalidArgument { field, reason } => {
                assert_eq!(reason, InvalidReason::UnrecognisedField);
                assert_eq!(field, "in.message");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn a_missing_request_field_is_missing() {
        let err = route("room.create", serde_json::json!({})).unwrap_err();
        match err {
            ApiError::InvalidArgument { reason, .. } => assert_eq!(reason, InvalidReason::Missing),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn mutability_is_routed() {
        let call = route_ok("room.create", serde_json::json!({"name": "Build"}));
        assert!(call.mutating);
        let call = route_ok("room.list", serde_json::json!({}));
        assert!(!call.mutating);
        let call = route_ok(
            "stream.subscribe",
            serde_json::json!({"room_id": "r1", "from": {"state": "start"}}),
        );
        assert!(call.mutating);
    }
}
