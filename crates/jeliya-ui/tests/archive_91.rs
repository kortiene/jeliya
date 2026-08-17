//! Focused integration tests for #91 (departed-room read-only archive).
//!
//! These tests sit outside inline `#[cfg(test)]` because they exercise facts
//! that span multiple source files (l10n catalog completeness, protocol-limit
//! constants, source-code invariants) or that would pull integration-level
//! dependencies unavailable to the inline test budget.
//!
//! Run with:
//!   cargo test -p jeliya-ui --features ui --test archive_91

use futures::executor::block_on;
use jeliya_api::{
    ApiError, Author, CapabilityToken, Cursor, Direction, Event, EventId, EventKindContent, Page,
    Role, RoomActivate, RoomArchive, RoomArchiveOut, RoomId, Standing, SubjectId, Timestamp,
    Truncated,
};
use jeliya_client::mock::{MockScript, Program};
use jeliya_client::{CallError, Dedup, LocalError};
use jeliya_ui::l10n::{catalog_for, Catalog, En, Fr, Locale};
use jeliya_ui::room::{capability, ArchiveView, DepartureFact, ARCHIVE_PAGE_LIMIT};

// ---- Protocol bounds -------------------------------------------------------

/// `ARCHIVE_PAGE_LIMIT` must be within the protocol-defined `1..=1024` paging
/// bound for `room.archive` (#91 §6 / R3). A value of 0 would cause the daemon
/// to refuse the call; a value > 1024 would violate the protocol ceiling and
/// trigger a daemon refusal with `invalid_argument`.
#[test]
fn archive_page_limit_is_within_protocol_paging_bounds() {
    assert!(
        ARCHIVE_PAGE_LIMIT >= 1,
        "ARCHIVE_PAGE_LIMIT={ARCHIVE_PAGE_LIMIT} is below the protocol minimum of 1"
    );
    assert!(
        ARCHIVE_PAGE_LIMIT <= 1024,
        "ARCHIVE_PAGE_LIMIT={ARCHIVE_PAGE_LIMIT} exceeds the protocol maximum of 1024 \
         (timeline_page_max per docs/protocol-v2.md)"
    );
}

// ---- Localization completeness and honesty ---------------------------------

/// Every `archive_*` method in the catalog returns a non-empty string for
/// both EN and FR. An empty string silently shows a blank to the user; the
/// compile-enforced key parity catches a *missing* key but not an empty value
/// (that is the node gate's job — this is the in-tree backup).
#[test]
fn archive_l10n_strings_are_non_empty_in_both_locales() {
    for locale in [Locale::En, Locale::Fr] {
        let s = catalog_for(locale);
        let tag = s.locale_tag();
        for (name, value) in [
            ("archive_banner_left_title", s.archive_banner_left_title()),
            (
                "archive_banner_removed_title",
                s.archive_banner_removed_title(),
            ),
            ("archive_banner_body", s.archive_banner_body()),
            ("archive_banner_rejoin", s.archive_banner_rejoin()),
            ("archive_timeline_label", s.archive_timeline_label()),
            ("archive_roster_heading", s.archive_roster_heading()),
            ("archive_load_more", s.archive_load_more()),
            ("archive_still_active", s.archive_still_active()),
            ("archive_empty", s.archive_empty()),
            ("archive_loading", s.archive_loading()),
        ] {
            assert!(!value.is_empty(), "[{tag}] {name} is empty");
        }
    }
}

/// Key archive copy must differ between EN and FR, confirming the FR catalog
/// is actually translated and not just left in English.
#[test]
fn archive_fr_strings_differ_from_en_for_key_copy() {
    for (name, en_val, fr_val) in [
        (
            "archive_banner_left_title",
            En.archive_banner_left_title(),
            Fr.archive_banner_left_title(),
        ),
        (
            "archive_banner_removed_title",
            En.archive_banner_removed_title(),
            Fr.archive_banner_removed_title(),
        ),
        (
            "archive_banner_body",
            En.archive_banner_body(),
            Fr.archive_banner_body(),
        ),
        (
            "archive_banner_rejoin",
            En.archive_banner_rejoin(),
            Fr.archive_banner_rejoin(),
        ),
        (
            "archive_roster_heading",
            En.archive_roster_heading(),
            Fr.archive_roster_heading(),
        ),
        (
            "archive_still_active",
            En.archive_still_active(),
            Fr.archive_still_active(),
        ),
    ] {
        assert_ne!(
            en_val, fr_val,
            "{name}: FR value is identical to EN — FR is not translated"
        );
    }
}

/// Archive copy must never imply that historical state is current or that
/// messages were seen or delivered. Checking for the specific words the
/// product-behavior contract prohibits in presence/status contexts (#91 §8 /
/// docs/product-behavior-contract.md §unread/presence honesty).
#[test]
fn archive_strings_avoid_honesty_violations() {
    let forbidden_words = ["seen", "delivered", "online", "current"];
    for locale in [Locale::En, Locale::Fr] {
        let s = catalog_for(locale);
        let tag = s.locale_tag();
        let strings: &[(&str, &str)] = &[
            ("archive_banner_left_title", s.archive_banner_left_title()),
            (
                "archive_banner_removed_title",
                s.archive_banner_removed_title(),
            ),
            ("archive_banner_body", s.archive_banner_body()),
            ("archive_banner_rejoin", s.archive_banner_rejoin()),
            ("archive_timeline_label", s.archive_timeline_label()),
            ("archive_roster_heading", s.archive_roster_heading()),
            ("archive_load_more", s.archive_load_more()),
            ("archive_still_active", s.archive_still_active()),
            ("archive_empty", s.archive_empty()),
            ("archive_loading", s.archive_loading()),
        ];
        for (name, value) in strings {
            let lower = value.to_lowercase();
            for word in forbidden_words {
                assert!(
                    !lower.contains(word),
                    "[{tag}] {name} contains forbidden word {word:?} — \
                     archive copy must not imply presence, delivery, or currency"
                );
            }
        }
    }
}

// ---- Capability set exhaustiveness ----------------------------------------

/// `FORBIDDEN_IN_ARCHIVE` must contain exactly the 26 room-scoped live
/// tokens and no account-scoped ops. The count is a regression guard:
/// if a new room-scoped op is added to the protocol but not to the forbidden
/// set, this test fails and forces a deliberate decision.
#[test]
fn forbidden_in_archive_has_the_expected_count() {
    assert_eq!(
        capability::FORBIDDEN_IN_ARCHIVE.len(),
        26,
        "FORBIDDEN_IN_ARCHIVE has the wrong count — a new room-scoped op may need adding, \
         or an entry was accidentally removed"
    );
}

/// The account-scoped ops (`subject.ensure`, `daemon.stop`, `room.create`,
/// `room.list`, `fleet.list`, `invite.redeem`) must NOT appear in
/// `FORBIDDEN_IN_ARCHIVE`. They are not room-scoped live actions; listing
/// them as forbidden would wrongly suggest the archive pane could otherwise
/// offer them (#91 capability.rs comment).
#[test]
fn account_scoped_tokens_are_not_in_the_forbidden_set() {
    let account_scoped = [
        CapabilityToken::SubjectEnsure,
        CapabilityToken::DaemonStop,
        CapabilityToken::RoomCreate,
        CapabilityToken::RoomList,
        CapabilityToken::FleetList,
        CapabilityToken::InviteRedeem,
    ];
    for token in account_scoped {
        assert!(
            !capability::FORBIDDEN_IN_ARCHIVE.contains(&token),
            "{token:?} is account-scoped and must not appear in FORBIDDEN_IN_ARCHIVE"
        );
    }
}

/// `room.archive` itself must not appear in `FORBIDDEN_IN_ARCHIVE` — it is
/// the one op the archive surface is allowed to dispatch (#91 D2).
#[test]
fn room_archive_token_is_not_forbidden() {
    assert!(
        !capability::FORBIDDEN_IN_ARCHIVE.contains(&CapabilityToken::RoomArchive),
        "RoomArchive is the archive dispatch op and must not be in FORBIDDEN_IN_ARCHIVE"
    );
}

/// Every element of `FORBIDDEN_IN_ARCHIVE` is distinct. A duplicate would
/// indicate a copy-paste error in the constant definition.
#[test]
fn forbidden_set_has_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for token in capability::FORBIDDEN_IN_ARCHIVE {
        assert!(
            seen.insert(token),
            "{token:?} appears more than once in FORBIDDEN_IN_ARCHIVE"
        );
    }
}

// ---- Source scan: archive pane issues no live ops --------------------------

/// The archive pane (`src/components/room_archive.rs`) must contain no call
/// to the live room operations that the archive contract forbids: `room.activate`,
/// `stream.subscribe`, `room.members`, `room.timeline`, or `room.peers`.
/// These are absent by construction (#91 D2/D7) but a source scan makes the
/// absence an explicit CI gate, preventing a future refactor from silently
/// introducing a live call.
///
/// This mirrors the #178 `no_legacy_key_reader_exists_in_shell_source` test.
#[test]
fn archive_pane_source_has_no_live_op_call_sites() {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/components/room_archive.rs"
    ));
    let source =
        std::fs::read_to_string(path).expect("src/components/room_archive.rs must be readable");

    let forbidden_patterns = [
        "room_activate",
        "room.activate",
        "stream_subscribe",
        "stream.subscribe",
        "room_members",
        "room.members",
        "room_timeline",
        "room.timeline",
        "room_peers",
        "room.peers",
    ];

    let mut offenders: Vec<String> = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        // Skip pure comment lines — they may mention these ops by name for
        // documentation purposes without constituting a call site.
        if line.trim_start().starts_with("//") {
            continue;
        }
        for pattern in forbidden_patterns {
            if line.contains(pattern) {
                offenders.push(format!(
                    "room_archive.rs:{} — contains forbidden live-op pattern {pattern:?}",
                    line_num + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "live-op call sites found in archive pane (AC-2 / #91 D7 — the archive must \
         dispatch only room.archive):\n{}",
        offenders.join("\n")
    );
}

// ---- Seam fixtures ---------------------------------------------------------

fn test_room_id() -> RoomId {
    RoomId::new("blake3:testroom")
}

fn make_archive_out(
    standing: Standing,
    events: Vec<Event>,
    truncated: Truncated,
) -> RoomArchiveOut {
    RoomArchiveOut {
        room_id: test_room_id(),
        standing,
        events,
        truncated,
    }
}

fn genesis_event() -> Event {
    Event {
        pos: 0,
        event_id: EventId::new("e-0"),
        at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
        author: Author::Resolved {
            subject_id: SubjectId::new("blake3:creator"),
            role: Role::Authority,
            standing: Standing::Active,
        },
        kind: EventKindContent::RoomCreated {
            name: "Archive Room".into(),
        },
    }
}

fn member_left_event(pos: u64, subject: &str) -> Event {
    Event {
        pos,
        event_id: EventId::new(format!("e-{pos}")),
        at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
        author: Author::Unresolved,
        kind: EventKindContent::MemberLeft {
            subject_id: SubjectId::new(subject),
        },
    }
}

fn member_removed_event(pos: u64, subject: &str, by: &str) -> Event {
    Event {
        pos,
        event_id: EventId::new(format!("e-{pos}")),
        at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
        author: Author::Unresolved,
        kind: EventKindContent::MemberRemoved {
            subject_id: SubjectId::new(subject),
            by: SubjectId::new(by),
        },
    }
}

fn archive_page_input(cursor: Cursor) -> RoomArchive {
    RoomArchive {
        room_id: test_room_id(),
        page: Page {
            cursor,
            direction: Direction::Forward,
            limit: ARCHIVE_PAGE_LIMIT,
        },
    }
}

// ---- Client seam: room_archive wire contract ------------------------------

/// `ClientHandle::room_archive` dispatches op `"room.archive"` with the
/// paired `RoomArchiveOut` output (#91 §6). The mock's strict op-key matching
/// is the assertion: if the wrapper dispatched under any other wire name the
/// scripted entry would not be found and the call would return
/// `CallError::Local(LocalError::Backend)` (an unscripted op is a
/// test-setup error).
#[test]
fn room_archive_seam_dispatches_correct_wire_op_and_returns_paired_output() {
    let out = make_archive_out(Standing::Left, vec![], Truncated::Complete);
    let (handle, controller) = MockScript::new()
        .on("room.archive", Program::reply_ok::<RoomArchive>(&out))
        .build();

    block_on(async {
        let fut = handle.room_archive(archive_page_input(Cursor::Start), Dedup::None);
        controller.deliver_next();
        let result = fut.await.expect("scripted reply_ok must succeed");
        assert_eq!(result.standing, Standing::Left);
        assert_eq!(result.room_id, test_room_id());
        assert!(result.events.is_empty());
    });
}

/// A `room_still_active` wire error propagates as `CallError::Wire` (#91 D8).
/// The archive pane pattern-matches on this variant to show the "active again"
/// notice; the error must reach the caller unmodified so that match is
/// possible.
#[test]
fn room_still_active_wire_error_propagates_as_call_error_wire() {
    let (handle, controller) = MockScript::new()
        .on(
            "room.archive",
            Program::reply_err(ApiError::RoomStillActive {
                room_id: test_room_id(),
            }),
        )
        .build();

    block_on(async {
        let fut = handle.room_archive(archive_page_input(Cursor::Start), Dedup::None);
        controller.deliver_next();
        let err = fut.await.expect_err("room_still_active must be an error");
        assert!(
            matches!(err, CallError::Wire(ApiError::RoomStillActive { .. })),
            "expected Wire(RoomStillActive), got: {err:?}"
        );
    });
}

/// An unscripted op returns `CallError::Local(LocalError::Backend)` — the
/// mock's deterministic "test-setup error" for any op not in the script.
/// This proves the enforcement mechanism the `archive_pane_source_has_no_live_op_call_sites`
/// source scan relies on: a live-op call added by a future refactor would
/// not silently pass but would fail immediately with a distinct error (#91 D2/D7).
#[test]
fn unscripted_live_op_returns_local_backend_error() {
    // Script nothing — every op is unscripted.
    let (handle, _controller) = MockScript::new().build();

    block_on(async {
        let err = handle
            .call::<RoomActivate>(
                RoomActivate {
                    room_id: test_room_id(),
                },
                Dedup::None,
            )
            .await
            .expect_err("unscripted op must error");
        assert!(
            matches!(err, CallError::Local(LocalError::Backend)),
            "unscripted op must return Local(Backend), got: {err:?}"
        );
    });
}

// ---- ArchiveView fold through the seam ------------------------------------

/// End-to-end Left fold: `room.archive` returns `Standing::Left` with a
/// genesis event and the caller's own `MemberLeft`. The `ArchiveView::fold`
/// produces a view with the correct room name, roster, departure fact, and an
/// empty `more` cursor, all derived from the seam output without calling any
/// live op (#91 D1/D3/D4/D7).
#[test]
fn archive_fold_left_room_end_to_end() {
    let me = SubjectId::new("blake3:me");
    let events = vec![genesis_event(), member_left_event(1, "blake3:me")];
    let out = make_archive_out(Standing::Left, events, Truncated::Complete);

    let (handle, controller) = MockScript::new()
        .on("room.archive", Program::reply_ok::<RoomArchive>(&out))
        .build();

    block_on(async {
        let fut = handle.room_archive(archive_page_input(Cursor::Start), Dedup::None);
        controller.deliver_next();
        let archive_out = fut.await.expect("scripted reply_ok succeeds");
        let view = ArchiveView::fold(None, archive_out, Some(&me));

        assert_eq!(
            view.room_name.as_deref(),
            Some("Archive Room"),
            "genesis event must seed the room name"
        );
        assert_eq!(view.my_standing, Standing::Left);
        assert_eq!(
            view.departure,
            DepartureFact::Left,
            "voluntary departure identified from signed event"
        );
        assert_eq!(
            view.more, None,
            "Truncated::Complete must clear the continuation cursor"
        );
        // Roster: creator (active) from genesis; caller's standing is left from their event.
        assert_eq!(
            view.roster.len(),
            2,
            "creator and caller both appear in the historical roster"
        );
    });
}

/// End-to-end Removed fold: `room.archive` returns `Standing::Removed` and
/// the caller's own `MemberRemoved{by: creator}` event is loaded. The fold
/// names the remover from the signed event (#91 D4/R5).
#[test]
fn archive_fold_removed_room_names_the_remover_end_to_end() {
    let me = SubjectId::new("blake3:me");
    let events = vec![
        genesis_event(),
        member_removed_event(1, "blake3:me", "blake3:creator"),
    ];
    let out = make_archive_out(Standing::Removed, events, Truncated::Complete);

    let (handle, controller) = MockScript::new()
        .on("room.archive", Program::reply_ok::<RoomArchive>(&out))
        .build();

    block_on(async {
        let fut = handle.room_archive(archive_page_input(Cursor::Start), Dedup::None);
        controller.deliver_next();
        let archive_out = fut.await.expect("scripted reply_ok succeeds");
        let view = ArchiveView::fold(None, archive_out, Some(&me));

        assert_eq!(
            view.departure,
            DepartureFact::Removed {
                by: Some(SubjectId::new("blake3:creator"))
            },
            "the remover must be named from the caller's own signed MemberRemoved event"
        );
    });
}

/// End-to-end two-page fold: the first page has `Truncated::More` and the
/// second has `Truncated::Complete`. After both reads the accumulated
/// `ArchiveView` holds all events (deduped by position), `more` is `None`,
/// and the roster agrees with a single-pass fold over all events (#91 D3/§6
/// paging contract / R1).
#[test]
fn archive_fold_two_pages_via_seam_accumulates_and_clears_more() {
    let continuation = Cursor::At { pos: 2 };

    // Page 1: genesis + member_joined, continues.
    let page1 = make_archive_out(
        Standing::Left,
        vec![
            genesis_event(),
            Event {
                pos: 1,
                event_id: EventId::new("e-1"),
                at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
                author: Author::Unresolved,
                kind: EventKindContent::MemberJoined {
                    subject_id: SubjectId::new("blake3:alice"),
                    role: Role::Member,
                },
            },
        ],
        Truncated::More {
            cursor: continuation.clone(),
        },
    );

    // Page 2: the caller's own MemberLeft, completes.
    let page2 = make_archive_out(
        Standing::Left,
        vec![member_left_event(2, "blake3:me")],
        Truncated::Complete,
    );

    let me = SubjectId::new("blake3:me");
    let (handle, controller) = MockScript::new()
        .on("room.archive", Program::reply_ok::<RoomArchive>(&page1))
        .on("room.archive", Program::reply_ok::<RoomArchive>(&page2))
        .build();

    block_on(async {
        // First page.
        let fut1 = handle.room_archive(archive_page_input(Cursor::Start), Dedup::None);
        controller.deliver_next();
        let out1 = fut1.await.expect("first page succeeds");
        let view1 = ArchiveView::fold(None, out1, Some(&me));
        assert_eq!(
            view1.more,
            Some(continuation.clone()),
            "first page must leave the continuation cursor"
        );
        assert_eq!(view1.events.len(), 2);

        // Second page: continuation cursor handed back to the seam.
        let fut2 = handle.room_archive(archive_page_input(continuation), Dedup::None);
        controller.deliver_next();
        let out2 = fut2.await.expect("second page succeeds");
        let view2 = ArchiveView::fold(Some(view1), out2, Some(&me));

        assert_eq!(
            view2.events.len(),
            3,
            "both pages contribute distinct events"
        );
        assert_eq!(
            view2.more, None,
            "Truncated::Complete on the second page must clear more"
        );
        // Events must still be in position order after merging.
        assert!(
            view2.events.windows(2).all(|w| w[0].pos < w[1].pos),
            "merged events must be sorted by position"
        );
        // Roster: creator, alice (from page 1), caller left (from page 2).
        assert_eq!(
            view2.roster.len(),
            3,
            "all membership events across both pages contribute to the roster"
        );
        assert_eq!(view2.departure, DepartureFact::Left);
    });
}
