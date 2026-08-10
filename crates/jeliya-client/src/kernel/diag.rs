//! Redaction: secrets, tokens, `op_id`, and payload bytes never enter
//! diagnostics (§K15, Security).
//!
//! Bearer tokens, browser session tickets, `client_id`, `op_id`, and request
//! `in`/reply `out` payload bytes are **never** rendered. Diagnostics name the
//! *operation* (its `op` path) and *counts/limits* — never payload bytes or an
//! identifier that grants or correlates. Anywhere a payload or key would
//! otherwise be formatted, the [`Redacted`] wrapper stands in, so a stray
//! `{:?}` cannot leak it.

use std::fmt;

/// A `Debug`/`Display` wrapper that renders its inner value as `<redacted>`,
/// regardless of the wrapped type. Use it for any field — a payload, a token,
/// an `op_id` — that must never appear in a log line or error string.
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
        let secret = Redacted(String::from("op-id-42"));
        assert_eq!(format!("{secret}"), "<redacted>");
    }

    #[test]
    fn the_wrapped_bytes_are_never_present_in_the_rendering() {
        let payload = Redacted(vec![1u8, 2, 3, 4]);
        let rendered = format!("{payload:?}");
        assert!(!rendered.contains('1'));
        assert!(!rendered.contains('2'));
        assert_eq!(rendered, "<redacted>");
    }
}
