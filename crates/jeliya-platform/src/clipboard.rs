//! The clipboard and share capabilities (#174 §D9).
//!
//! - [`Clipboard::write_text`] — an **asynchronous** write whose returned
//!   future resolves to `Err` (typically [`CapabilityError::Denied`]) on
//!   failure, never `Ok`; the UI must not read a failed copy as success (today
//!   the settings panel clears the "copied" flag and raises a manual-copy note
//!   on failure — that honesty is preserved by resolving with the failure).
//!   The outcome is asynchronous by nature: on the browser it is known only
//!   when the `navigator.clipboard.writeText` promise settles (a permission
//!   denial is a promise rejection), so a synchronous signature could only
//!   fire-and-forget and falsely report `Ok`. Clipboard *read* is deliberately
//!   out of scope (no required behavior reads the clipboard); if added later it
//!   is a separate method with its own `Denied` path.
//! - [`Share::share`] — the OS share sheet. Dismissing it is
//!   [`CapabilityError::Cancelled`], not `Ok`.
//!
//! The daemon token stays native (§K5): a [`ShareContent`] carries a typed
//! [`LocalFileRef`] or [`ShareableBlob`], never a URL — no raw location is
//! representable in share content, and no token ever crosses this surface.

use crate::cancel::CancelToken;
use crate::error::CapabilityError;
use crate::files::{LocalFileRef, ShareableBlob};
use crate::BoxFuture;

/// An anchor rectangle for popover presentation of the share sheet (iPad).
/// Coordinates are in logical points relative to the presenting view.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShareAnchor {
    /// The anchor's left edge.
    pub x: f64,
    /// The anchor's top edge.
    pub y: f64,
    /// The anchor's width.
    pub width: f64,
    /// The anchor's height.
    pub height: f64,
}

/// A file attachment carried by [`ShareContent`].
///
/// Either a daemon-fetched local copy ([`LocalFileRef`], resolved to a
/// token-carrying URL inside the service) or a freshly staged
/// [`ShareableBlob`]. A raw path is not representable, so a component cannot
/// attach an arbitrary file.
#[derive(Clone, PartialEq, Debug)]
pub enum ShareAttachment {
    /// A daemon-fetched local copy.
    Local(LocalFileRef),
    /// A staged, daemon-shareable blob.
    Blob(ShareableBlob),
}

/// The content handed to the OS share sheet.
///
/// Carries optional text and/or a file [`ShareAttachment`], plus an optional
/// [`ShareAnchor`] for iPad popover presentation. At least one of `text` /
/// `attachment` is expected; an all-empty `ShareContent` is a caller error the
/// service surfaces as a failure rather than a silent success.
#[derive(Clone, PartialEq, Debug)]
pub struct ShareContent {
    /// Text to share, if any.
    pub text: Option<String>,
    /// A file to share, if any.
    pub attachment: Option<ShareAttachment>,
    /// The popover anchor, if the platform needs one.
    pub anchor: Option<ShareAnchor>,
}

impl ShareContent {
    /// Share plain text.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            attachment: None,
            anchor: None,
        }
    }

    /// Share a file attachment.
    pub fn attachment(attachment: ShareAttachment) -> Self {
        Self {
            text: None,
            attachment: Some(attachment),
            anchor: None,
        }
    }

    /// Attach a popover anchor (fluent).
    pub fn with_anchor(mut self, anchor: ShareAnchor) -> Self {
        self.anchor = Some(anchor);
        self
    }

    /// Whether this content is empty (no text and no attachment) — a caller
    /// error the service must not treat as a successful share.
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.attachment.is_none()
    }
}

/// The clipboard capability.
pub trait Clipboard {
    /// Write plain text to the platform clipboard. The returned future
    /// resolves to `Err` (typically [`CapabilityError::Denied`]), never `Ok`,
    /// on failure — and on the browser the outcome is known only when the
    /// `navigator.clipboard.writeText` promise settles, which is why this is
    /// a future and not a plain `Result`. No [`CancelToken`]: §D12 reserves
    /// tokens for user dialogs and bounded copies, and a clipboard write is
    /// neither.
    fn write_text(&self, text: &str) -> BoxFuture<'_, Result<(), CapabilityError>>;
}

/// The share capability (the OS share sheet).
pub trait Share {
    /// Present the share sheet for `content`. Resolves to `Ok(())` only when
    /// the platform accepted the share; a dismissed sheet is
    /// [`CapabilityError::Cancelled`], a refused target is
    /// [`CapabilityError::Denied`].
    fn share(
        &self,
        content: ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>>;
}
