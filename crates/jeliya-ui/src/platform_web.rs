//! The browser `Platform` bindings (#178 §5.A), behind the `web` feature.
//!
//! This is the **binding** half of the browser target: `web-sys` shims over the
//! History API (navigation), browser events (lifecycle), an in-memory
//! session-scoped preference/secret store, and the boot-time legacy purge. All
//! the *decisions* — route canonicalization, the preference envelope + recovery,
//! the legacy allowlist, the bootstrap fold — live in the host-testable
//! [`crate::shell`] / [`crate::prefs`] modules; this file holds only the shims.
//!
//! Per the architecture record's Decision-3 (no platform `cfg` in shared
//! components), every `web-sys` call is confined here and to the `web`-gated
//! parts of [`crate::compose`]. The spec's primary recommendation is a separate
//! `jeliya-platform-web` crate; the module form is the sanctioned fallback (spec
//! §3.1) — chosen here to reuse the existing `web` feature/build infrastructure
//! and keep the pure logic host-testable, which it already is.
//!
//! Out-of-scope capabilities (files, share, window, private directory, external
//! URLs, clipboard) are delegated to the browser-shaped deterministic fake —
//! their real browser implementations land with Files/Pipes (#181) and the live
//! transport (#171); until then the shell renders against the mock, and these
//! capabilities behave exactly as the current `fake_browser` composition does.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use jeliya_platform::clipboard::{Clipboard, Share};
use jeliya_platform::files::Files;
use jeliya_platform::launcher::UrlLauncher;
use jeliya_platform::lifecycle::{
    BackgroundPhase, Lifecycle, LifecycleBus, LifecycleEvent, LifecycleSubscription,
};
use jeliya_platform::navigation::{Navigation, Route};
use jeliya_platform::storage::{
    Durability, PreferenceKey, Preferences, PrivateDirectory, Secret, SecretKey, SecretStore,
    WriteOutcome,
};
use jeliya_platform::window::WindowActions;
use jeliya_platform::{Platform, PlatformServices};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::prefs::backend::InMemoryBackend;
use crate::prefs::{legacy, PreferenceSchema, SchemaAnomaly};

/// The browser [`Platform`]: real navigation / lifecycle / preferences /
/// secrets, with the remaining capabilities delegated to the browser fake.
pub struct WebPlatform {
    navigation: WebNavigation,
    lifecycle: WebLifecycle,
    preferences: WebPreferences,
    secrets: WebSecretStore,
    /// The browser-shaped deterministic fake, standing in for the out-of-scope
    /// capabilities (Files/Share/Window/PrivateDirectory/UrlLauncher/Clipboard)
    /// until their browser bindings land (#181/#171).
    fake: PlatformServices,
}

impl WebPlatform {
    /// Build the browser platform, wrapped in a ready-to-inject
    /// [`PlatformServices`]. Runs the boot-time legacy purge and the
    /// preference-schema boot scan, returning `(services, anomalies)` — the
    /// anomalies drive the visible recovery banner (§6.4).
    ///
    /// Not named `new` because it returns a facade + anomalies, not `Self`.
    pub fn build() -> (PlatformServices, Vec<SchemaAnomaly>) {
        let preferences = WebPreferences::new();
        let anomalies = preferences.schema().boot_scan();
        let platform = WebPlatform {
            navigation: WebNavigation,
            lifecycle: WebLifecycle::install(),
            preferences,
            secrets: WebSecretStore::default(),
            fake: PlatformServices::fake_browser(),
        };
        // The browser target is single-threaded, so an `Arc` over a non-`Send`/
        // `Sync` `WebPlatform` (it holds `web-sys` handles and `RefCell`s) is
        // correct here — the `PlatformServices` facade only needs `Clone`, never
        // `Send`. This mirrors why `jeliya-platform::BoxFuture` is deliberately
        // `!Send` for the web target.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(platform);
        (PlatformServices::new(inner), anomalies)
    }
}

impl Platform for WebPlatform {
    fn files(&self) -> &dyn Files {
        self.fake.files()
    }
    fn preferences(&self) -> &dyn Preferences {
        &self.preferences
    }
    fn secret_store(&self) -> &dyn SecretStore {
        &self.secrets
    }
    fn private_directory(&self) -> &dyn PrivateDirectory {
        // The browser has no data directory — the fake reports Unavailable.
        self.fake.private_directory()
    }
    fn lifecycle(&self) -> &dyn Lifecycle {
        &self.lifecycle
    }
    fn url_launcher(&self) -> &dyn UrlLauncher {
        self.fake.url_launcher()
    }
    fn clipboard(&self) -> &dyn Clipboard {
        // Delegated until the real `navigator.clipboard` binding lands; the shell
        // renders against the mock today.
        self.fake.clipboard()
    }
    fn share(&self) -> &dyn Share {
        self.fake.share()
    }
    fn navigation(&self) -> &dyn Navigation {
        &self.navigation
    }
    fn window(&self) -> &dyn WindowActions {
        // The browser has no OS window — the fake reports Unavailable.
        self.fake.window()
    }
}

/// The current `location.pathname`, for the router's mount canonicalization
/// (which needs the raw pre-parse path). Confined here; composition passes it in.
pub fn location_pathname() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_else(|| "/".to_owned())
}

// ---- Navigation (History API) -------------------------------------------

/// [`Navigation`] over the browser History API.
struct WebNavigation;

impl WebNavigation {
    /// The URL for `route`, preserving the current `search` and `hash` — query
    /// and fragment are not navigation state, and a redirect that dropped
    /// `?daemon=`/`?boot=` would repoint the daemon or unfixture the e2e suite
    /// (§5.A / the retiring `history.ts` contract).
    fn url_for(route: &Route) -> String {
        let path = route.to_path();
        let (search, hash) = web_sys::window()
            .map(|w| {
                let loc = w.location();
                (
                    loc.search().unwrap_or_default(),
                    loc.hash().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        format!("{path}{search}{hash}")
    }
}

impl Navigation for WebNavigation {
    fn route(&self) -> Route {
        let path = web_sys::window()
            .and_then(|w| w.location().pathname().ok())
            .unwrap_or_else(|| "/".to_owned());
        Route::parse(&path).unwrap_or(Route::Rooms)
    }

    fn navigate(&self, route: Route) {
        if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
            let url = Self::url_for(&route);
            let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&url));
        }
    }

    fn navigate_replace(&self, route: Route) {
        if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
            let url = Self::url_for(&route);
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
        }
    }

    fn hand_back_to_platform(&self) {
        // Web Back is the browser's native popstate; delegate to it.
        if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
            let _ = history.back();
        }
    }
}

// ---- Lifecycle (browser events) -----------------------------------------

/// [`Lifecycle`] over browser events. Owns one [`LifecycleBus`] and, at
/// construction, wires `popstate` → [`LifecycleEvent::NavigationRequested`],
/// `visibilitychange` → `Resumed`/`Backgrounded`, and `pagehide` →
/// `Backgrounded{Detached}`. The closures are leaked for the tab's lifetime,
/// exactly as the existing e2e hook does (`compose.rs`).
struct WebLifecycle {
    bus: Arc<LifecycleBus>,
}

impl WebLifecycle {
    fn install() -> Self {
        let bus = Arc::new(LifecycleBus::new());
        let Some(window) = web_sys::window() else {
            return Self { bus };
        };

        // popstate → NavigationRequested (browser Back/Forward/deep-link). The
        // path is mapped through the same fail-safe parse the router uses.
        {
            let bus = bus.clone();
            let closure = Closure::<dyn Fn()>::new(move || {
                let path = web_sys::window()
                    .and_then(|w| w.location().pathname().ok())
                    .unwrap_or_else(|| "/".to_owned());
                let route = Route::parse(&path).unwrap_or(Route::Rooms);
                bus.emit(LifecycleEvent::NavigationRequested { route });
            });
            let _ = window
                .add_event_listener_with_callback("popstate", closure.as_ref().unchecked_ref());
            closure.forget();
        }

        // visibilitychange → Resumed / Backgrounded{Hidden}.
        if let Some(document) = window.document() {
            let bus = bus.clone();
            let closure = Closure::<dyn Fn()>::new(move || {
                let hidden = web_sys::window()
                    .and_then(|w| w.document())
                    .map(|d| d.visibility_state() == web_sys::VisibilityState::Hidden)
                    .unwrap_or(false);
                let event = if hidden {
                    LifecycleEvent::Backgrounded {
                        phase: BackgroundPhase::Hidden,
                    }
                } else {
                    LifecycleEvent::Resumed
                };
                bus.emit(event);
            });
            let _ = document.add_event_listener_with_callback(
                "visibilitychange",
                closure.as_ref().unchecked_ref(),
            );
            closure.forget();
        }

        // pagehide → Backgrounded{Detached} (the tab is going away). The browser
        // has no ProcessRestored / window events — those stay native.
        {
            let bus = bus.clone();
            let closure = Closure::<dyn Fn()>::new(move || {
                bus.emit(LifecycleEvent::Backgrounded {
                    phase: BackgroundPhase::Detached,
                });
            });
            let _ = window
                .add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref());
            closure.forget();
        }

        Self { bus }
    }
}

impl Lifecycle for WebLifecycle {
    fn subscribe(&self) -> LifecycleSubscription {
        self.bus.subscribe()
    }
}

// ---- Preferences (session-scoped, versioned schema) ---------------------

/// [`Preferences`] = the fresh [`PreferenceSchema`] over an [`InMemoryBackend`]
/// (D3: nothing survives the tab), plus the boot-time legacy purge (§6.5).
struct WebPreferences {
    schema: PreferenceSchema<InMemoryBackend>,
}

impl WebPreferences {
    fn new() -> Self {
        // Boot-time legacy purge: REMOVE (never read) each enumerated legacy
        // React key from `localStorage`. Bounded to the closed allowlist.
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            // Exact keys.
            for key in legacy::LEGACY_WEB_KEYS {
                let _ = storage.remove_item(key);
            }
            // Prefix families (per-room drafts): enumerate localStorage keys and
            // remove only those the allowlist classifies as legacy — never the
            // fresh namespace, never arbitrary keys.
            if let Ok(length) = storage.length() {
                let mut to_remove = Vec::new();
                for i in 0..length {
                    if let Ok(Some(key)) = storage.key(i) {
                        if legacy::is_legacy_web_key(&key) {
                            to_remove.push(key);
                        }
                    }
                }
                for key in to_remove {
                    let _ = storage.remove_item(&key);
                }
            }
        }
        Self {
            schema: PreferenceSchema::new(InMemoryBackend::new()),
        }
    }

    fn schema(&self) -> &PreferenceSchema<InMemoryBackend> {
        &self.schema
    }
}

impl Preferences for WebPreferences {
    fn get(&self, key: &PreferenceKey) -> Option<String> {
        self.schema.get(key)
    }

    fn set(&self, key: PreferenceKey, value: &str) -> WriteOutcome {
        self.schema.set(&key, value);
        // The browser store is in-memory and dies with the tab (D3).
        WriteOutcome::SessionOnly
    }

    fn remove(&self, key: &PreferenceKey) -> WriteOutcome {
        self.schema.remove(key);
        WriteOutcome::SessionOnly
    }

    fn durability(&self) -> Durability {
        Durability::SessionScoped
    }
}

// ---- Secrets (in-memory, tab-scoped) ------------------------------------

/// [`SecretStore`] holding only the tab-scoped session credential and invite
/// tickets, in memory, dying with the tab. The daemon token never enters here
/// (§K5).
#[derive(Default)]
struct WebSecretStore {
    secrets: RefCell<BTreeMap<SecretKey, Secret>>,
}

impl SecretStore for WebSecretStore {
    fn get(&self, key: &SecretKey) -> Option<Secret> {
        self.secrets.borrow().get(key).cloned()
    }

    fn set(&self, key: SecretKey, secret: Secret) -> WriteOutcome {
        self.secrets.borrow_mut().insert(key, secret);
        WriteOutcome::SessionOnly
    }

    fn remove(&self, key: &SecretKey) -> WriteOutcome {
        self.secrets.borrow_mut().remove(key);
        WriteOutcome::SessionOnly
    }

    fn durability(&self) -> Durability {
        Durability::SessionScoped
    }
}
