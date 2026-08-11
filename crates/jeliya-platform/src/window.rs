//! The window-actions capability (#174 §D11): meaningful on packaged desktop,
//! [`crate::Availability::Unavailable`] on browser
//! and Android.
//!
//! A component that offers a window control renders it as absent where the
//! capability is unavailable, driven by the returned outcome — **never** by a
//! `cfg(target_os = …)` fork in the component (§K10). The structural fact is
//! modelled by each fake shape.

use crate::error::{Availability, CapabilityError};

/// A window event on platforms that have windows (desktop).
///
/// [`WindowEvent::CloseRequested`] is a **control intent** the lifecycle
/// subscription never drops (§K8); a close restated while the previous request
/// is still undelivered is absorbed into it, so a stalled consumer holds at
/// most one pending close and answers it exactly once. The others are ordinary
/// and may be dropped under backpressure with a loss-visible marker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowEvent {
    /// The window gained focus.
    Focused,
    /// The window lost focus.
    Blurred,
    /// The window was resized.
    Resized {
        /// The new width in logical pixels.
        width: u32,
        /// The new height in logical pixels.
        height: u32,
    },
    /// The user asked to close the window. Terminal; delivered distinctly so a
    /// consumer can persist state before the window goes away.
    CloseRequested,
}

impl WindowEvent {
    /// Whether this is a control intent that must never be lost or coalesced.
    pub fn is_control(self) -> bool {
        matches!(self, WindowEvent::CloseRequested)
    }
}

/// A window command a component may request. A value form of the
/// [`WindowActions`] methods, so an intent can be recorded, forwarded, or
/// tested without invoking the platform.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WindowCommand {
    /// Minimize the window.
    Minimize,
    /// Set the window title.
    SetTitle(String),
    /// Request the window be closed (the app may intercept).
    RequestClose,
    /// Request the application exit (the explicit alternative to an unconsumed
    /// back gesture becoming an exit; see [`crate::navigation`]).
    RequestExit,
    /// Bring the window to the foreground.
    Focus,
}

/// The window-actions capability. Every method returns
/// [`CapabilityError::Unavailable`] where the platform has no window (browser
/// tab, Android).
pub trait WindowActions {
    /// Whether window actions are available on this platform.
    fn availability(&self) -> Availability;

    /// Minimize the window.
    fn minimize(&self) -> Result<(), CapabilityError>;

    /// Set the window title.
    fn set_title(&self, title: &str) -> Result<(), CapabilityError>;

    /// Request the window be closed (the app may intercept the request).
    fn request_close(&self) -> Result<(), CapabilityError>;

    /// Request the application exit. This is the **explicit** exit action; an
    /// unconsumed back gesture must resolve through here rather than silently
    /// becoming an exit.
    fn request_exit(&self) -> Result<(), CapabilityError>;

    /// Bring the window to the foreground.
    fn focus(&self) -> Result<(), CapabilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_requested_is_the_only_control_window_event() {
        assert!(WindowEvent::CloseRequested.is_control());
        assert!(!WindowEvent::Focused.is_control());
        assert!(!WindowEvent::Resized {
            width: 1,
            height: 1
        }
        .is_control());
    }
}
