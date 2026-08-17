//! The People (room) destination (§7.1): the signed roster, presence as a
//! separate fact, the invitations list, the issue-invitation form, and the
//! capability-gated member/invite/leave actions.
//!
//! Membership (`room.members`) and presence (`room.peers`, live rooms only) are
//! rendered as **two facts**, never blended (D1); invitations (`invite.list`)
//! are a **separate fact** from roster standing (D2); every mutation is
//! single-submit and routes destructive actions through [`ConfirmDialog`]
//! (D4/D5); action affordances are typed-capability-gated (D6).

use dioxus::prelude::*;
use jeliya_api::{
    CapabilityToken, InviteId, InviteList, InviteMint, InviteRevoke, MemberRemove, Page, Role,
    RoomLeave, RoomMembers, RoomPeers, RoomRow, SubjectId, Timestamp,
};
use jeliya_client::{ClientHandle, Dedup, Execution};
use jeliya_platform::navigation::{RoomDest, Route};
use jeliya_platform::PreferenceKey;

use super::confirm::ConfirmDialog;
use super::invite_form::{InviteDraft, InviteForm};
use super::read::ready_call;
use super::Heading;
use crate::l10n::{use_formats, use_strings};
use crate::shell::router::NavIntent;
use crate::status;
use crate::view::alias::AliasMap;
use crate::view::capability::{authorized, Action};
use crate::view::invites::{fold_invites, InviteView};
use crate::view::load::{LoadState, ReadOutcome};
use crate::view::roster::{fold_roster, MemberPresence, RosterView};
use crate::PlatformServices;

/// A bounded first page for the invite list (AC-6 boundedness).
fn first_page() -> Page {
    Page {
        cursor: jeliya_api::Cursor::Start,
        direction: jeliya_api::Direction::Forward,
        limit: 50,
    }
}

/// Which destructive confirmation is open, if any.
#[derive(Clone, PartialEq, Eq)]
enum Confirm {
    Remove(SubjectId),
    Revoke(InviteId),
    Leave,
}

/// The People destination pane.
#[component]
pub fn PeoplePane(
    room: RoomRow,
    #[props(default)] item: Option<SubjectId>,
    handle: ClientHandle,
    services: PlatformServices,
    navigate: Callback<NavIntent>,
    now: Timestamp,
    #[props(default)] self_id: Option<SubjectId>,
) -> Element {
    let strings = use_strings();
    let room_id = room.room_id.clone();
    let live = room.live;
    let caps = room.capabilities.clone();

    // The read models and their truthful states.
    let roster = use_signal(|| LoadState::<RosterView>::Loading);
    let invites = use_signal(|| LoadState::<InviteView>::Loading);
    // Bump to re-run the reads after a mutation settles.
    let mut refresh = use_signal(|| 0u32);
    // The open destructive confirmation, the mutation guard, the once-shown
    // minted capability, and the ambiguous-outcome flag.
    let confirm = use_signal(|| None::<Confirm>);
    let in_flight = use_signal(|| false);
    let minted = use_signal(|| None::<String>);
    let couldnt_confirm = use_signal(|| false);

    // The reads. Re-runs when `refresh` changes (read synchronously below).
    {
        let handle = handle.clone();
        let room_id = room_id.clone();
        let mut roster = roster;
        let mut invites = invites;
        use_future(move || {
            let _dep = refresh();
            let handle = handle.clone();
            let room_id = room_id.clone();
            async move {
                roster.set(LoadState::Loading);
                invites.set(LoadState::Loading);

                // Agent markers + roster come from members, fleet, and (live) peers.
                let members = ready_call(
                    &handle,
                    RoomMembers {
                        room_id: room_id.clone(),
                    },
                )
                .await;
                let fleet = ready_call(&handle, jeliya_api::FleetList {}).await;
                let peers = if live {
                    Some(
                        ready_call(
                            &handle,
                            RoomPeers {
                                room_id: room_id.clone(),
                            },
                        )
                        .await,
                    )
                } else {
                    None
                };
                match members {
                    Ok(members) => {
                        let agents = fleet
                            .as_ref()
                            .map(|f| crate::view::agents::agent_subjects_in_room(f, &room_id))
                            .unwrap_or_default();
                        let peers_ok = match peers {
                            Some(Ok(p)) => Some(p),
                            _ => None,
                        };
                        let view = fold_roster(&members, peers_ok.as_ref(), &agents, live);
                        roster.set(if view.is_empty() {
                            LoadState::Empty
                        } else {
                            LoadState::Loaded(view)
                        });
                    }
                    Err(err) => roster.set(load_error(&err)),
                }

                match ready_call(
                    &handle,
                    InviteList {
                        room_id: room_id.clone(),
                        page: first_page(),
                    },
                )
                .await
                {
                    Ok(list) => {
                        let view = fold_invites(&list);
                        invites.set(if view.is_empty() {
                            LoadState::Empty
                        } else {
                            LoadState::Loaded(view)
                        });
                    }
                    Err(err) => invites.set(load_error(&err)),
                }
            }
        });
    }

    // Alias resolution reads the device-local map fresh each render (a live edit
    // in Settings re-resolves names here).
    let aliases = AliasMap::parse(
        services
            .preferences()
            .get(&PreferenceKey::Aliases)
            .as_deref(),
    );
    let self_id = self_id.clone();

    // A shared mutation runner: dispatch a signed mutation with `Dedup::None`
    // (never auto-replayed — D5), clear the guard, and either refresh on success
    // or surface the ambiguous-outcome state on an unknown-execution failure.
    let run_mutation = {
        let handle = handle.clone();
        let mut in_flight = in_flight;
        let mut confirm = confirm;
        let mut couldnt = couldnt_confirm;
        move |op: MutationOp, navigate_home: bool, nav: Callback<NavIntent>| {
            // Re-entrancy guard: if a signed mutation is already settling, this
            // is a duplicate press that slipped in before the control disabled —
            // drop it so the op is never dispatched twice (D5, no duplicate
            // submissions).
            if *in_flight.peek() {
                return;
            }
            let handle = handle.clone();
            in_flight.set(true);
            couldnt.set(false);
            spawn(async move {
                let outcome = dispatch_mutation(&handle, op).await;
                in_flight.set(false);
                confirm.set(None);
                match outcome {
                    Ok(()) => {
                        if navigate_home {
                            nav.call(NavIntent::push(Route::Rooms));
                        } else {
                            let next = refresh.peek().wrapping_add(1);
                            refresh.set(next);
                        }
                    }
                    Err(execution) => {
                        // A signed, irreversible mutation that may or may not
                        // have committed: never auto-replay — say so and let the
                        // user reload to read the truth (D5).
                        if execution == Execution::Unknown {
                            couldnt.set(true);
                        }
                        let next = refresh.peek().wrapping_add(1);
                        refresh.set(next);
                    }
                }
            });
        }
    };

    // Re-invite (mint fresh for the same identity + revoke the stale one, D3),
    // disclosing the fresh capability once.
    let run_reinvite = {
        let handle = handle.clone();
        let mut in_flight = in_flight;
        let mut minted = minted;
        let mut couldnt = couldnt_confirm;
        let room_id = room_id.clone();
        move |subject: SubjectId, role: Role, stale: InviteId, expires_at: Timestamp| {
            // Re-entrancy guard: a re-invite already settling means this is a
            // duplicate press — drop it so we never mint two live capabilities
            // (and revoke twice) for the same identity (D3, no duplicate submissions).
            if *in_flight.peek() {
                return;
            }
            let handle = handle.clone();
            let room_id = room_id.clone();
            in_flight.set(true);
            couldnt.set(false);
            spawn(async move {
                let mint = handle
                    .call::<InviteMint>(
                        InviteMint {
                            room_id: room_id.clone(),
                            subject_id: subject,
                            role,
                            expires_at,
                        },
                        Dedup::None,
                    )
                    .await;
                match mint {
                    Ok(out) => {
                        // Disclose the fresh capability ONCE (never persisted).
                        minted.set(Some(out.capability));
                        // Revoke the stale ticket so it cannot be presented.
                        let _ = handle
                            .call::<InviteRevoke>(
                                InviteRevoke {
                                    room_id,
                                    invite_id: stale,
                                },
                                Dedup::None,
                            )
                            .await;
                    }
                    Err(err) => {
                        if err.execution() == Execution::Unknown {
                            couldnt.set(true);
                        }
                    }
                }
                in_flight.set(false);
                let next = refresh.peek().wrapping_add(1);
                refresh.set(next);
            });
        }
    };

    // Issue an invitation from the form draft (mint + disclose once).
    let run_invite = {
        let handle = handle.clone();
        let mut in_flight = in_flight;
        let mut minted = minted;
        let mut couldnt = couldnt_confirm;
        let room_id = room_id.clone();
        move |draft: InviteDraft| {
            let handle = handle.clone();
            let room_id = room_id.clone();
            in_flight.set(true);
            couldnt.set(false);
            spawn(async move {
                let out = handle
                    .call::<InviteMint>(
                        InviteMint {
                            room_id,
                            subject_id: draft.subject_id,
                            role: draft.role,
                            expires_at: draft.expires_at,
                        },
                        Dedup::None,
                    )
                    .await;
                match out {
                    Ok(out) => minted.set(Some(out.capability)),
                    Err(err) => {
                        if err.execution() == Execution::Unknown {
                            couldnt.set(true);
                        }
                    }
                }
                in_flight.set(false);
                let next = refresh.peek().wrapping_add(1);
                refresh.set(next);
            });
        }
    };

    let heading = strings.people_heading();
    let can_invite = authorized(Action::Invite, &caps);
    let guard = in_flight();

    rsx! {
        section { class: "people-pane", id: "people-pane",
            Heading { level: 2, class: "people-title".to_string(), "{heading}" }

            // The ambiguous-outcome banner (a signed mutation we couldn't confirm).
            if couldnt_confirm() {
                p { class: "error-note", id: "people-couldnt-confirm", "{strings.couldnt_confirm()}" }
            }

            // -- Presence summary (D1) -------------------------------------
            PresenceSummary { roster: roster.read().clone(), caps: caps.clone(), in_flight: guard,
                on_activate: {
                    let run = run_mutation.clone();
                    let room_id = room_id.clone();
                    move |_| {
                        let mut run = run.clone();
                        run(MutationOp::Activate(room_id.clone()), false, navigate);
                    }
                },
            }

            // -- Roster (room.members) -------------------------------------
            RosterSection {
                state: roster.read().clone(),
                aliases: aliases.clone(),
                self_id: self_id.clone(),
                caps: caps.clone(),
                selected: item.clone(),
                room_id: room_id.clone(),
                navigate,
                on_remove: {
                    let mut confirm = confirm;
                    move |subject: SubjectId| confirm.set(Some(Confirm::Remove(subject)))
                },
            }

            // -- Invitations (invite.list, D2) -----------------------------
            InvitesSection {
                state: invites.read().clone(),
                aliases: aliases.clone(),
                self_id: self_id.clone(),
                caps: caps.clone(),
                in_flight: guard,
                on_revoke: {
                    let mut confirm = confirm;
                    move |id: InviteId| confirm.set(Some(Confirm::Revoke(id)))
                },
                on_reinvite: {
                    let run = run_reinvite.clone();
                    move |(subject, role, stale): (SubjectId, Role, InviteId)| {
                        let mut run = run.clone();
                        run(subject, role, stale, ExpiryDefault::at(now));
                    }
                },
            }

            // The once-shown minted-capability disclosure (never persisted, §11).
            if let Some(cap) = minted() {
                CapabilityDisclosure {
                    capability: cap,
                    services: services.clone(),
                    on_dismiss: {
                        let mut minted = minted;
                        move |_| minted.set(None)
                    },
                }
            }

            // -- Issue an invitation (capability-gated) --------------------
            if can_invite {
                InviteForm {
                    now,
                    in_flight: guard,
                    on_submit: {
                        let run = run_invite.clone();
                        move |draft: InviteDraft| {
                            let mut run = run.clone();
                            run(draft);
                        }
                    },
                }
            }

            // -- Leave room (capability-gated, destructive) ----------------
            if authorized(Action::Leave, &caps) {
                button {
                    r#type: "button",
                    class: "btn btn-danger btn-sm",
                    id: "people-leave",
                    onclick: {
                        let mut confirm = confirm;
                        move |_| confirm.set(Some(Confirm::Leave))
                    },
                    "{strings.action_leave()}"
                }
            }

            // The one destructive-confirmation dialog (D4).
            match confirm() {
                Some(Confirm::Remove(subject)) => rsx! {
                    ConfirmDialog {
                        title: strings.confirm_remove_title().to_string(),
                        body: strings.confirm_remove_body().to_string(),
                        confirm_label: strings.confirm_remove_confirm().to_string(),
                        room_short: short_room(&room_id),
                        in_flight: guard,
                        on_confirm: {
                            let run = run_mutation.clone();
                            let room_id = room_id.clone();
                            let subject = subject.clone();
                            move |_| {
                                let mut run = run.clone();
                                run(MutationOp::Remove(room_id.clone(), subject.clone()), false, navigate);
                            }
                        },
                        on_cancel: {
                            let mut confirm = confirm;
                            move |_| confirm.set(None)
                        },
                    }
                },
                Some(Confirm::Revoke(id)) => rsx! {
                    ConfirmDialog {
                        title: strings.confirm_revoke_title().to_string(),
                        body: strings.confirm_revoke_body().to_string(),
                        confirm_label: strings.confirm_revoke_confirm().to_string(),
                        room_short: short_room(&room_id),
                        in_flight: guard,
                        on_confirm: {
                            let run = run_mutation.clone();
                            let room_id = room_id.clone();
                            let id = id.clone();
                            move |_| {
                                let mut run = run.clone();
                                run(MutationOp::Revoke(room_id.clone(), id.clone()), false, navigate);
                            }
                        },
                        on_cancel: {
                            let mut confirm = confirm;
                            move |_| confirm.set(None)
                        },
                    }
                },
                Some(Confirm::Leave) => rsx! {
                    ConfirmDialog {
                        title: strings.confirm_leave_title().to_string(),
                        body: strings.confirm_leave_body().to_string(),
                        confirm_label: strings.confirm_leave_confirm().to_string(),
                        room_short: short_room(&room_id),
                        in_flight: guard,
                        on_confirm: {
                            let run = run_mutation.clone();
                            let room_id = room_id.clone();
                            move |_| {
                                let mut run = run.clone();
                                run(MutationOp::Leave(room_id.clone()), true, navigate);
                            }
                        },
                        on_cancel: {
                            let mut confirm = confirm;
                            move |_| confirm.set(None)
                        },
                    }
                },
                None => rsx! {},
            }
        }
    }
}

/// The default expiry for a re-invite (7 days) — the operator confirms the
/// re-invite through the row action; a full duration picker is the issue form's.
struct ExpiryDefault;
impl ExpiryDefault {
    fn at(now: Timestamp) -> Timestamp {
        super::invite_form::ExpiryChoice::Week.at(now)
    }
}

/// A signed mutation the pane may dispatch.
enum MutationOp {
    Remove(jeliya_api::RoomId, SubjectId),
    Revoke(jeliya_api::RoomId, InviteId),
    Leave(jeliya_api::RoomId),
    Activate(jeliya_api::RoomId),
}

/// Dispatch a mutation with `Dedup::None`. `Ok(())` on success; `Err(execution)`
/// carries the may-have-executed classification for the ambiguous-outcome path.
async fn dispatch_mutation(handle: &ClientHandle, op: MutationOp) -> Result<(), Execution> {
    match op {
        MutationOp::Remove(room_id, subject_id) => handle
            .call::<MemberRemove>(
                MemberRemove {
                    room_id,
                    subject_id,
                },
                Dedup::None,
            )
            .await
            .map(|_| ())
            .map_err(|e| e.execution()),
        MutationOp::Revoke(room_id, invite_id) => handle
            .call::<InviteRevoke>(InviteRevoke { room_id, invite_id }, Dedup::None)
            .await
            .map(|_| ())
            .map_err(|e| e.execution()),
        MutationOp::Leave(room_id) => handle
            .call::<RoomLeave>(RoomLeave { room_id }, Dedup::None)
            .await
            .map(|_| ())
            .map_err(|e| e.execution()),
        MutationOp::Activate(room_id) => handle
            .call::<jeliya_api::RoomActivate>(jeliya_api::RoomActivate { room_id }, Dedup::None)
            .await
            .map(|_| ())
            .map_err(|e| e.execution()),
    }
}

/// Map a read error to the truthful [`LoadState`] arm (idempotent-read policy).
fn load_error<T>(err: &jeliya_client::CallError) -> LoadState<T> {
    // `member.remove`-style authorization is a wire refusal; a coarse check is
    // enough here (the typed code lives in jeliya-api).
    let unauthorized = matches!(err, jeliya_client::CallError::Wire(_));
    match ReadOutcome::classify(err, unauthorized) {
        ReadOutcome::Offline => LoadState::Offline,
        ReadOutcome::Unauthorized => LoadState::Unauthorized,
        ReadOutcome::Failed => LoadState::Failed(crate::l10n::FriendlyError {
            title: String::new(),
            message: String::new(),
        }),
    }
}

/// The room's short-id disambiguator (repeated in confirm dialogs and homonym
/// contexts).
fn short_room(room_id: &jeliya_api::RoomId) -> String {
    let raw = room_id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    // ---- short_room: stable, bounded room disambiguator (AC-1) ------------

    #[test]
    fn short_room_takes_the_tail_after_the_last_colon() {
        let id = jeliya_api::RoomId::new("room:prefix:abcdefgh");
        assert_eq!(short_room(&id), "abcdefgh");
    }

    #[test]
    fn short_room_is_truncated_to_8_chars() {
        let id = jeliya_api::RoomId::new("room:abcdefghijklmnop");
        let short = short_room(&id);
        assert_eq!(short.chars().count(), 8);
        assert_eq!(short, "abcdefgh");
    }

    #[test]
    fn short_room_with_no_colon_returns_the_raw_id_truncated() {
        let id = jeliya_api::RoomId::new("abcdefghijklmnop");
        assert_eq!(short_room(&id), "abcdefgh");
    }

    #[test]
    fn short_room_shorter_than_8_chars_is_returned_whole() {
        let id = jeliya_api::RoomId::new("room:abc");
        assert_eq!(short_room(&id), "abc");
    }

    // ---- ExpiryDefault: re-invite window is always 7 days ----------------

    #[test]
    fn expiry_default_is_exactly_7_days_from_now() {
        let now = jeliya_api::Timestamp::new(OffsetDateTime::UNIX_EPOCH);
        let expiry = ExpiryDefault::at(now);
        let delta = expiry.into_inner() - now.into_inner();
        assert_eq!(delta.whole_days(), 7, "re-invite window must be 7 days");
    }
}

/// The presence summary region (D1): the room-session Open/Closed fact and, for
/// a live room, the aggregate reachability; a Closed room shows the honest
/// "presence unavailable" note plus, when authorized, an Activate action.
#[component]
fn PresenceSummary(
    roster: LoadState<RosterView>,
    caps: Vec<CapabilityToken>,
    #[props(default)] in_flight: bool,
    on_activate: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    let heading = strings.people_presence_heading();
    let Some(view) = roster.value() else {
        return rsx! {};
    };
    let session = status::room_session_label(strings, view.session_open);
    rsx! {
        section { class: "presence-summary", id: "presence-summary",
            Heading { level: 3, class: "presence-title".to_string(), "{heading}" }
            p { class: "presence-session",
                span { class: "presence-session-word", "{session}" }
            }
            if view.session_open {
                if let Some(reach) = view.reachability {
                    p { class: "presence-reachability muted",
                        "{status::reachability_label(strings, reach)}"
                    }
                }
            } else {
                p { class: "presence-closed muted", id: "presence-closed-note",
                    "{strings.presence_unavailable_closed()}"
                }
                if authorized(Action::Activate, &caps) {
                    button {
                        r#type: "button",
                        class: "btn btn-sm",
                        id: "presence-activate",
                        // Disabled while a mutation settles, so a rapid double-click
                        // cannot dispatch a second Activate (matches ConfirmDialog).
                        disabled: in_flight,
                        onclick: move |_| on_activate.call(()),
                        "{strings.action_activate()}"
                    }
                }
            }
        }
    }
}

/// The roster region.
#[component]
fn RosterSection(
    state: LoadState<RosterView>,
    aliases: AliasMap,
    self_id: Option<SubjectId>,
    caps: Vec<CapabilityToken>,
    selected: Option<SubjectId>,
    room_id: jeliya_api::RoomId,
    navigate: Callback<NavIntent>,
    on_remove: EventHandler<SubjectId>,
) -> Element {
    let strings = use_strings();
    let heading = strings.people_roster_heading();
    let can_remove = authorized(Action::Remove, &caps);

    rsx! {
        section { class: "roster-section", id: "roster-section",
            Heading { level: 3, class: "roster-title".to_string(), "{heading}" }
            match &state {
                LoadState::Loading => rsx! { p { class: "muted", id: "roster-loading", "{strings.state_loading()}" } },
                LoadState::Empty => rsx! { p { class: "muted", id: "roster-empty", "{strings.people_no_members()}" } },
                LoadState::Offline => rsx! { p { class: "muted", id: "roster-offline", "{strings.state_offline()}" } },
                LoadState::Unauthorized => rsx! { p { class: "muted", id: "roster-unauth", "{strings.state_unauthorized()}" } },
                LoadState::Failed(_) => rsx! { p { class: "error-note", id: "roster-failed", "{strings.state_failed_body()}" } },
                LoadState::Loaded(view) | LoadState::Stale(view) => rsx! {
                    ul { class: "roster-list", id: "roster-list",
                        for row in view.rows.iter() {
                            RosterRowView {
                                key: "{row.subject_id}",
                                row: row.clone(),
                                aliases: aliases.clone(),
                                self_id: self_id.clone(),
                                session_open: view.session_open,
                                can_remove,
                                is_selected: selected.as_ref() == Some(&row.subject_id),
                                room_id: room_id.clone(),
                                navigate,
                                on_remove,
                            }
                        }
                    }
                },
            }
        }
    }
}

/// One roster row.
#[component]
fn RosterRowView(
    row: crate::view::roster::RosterRow,
    aliases: AliasMap,
    self_id: Option<SubjectId>,
    session_open: bool,
    can_remove: bool,
    is_selected: bool,
    room_id: jeliya_api::RoomId,
    navigate: Callback<NavIntent>,
    on_remove: EventHandler<SubjectId>,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let resolved = aliases.resolve(&row.subject_id, self_id.as_ref());
    let name = status::resolved_name(strings, &resolved);
    let is_self = resolved.is_self();
    let role = status::role_label(strings, row.role);
    let standing = status::standing_label(strings, row.standing);
    let joined = status::date_of(formats, row.joined_at);
    let class = if is_selected {
        "roster-row selected"
    } else {
        "roster-row"
    };
    let subject = row.subject_id.clone();
    let subject_for_remove = row.subject_id.clone();
    let subject_attr = row.subject_id.as_str().to_owned();

    rsx! {
        li { class: "{class}", "data-subject": "{subject_attr}",
            // Selecting the row deep-links the member (stable, Back-closeable).
            button {
                r#type: "button",
                class: "roster-select",
                onclick: {
                    let room_id = room_id.clone();
                    let subject = subject.clone();
                    move |_| {
                        navigate
                            .call(
                                NavIntent::push(Route::Room {
                                    room_id: room_id.clone(),
                                    dest: RoomDest::People { item: Some(subject.clone()) },
                                }),
                            )
                    }
                },
                span { class: "roster-name", "{name}" }
                if is_self {
                    span { class: "roster-self-marker muted", " ({strings.self_this_device()})" }
                }
                if row.is_agent {
                    span { class: "roster-agent-marker", " {strings.roster_agent_marker()}" }
                }
            }
            span { class: "roster-role", "{strings.roster_role_label()} {role}" }
            span { class: "roster-standing", "{strings.roster_standing_label()} {standing}" }
            span { class: "roster-joined muted", "{strings.roster_joined_label()} {joined}" }
            // Presence — a SEPARATE line, only for a live room (D1).
            if session_open {
                span { class: "roster-presence",
                    "{strings.roster_presence_label()} "
                    {presence_text(strings, &row.presence)}
                }
            }
            if can_remove && !is_self {
                button {
                    r#type: "button",
                    class: "btn btn-danger btn-sm roster-remove",
                    onclick: move |_| on_remove.call(subject_for_remove.clone()),
                    "{strings.action_remove()}"
                }
            }
        }
    }
}

/// Render a member's presence as text (D1): the per-device links, or the honest
/// "not connected" when the live room shows no link.
fn presence_text(strings: &dyn crate::l10n::Catalog, presence: &MemberPresence) -> Element {
    match presence {
        MemberPresence::Unknown => rsx! { span { class: "muted", "{strings.presence_absent()}" } },
        MemberPresence::Links(links) if links.is_empty() => {
            rsx! { span { class: "muted", "{strings.presence_absent()}" } }
        }
        MemberPresence::Links(links) => rsx! {
            span { class: "roster-links",
                for (i , link) in links.iter().enumerate() {
                    span { key: "{i}", class: "roster-link mono", "{status::link_label(strings, link)}" }
                }
            }
        },
    }
}

/// The invitations region (D2).
#[component]
fn InvitesSection(
    state: LoadState<InviteView>,
    aliases: AliasMap,
    self_id: Option<SubjectId>,
    caps: Vec<CapabilityToken>,
    #[props(default)] in_flight: bool,
    on_revoke: EventHandler<InviteId>,
    on_reinvite: EventHandler<(SubjectId, Role, InviteId)>,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let heading = strings.people_invites_heading();
    let can_revoke = authorized(Action::Revoke, &caps);
    let can_invite = authorized(Action::Invite, &caps);

    rsx! {
        section { class: "invites-section", id: "invites-section",
            Heading { level: 3, class: "invites-title".to_string(), "{heading}" }
            match &state {
                LoadState::Loading => rsx! { p { class: "muted", id: "invites-loading", "{strings.state_loading()}" } },
                LoadState::Empty => rsx! { p { class: "muted", id: "invites-empty", "{strings.invites_empty()}" } },
                LoadState::Offline => rsx! { p { class: "muted", id: "invites-offline", "{strings.state_offline()}" } },
                LoadState::Unauthorized => rsx! { p { class: "muted", id: "invites-unauth", "{strings.state_unauthorized()}" } },
                LoadState::Failed(_) => rsx! { p { class: "error-note", id: "invites-failed", "{strings.state_failed_body()}" } },
                LoadState::Loaded(view) | LoadState::Stale(view) => rsx! {
                    ul { class: "invites-list", id: "invites-list",
                        for invite in view.rows.iter() {
                            {
                                let resolved = aliases.resolve(&invite.subject_id, self_id.as_ref());
                                let bound = status::resolved_name(strings, &resolved);
                                let expires = status::date_of(formats, invite.expires_at);
                                let redeem = status::redeemability_label(strings, invite.redeemability);
                                let target = invite.reinvite_target();
                                let stale = invite.invite_id.clone();
                                let invite_id = invite.invite_id.clone();
                                rsx! {
                                    li { key: "{invite.invite_id}", class: "invite-row", "data-invite": "{invite.invite_id}",
                                        span { class: "invite-bound", "{strings.invite_bound_label()} {bound}" }
                                        span { class: "invite-redeem", "{redeem}" }
                                        span { class: "invite-expires muted", "{strings.invite_expires_label()} {expires}" }
                                        if can_revoke && invite.can_revoke {
                                            button {
                                                r#type: "button",
                                                class: "btn btn-sm invite-revoke",
                                                onclick: move |_| on_revoke.call(invite_id.clone()),
                                                "{strings.action_revoke()}"
                                            }
                                        }
                                        if can_invite {
                                            if let Some((subject, role)) = target {
                                                button {
                                                    r#type: "button",
                                                    class: "btn btn-sm invite-reinvite",
                                                    // Disabled while a mutation settles, so a rapid
                                                    // double-click cannot mint two live capabilities
                                                    // for the same identity (matches ConfirmDialog).
                                                    disabled: in_flight,
                                                    onclick: move |_| on_reinvite.call((subject.clone(), role, stale.clone())),
                                                    "{strings.action_reinvite()}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// The once-shown minted-capability disclosure (§11): copyable, never persisted.
#[component]
fn CapabilityDisclosure(
    capability: String,
    services: PlatformServices,
    on_dismiss: EventHandler<()>,
) -> Element {
    let strings = use_strings();
    rsx! {
        div { class: "capability-disclosure", id: "capability-disclosure",
            Heading { level: 3, class: "capability-title".to_string(), "{strings.invite_capability_heading()}" }
            p { class: "capability-note muted", "{strings.invite_capability_note()}" }
            code { class: "capability-value mono", id: "capability-value", "{capability}" }
            div { class: "capability-actions",
                button {
                    r#type: "button",
                    class: "btn btn-sm",
                    id: "capability-copy",
                    onclick: {
                        let services = services.clone();
                        let capability = capability.clone();
                        move |_| {
                            let services = services.clone();
                            let capability = capability.clone();
                            spawn(async move {
                                let _ = services.clipboard().write_text(&capability).await;
                            });
                        }
                    },
                    "{strings.invite_capability_copy()}"
                }
                button {
                    r#type: "button",
                    class: "btn btn-ghost btn-sm",
                    id: "capability-dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "{strings.confirm_cancel()}"
                }
            }
        }
    }
}
