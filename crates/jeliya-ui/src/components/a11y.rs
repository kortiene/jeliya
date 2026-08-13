//! Semantic accessibility primitives: landmarks, headings, skip links, and
//! visually-hidden text (§5.6). These are shared components, not raw markup, so
//! the accessible behavior cannot be bypassed (Decision-6: the primitives are
//! the only path).
//!
//! Every class name is one the canonical `ui/src/styles.css` already styles
//! (`.skip-links`, `.skip-link`, `.visually-hidden`), so the foundation consumes
//! the design system unchanged — no divergent CSS (AC-4).

use dioxus::prelude::*;

/// The main content landmark. Exactly one visible instance per foundation route.
///
/// `tabindex="-1"` makes it a programmatic focus target so a skip link can move
/// focus here (an anchor moves focus only to a focusable target); it is not a
/// tab stop.
#[component]
pub fn MainRegion(id: String, children: Element) -> Element {
    rsx! {
        main { class: "center", id, tabindex: "-1", {children} }
    }
}

/// A heading whose level is explicit, so heading order (h1 → h2 → …) is a
/// property of the call site rather than an accident of nesting. RSX cannot pick
/// a tag by value, so the supported levels are matched out; the foundation uses
/// 1 and 2.
#[component]
pub fn Heading(level: u8, #[props(default)] class: String, children: Element) -> Element {
    match level {
        1 => rsx! { h1 { class, {children} } },
        2 => rsx! { h2 { class, {children} } },
        3 => rsx! { h3 { class, {children} } },
        // A level outside the supported range is a bug at the call site; render
        // an h2 (a safe mid-document default) rather than silently dropping the
        // heading, and keep the copy visible so the mistake is noticed.
        _ => rsx! { h2 { class, {children} } },
    }
}

/// Text present in the accessible name / for screen readers but removed from the
/// visual layout (the standard clip pattern the stylesheet defines).
#[component]
pub fn VisuallyHidden(children: Element) -> Element {
    rsx! {
        span { class: "visually-hidden", {children} }
    }
}

/// The skip-links container — the first focusable region on the page, so its
/// links are the first tab stops.
#[component]
pub fn SkipLinks(children: Element) -> Element {
    rsx! {
        div { class: "skip-links", {children} }
    }
}

/// One skip link: invisible until focused, and it **moves focus** (not just
/// scrolls) because it points at a `tabindex="-1"` landmark. Not offered where
/// the destination does not exist — the caller renders it only alongside it.
/// (`anchor` names the destination element id, deliberately not `target_*`,
/// which the shared-component cfg-fork scan reserves for platform predicates.)
#[component]
pub fn SkipLink(anchor: String, label: String) -> Element {
    rsx! {
        a { class: "skip-link", href: "#{anchor}", "{label}" }
    }
}
