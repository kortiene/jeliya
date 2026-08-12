//! The modal dialog primitive (§5.6). The **only** path to a dialog
//! (Decision-6), so `role="dialog"` + `aria-modal`, focus containment, Escape,
//! and safe initial focus cannot be bypassed.
//!
//! Focus (portable across the browser and system-WebView targets via the Dioxus
//! 0.7.9 `set_focus` mounted API — §14 Q4, no `web-sys`/`cfg` fork). The
//! `dioxus/mounted` feature is **load-bearing**: without it, `onmounted` and
//! `set_focus` compile against the silent `()` fallback backing and never
//! fire — the #177 e2e dialog-focus contracts are the regression detector.
//! - **Initial focus** lands on the dialog panel (`tabindex="-1"`), never on a
//!   destructive control.
//! - **Containment** uses two visually-hidden focus sentinels: when focus tabs
//!   past either edge it is redirected back into the panel, so it cannot escape
//!   to the page behind the scrim. The sentinels are role-less `div`s in the
//!   tab order, **not** `aria-hidden` buttons: an `aria-hidden` element that
//!   can receive focus is itself an accessibility defect (axe
//!   `aria-hidden-focus`, serious), and a nameless button fails `button-name`.
//!   A bare `div` with `tabindex="0"` carries no role, so no accessible name
//!   is owed and assistive tech announces nothing on the redirect hop.
//! - **Escape** and a scrim click call `on_close`; the opener restores focus to
//!   its trigger (it owns that reference).
//!
//! Classes are the canonical stylesheet's (`.modal-backdrop`, `.modal`,
//! `.modal-head`, `.modal-body`) — no divergent CSS (AC-4).

use dioxus::prelude::*;

/// A modal dialog. `title` is the accessible name and the visible heading;
/// `close_label` is catalog copy for the close control; `on_close` fires on
/// Escape, a scrim click, or the close button. `children` are the dialog body.
#[component]
pub fn Dialog(
    title: String,
    close_label: String,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    // The panel's mounted handle, for INITIAL focus (never a destructive
    // control). The close button's handle, for the sentinels: the dialog's
    // only tabbable control is the close button (the body is non-interactive
    // content), so a sentinel redirects focus to IT, which cycles correctly in
    // both directions — Shift+Tab from the close button lands back on the close
    // button rather than dead-ending on the `tabindex="-1"` panel. (A dialog
    // with multiple interactive controls needs first/last-tabbable DOM queries;
    // this foundation dialog has exactly one.)
    let mut panel = use_signal(|| None::<MountedEvent>);
    let mut close_btn = use_signal(|| None::<MountedEvent>);

    let refocus_control = move || {
        if let Some(element) = close_btn() {
            spawn(async move {
                let _ = element.set_focus(true).await;
            });
        }
    };

    rsx! {
        div {
            class: "modal-backdrop",
            id: "dialog-backdrop",
            // A scrim click dismisses; the panel stops propagation so a click
            // inside it does not.
            onclick: move |_| on_close.call(()),
            // Escape closes from anywhere in the dialog subtree.
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Escape {
                    on_close.call(());
                }
            },
            // Leading focus sentinel: reached by Shift+Tab from the first
            // control; redirect to the close control so reverse traversal cycles.
            div {
                class: "visually-hidden",
                id: "dialog-sentinel-lead",
                tabindex: "0",
                onfocus: move |_| refocus_control(),
            }
            div {
                class: "modal",
                id: "dialog-panel",
                role: "dialog",
                "aria-modal": "true",
                "aria-label": "{title}",
                tabindex: "-1",
                onclick: move |evt| evt.stop_propagation(),
                onmounted: move |evt: MountedEvent| {
                    panel.set(Some(evt.clone()));
                    spawn(async move {
                        let _ = evt.set_focus(true).await;
                    });
                },
                div { class: "modal-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-btn",
                        "aria-label": "{close_label}",
                        onmounted: move |evt: MountedEvent| close_btn.set(Some(evt)),
                        onclick: move |_| on_close.call(()),
                        // A glyph, not the localized word: `.icon-btn` is a fixed
                        // 26x26 target, so rendering `Close`/`Fermer` inside it
                        // overflows the border. `close_label` is the accessible
                        // name (aria-label); the `×` is decorative (aria-hidden).
                        span { "aria-hidden": "true", "\u{00d7}" }
                    }
                }
                div { class: "modal-body", {children} }
            }
            // Trailing focus sentinel: reached by Tab from the last control.
            div {
                class: "visually-hidden",
                id: "dialog-sentinel-trail",
                tabindex: "0",
                onfocus: move |_| refocus_control(),
            }
        }
    }
}
