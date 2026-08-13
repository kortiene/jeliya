//! Redaction: room ids, positions, event ids, and counts are safe to name in
//! diagnostics; tokens, `op_id`, `client_id`, and payload bytes never are
//! (#169 §R16, Security).
//!
//! The reconciler reuses the kernel's posture (§K15): `room_id`, `pos`,
//! `event_id`, and counts are already in the wire model and safe to render;
//! bearer tokens, browser tickets, `client_id`, `op_id`, and payload bytes
//! (message bodies, file digests) are **never** rendered. Anywhere a payload or
//! key would otherwise be formatted, the [`Redacted`] wrapper stands in, so a
//! stray `{:?}` cannot leak it.

use std::fmt;

/// A `Debug`/`Display` wrapper that renders its inner value as `<redacted>`,
/// regardless of the wrapped type. Use it for any field — a message body, a
/// file digest, an `op_id` — that must never appear in a log line or error
/// string.
pub(crate) struct Redacted<T>(pub(crate) T);

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_inner_value() {
        let secret = Redacted("bearer-abc123");
        assert_eq!(format!("{secret:?}"), "<redacted>");
    }

    #[test]
    fn display_hides_the_inner_value() {
        let body = Redacted(String::from("a private message body"));
        assert_eq!(format!("{body}"), "<redacted>");
        assert!(!format!("{body}").contains("private"));
    }
}
