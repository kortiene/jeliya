//! `WebUrlLauncher` — the browser external-URL launcher (#181 §5.A, #174 §D8).
//!
//! Accepts only a vetted [`SafeExternalUrl`] (fail-closed on
//! `javascript:`/`data:`/`file:`/`content:` and any non-`https`/`mailto`/`tel`
//! scheme — enforced by the type, not here) and opens it in a new tab. A failed
//! open is surfaced, never swallowed.

use jeliya_platform::launcher::{SafeExternalUrl, UrlLauncher};
use jeliya_platform::{CapabilityError, FailureKind};

/// The browser [`UrlLauncher`]: `window.open(url, "_blank")`.
pub struct WebUrlLauncher;

impl UrlLauncher for WebUrlLauncher {
    fn open_external(&self, url: SafeExternalUrl) -> Result<(), CapabilityError> {
        let window = web_sys::window().ok_or(CapabilityError::Unavailable)?;
        match window.open_with_url_and_target(url.as_str(), "_blank") {
            Ok(Some(_)) => Ok(()),
            // A popup blocker or a failed open is a real miss the UI surfaces.
            Ok(None) | Err(_) => Err(CapabilityError::Failed(FailureKind::Io)),
        }
    }
}
