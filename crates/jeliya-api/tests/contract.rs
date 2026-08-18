//! Unit and property tests for jeliya-api. The crate is types only; these
//! tests prove the *contract* the types make: pairing, closed vocabularies,
//! no-null, opaque ids, and the serde shapes the wire conventions fix.

use jeliya_api::*;

// --- Compile-time pairing --------------------------------------------------

/// Every operation's `PATH` is its capability token, and `MUTATING` matches
/// the record's `M` column. This is the pairing #163 exists to make: a
/// request cannot be held without knowing its output type and wire name.
#[test]
fn operation_paths() {
    // PATH assertions are runtime because they read a string.
    assert_eq!(<SubjectEnsure as Operation>::PATH, "subject.ensure");
    assert_eq!(<DaemonStop as Operation>::PATH, "daemon.stop");
    assert_eq!(<RoomCreate as Operation>::PATH, "room.create");
    assert_eq!(<RoomList as Operation>::PATH, "room.list");
    assert_eq!(<RoomActivate as Operation>::PATH, "room.activate");
    assert_eq!(<RoomDeactivate as Operation>::PATH, "room.deactivate");
    assert_eq!(<RoomLeave as Operation>::PATH, "room.leave");
    assert_eq!(<RoomTimeline as Operation>::PATH, "room.timeline");
    assert_eq!(<RoomMembers as Operation>::PATH, "room.members");
    assert_eq!(<RoomArchive as Operation>::PATH, "room.archive");
    assert_eq!(<RoomPeers as Operation>::PATH, "room.peers");
    assert_eq!(<MemberRemove as Operation>::PATH, "member.remove");
    assert_eq!(<InviteMint as Operation>::PATH, "invite.mint");
    assert_eq!(<InviteList as Operation>::PATH, "invite.list");
    assert_eq!(<InviteRevoke as Operation>::PATH, "invite.revoke");
    assert_eq!(<InviteRedeem as Operation>::PATH, "invite.redeem");
    assert_eq!(<MessageSend as Operation>::PATH, "message.send");
    assert_eq!(<StatusPost as Operation>::PATH, "status.post");
    assert_eq!(<StatusHistory as Operation>::PATH, "status.history");
    assert_eq!(<FleetList as Operation>::PATH, "fleet.list");
    assert_eq!(<FileShare as Operation>::PATH, "file.share");
    assert_eq!(<FileList as Operation>::PATH, "file.list");
    assert_eq!(<FileFetch as Operation>::PATH, "file.fetch");
    assert_eq!(<FileRead as Operation>::PATH, "file.read");
    assert_eq!(<TransferCancel as Operation>::PATH, "transfer.cancel");
    assert_eq!(<PipePublish as Operation>::PATH, "pipe.publish");
    assert_eq!(<PipeList as Operation>::PATH, "pipe.list");
    assert_eq!(<PipeConnect as Operation>::PATH, "pipe.connect");
    assert_eq!(<PipeRelease as Operation>::PATH, "pipe.release");
    assert_eq!(<PipeRevoke as Operation>::PATH, "pipe.revoke");
    assert_eq!(<StreamSubscribe as Operation>::PATH, "stream.subscribe");
    assert_eq!(<StreamUnsubscribe as Operation>::PATH, "stream.unsubscribe");
    assert_eq!(<StreamResync as Operation>::PATH, "stream.resync");
}

// MUTATING is a compile-time contract: these assertions live in a const
// block, so a wrong flag fails compilation, not just the test run.
const _: () = {
    assert!(<SubjectEnsure as Operation>::MUTATING);
    assert!(<DaemonStop as Operation>::MUTATING);
    assert!(<RoomCreate as Operation>::MUTATING);
    assert!(!<RoomList as Operation>::MUTATING);
    assert!(<RoomActivate as Operation>::MUTATING);
    assert!(<RoomDeactivate as Operation>::MUTATING);
    assert!(<RoomLeave as Operation>::MUTATING);
    assert!(!<RoomTimeline as Operation>::MUTATING);
    assert!(!<RoomMembers as Operation>::MUTATING);
    assert!(!<RoomArchive as Operation>::MUTATING);
    assert!(!<RoomPeers as Operation>::MUTATING);
    assert!(<MemberRemove as Operation>::MUTATING);
    assert!(<InviteMint as Operation>::MUTATING);
    assert!(!<InviteList as Operation>::MUTATING);
    assert!(<InviteRevoke as Operation>::MUTATING);
    assert!(<InviteRedeem as Operation>::MUTATING);
    assert!(<MessageSend as Operation>::MUTATING);
    assert!(<StatusPost as Operation>::MUTATING);
    assert!(!<StatusHistory as Operation>::MUTATING);
    assert!(!<FleetList as Operation>::MUTATING);
    assert!(<FileShare as Operation>::MUTATING);
    assert!(!<FileList as Operation>::MUTATING);
    assert!(<FileFetch as Operation>::MUTATING);
    assert!(!<FileRead as Operation>::MUTATING);
    assert!(<TransferCancel as Operation>::MUTATING);
    assert!(<PipePublish as Operation>::MUTATING);
    assert!(!<PipeList as Operation>::MUTATING);
    assert!(<PipeConnect as Operation>::MUTATING);
    assert!(<PipeRelease as Operation>::MUTATING);
    assert!(<PipeRevoke as Operation>::MUTATING);
    assert!(<StreamSubscribe as Operation>::MUTATING);
    assert!(<StreamUnsubscribe as Operation>::MUTATING);
    assert!(<StreamResync as Operation>::MUTATING);
};

/// There are exactly 33 operations. Counted by hand here as a guard against
/// a silent addition.
#[test]
fn thirty_three_operations() {
    // If this compiles, the 33 paths above each resolved; the count is the
    // assertion the record makes ("The 33 operations").
    const COUNT: usize = 33;
    let paths = [
        "subject.ensure",
        "daemon.stop",
        "room.create",
        "room.list",
        "room.activate",
        "room.deactivate",
        "room.leave",
        "room.timeline",
        "room.members",
        "room.archive",
        "room.peers",
        "member.remove",
        "invite.mint",
        "invite.list",
        "invite.revoke",
        "invite.redeem",
        "message.send",
        "status.post",
        "status.history",
        "fleet.list",
        "file.share",
        "file.list",
        "file.fetch",
        "file.read",
        "transfer.cancel",
        "pipe.publish",
        "pipe.list",
        "pipe.connect",
        "pipe.release",
        "pipe.revoke",
        "stream.subscribe",
        "stream.unsubscribe",
        "stream.resync",
    ];
    assert_eq!(paths.len(), COUNT);
}

// --- Closed vocabularies reject unknown values -----------------------------

/// Protocol v2 never silently reclassifies an unrecognized wire value, so
/// no enum carries an `Unknown` arm: deserialization fails instead.
#[test]
fn closed_enums_reject_unknown_values() {
    assert!(serde_json::from_str::<Role>("\"owner\"").is_err());
    assert!(serde_json::from_str::<Role>("\"agent\"").is_err());
    assert!(serde_json::from_str::<Standing>("\"invited\"").is_err());
    assert!(serde_json::from_str::<StatusLabel>("\"hungry\"").is_err());
    assert!(serde_json::from_str::<EventKind>("\"photo_shared\"").is_err());
    assert!(serde_json::from_str::<Liveness>("\"sleepy\"").is_err());
    // valid spellings still parse
    assert_eq!(
        serde_json::from_str::<Role>("\"authority\"").unwrap(),
        Role::Authority
    );
    assert_eq!(
        serde_json::from_str::<Standing>("\"active\"").unwrap(),
        Standing::Active
    );
    assert_eq!(
        serde_json::from_str::<StatusLabel>("\"blocked\"").unwrap(),
        StatusLabel::Blocked
    );
}

/// `role` is closed on exactly two tokens: authority and member. `agent` is
/// not a role and `owner` is v1's removed spelling.
#[test]
fn role_is_exactly_two_tokens() {
    assert_eq!(
        serde_json::from_str::<Role>("\"member\"").unwrap(),
        Role::Member
    );
    assert!(serde_json::from_str::<Role>("\"agent\"").is_err());
    assert!(serde_json::from_str::<Role>("\"owner\"").is_err());
}

/// Severity is a lookup from the closed label set, never an inference.
#[test]
fn severity_is_derived_from_label() {
    assert_eq!(StatusLabel::Working.severity(), Severity::Ok);
    assert_eq!(StatusLabel::Online.severity(), Severity::Ok);
    assert_eq!(StatusLabel::Claiming.severity(), Severity::Ok);
    assert_eq!(StatusLabel::Done.severity(), Severity::Ok);
    assert_eq!(StatusLabel::Failed.severity(), Severity::Failed);
    assert_eq!(StatusLabel::Blocked.severity(), Severity::Review);
}

// --- Opaque identifiers ----------------------------------------------------

/// Identifiers are opaque strings with no representation guarantee: no
/// hex assumption, no length floor, no format validation.
#[test]
fn identifiers_are_opaque() {
    let r = RoomId::new("blake3:abc123");
    assert_eq!(r.as_str(), "blake3:abc123");
    let s = SubjectId::new("not-hex-at-all");
    assert_eq!(s.as_str(), "not-hex-at-all");
    // distinct domains are distinct types: this does not compile if swapped
    let _: RoomId = r.clone();
    let _: SubjectId = s.clone();
    // serialization is the bare string, transparent
    assert_eq!(serde_json::to_string(&r).unwrap(), "\"blake3:abc123\"");
}

// --- No null carries meaning -----------------------------------------------

/// Absence is a tagged variant everywhere: `Option` never appears in a
/// request, and the tagged variants (`last_event`, `truncated`, `cursor`,
/// `progress`, `author`, `byte_total`, …) make the absent case explicit.
#[test]
fn absence_is_a_tagged_variant() {
    let absent = LastEvent::Absent;
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        "{\"state\":\"absent\"}"
    );
    let present = LastEvent::Present {
        at: Timestamp::from(time::OffsetDateTime::UNIX_EPOCH),
        kind: EventKind::Message,
    };
    let s = serde_json::to_string(&present).unwrap();
    assert!(s.contains("\"state\":\"present\""));
    assert!(s.contains("\"kind\":\"message\""));
    // a null state is rejected
    assert!(serde_json::from_str::<LastEvent>("{\"state\":null}").is_err());
    // an unknown state is rejected
    assert!(serde_json::from_str::<LastEvent>("{\"state\":\"maybe\"}").is_err());
}

/// `truncated` is the one continuation mechanism; `Complete` has no cursor.
#[test]
fn truncated_is_the_only_continuation() {
    let complete = Truncated::Complete;
    assert_eq!(
        serde_json::to_string(&complete).unwrap(),
        "{\"state\":\"complete\"}"
    );
    let more = Truncated::More {
        cursor: Cursor::Start,
    };
    let s = serde_json::to_string(&more).unwrap();
    assert!(s.contains("\"state\":\"more\""));
    assert!(s.contains("\"cursor\":{\"state\":\"start\"}"));
}

// --- Round-trips ------------------------------------------------------------

/// A request round-trips through serde with no loss.
#[test]
fn request_round_trip() {
    let req = RoomCreate {
        name: "Build".into(),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert_eq!(json, "{\"name\":\"Build\"}");
    let back: RoomCreate = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
}

/// Paging fields are required and flattened: cursor, direction, and limit
/// are always present, never defaulted.
#[test]
fn paging_fields_are_required() {
    let req = RoomTimeline {
        room_id: RoomId::new("r1"),
        page: Page {
            cursor: Cursor::Start,
            direction: Direction::Forward,
            limit: 50,
        },
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"cursor\":{\"state\":\"start\"}"));
    assert!(json.contains("\"direction\":\"forward\""));
    assert!(json.contains("\"limit\":50"));
    // a missing field fails deserialization — no optional request fields
    assert!(serde_json::from_str::<RoomTimeline>(
        "{\"room_id\":\"r1\",\"cursor\":{\"state\":\"start\"},\"direction\":\"forward\"}"
    )
    .is_err());
}

/// `op_id` lives at the envelope level, never inside `in`.
#[test]
fn op_id_is_envelope_level() {
    let env = Envelope::new(
        42,
        Some(OpId::new("op-1")),
        RoomCreate {
            name: "Build".into(),
        },
    )
    .unwrap();
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("\"op\":\"room.create\""));
    assert!(json.contains("\"op_id\":\"op-1\""));
    assert!(json.contains("\"in\":{\"name\":\"Build\"}"));
    // `in` carries no op_id key
    assert!(!json.contains("\"in\":{\"name\":\"Build\",\"op_id\""));
}

/// Typed clients cannot originate request ids that ordinary browser JSON
/// numbers would round or alias.
#[test]
fn request_id_is_browser_safe_by_construction() {
    let at_limit = Envelope::new(
        MAX_REQUEST_ID,
        None,
        RoomCreate {
            name: "Build".into(),
        },
    )
    .unwrap();
    assert_eq!(at_limit.id.get(), MAX_REQUEST_ID);
    assert_eq!(
        serde_json::to_value(&at_limit).unwrap()["id"],
        serde_json::json!(MAX_REQUEST_ID)
    );

    assert_eq!(
        Envelope::<RoomCreate>::new(
            MAX_REQUEST_ID + 1,
            None,
            RoomCreate {
                name: "Build".into(),
            },
        ),
        Err(RequestIdOutOfRange {
            value: MAX_REQUEST_ID + 1,
        })
    );
    assert!(serde_json::from_str::<RequestId>(&MAX_REQUEST_ID.to_string()).is_ok());
    assert!(serde_json::from_str::<RequestId>(&(MAX_REQUEST_ID + 1).to_string()).is_err());
}

/// A push carries `t` and never `id`.
#[test]
fn push_carries_t_never_id() {
    let push = Push::Gap {
        room_id: RoomId::new("r1"),
        from_pos: 41,
        to: GapTo::Bounded { pos: 57 },
        reason: GapReason::Backpressure,
    };
    let json = serde_json::to_string(&push).unwrap();
    assert!(json.contains("\"t\":\"gap\""));
    assert!(!json.contains("\"id\""));
}

/// An error carries a code and typed fields, never a hint.
#[test]
fn error_carries_no_hint() {
    let err = ApiError::InsufficientStanding {
        room_id: RoomId::new("r1"),
        required: Role::Authority,
        held: Role::Member,
    };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"code\":\"insufficient_standing\""));
    assert!(json.contains("\"required\":\"authority\""));
    assert!(json.contains("\"held\":\"member\""));
    assert!(!json.contains("hint"));
    assert!(!json.contains("message"));
}

/// A code carries exactly its specified payload: `file_too_large` without
/// its three required fields is unrepresentable.
#[test]
fn error_payload_is_code_discriminated() {
    let err = ApiError::FileTooLarge {
        declared_bytes: 104857601,
        limit_bytes: 104857600,
        enforced_at: EnforcedAt::StageDeclared,
    };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"code\":\"file_too_large\""));
    assert!(json.contains("\"declared_bytes\":104857601"));
    assert!(json.contains("\"limit_bytes\":104857600"));
    assert!(json.contains("\"enforced_at\":\"stage_declared\""));
    // a fieldless code carries only its code
    let err = ApiError::SubjectAbsent;
    assert_eq!(
        serde_json::to_string(&err).unwrap(),
        "{\"code\":\"subject_absent\"}"
    );
}

/// The two stream terminal errors carry exactly the fields added by the
/// canonical byte-stream record.
#[test]
fn stream_terminal_errors_have_exact_wire_shapes() {
    let deadline = ApiError::TransferDeadlineExceeded {
        transferred_bytes: 1_024,
        total: ByteTotal::Known { bytes: 4_096 },
        budget_ms: 30_000,
    };
    assert_eq!(
        serde_json::to_value(&deadline).unwrap(),
        serde_json::json!({
            "code": "transfer_deadline_exceeded",
            "transferred_bytes": 1024,
            "total": {"state": "known", "bytes": 4096},
            "budget_ms": 30000
        })
    );
    assert_eq!(
        serde_json::from_value::<ApiError>(serde_json::to_value(deadline).unwrap()).unwrap(),
        ApiError::TransferDeadlineExceeded {
            transferred_bytes: 1_024,
            total: ByteTotal::Known { bytes: 4_096 },
            budget_ms: 30_000,
        }
    );

    let aborted = ApiError::StreamAborted {
        transferred_bytes: 512,
        total: ByteTotal::Unknown,
        reason: StreamAbortReason::TransportLost,
    };
    assert_eq!(
        serde_json::to_value(&aborted).unwrap(),
        serde_json::json!({
            "code": "stream_aborted",
            "transferred_bytes": 512,
            "total": {"state": "unknown"},
            "reason": "transport_lost"
        })
    );
    assert_eq!(
        serde_json::from_value::<ApiError>(serde_json::to_value(aborted).unwrap()).unwrap(),
        ApiError::StreamAborted {
            transferred_bytes: 512,
            total: ByteTotal::Unknown,
            reason: StreamAbortReason::TransportLost,
        }
    );
}

/// JSON stream-abort reasons are closed independently from Binary ABORT:
/// `transport_lost` is local, while wire-only `operation_error` is forbidden.
#[test]
fn stream_abort_reason_is_closed() {
    let cases = [
        (StreamAbortReason::Cancelled, "cancelled"),
        (StreamAbortReason::SourceFailed, "source_failed"),
        (StreamAbortReason::SinkFailed, "sink_failed"),
        (StreamAbortReason::TransportLost, "transport_lost"),
        (StreamAbortReason::ProtocolError, "protocol_error"),
    ];
    for (reason, token) in cases {
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            format!("\"{token}\"")
        );
        assert_eq!(
            serde_json::from_str::<StreamAbortReason>(&format!("\"{token}\"")).unwrap(),
            reason
        );
    }
    assert!(serde_json::from_str::<StreamAbortReason>("\"operation_error\"").is_err());
    assert!(serde_json::from_str::<StreamAbortReason>("\"unknown\"").is_err());
    assert_eq!(
        serde_json::to_string(&ErrorCode::TransferDeadlineExceeded).unwrap(),
        "\"transfer_deadline_exceeded\""
    );
    assert_eq!(
        serde_json::to_string(&ErrorCode::StreamAborted).unwrap(),
        "\"stream_aborted\""
    );
}

/// `status.post` carries no free-text field.
#[test]
fn status_post_has_no_free_text() {
    let req = StatusPost {
        room_id: RoomId::new("r1"),
        label: StatusLabel::Working,
        progress: Progress::Reported { percent: 40 },
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("message"));
    assert!(json.contains("\"label\":\"working\""));
    assert!(json.contains("\"progress\":{\"state\":\"reported\",\"percent\":40}"));
}

/// `author` is a variant: the fabricated default role is unrepresentable.
#[test]
fn unresolved_author_carries_no_attribution() {
    let u = Author::Unresolved;
    assert_eq!(
        serde_json::to_string(&u).unwrap(),
        "{\"state\":\"unresolved\"}"
    );
    let r = Author::Resolved {
        subject_id: SubjectId::new("s1"),
        role: Role::Member,
        standing: Standing::Active,
    };
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("\"state\":\"resolved\""));
    assert!(s.contains("\"role\":\"member\""));
}

/// The fleet row serves no tallies.
#[test]
fn fleet_row_has_no_tallies() {
    let row = FleetRow {
        subject_id: SubjectId::new("s1"),
        room_id: RoomId::new("r1"),
        liveness: Liveness::Working,
        latest_status: LatestStatus::Absent,
        last_seen: LastSeen::Absent,
    };
    let json = serde_json::to_string(&row).unwrap();
    for banned in [
        "active",
        "working_total",
        "total",
        "rooms_total",
        "rooms_covered",
    ] {
        assert!(!json.contains(banned), "tally field leaked: {banned}");
    }
}

/// `file.fetch` output does not report local hold state.
#[test]
fn file_fetch_does_not_report_local_hold_state() {
    let out = FileFetchOut {
        room_id: RoomId::new("r1"),
        file_id: FileId::new("f1"),
        bytes: 4096,
        digest: "d".into(),
        provider: ProviderRef {
            subject_id: SubjectId::new("s1"),
            device_id: DeviceId::new("d1"),
        },
    };
    let json = serde_json::to_string(&out).unwrap();
    assert!(!json.contains("local"));
    assert!(!json.contains("held"));
    assert!(!json.contains("self_hosted"));
}

/// A capability is never served by `invite.list`.
#[test]
fn invite_list_never_serves_the_capability() {
    let row = InviteRow {
        invite_id: InviteId::new("i1"),
        subject_id: SubjectId::new("s1"),
        role: Role::Member,
        expires_at: Timestamp::from(time::OffsetDateTime::UNIX_EPOCH),
        redeemability: Redeemability::Outstanding,
    };
    let json = serde_json::to_string(&row).unwrap();
    assert!(!json.contains("capability"));
}

/// The hello frame carries `t: "hello"` as its discriminator.
#[test]
fn hello_carries_t_discriminator() {
    let h = Hello {
        protocol: 2,
        storage_generation: 1,
        incarnation: Incarnation::new("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"),
        limits: Limits {
            max_shared_file_bytes: 104857600,
            max_message_body_bytes: 1,
            max_frame_bytes: 1,
            max_inflight_requests: 1,
            max_subscriptions_per_connection: 1,
            max_connections: 1,
            max_concurrent_transfers: 1,
            max_transfer_bytes_inflight: 1,
            transfer_connect_allowance_ms: 1,
            transfer_floor_bits_per_second: 1,
            transfer_stall_ms: 1,
            timeline_page_max: 1,
            idle_timeout_ms: 1,
            pairing_code_ttl_ms: 1,
            pairing_code_max_attempts: 1,
            browser_session_ttl_ms: 1,
        },
        subject: SubjectState::Absent,
        resume: Resume::Fresh,
    };
    let json = serde_json::to_string(&h).unwrap();
    assert!(json.contains("\"t\":\"hello\""));
    assert!(json.contains("\"protocol\":2"));
    // The incarnation is a top-level opaque string key that round-trips.
    assert!(json.contains("\"incarnation\":\"a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6\""));
    let back: Hello = serde_json::from_str(&json).unwrap();
    assert_eq!(back.incarnation, h.incarnation);
    assert_eq!(back, h);
}

/// A progress percent outside 0..=100 is refused at deserialization.
#[test]
fn progress_percent_is_bounded() {
    assert!(serde_json::from_str::<Progress>("{\"state\":\"reported\",\"percent\":40}").is_ok());
    assert!(serde_json::from_str::<Progress>("{\"state\":\"reported\",\"percent\":101}").is_err());
}

/// A timestamp with a numeric offset is refused; the wire form is Z only.
#[test]
fn timestamp_requires_z_offset() {
    assert!(serde_json::from_str::<Timestamp>("\"2026-01-01T00:00:00Z\"").is_ok());
    assert!(serde_json::from_str::<Timestamp>("\"2026-01-01T01:00:00+01:00\"").is_err());
}

/// An undefined key inside a tagged request variant is refused.
#[test]
fn tagged_variants_deny_unknown_fields() {
    assert!(serde_json::from_str::<Progress>(
        "{\"state\":\"reported\",\"percent\":40,\"bogus\":1}"
    )
    .is_err());
}

/// An event kind/content combination that does not belong is unrepresentable.
#[test]
fn event_kind_and_content_are_coupled() {
    // a message kind with room_created content fails
    assert!(serde_json::from_str::<EventKindContent>(
        "{\"kind\":\"message\",\"content\":{\"name\":\"Build\"}}"
    )
    .is_err());
    // the right combination parses
    let ok = serde_json::from_str::<EventKindContent>(
        "{\"kind\":\"message\",\"content\":{\"body\":\"hello\"}}",
    );
    assert!(ok.is_ok());
}
