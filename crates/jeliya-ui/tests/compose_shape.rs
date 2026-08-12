//! End-to-end composition-shape verification for #174 / #176.
//!
//! The jeliya-ui ↔ jeliya-platform seam: `compose::web_composition()` (and, when
//! the `native` feature is on, `compose::native_composition()`) selects the
//! concrete [`jeliya_platform::PlatformServices`] shape and hands it to
//! [`jeliya_ui::AppRoot`] as a prop. These tests verify:
//!
//! - The browser composition injects browser-shaped services (session-scoped
//!   preferences, no OS window, no private directory) — the facts a shared
//!   component must read from the injected seam instead of branching on `cfg`.
//! - Cloning the services from a composition returns the same underlying Arc
//!   (pointer-equal; K11) — a component prop-clone never forks the implementation.
//! - Two `web_composition()` calls produce independent `PlatformServices` objects,
//!   so in-process tests do not share state between compositions.
//! - (feature = "native") The desktop composition injects desktop-shaped services
//!   (persistent preferences, window actions available).
//!
//! Run with:
//!   cargo test -p jeliya-ui --features ui --test compose_shape
//!   cargo test -p jeliya-ui --features ui,native --test compose_shape  # includes native tests

use jeliya_platform::Durability;
use jeliya_ui::compose::web_composition;

/// `web_composition()` selects browser-shaped `PlatformServices`: session-scoped
/// preferences (not persisted across page reload), no OS window, and no protected
/// private directory (the browser has no app-support folder).
///
/// This is the AC-2 / AC-5 seam check at the composition layer: the right shape is
/// injected into `AppRoot`, and no shared component needs a `cfg` fork to discover
/// these platform facts — it reads them from the injected services.
#[test]
fn web_composition_services_are_browser_shaped() {
    let c = web_composition();
    assert_eq!(
        c.services.preferences().durability(),
        Durability::SessionScoped,
        "browser target: preferences are session-scoped, not persisted across page reload"
    );
    assert!(
        !c.services.window().availability().is_available(),
        "browser target: no OS window — window actions (minimize/resize) are unavailable"
    );
    assert!(
        !c.services.private_directory().availability().is_available(),
        "browser target: no protected private directory — the browser has no app-support folder"
    );
}

/// Cloning the services from a composition produces the same underlying object
/// (pointer-equal `Arc`; §K11). `AppRoot` and its children share one `Platform`
/// implementation without allocating a new one per prop-clone.
#[test]
fn web_composition_services_clone_is_the_same_object() {
    let c = web_composition();
    let clone = c.services.clone();
    assert_eq!(
        c.services, clone,
        "services clone is pointer-equal (Arc bump): prop cloning never forks the implementation"
    );
}

/// Two independent `web_composition()` calls produce separate `PlatformServices`
/// objects. In-process test runs do not share preference state or scripted errors
/// across compositions.
#[test]
fn two_web_compositions_are_independent() {
    let a = web_composition();
    let b = web_composition();
    assert_ne!(
        a.services, b.services,
        "each web_composition() call produces a fresh, independent PlatformServices"
    );
}

/// The desktop-shaped composition stub (gated on the `native` feature). Verifies
/// the composition seam hands `AppRoot` desktop-shaped services — persistent
/// preferences and available window actions — before M4's real implementations
/// replace the fakes.
#[cfg(feature = "native")]
mod native_shape {
    use jeliya_platform::Durability;
    use jeliya_ui::compose::native_composition;

    #[test]
    fn native_composition_services_are_desktop_shaped() {
        let c = native_composition();
        assert_eq!(
            c.services.preferences().durability(),
            Durability::Persistent,
            "desktop target: preferences persist across launches"
        );
        assert!(
            c.services.window().availability().is_available(),
            "desktop target: window actions available (there IS an OS window to manage)"
        );
    }
}
