//! The **provisional** `PlatformServices` injection seam (§6.3, Open
//! Question O-1).
//!
//! `PlatformServices` is the injectable boundary for platform authority —
//! files, persistence, lifecycle, URLs, clipboard/share, navigation, window
//! actions. Its canonical contract is **#174 (`PlatformServices`), which is
//! not yet landed**. #176 is not formally blocked by #174, yet the issue says
//! to "consume PlatformServices". So this crate defines the injection *shape*
//! minimally, without fabricating #174's contract:
//!
//! - Only the members the M3 web foundation genuinely needs are declared here
//!   (UI-preference persistence, opening a URL, clipboard, navigation).
//! - Every member has a **deterministic in-process implementation**
//!   ([`WebPlatformServices`]), mirroring #174's own requirement that "every
//!   service has a deterministic test implementation".
//! - It is injected **separately** from [`jeliya_client::ClientHandle`] and is
//!   never merged into it.
//!
//! When #174 lands, `jeliya-ui` adopts the canonical trait by replacing this
//! local seam with a re-export — a mechanical change, not a redesign. This is
//! recorded as a provisional seam in `docs/dioxus-architecture.md` so the
//! architecture record's "Planned" row and this crate stay honest.
//!
//! No real file/lifecycle/window authority is implemented here — that is
//! native (M4). The web foundation uses browser-appropriate deterministic
//! implementations only.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The platform-authority members the M3 web foundation consumes.
///
/// Deliberately minimal. Real file/lifecycle/window authority is native (M4)
/// and is **not** part of this provisional seam.
pub trait PlatformServicesImpl {
    /// Read a persisted UI preference (for example the last-opened room), or
    /// `None` if unset. This is *UI* preference storage, not application state
    /// or protocol data.
    fn preference(&self, key: &str) -> Option<String>;

    /// Persist a UI preference under `key`. Overwrites any existing value.
    fn set_preference(&self, key: &str, value: &str);

    /// Open an external URL through the platform (a new browser tab on the
    /// web). The deterministic implementation records it rather than
    /// navigating.
    fn open_url(&self, url: &str);

    /// Write plain text to the platform clipboard. The deterministic
    /// implementation records the last written value.
    fn write_clipboard(&self, text: &str);

    /// Navigate the in-app route (the browser history / hash route). The
    /// deterministic implementation records the current route.
    fn navigate(&self, route: &str);

    /// The current in-app route.
    fn route(&self) -> String;
}

/// The injected platform-authority seam, cloneable and comparable so it can be
/// a Dioxus component prop.
///
/// Cheap to clone (an `Arc` bump); every clone shares one implementation.
/// Equality is pointer identity — a `clone()` handed to a child is the same
/// services object, so a component holding it does not re-render merely because
/// a fresh clone arrived. It is injected **separately** from
/// [`jeliya_client::ClientHandle`]; the two are never entangled.
#[derive(Clone)]
pub struct PlatformServices {
    inner: Arc<dyn PlatformServicesImpl>,
}

impl PlatformServices {
    /// Wrap a concrete implementation. Composition (`compose.rs`) selects the
    /// per-target implementation and injects it here.
    pub fn new(inner: Arc<dyn PlatformServicesImpl>) -> Self {
        Self { inner }
    }

    /// The browser-appropriate deterministic implementation
    /// ([`WebPlatformServices`]) wrapped for injection.
    pub fn web_default() -> Self {
        Self::new(Arc::new(WebPlatformServices::new()))
    }

    /// Read a persisted UI preference, or `None` if unset.
    pub fn preference(&self, key: &str) -> Option<String> {
        self.inner.preference(key)
    }

    /// Persist a UI preference under `key`.
    pub fn set_preference(&self, key: &str, value: &str) {
        self.inner.set_preference(key, value);
    }

    /// Open an external URL through the platform.
    pub fn open_url(&self, url: &str) {
        self.inner.open_url(url);
    }

    /// Write plain text to the platform clipboard.
    pub fn write_clipboard(&self, text: &str) {
        self.inner.write_clipboard(text);
    }

    /// Navigate the in-app route.
    pub fn navigate(&self, route: &str) {
        self.inner.navigate(route);
    }

    /// The current in-app route.
    pub fn route(&self) -> String {
        self.inner.route()
    }
}

impl PartialEq for PlatformServices {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl std::fmt::Debug for PlatformServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformServices").finish_non_exhaustive()
    }
}

/// The deterministic in-process implementation used by the web foundation and
/// by tests.
///
/// It is clock-free, network-free, and has no side effects that leave the
/// process: preferences, clipboard, and navigation are held in memory, and
/// "opening a URL" is recorded, not performed. This is the browser-appropriate
/// analogue of #174's required deterministic test implementations.
#[derive(Default)]
pub struct WebPlatformServices {
    state: Mutex<ServicesState>,
}

#[derive(Default)]
struct ServicesState {
    preferences: BTreeMap<String, String>,
    clipboard: Option<String>,
    opened_urls: Vec<String>,
    route: Option<String>,
}

impl WebPlatformServices {
    /// A fresh, empty deterministic services implementation.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last text written to the clipboard, if any. For tests.
    pub fn last_clipboard(&self) -> Option<String> {
        self.state
            .lock()
            .expect("services poisoned")
            .clipboard
            .clone()
    }

    /// Every URL an `open_url` recorded, in order. For tests.
    pub fn opened_urls(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("services poisoned")
            .opened_urls
            .clone()
    }
}

impl PlatformServicesImpl for WebPlatformServices {
    fn preference(&self, key: &str) -> Option<String> {
        self.state
            .lock()
            .expect("services poisoned")
            .preferences
            .get(key)
            .cloned()
    }

    fn set_preference(&self, key: &str, value: &str) {
        self.state
            .lock()
            .expect("services poisoned")
            .preferences
            .insert(key.to_owned(), value.to_owned());
    }

    fn open_url(&self, url: &str) {
        self.state
            .lock()
            .expect("services poisoned")
            .opened_urls
            .push(url.to_owned());
    }

    fn write_clipboard(&self, text: &str) {
        self.state.lock().expect("services poisoned").clipboard = Some(text.to_owned());
    }

    fn navigate(&self, route: &str) {
        self.state.lock().expect("services poisoned").route = Some(route.to_owned());
    }

    fn route(&self) -> String {
        self.state
            .lock()
            .expect("services poisoned")
            .route
            .clone()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_deterministically() {
        let services = PlatformServices::web_default();
        assert_eq!(services.preference("last_room"), None);
        services.set_preference("last_room", "r-42");
        assert_eq!(services.preference("last_room").as_deref(), Some("r-42"));
    }

    #[test]
    fn clone_is_the_same_services_object() {
        let services = PlatformServices::web_default();
        let clone = services.clone();
        // Pointer identity: a clone handed to a child is the same object.
        assert_eq!(services, clone);
        clone.set_preference("k", "v");
        assert_eq!(services.preference("k").as_deref(), Some("v"));
    }

    #[test]
    fn set_preference_overwrites_same_key() {
        let svc = WebPlatformServices::new();
        svc.set_preference("k", "first");
        svc.set_preference("k", "second");
        assert_eq!(svc.preference("k").as_deref(), Some("second"));
    }

    #[test]
    fn independent_preference_keys_are_independent() {
        let svc = WebPlatformServices::new();
        svc.set_preference("a", "alpha");
        svc.set_preference("b", "beta");
        assert_eq!(svc.preference("a").as_deref(), Some("alpha"));
        assert_eq!(svc.preference("b").as_deref(), Some("beta"));
        assert_eq!(svc.preference("c"), None);
    }

    #[test]
    fn open_url_is_recorded_in_order() {
        let svc = WebPlatformServices::new();
        assert!(svc.opened_urls().is_empty());
        svc.open_url("https://example.com");
        svc.open_url("https://example.org");
        assert_eq!(
            svc.opened_urls(),
            vec!["https://example.com", "https://example.org"]
        );
    }

    #[test]
    fn write_clipboard_overwrites_and_returns_last() {
        let svc = WebPlatformServices::new();
        assert_eq!(svc.last_clipboard(), None);
        svc.write_clipboard("first");
        svc.write_clipboard("second");
        assert_eq!(svc.last_clipboard().as_deref(), Some("second"));
    }

    #[test]
    fn navigate_sets_and_returns_route() {
        let svc = WebPlatformServices::new();
        // Before any navigation the route is an empty string.
        assert_eq!(svc.route(), "");
        svc.navigate("/rooms");
        assert_eq!(svc.route(), "/rooms");
        svc.navigate("/rooms/r-99");
        assert_eq!(svc.route(), "/rooms/r-99");
    }

    #[test]
    fn two_distinct_instances_are_not_equal() {
        let a = PlatformServices::web_default();
        let b = PlatformServices::web_default();
        // Equality is pointer identity — two fresh instances are distinct.
        assert_ne!(a, b);
    }
}
