//! `WebShare` and `WebClipboard` — the browser share sheet and clipboard
//! (#181 §5.A, #174 §D9).
//!
//! - [`WebShare`] reports [`CapabilityError::Unavailable`] honestly: Web Share
//!   Level 2 file sharing is not broadly enough supported to ship as the primary
//!   path (spec Q4), so callers fall back to export/download rather than a silent
//!   no-op. (When the target's `navigator.canShare({ files })` becomes reliable,
//!   this is the one place that changes.)
//! - [`WebClipboard`] writes text through `navigator.clipboard.writeText`, whose
//!   returned promise settling is the only honest success signal — a permission
//!   denial resolves `Err(Denied)`, never a false `Ok`.

use jeliya_platform::clipboard::{Clipboard, Share, ShareContent};
use jeliya_platform::{BoxFuture, CancelToken, CapabilityError};

/// The browser [`Share`] — honest `Unavailable` fallback (spec Q4).
pub struct WebShare;

impl Share for WebShare {
    fn share(
        &self,
        _content: &ShareContent,
        _ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        Box::pin(async { Err(CapabilityError::Unavailable) })
    }
}

/// The browser [`Clipboard`] over `navigator.clipboard`.
pub struct WebClipboard;

impl Clipboard for WebClipboard {
    fn write_text(&self, text: &str) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let text = text.to_string();
        Box::pin(async move {
            let Some(window) = web_sys::window() else {
                return Err(CapabilityError::Unavailable);
            };
            let clipboard = window.navigator().clipboard();
            let promise = clipboard.write_text(&text);
            match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(_) => Ok(()),
                // A rejected promise is a permission denial (or an insecure
                // context) — never reported as a successful copy.
                Err(_) => Err(CapabilityError::Denied),
            }
        })
    }
}
