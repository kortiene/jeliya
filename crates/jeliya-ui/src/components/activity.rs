//! The room Activity pane (#179 §7/§9): the signed timeline, the reconcile
//! notices, the scroll anchor + new-activity affordance, the view-only filters,
//! and the composer — the real pane that replaces the `RoomShell` Activity
//! skeleton.
//!
//! The pane owns the DOM I/O (Decision-6): it activates/deactivates the room on
//! the injected [`Reconciler`], folds its [`RoomUpdate`](jeliya_client::RoomUpdate)
//! fan-out into a [`RoomActivityState`], and feeds Dioxus-measured scroll numbers
//! to the pure [`crate::room::scroll`] model, applying the result through the
//! mounted element API. Every projection, fold, and scroll decision is the pure
//! `room/` core; this component only renders it and moves pixels.

use std::collections::HashMap;
use std::rc::Rc;

use dioxus::html::geometry::PixelsVector2D;
use dioxus::prelude::*;
use futures::StreamExt;
use jeliya_api::{CapabilityToken, EventKindContent, RoomId, SubjectId};
use jeliya_client::Reconciler;

use crate::components::composer::{run_send, Composer};
use crate::components::timeline_row::TimelineRowView;
use crate::l10n::{plural_category, use_formats, use_locale, use_strings};
use crate::room::projection::{build_render_units, decorate};
use crate::room::runs::{ActivityCategory, ACTIVITY_CATEGORIES};
use crate::room::scroll::{Measure, SavedView, ScrollAction, ScrollModel};
use crate::room::send::{epoch_timestamp, SendId};
use crate::room::RoomActivityState;
use crate::shell::Shell;
use crate::PlatformServices;

/// The App-level per-room saved reading positions, provided by `AppRoot` so a
/// room switch (which remounts the pane) restores the same position (§7 AC).
pub type SavedViewMap = Signal<HashMap<RoomId, SavedView>>;

/// Apply one pure [`ScrollAction`] through the mounted element API: stick to the
/// bottom by scrolling the bottom anchor into view, or restore a saved offset by
/// scrolling the scroller to it. `None` never fights the user's own scroll.
async fn apply_scroll(
    scroller: Option<MountedEvent>,
    bottom: Option<MountedEvent>,
    action: ScrollAction,
) {
    match action {
        ScrollAction::None => {}
        ScrollAction::ToBottom => {
            if let Some(anchor) = bottom {
                let _ = anchor.scroll_to(ScrollBehavior::Instant).await;
            }
        }
        ScrollAction::ToOffset(y) => {
            if let Some(el) = scroller {
                let _ = el
                    .scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant)
                    .await;
            }
        }
    }
}

fn filter_label(strings: &dyn crate::l10n::Catalog, category: ActivityCategory) -> &'static str {
    match category {
        ActivityCategory::Conversation => strings.filter_conversation(),
        ActivityCategory::AgentRuns => strings.filter_agent_runs(),
        ActivityCategory::Membership => strings.filter_membership(),
        ActivityCategory::Files => strings.filter_files(),
        ActivityCategory::Pipes => strings.filter_pipes(),
    }
}

/// The Activity destination pane for one room.
#[component]
pub fn ActivityPane(
    handle: jeliya_client::ClientHandle,
    services: PlatformServices,
    room_id: RoomId,
    capabilities: Vec<CapabilityToken>,
    shell: Shell,
    #[props(default)] self_id: Option<SubjectId>,
) -> Element {
    let reconciler = use_context::<Rc<Reconciler>>();
    let mut saved_views = use_context::<SavedViewMap>();
    let strings = use_strings();
    let formats = use_formats();
    let locale = use_locale();

    let mut activity = use_signal(RoomActivityState::new);
    let mut scroll = use_signal(ScrollModel::new);
    let mut scroller = use_signal(|| None::<MountedEvent>);
    let mut bottom = use_signal(|| None::<MountedEvent>);
    let mut last_measure = use_signal(|| None::<Measure>);
    let mut filters = use_signal(Vec::<ActivityCategory>::new);

    let can_send = capabilities.contains(&CapabilityToken::MessageSend);

    // A failed send's Retry (§6, D4): re-issue the SAME entry under its stable
    // op_id, so `message.send` is idempotent on this connection. The pure state
    // machine (`PendingSends::retry`) flips the phase back to Pending and returns
    // the reused op_id; `run_send` then settles the phase from the reply/error.
    // A non-failed or dropped entry yields `None` and dispatches nothing.
    let retry = {
        let handle = handle.clone();
        let room_id = room_id.clone();
        use_callback(move |client_id: SendId| {
            // Release the write lock before the dispatch reads the signal again.
            let retried = activity.write().sends.retry(client_id);
            if let Some((body, op_id)) = retried {
                let handle = handle.clone();
                let room_id = room_id.clone();
                spawn(async move {
                    let _ = run_send(handle, activity, room_id, client_id, body, op_id).await;
                });
            }
        })
    };

    // Activate the room and restore any saved reading position on mount.
    {
        let reconciler = reconciler.clone();
        let room_id = room_id.clone();
        use_hook(move || {
            let _ = reconciler.activate_room(room_id.clone(), 0);
            let saved = saved_views.peek().get(&room_id).copied();
            scroll.write().restore(saved);
        });
    }
    // Save the reading position and deactivate the room on unmount, so a route
    // change both preserves the position (§7 AC) and stops tracking the room.
    {
        let reconciler = reconciler.clone();
        let room_id = room_id.clone();
        use_drop(move || {
            if let Some(measure) = *last_measure.peek() {
                match scroll.peek().save_view(measure) {
                    Some(view) => {
                        saved_views.write().insert(room_id.clone(), view);
                    }
                    None => {
                        saved_views.write().remove(&room_id);
                    }
                }
            }
            let _ = reconciler.deactivate_room(room_id.clone());
        });
    }

    // Drive the reconciler's RoomUpdate fan-out for this room, folding each
    // update into the per-room state and updating the scroll new-item accounting.
    {
        let reconciler = reconciler.clone();
        let room_id = room_id.clone();
        use_future(move || {
            let reconciler = reconciler.clone();
            let room_id = room_id.clone();
            async move {
                let mut sub = reconciler.subscribe();
                while let Some(update) = sub.next().await {
                    activity.write().apply_update(&room_id, &update);
                    // Scroll accounting counts SIGNED items only — a local pending
                    // send is the user's own work, never announced as "new activity".
                    let (count, loading) = {
                        let snapshot = activity.peek();
                        (snapshot.timeline().len(), snapshot.resyncing)
                    };
                    let action = scroll.write().on_items_changed(count, loading);
                    spawn(apply_scroll(
                        scroller.peek().clone(),
                        bottom.peek().clone(),
                        action,
                    ));
                }
            }
        });
    }

    // Measure the scroller and feed the pure model (a user scroll).
    let measure_scroll = move || {
        if let Some(el) = scroller() {
            spawn(async move {
                if let (Ok(offset), Ok(size), Ok(rect)) = (
                    el.get_scroll_offset().await,
                    el.get_scroll_size().await,
                    el.get_client_rect().await,
                ) {
                    let measure = Measure {
                        scroll_top: offset.y,
                        scroll_height: size.height,
                        client_height: rect.size.height,
                    };
                    last_measure.set(Some(measure));
                    let action = scroll.write().on_scroll(measure);
                    apply_scroll(Some(el), bottom.peek().clone(), action).await;
                }
            });
        }
    };
    // Re-run the reconciliation point on a layout change (resize/reveal).
    let sync_scroll = move || {
        if let Some(el) = scroller() {
            spawn(async move {
                if let (Ok(offset), Ok(size), Ok(rect)) = (
                    el.get_scroll_offset().await,
                    el.get_scroll_size().await,
                    el.get_client_rect().await,
                ) {
                    let measure = Measure {
                        scroll_top: offset.y,
                        scroll_height: size.height,
                        client_height: rect.size.height,
                    };
                    last_measure.set(Some(measure));
                    let action = scroll.peek().sync(measure);
                    apply_scroll(Some(el), bottom.peek().clone(), action).await;
                }
            });
        }
    };

    let snapshot = activity.read();
    let active_filters = filters.read().clone();
    let units = build_render_units(
        snapshot.timeline(),
        snapshot.sends.entries(),
        &active_filters,
    );
    let rows = decorate(&units, self_id.as_ref());
    let base_at = snapshot.newest_signed_at().unwrap_or_else(epoch_timestamp);

    let loading = snapshot.view.is_none();
    let is_empty = !loading && snapshot.timeline().is_empty() && snapshot.sends.is_empty();
    let resyncing = snapshot.resyncing;
    let lost = snapshot.lost;

    // The new-activity affordance, worded by what the trailing new items are
    // (messages-only vs. mixed), shown only while scrolled up.
    let scroll_state = scroll.read();
    let new_count = scroll_state.new_item_count;
    let show_new = new_count > 0 && !scroll_state.stick_to_bottom;
    let new_label = if show_new {
        let timeline = snapshot.timeline();
        let start = timeline.len().saturating_sub(new_count);
        let all_messages = timeline[start..]
            .iter()
            .all(|event| matches!(event.kind, EventKindContent::Message { .. }));
        let count_display = formats.count(new_count as u64);
        let category = plural_category(locale.text, new_count as u64);
        Some(if all_messages {
            strings.activity_new_messages(&count_display, category)
        } else {
            strings.activity_new_activity(&count_display, category)
        })
    } else {
        None
    };

    let empty_label = strings.activity_empty();
    let loading_label = strings.activity_loading();
    let resync_label = strings.activity_resyncing();
    let loss_label = strings.activity_recovering_loss();

    rsx! {
        section { class: "activity-pane", id: "activity-pane",
            // View-only activity filters (multi-select; empty = everything). D1.
            div { class: "activity-filters", id: "activity-filters",
                for category in ACTIVITY_CATEGORIES {
                    {
                        let active = active_filters.contains(&category);
                        let label = filter_label(strings, category);
                        rsx! {
                            button {
                                r#type: "button",
                                class: if active { "filter-chip selected" } else { "filter-chip" },
                                "aria-pressed": "{active}",
                                onclick: move |_| {
                                    let mut current = filters.write();
                                    if let Some(index) = current.iter().position(|c| *c == category) {
                                        current.remove(index);
                                    } else {
                                        current.push(category);
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }

            // Reconcile notices — non-blocking, so every resync cause is
            // observable and a local loss is honest (§9, D7).
            if resyncing {
                div { class: "activity-notice resyncing", id: "activity-resyncing", "{resync_label}" }
            }
            if let Some(dropped) = lost {
                div {
                    class: "activity-notice lost",
                    id: "activity-lost",
                    "data-dropped": "{dropped}",
                    "{loss_label}"
                }
            }

            // The scroller. Booting is UNKNOWN, not zero: a room with no
            // converged view shows Loading, never an empty timeline (D7).
            div {
                class: "activity-timeline",
                id: "activity-timeline",
                onscroll: move |_| measure_scroll(),
                onresize: move |_| sync_scroll(),
                onmounted: move |evt: MountedEvent| {
                    scroller.set(Some(evt));
                    sync_scroll();
                },
                if loading {
                    div { class: "activity-loading muted", id: "activity-loading", "{loading_label}" }
                } else if is_empty {
                    div { class: "activity-empty muted", id: "activity-empty", "{empty_label}" }
                } else {
                    for (index , row) in rows.into_iter().enumerate() {
                        TimelineRowView { key: "{index}", row, self_id: self_id.clone(), on_retry: retry }
                    }
                }
                // The stick-to-bottom anchor: scrolled into view to follow new
                // content (§7). Zero-height, purely a scroll target.
                div {
                    class: "activity-bottom",
                    id: "activity-bottom",
                    onmounted: move |evt: MountedEvent| bottom.set(Some(evt)),
                }
            }

            if let Some(label) = new_label {
                button {
                    r#type: "button",
                    class: "activity-new-affordance",
                    id: "activity-new",
                    onclick: move |_| {
                        let action = scroll.write().jump_to_new();
                        spawn(apply_scroll(scroller.peek().clone(), bottom.peek().clone(), action));
                    },
                    "{label}"
                }
            }

            Composer {
                handle,
                services,
                room_id,
                can_send,
                shell,
                base_at,
                activity,
            }
        }
    }
}
