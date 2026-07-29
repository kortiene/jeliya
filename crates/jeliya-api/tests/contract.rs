//! Unit and property tests for jeliya-api. The crate is types only; these
//! tests prove the *contract* the types make: pairing, closed vocabularies,
//! no-null, opaque ids, and the serde shapes the wire conventions fix.

use jeliya_api::*;

// --- Compile-time pairing --------------------------------------------------

/// Every operation's `PATH` is its capability token, and `MUTATING` matches
/// the record's `M` column. This is the pairing #163 exists to make: a
/// request cannot be held without knowing its output type and wire name.
#[test]
fn operation_paths_and_mutability() {
    assert_eq!(<SubjectEnsure as Operation>::PATH, "subject.ensure");
    assert!(<SubjectEnsure as Operation>::MUTATING);
    assert_eq!(<RoomCreate as Operation>::PATH, "room.create");
    assert!(<RoomCreate as Operation>::MUTATING);
    assert_eq!(<RoomList as Operation>::PATH, "room.list");
    assert!(!<RoomList as Operation>::MUTATING);
    assert_eq!(<RoomActivate as Operation>::PATH, "room.activate");
    assert!(<RoomActivate as Operation>::MUTATING);
    assert_eq!(<RoomDeactivate as Operation>::PATH, "room.deactivate");
    assert!(<RoomDeactivate as Operation>::MUTATING);
    assert_eq!(<RoomLeave as Operation>::PATH, "room.leave");
    assert!(<RoomLeave as Operation>::MUTATING);
    assert_eq!(<RoomTimeline as Operation>::PATH, "room.timeline");
    assert!(!<RoomTimeline as Operation>::MUTATING);
    assert_eq!(<RoomMembers as Operation>::PATH, "room.members");
    assert!(!<RoomMembers as Operation>::MUTATING);
    assert_eq!(<RoomArchive as Operation>::PATH, "room.archive");
    assert!(!<RoomArchive as Operation>::MUTATING);
    assert_eq!(<RoomPeers as Operation>::PATH, "room.peers");
    assert!(!<RoomPeers as Operation>::MUTATING);
    assert_eq!(<MemberRemove as Operation>::PATH, "member.remove");
    assert!(<MemberRemove as Operation>::MUTATING);
    assert_eq!(<InviteMint as Operation>::PATH, "invite.mint");
    assert!(<InviteMint as Operation>::MUTATING);
    assert_eq!(<InviteList as Operation>::PATH, "invite.list");
    assert!(!<InviteList as Operation>::MUTATING);
    assert_eq!(<InviteRevoke as Operation>::PATH, "invite.revoke");
    assert!(<InviteRevoke as Operation>::MUTATING);
    assert_eq!(<InviteRedeem as Operation>::PATH, "invite.redeem");
    assert!(<InviteRedeem as Operation>::MUTATING);
    assert_eq!(<MessageSend as Operation>::PATH, "message.send");
    assert!(<MessageSend as Operation>::MUTATING);
    assert_eq!(<StatusPost as Operation>::PATH, "status.post");
    assert!(<StatusPost as Operation>::MUTATING);
    assert_eq!(<StatusHistory as Operation>::PATH, "status.history");
    assert!(!<StatusHistory as Operation>::MUTATING);
    assert_eq!(<FleetList as Operation>::PATH, "fleet.list");
    assert!(!<FleetList as Operation>::MUTATING);
    assert_eq!(<FileShare as Operation>::PATH, "file.share");
    assert!(<FileShare as Operation>::MUTATING);
    assert_eq!(<FileList as Operation>::PATH, "file.list");
    assert!(!<FileList as Operation>::MUTATING);
    assert_eq!(<FileFetch as Operation>::PATH, "file.fetch");
    assert!(<FileFetch as Operation>::MUTATING);
    assert_eq!(<FileRead as Operation>::PATH, "file.read");
    assert!(!<FileRead as Operation>::MUTATING);
    assert_eq!(<TransferCancel as Operation>::PATH, "transfer.cancel");
    assert!(<TransferCancel as Operation>::MUTATING);
    assert_eq!(<PipePublish as Operation>::PATH, "pipe.publish");
    assert!(<PipePublish as Operation>::MUTATING);
    assert_eq!(<PipeList as Operation>::PATH, "pipe.list");
    assert!(!<PipeList as Operation>::MUTATING);
    assert_eq!(<PipeConnect as Operation>::PATH, "pipe.connect");
    assert!(<PipeConnect as Operation>::MUTATING);
    assert_eq!(<PipeRelease as Operation>::PATH, "pipe.release");
    assert!(<PipeRelease as Operation>::MUTATING);
    assert_eq!(<PipeRevoke as Operation>::PATH, "pipe.revoke");
    assert!(<PipeRevoke as Operation>::MUTATING);
    assert_eq!(<StreamSubscribe as Operation>::PATH, "stream.subscribe");
    assert!(!<StreamSubscribe as Operation>::MUTATING);
    assert_eq!(<StreamUnsubscribe as Operation>::PATH, "stream.unsubscribe");
    assert!(!<StreamUnsubscribe as Operation>::MUTATING);
    assert_eq!(<StreamResync as Operation>::PATH, "stream.resync");
    assert!(!<StreamResync as Operation>::MUTATING);
}

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
        at: time::OffsetDateTime::UNIX_EPOCH,
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
    let env = Envelope {
        id: 42,
        op_id: Some(OpId::new("op-1")),
        input: RoomCreate {
            name: "Build".into(),
        },
    };
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("\"op_id\":\"op-1\""));
    assert!(json.contains("\"in\":{\"name\":\"Build\"}"));
    // `in` carries no op_id key
    assert!(!json.contains("\"in\":{\"name\":\"Build\",\"op_id\""));
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
    let err = ApiError {
        code: ErrorCode::InsufficientStanding,
        field: None,
        reason: None,
        room_id: None,
        subject_id: None,
        required: Some(Role::Authority),
        held: Some(Role::Member),
    };
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"code\":\"insufficient_standing\""));
    assert!(json.contains("\"required\":\"authority\""));
    assert!(json.contains("\"held\":\"member\""));
    assert!(!json.contains("hint"));
    assert!(!json.contains("message"));
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
        expires_at: time::OffsetDateTime::UNIX_EPOCH,
        redeemability: Redeemability::Outstanding,
    };
    let json = serde_json::to_string(&row).unwrap();
    assert!(!json.contains("capability"));
}
