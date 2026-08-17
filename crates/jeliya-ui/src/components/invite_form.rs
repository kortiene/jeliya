//! The accessible issue-invitation form (§7.1.4, D4/D5).
//!
//! The subject-id field **starts empty** (never pre-seeded with the user's own
//! id — contract §"Identity"); the role is **member only** today (`authority` is
//! `role_not_grantable`, so the form states the limitation rather than offering a
//! broken option); the expiry is a small duration set mapped to an absolute
//! `Timestamp` (Q3). It is **single-submit** — the submit control latches on the
//! first press and stays disabled until the parent's `in_flight` guard clears.
//! The form only *collects*; the parent owns the `invite.mint` dispatch and the
//! once-shown capability disclosure.

use dioxus::prelude::*;
use jeliya_api::{Role, SubjectId, Timestamp};
use time::Duration;

use super::form::Field;
use crate::l10n::use_strings;

/// A chosen invite expiry, as a duration the wire maps to an absolute instant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExpiryChoice {
    /// One hour.
    Hour,
    /// One day.
    Day,
    /// Seven days.
    Week,
}

impl ExpiryChoice {
    /// The absolute expiry instant this choice maps to, from `now`.
    pub fn at(self, now: Timestamp) -> Timestamp {
        let delta = match self {
            ExpiryChoice::Hour => Duration::hours(1),
            ExpiryChoice::Day => Duration::days(1),
            ExpiryChoice::Week => Duration::days(7),
        };
        Timestamp::new(now.into_inner() + delta)
    }

    /// The structural `<option>` value (not user copy).
    fn value(self) -> &'static str {
        match self {
            ExpiryChoice::Hour => "1h",
            ExpiryChoice::Day => "1d",
            ExpiryChoice::Week => "7d",
        }
    }

    fn parse(raw: &str) -> ExpiryChoice {
        match raw {
            "1h" => ExpiryChoice::Hour,
            "7d" => ExpiryChoice::Week,
            _ => ExpiryChoice::Day,
        }
    }
}

/// A collected, validated invitation draft the parent mints.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InviteDraft {
    /// The identity the capability binds to.
    pub subject_id: SubjectId,
    /// The granted role (always `member` today).
    pub role: Role,
    /// The absolute expiry.
    pub expires_at: Timestamp,
}

impl InviteDraft {
    /// A draft granting the (only grantable) `member` role.
    pub fn member(subject_id: SubjectId, expires_at: Timestamp) -> Self {
        InviteDraft {
            subject_id,
            role: Role::Member,
            expires_at,
        }
    }
}

/// The issue-invitation form. `now` is the base instant expiry is measured from
/// (supplied by the parent); `in_flight` disables submit while a mint settles;
/// `on_submit` receives the collected [`InviteDraft`].
#[component]
pub fn InviteForm(
    now: Timestamp,
    #[props(default)] in_flight: bool,
    on_submit: EventHandler<InviteDraft>,
) -> Element {
    let strings = use_strings();
    let heading = strings.invite_form_heading();
    let subject_label = strings.invite_subject_label().to_string();
    let subject_help = strings.invite_subject_help().to_string();
    let role_label = strings.invite_role_label().to_string();
    let role_note = strings.invite_role_member_note();
    let expiry_label = strings.invite_expiry_label().to_string();
    let submit_label = strings.invite_submit();
    let member_word = strings.wire_role_member();

    let mut subject = use_signal(String::new);
    let mut expiry = use_signal(|| ExpiryChoice::Day);
    let mut submitting = use_signal(|| false);

    // Release the single-submit latch once the parent's mint settles (its
    // `in_flight` guard falls back to false), so the form is reusable for the
    // next invite — and for retrying a failed one — rather than staying disabled
    // for the rest of the visit (contract §"single-submit"). `submitting` only
    // rises together with `in_flight` in the same click tick, so this never
    // releases the latch mid-flight.
    use_effect(use_reactive!(|in_flight| {
        if !in_flight && *submitting.peek() {
            submitting.set(false);
        }
    }));

    let disabled = submitting() || in_flight;

    let subject_id = "invite-subject";
    let expiry_id = "invite-expiry";

    rsx! {
        form {
            class: "invite-form",
            id: "invite-form",
            // Prevent the browser's default full-page submit; dispatch is ours.
            onsubmit: move |evt| evt.stop_propagation(),
            crate::components::Heading { level: 3, class: "invite-form-title".to_string(), "{heading}" }

            Field { id: subject_id.to_string(), label: subject_label, hint: subject_help.clone(),
                input {
                    class: "input",
                    id: subject_id.to_string(),
                    r#type: "text",
                    aria_describedby: format!("{subject_id}-hint"),
                    value: "{subject}",
                    oninput: move |evt| subject.set(evt.value()),
                }
            }

            Field { id: expiry_id.to_string(), label: expiry_label,
                select {
                    class: "input",
                    id: expiry_id.to_string(),
                    onchange: move |evt| expiry.set(ExpiryChoice::parse(&evt.value())),
                    option { value: "{ExpiryChoice::Hour.value()}", "{strings.invite_expiry_1h()}" }
                    option {
                        value: "{ExpiryChoice::Day.value()}",
                        selected: true,
                        "{strings.invite_expiry_1d()}"
                    }
                    option { value: "{ExpiryChoice::Week.value()}", "{strings.invite_expiry_7d()}" }
                }
            }

            // Role is member-only today; the limitation is stated, not a broken
            // option offered.
            div { class: "invite-role",
                span { class: "field-label", "{role_label}" }
                span { class: "invite-role-value", "{member_word}" }
                p { class: "field-hint muted", "{role_note}" }
            }

            button {
                r#type: "button",
                class: "btn btn-primary",
                id: "invite-submit",
                disabled,
                onclick: move |_| {
                    let raw = subject.peek().trim().to_string();
                    // An empty identity is not a valid invite; do nothing (the
                    // field help already asks for one).
                    if raw.is_empty() || submitting() {
                        return;
                    }
                    submitting.set(true);
                    on_submit
                        .call(InviteDraft::member(SubjectId::new(raw), expiry.peek().at(now)));
                },
                "{submit_label}"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[test]
    fn expiry_choices_map_to_future_absolute_instants() {
        let now = Timestamp::new(OffsetDateTime::UNIX_EPOCH);
        assert!(ExpiryChoice::Hour.at(now).into_inner() > now.into_inner());
        assert!(ExpiryChoice::Week.at(now).into_inner() > ExpiryChoice::Day.at(now).into_inner());
    }

    #[test]
    fn expiry_parse_round_trips_the_option_values() {
        for choice in [ExpiryChoice::Hour, ExpiryChoice::Day, ExpiryChoice::Week] {
            assert_eq!(ExpiryChoice::parse(choice.value()), choice);
        }
        // An unknown value falls back to the default day.
        assert_eq!(ExpiryChoice::parse("bogus"), ExpiryChoice::Day);
    }
}
