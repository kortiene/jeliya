//! The `PlatformServices` facade and the `Platform` supertrait (#174 §D1/§D2).
//!
//! [`PlatformServices`] is the **third separately injected input** to the
//! shared UI, beside `jeliya_api` view models and the
//! `jeliya_client::ClientHandle` seam. It is a concrete, cloneable struct
//! holding one `Arc<dyn Platform>`; cloning is an `Arc` bump and every clone
//! shares one implementation. `PartialEq` is pointer identity (a `clone()`
//! handed to a child is the same services object, so a component holding it
//! does not re-render merely because a fresh clone arrived) and `Debug` is
//! opaque — exactly the shape `jeliya_client::ClientHandle` uses, and exactly
//! what a Dioxus prop needs (`Clone + PartialEq`), provided without depending
//! on Dioxus.
//!
//! It is injected **separately** from `ClientHandle` (both are distinct
//! `AppRoot` props) and never entangled: a component that needs a call uses the
//! handle; a component that needs platform authority uses the services; neither
//! reaches through the other (§K11).

use std::sync::Arc;

use crate::clipboard::{Clipboard, Share};
use crate::files::Files;
use crate::launcher::UrlLauncher;
use crate::lifecycle::Lifecycle;
use crate::navigation::Navigation;
use crate::storage::{Preferences, PrivateDirectory, SecretStore};
use crate::window::WindowActions;

/// The bundled platform capability supertrait.
///
/// One `Arc<dyn Platform>` backs a [`PlatformServices`], so there is no partial
/// services object: a target that lacks a capability implements it as
/// `Unavailable` (the very fact the UI needs), never as an absent trait. Each
/// accessor returns `&dyn Trait`, so composition can swap a browser
/// implementation for a desktop one behind the same facade with no component
/// change — the concrete target type never leaks into shared components.
///
/// Object-safe: every method returns a `&dyn` trait object, so the erased
/// implementation stays behind `Arc<dyn Platform>`.
pub trait Platform {
    /// The files capability.
    fn files(&self) -> &dyn Files;
    /// The device-local preferences store.
    fn preferences(&self) -> &dyn Preferences;
    /// The secret custody store.
    fn secret_store(&self) -> &dyn SecretStore;
    /// The protected/private directory facts.
    fn private_directory(&self) -> &dyn PrivateDirectory;
    /// The lifecycle event source.
    fn lifecycle(&self) -> &dyn Lifecycle;
    /// The external-URL launcher.
    fn url_launcher(&self) -> &dyn UrlLauncher;
    /// The clipboard.
    fn clipboard(&self) -> &dyn Clipboard;
    /// The OS share sheet.
    fn share(&self) -> &dyn Share;
    /// The navigation state.
    fn navigation(&self) -> &dyn Navigation;
    /// The window actions.
    fn window(&self) -> &dyn WindowActions;
}

/// The injected platform-authority seam: cloneable, comparable, and opaque, so
/// it can be a Dioxus component prop.
///
/// It carries typed accessor methods (`files()`, `preferences()`, …) returning
/// `&dyn Trait`, so a component names a capability explicitly rather than
/// reaching a god-object.
#[derive(Clone)]
pub struct PlatformServices {
    inner: Arc<dyn Platform>,
}

impl PlatformServices {
    /// Wrap a concrete [`Platform`] implementation. Composition (the crate root
    /// `compose.rs` and the per-target `bin`) selects the implementation and
    /// injects it here.
    pub fn new(inner: Arc<dyn Platform>) -> Self {
        Self { inner }
    }

    /// The browser-shaped deterministic fake (session-scoped preferences; no
    /// window actions or private directory), ready to inject. Behind the `fake`
    /// feature.
    #[cfg(feature = "fake")]
    pub fn fake_browser() -> Self {
        crate::fake::browser().0
    }

    /// The desktop-shaped deterministic fake (persistent preferences; window
    /// actions and a private directory available), ready to inject. Behind the
    /// `fake` feature.
    #[cfg(feature = "fake")]
    pub fn fake_desktop() -> Self {
        crate::fake::desktop().0
    }

    /// The Android-shaped deterministic fake (persistent preferences; a
    /// protected private directory; no window actions; `content://` sources
    /// streamed through bounded staging), ready to inject. Behind the `fake`
    /// feature.
    #[cfg(feature = "fake")]
    pub fn fake_android() -> Self {
        crate::fake::android().0
    }

    /// The files capability.
    pub fn files(&self) -> &dyn Files {
        self.inner.files()
    }

    /// The device-local preferences store.
    pub fn preferences(&self) -> &dyn Preferences {
        self.inner.preferences()
    }

    /// The secret custody store.
    pub fn secret_store(&self) -> &dyn SecretStore {
        self.inner.secret_store()
    }

    /// The protected/private directory facts.
    pub fn private_directory(&self) -> &dyn PrivateDirectory {
        self.inner.private_directory()
    }

    /// The lifecycle event source.
    pub fn lifecycle(&self) -> &dyn Lifecycle {
        self.inner.lifecycle()
    }

    /// The external-URL launcher.
    pub fn url_launcher(&self) -> &dyn UrlLauncher {
        self.inner.url_launcher()
    }

    /// The clipboard.
    pub fn clipboard(&self) -> &dyn Clipboard {
        self.inner.clipboard()
    }

    /// The OS share sheet.
    pub fn share(&self) -> &dyn Share {
        self.inner.share()
    }

    /// The navigation state.
    pub fn navigation(&self) -> &dyn Navigation {
        self.inner.navigation()
    }

    /// The window actions.
    pub fn window(&self) -> &dyn WindowActions {
        self.inner.window()
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
