//! The departed-room read-only archive surface (#91): [`RoomArchivePane`], the
//! composition a `Left`/`Removed` room reaches instead of the live `RoomShell`.
//!
//! This is the deliberate widening of the retained Room Workbench floor ("a
//! departed room states the signed fact and does not open"): #91 opens it
//! **truthfully** — the locally held signed timeline, a historical roster
//! reconstructed from that timeline, the signed left/removed fact stated plainly
//! and permanently, and **every** live action suppressed by **absent
//! capability** — while starting **no** networking.
//!
//! The pane dispatches **only** the typed `room.archive` read (pure,
//! network-free, mutation-free — `MUTATING = false`, no `op_id`) and never
//! `room.activate`, `stream.subscribe`, `room.timeline`, `room.members`,
//! `file.*`, or `pipe.*` (#91 D1/D7). It reuses no live nav strip and offers no
//! composer/Invite/Leave/file/Pipe affordance — those are absent by
//! construction, not present-and-disabled (#91 D2). The pure projection, roster
//! fold, and capability set live in [`crate::room`]; this is the thin RSX view.

use dioxus::prelude::*;
use jeliya_api::{
    ApiError, Cursor, Direction, Event, EventKindContent, Page, RoomArchive, RoomId, Standing,
    SubjectId,
};
use jeliya_client::{ClientHandle, Dedup};
use jeliya_platform::navigation::Route;

use crate::components::Heading;
use crate::l10n::{use_strings, wire};
use crate::room::{ArchiveView, HistoricalMember, ARCHIVE_PAGE_LIMIT};
use crate::shell::router::NavIntent;

/// The archive read's progress. `Loading` never fabricates content; a transport
/// failure that cannot complete the read parks here (offline reconciler-read
/// scripting is deferred to the tests phase, exactly as #179 deferred it).
#[derive(Clone, PartialEq)]
enum ArchiveState {
    /// The first page has not yet landed.
    Loading,
    /// The accumulated archive view.
    Loaded(ArchiveView),
    /// `room.archive` reported the room is active again (the room_still_active
    /// race, #91 D8) — an honest notice, never a silent redirect into a live
    /// surface.
    StillActive,
    /// The daemon answered a typed refusal other than room_still_active.
    Failed,
}

/// A short, human-scannable disambiguator for a subject id: the last segment
/// after any `:` prefix, truncated. Never presented as the full identity — a
/// compact historical reference in a timeline/roster row.
fn short_subject(subject_id: &SubjectId) -> String {
    let raw = subject_id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}

/// The read-only archive pane for a departed (`Left`/`Removed`) room. Renders
/// the permanent departure banner, the signed timeline, and the historical
/// roster; it holds no live-action affordance and issues only `room.archive`.
#[component]
pub fn RoomArchivePane(
    room_id: RoomId,
    /// The caller's own standing from the `room.list` row (`Left`/`Removed`).
    /// Drives the banner's left-vs-removed copy immediately, before the read
    /// lands; cross-checked against the authoritative `RoomArchiveOut.standing`.
    my_standing: Standing,
    /// The caller's subject when the seam can name it (from `Hello`), used to
    /// identify the caller's own signed departure event. `None` is the honest
    /// default while the seam does not surface it (#178 D2/R1).
    #[props(default)]
    me: Option<SubjectId>,
    navigate: Callback<NavIntent>,
    handle: ClientHandle,
) -> Element {
    let strings = use_strings();
    let mut load = use_signal(|| ArchiveState::Loading);

    // The one mount read: a single forward page from the start, folded into the
    // view. `room.archive` is normatively network-free and mutation-free, and
    // this path issues NO `stream.subscribe`/`room.activate`/other op — the
    // whole-surface "zero network, zero mutation" guarantee (#91 D7/§10).
    //
    // The pane is only reached once `room.list` has answered (its row is what
    // selected this composition), so the client has already been `Ready`; a
    // transient transport failure parks on `Loading` rather than fabricating
    // content (retry-on-recovery is deferred with the live transport, #171).
    {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let me = me.clone();
        use_future(move || {
            let handle = handle.clone();
            let room_id = room_id.clone();
            let me = me.clone();
            async move {
                let input = RoomArchive {
                    room_id,
                    page: Page {
                        cursor: Cursor::Start,
                        direction: Direction::Forward,
                        limit: ARCHIVE_PAGE_LIMIT,
                    },
                };
                match handle.room_archive(input, Dedup::None).await {
                    Ok(out) => {
                        load.set(ArchiveState::Loaded(ArchiveView::fold(
                            None,
                            out,
                            me.as_ref(),
                        )));
                    }
                    Err(error) => match error.as_wire() {
                        // The room was re-activated between the room.list read
                        // and this open (#91 D8).
                        Some(ApiError::RoomStillActive { .. }) => {
                            load.set(ArchiveState::StillActive);
                        }
                        // Any other typed daemon refusal is an honest failure.
                        Some(_) => load.set(ArchiveState::Failed),
                        // A transport failure (disconnect/timeout/local): park on
                        // Loading; never fabricate an empty archive.
                        None => {}
                    },
                }
            }
        });
    }

    // "Show earlier activity" loads the next forward page and folds it into the
    // accumulated view. On failure the existing view is kept (the control
    // remains for a retry); the read stays a pure, network-free `room.archive`.
    let more_handle = handle.clone();
    let more_me = me.clone();
    let on_more = move |_| {
        let ArchiveState::Loaded(current) = load.peek().clone() else {
            return;
        };
        let Some(cursor) = current.more.clone() else {
            return;
        };
        let handle = more_handle.clone();
        let me = more_me.clone();
        let room_id = current.room_id.clone();
        spawn(async move {
            let input = RoomArchive {
                room_id,
                page: Page {
                    cursor,
                    direction: Direction::Forward,
                    limit: ARCHIVE_PAGE_LIMIT,
                },
            };
            if let Ok(out) = handle.room_archive(input, Dedup::None).await {
                load.set(ArchiveState::Loaded(ArchiveView::fold(
                    Some(current),
                    out,
                    me.as_ref(),
                )));
            }
        });
    };

    let timeline_label = strings.archive_timeline_label();
    let load_more_label = strings.archive_load_more();
    let empty_label = strings.archive_empty();
    let loading_label = strings.archive_loading();

    rsx! {
        // A single read-only view (no five-tab live destination strip): the
        // archive is a document, not the live surface (#91 D2/§7).
        section { class: "room-archive", id: "room-archive",
            // The permanent departure fact, first and always — its standing is
            // known from the room row before the read lands, so it never flashes.
            DepartureBanner { standing: my_standing }
            match load() {
                ArchiveState::Loading => rsx! {
                    div { class: "archive-loading muted", id: "archive-loading", "{loading_label}" }
                },
                ArchiveState::Loaded(view) => rsx! {
                    section { class: "archive-timeline-region", id: "archive-timeline",
                        Heading { level: 2, class: "archive-heading".to_string(), "{timeline_label}" }
                        if view.events.is_empty() {
                            div { class: "archive-empty muted", id: "archive-empty", "{empty_label}" }
                        } else {
                            ul { class: "archive-timeline-list",
                                for event in view.events.iter() {
                                    ArchiveEventRow { key: "{event.pos}", event: event.clone() }
                                }
                            }
                            if view.more.is_some() {
                                button {
                                    class: "btn btn-sm",
                                    id: "archive-load-more",
                                    onclick: on_more,
                                    "{load_more_label}"
                                }
                            }
                        }
                    }
                    HistoricalRoster { roster: view.roster.clone() }
                },
                ArchiveState::StillActive => rsx! {
                    ArchiveNotice { message: strings.archive_still_active().to_string(), navigate }
                },
                ArchiveState::Failed => rsx! {
                    ArchiveNotice { message: strings.err_unknown_message().to_string(), navigate }
                },
            }
        }
    }
}

/// The permanent, non-dismissable departure fact (#91 D5): left-vs-removed copy,
/// the "local read-only archive" statement, and the explanation that rejoining
/// requires a fresh invite. There is deliberately **no** rejoin affordance — no
/// self-service rejoin op exists, so offering one would be a control that fails
/// (#91 §10). It is steady-state reading content with a heading, not an alert.
#[component]
fn DepartureBanner(standing: Standing) -> Element {
    let strings = use_strings();
    // Left vs removed from the caller's own ended standing. `Active` never
    // reaches this pane (the room_still_active race is handled before it), so it
    // defaults to the "left" copy defensively rather than asserting removal.
    let title = match standing {
        Standing::Removed => strings.archive_banner_removed_title(),
        Standing::Left | Standing::Active => strings.archive_banner_left_title(),
    };
    let body = strings.archive_banner_body();
    let rejoin = strings.archive_banner_rejoin();
    rsx! {
        div { class: "archive-banner", id: "archive-banner",
            Heading { level: 2, class: "archive-banner-title".to_string(), "{title}" }
            p { class: "archive-banner-body", "{body}" }
            p { class: "archive-banner-rejoin", "{rejoin}" }
        }
    }
}

/// One row of the archived, read-only timeline. A stand-in for #179's
/// `TimelineRow` (not yet in this branch, #91 R4): it renders each signed event
/// as a localized activity label plus its data (message body, subject
/// disambiguator, or file name). No composer, no per-row live action.
#[component]
fn ArchiveEventRow(event: Event) -> Element {
    let strings = use_strings();
    // The activity label (catalog copy) and the event's own data detail. A
    // message renders its body with no kind label; every other signed kind
    // renders a localized label and, where the kind carries one, a data detail.
    let (label, detail): (Option<&'static str>, Option<String>) = match &event.kind {
        EventKindContent::RoomCreated { name } => {
            (Some(strings.event_room_created()), Some(name.clone()))
        }
        EventKindContent::Message { body } => (None, Some(body.clone())),
        EventKindContent::AgentStatus { .. } => (Some(strings.event_agent_status()), None),
        EventKindContent::MemberJoined { subject_id, .. } => (
            Some(strings.event_member_joined()),
            Some(short_subject(subject_id)),
        ),
        EventKindContent::MemberLeft { subject_id } => (
            Some(strings.event_member_left()),
            Some(short_subject(subject_id)),
        ),
        EventKindContent::MemberRemoved { subject_id, .. } => (
            Some(strings.event_member_removed()),
            Some(short_subject(subject_id)),
        ),
        EventKindContent::InviteRevoked { .. } => (Some(strings.event_invite_revoked()), None),
        EventKindContent::FileShared { name, .. } => {
            (Some(strings.event_file_shared()), Some(name.clone()))
        }
        EventKindContent::PipePublished { .. } => (Some(strings.event_pipe_published()), None),
        EventKindContent::PipeRevoked { .. } => (Some(strings.event_pipe_revoked()), None),
    };
    rsx! {
        li { class: "archive-event", "data-pos": "{event.pos}",
            if let Some(label) = label {
                span { class: "archive-event-kind", "{label}" }
            }
            if let Some(detail) = detail {
                span { class: "archive-event-detail", "{detail}" }
            }
        }
    }
}

/// The historical roster (#91 D3): membership reconstructed from the signed
/// timeline, labeled **historical** ("Members when you left"), never "current".
/// Each row shows the subject disambiguator, role, and ended-or-departure
/// standing — and deliberately **no** presence dot, **no** reachability, and
/// **no** live/agent affordance (a departed archive has no basis to assert live
/// facts).
#[component]
fn HistoricalRoster(roster: Vec<HistoricalMember>) -> Element {
    let strings = use_strings();
    let heading = strings.archive_roster_heading();
    rsx! {
        section { class: "archive-roster", id: "archive-roster",
            Heading { level: 2, class: "archive-heading".to_string(), "{heading}" }
            ul { class: "archive-roster-list",
                for member in roster.iter() {
                    HistoricalRosterRow { key: "{member.subject_id}", member: member.clone() }
                }
            }
        }
    }
}

/// One historical-roster row: subject disambiguator, role, and ended standing.
#[component]
fn HistoricalRosterRow(member: HistoricalMember) -> Element {
    let strings = use_strings();
    let subject = short_subject(&member.subject_id);
    let role = wire::role_word(strings, member.role);
    let standing = wire::standing_word(strings, member.standing);
    rsx! {
        li { class: "archive-roster-item", "data-subject": "{member.subject_id}",
            span { class: "roster-subject mono", "{subject}" }
            span { class: "roster-role", "{role}" }
            span { class: "roster-standing", "{standing}" }
        }
    }
}

/// A recoverable archive notice (the room_still_active race #91 D8, or a typed
/// read failure): the plain fact plus a way back to Rooms — never a blank panel
/// and never a silent redirect into a live surface (contract rule 6 / #91 §10).
#[component]
fn ArchiveNotice(message: String, navigate: Callback<NavIntent>) -> Element {
    let strings = use_strings();
    let rooms = strings.dest_rooms();
    rsx! {
        section { class: "archive-notice", id: "archive-notice",
            p { class: "muted", "{message}" }
            button {
                class: "btn btn-sm",
                id: "archive-notice-rooms",
                onclick: move |_| navigate.call(NavIntent::push(Route::Rooms)),
                "{rooms}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_subject_takes_the_tail_after_a_colon() {
        assert_eq!(
            short_subject(&SubjectId::new("blake3:abcdefghmore")),
            "abcdefgh"
        );
        assert_eq!(short_subject(&SubjectId::new("s-1")), "s-1");
    }
}
