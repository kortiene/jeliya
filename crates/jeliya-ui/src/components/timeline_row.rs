//! Per-kind timeline row renderers (#179 §5.2) — exhaustive and total.
//!
//! One [`RenderUnit`] becomes one row. The `match` over
//! [`EventKindContent`](jeliya_api::EventKindContent) is **exhaustive by
//! construction** (D2): a new signed kind will not compile until it has a row,
//! so there is no `return null` silent drop of a signed fact. A kind with no
//! bespoke card would fall to an inspectable generic row — but the closed 10-kind
//! set has a bespoke arm for every kind, so that arm is unreachable today and the
//! compiler is what keeps the projection total.
//!
//! All user-visible copy comes from the catalog ([`use_strings`]); opaque ids are
//! shortened, mono, and never assumed hex (§11). This component owns no DOM
//! measurement (Decision-6) — it is pure rendering fed the pure projection.

use dioxus::prelude::*;
use jeliya_api::{Author, EventKindContent, Progress, SubjectId, Timestamp};

use crate::l10n::{use_formats, use_strings, wire, Formats};
use crate::room::projection::{DayKey, RenderUnit, Side, TimelineRow};
use crate::room::runs::AgentRun;
use crate::room::send::{SendEntry, SendId, SendPhase};

/// A short, human-scannable disambiguator for an opaque subject id: the tail
/// after any `:` prefix, truncated. Never assumed hex; a data element, not copy.
fn short_subject(id: &SubjectId) -> String {
    let raw = id.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(8).collect()
}

/// The display name for an author (§5.2 / contract §"Identity, aliases"): self →
/// the catalog "You" (alias resolution is #180's roster concern), a resolved peer
/// → its short id, an unresolved author → the catalog's unresolved-sender label
/// (nothing is asserted).
fn author_name(
    author: &Author,
    self_id: Option<&SubjectId>,
    strings: &dyn crate::l10n::Catalog,
) -> String {
    match author {
        Author::Resolved { subject_id, .. } if Some(subject_id) == self_id => {
            strings.timeline_you().to_string()
        }
        Author::Resolved { subject_id, .. } => short_subject(subject_id),
        Author::Unresolved => strings.timeline_unresolved_sender().to_string(),
    }
}

/// The signed instant as a wall-clock string under the formatting locale.
fn clock(formats: Formats, at: Timestamp) -> String {
    let dt = at.into_inner();
    formats.clock(u32::from(dt.hour()), u32::from(dt.minute()))
}

fn side_class(side: Side) -> &'static str {
    match side {
        Side::Own => "timeline-row own",
        Side::Remote => "timeline-row remote",
        Side::System => "timeline-row system",
    }
}

/// A day divider, rendered before the first row of each calendar day. Uses the
/// absolute localized date (no wall clock is available to a renderer-agnostic
/// component, so no relative "today"/"yesterday" is claimed — D6).
#[component]
fn DayDivider(day: DayKey) -> Element {
    let formats = use_formats();
    let label = formats.date(u32::from(day.day), u32::from(day.month), day.year);
    rsx! {
        div { class: "day-divider", "aria-label": "{label}",
            span { class: "day-divider-label", "{label}" }
        }
    }
}

/// One decorated timeline row: its optional day divider plus the projected unit.
/// `on_retry` re-issues a failed local send (§6); the pane owns the dispatch, so
/// this row only surfaces the affordance and forwards the entry's local id.
#[component]
pub fn TimelineRowView(
    row: TimelineRow,
    self_id: Option<SubjectId>,
    on_retry: Callback<SendId>,
) -> Element {
    let self_ref = self_id.as_ref();
    rsx! {
        if let Some(day) = row.divider {
            DayDivider { day }
        }
        div { class: side_class(row.side),
            match row.unit.clone() {
                RenderUnit::Event(event) => render_event(&event, self_ref, row.compact),
                RenderUnit::Run(run) => render_run(&run, self_ref),
                RenderUnit::Pending(entry) => render_pending(&entry, on_retry),
            }
        }
    }
}

/// Render one signed event by kind — the exhaustive, total match (D2).
fn render_event(event: &jeliya_api::Event, self_id: Option<&SubjectId>, compact: bool) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let name = author_name(&event.author, self_id, strings);
    let time = clock(formats, event.at);
    let meta_hidden = if compact { "true" } else { "false" };

    match &event.kind {
        EventKindContent::Message { body } => rsx! {
            div { class: "msg",
                if !compact {
                    div { class: "msg-meta", "aria-hidden": "{meta_hidden}",
                        span { class: "msg-author", "{name}" }
                        span { class: "msg-time mono", "{time}" }
                    }
                }
                div { class: "msg-body", "{body}" }
            }
        },
        EventKindContent::AgentStatus { label, progress } => rsx! {
            AgentStatusCard {
                author: name,
                time,
                label: *label,
                progress: progress.clone(),
            }
        },
        EventKindContent::RoomCreated { .. } => sysline(strings.sysline_room_created(&name), time),
        EventKindContent::MemberJoined { subject_id, role } => {
            let who = short_subject(subject_id);
            let role_word = wire::role_label(strings, wire_role_str(*role));
            sysline(strings.sysline_member_joined(&who, &role_word), time)
        }
        EventKindContent::MemberLeft { subject_id } => sysline(
            strings.sysline_member_left(&short_subject(subject_id)),
            time,
        ),
        EventKindContent::MemberRemoved { subject_id, by } => sysline(
            strings.sysline_member_removed(&short_subject(subject_id), &short_subject(by)),
            time,
        ),
        EventKindContent::InviteRevoked { .. } => {
            sysline(strings.sysline_invite_revoked().to_string(), time)
        }
        EventKindContent::FileShared {
            name: file_name,
            bytes,
            ..
        } => rsx! {
            FileReference { file_name: file_name.clone(), bytes: *bytes, author: name, time }
        },
        EventKindContent::PipePublished { target, .. } => rsx! {
            PipeReference { host: target.host.clone(), port: target.port, author: name, time }
        },
        EventKindContent::PipeRevoked { .. } => {
            sysline(strings.sysline_pipe_revoked().to_string(), time)
        }
    }
}

fn wire_role_str(role: jeliya_api::Role) -> &'static str {
    match role {
        jeliya_api::Role::Authority => "authority",
        jeliya_api::Role::Member => "member",
    }
}

/// A membership/lifecycle sysline row.
fn sysline(text: String, time: String) -> Element {
    rsx! {
        div { class: "sysline",
            span { class: "sysline-text", "{text}" }
            span { class: "sysline-time mono", "{time}" }
        }
    }
}

#[component]
fn AgentStatusCard(
    author: String,
    time: String,
    label: jeliya_api::StatusLabel,
    progress: Progress,
) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let chip = strings.timeline_agent_chip();
    // The label is a wire token rendered verbatim, never translated (§5.2).
    let token = wire::status_label_token(label);
    let percent = match progress {
        Progress::Reported { percent } => Some(formats.percent(f64::from(percent), 0)),
        Progress::Absent => None,
    };
    rsx! {
        div { class: "agent-status",
            div { class: "agent-status-head",
                span { class: "agent-chip", "{chip}" }
                span { class: "agent-status-author", "{author}" }
                span { class: "agent-status-time mono", "{time}" }
            }
            div { class: "agent-status-body",
                span { class: "agent-status-label mono", "{token}" }
                if let Some(pct) = percent {
                    span { class: "agent-status-progress mono", "{pct}" }
                }
            }
        }
    }
}

/// A folded agent-status run: the latest card plus honest run evidence and the
/// count of folded status posts (expand/collapse is a later view toggle).
fn render_run(run: &AgentRun, self_id: Option<&SubjectId>) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let count = run.summary.count as u64;
    let category = crate::l10n::plural_category(use_strings_locale(), count);
    let count_display = formats.count(count);
    let summary = strings.timeline_run_summary(&count_display, category);
    let latest = &run.summary.latest;
    let name = author_name(&latest.author, self_id, strings);
    let time = clock(formats, latest.at);
    let (label, progress) = match &latest.kind {
        EventKindContent::AgentStatus { label, progress } => (*label, progress.clone()),
        // A run is built only from AgentStatus events (runs.rs), so this is
        // unreachable; render the latest as a status card regardless.
        _ => (jeliya_api::StatusLabel::Working, Progress::Absent),
    };
    rsx! {
        div { class: "agent-run",
            div { class: "agent-run-summary", "{summary}" }
            AgentStatusCard { author: name, time, label, progress }
        }
    }
}

/// The text-locale used for plural selection, read from the locale context.
fn use_strings_locale() -> crate::l10n::Locale {
    crate::l10n::use_locale().text
}

#[component]
fn FileReference(file_name: String, bytes: u64, author: String, time: String) -> Element {
    let strings = use_strings();
    let formats = use_formats();
    let open = strings.file_open_in_files();
    let size = formats.bytes(bytes);
    rsx! {
        div { class: "file-ref",
            div { class: "file-ref-meta",
                span { class: "file-ref-author", "{author}" }
                span { class: "file-ref-time mono", "{time}" }
            }
            div { class: "file-ref-body",
                // The declared file name is untrusted data, rendered as text.
                span { class: "file-ref-name", "{file_name}" }
                span { class: "file-ref-size mono", "{size}" }
            }
            // Inert until #181 — an honest, disabled affordance, not a fake action.
            button { class: "btn btn-sm", disabled: true, "{open}" }
        }
    }
}

#[component]
fn PipeReference(host: String, port: u64, author: String, time: String) -> Element {
    let strings = use_strings();
    let open = strings.pipe_open_in_pipes();
    // Build the opaque `host:port` target without a copy string literal (the
    // colon is structural data, not translatable copy).
    let mut target = host;
    target.push(':');
    target.push_str(&port.to_string());
    rsx! {
        div { class: "pipe-ref",
            div { class: "pipe-ref-meta",
                span { class: "pipe-ref-author", "{author}" }
                span { class: "pipe-ref-time mono", "{time}" }
            }
            span { class: "pipe-ref-target mono", "{target}" }
            button { class: "btn btn-sm", disabled: true, "{open}" }
        }
    }
}

/// A local pending send: its body plus an honest phase label (never a delivery
/// receipt). A `Failed` phase names the classification and offers a Retry (§6),
/// which re-issues the SAME entry under its stable `op_id` (D4) — idempotent on
/// this connection — via the pane-owned `on_retry` dispatch.
fn render_pending(entry: &SendEntry, on_retry: Callback<SendId>) -> Element {
    let strings = use_strings();
    let client_id = entry.client_id;
    let (state_class, label, failed) = match &entry.phase {
        SendPhase::Pending | SendPhase::Syncing { .. } => {
            ("pending syncing", strings.send_sending(), false)
        }
        SendPhase::Failed {
            execution: jeliya_client::Execution::Unknown,
            ..
        } => ("pending failed", strings.send_failed_maybe(), true),
        SendPhase::Failed { .. } => ("pending failed", strings.send_failed_not_sent(), true),
    };
    let retry_label = strings.send_retry();
    rsx! {
        div { class: "msg {state_class}",
            div { class: "msg-body", "{entry.body}" }
            span { class: "send-state mono", "{label}" }
            // A failed send is retryable, never a dead end: the daemon returns
            // the original event_id under the reused op_id, so no second effect.
            if failed {
                button {
                    r#type: "button",
                    class: "btn btn-sm send-retry",
                    onclick: move |_| on_retry.call(client_id),
                    "{retry_label}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::Role;

    #[test]
    fn short_subject_truncates_after_the_colon() {
        assert_eq!(
            short_subject(&SubjectId::new("blake3:abcdefghXYZ")),
            "abcdefgh"
        );
        assert_eq!(short_subject(&SubjectId::new("s-1")), "s-1");
    }

    #[test]
    fn author_name_resolves_self_peer_and_unresolved() {
        let me = SubjectId::new("me");
        let other = SubjectId::new("blake3:otherlongid");
        let strings = crate::l10n::catalog_for(crate::l10n::Locale::En);
        let self_author = Author::Resolved {
            subject_id: me.clone(),
            role: Role::Member,
            standing: jeliya_api::Standing::Active,
        };
        assert_eq!(author_name(&self_author, Some(&me), strings), "You");
        let peer = Author::Resolved {
            subject_id: other,
            role: Role::Member,
            standing: jeliya_api::Standing::Active,
        };
        assert_eq!(author_name(&peer, Some(&me), strings), "otherlon");
        assert_eq!(
            author_name(&Author::Unresolved, Some(&me), strings),
            "Unknown sender"
        );
    }
}
