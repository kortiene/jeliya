//! Friendly error copy and the raw-detail split (§5.7–5.8; i18n.md rule 3).
//!
//! Primary UI shows designed catalog copy; the raw code/detail lives only in the
//! Diagnostics disclosure. The daemon's own `message`/`hint` are technical
//! detail, never the designed sentence — translating diagnostics is a support
//! liability, and an unmapped code must not leak an English daemon string into
//! French primary copy. Unknown codes get the generic friendly lead while the
//! raw error is preserved for diagnostics.
//!
//! Nothing here reads a secret. [`ErrorDisplay::diagnostic_detail`] additionally
//! scrubs any daemon-token-shaped substring before the raw text can reach a DOM
//! attribute, honoring the secret boundary (Decision-7 / #159).

use jeliya_client::CallError;

use super::Catalog;

/// Plain-language title/message pair shared by every error surface. No raw
/// daemon text: those fields are catalog copy only.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FriendlyError {
    /// The designed heading.
    pub title: String,
    /// The designed body sentence.
    pub message: String,
}

/// Maps client/daemon errors to designed copy and to a scrubbed raw detail.
/// A namespace, not state — every function resolves from the catalog the caller
/// already holds, so a live language switch re-resolves error copy too.
pub struct ErrorDisplay;

impl ErrorDisplay {
    /// The friendly copy for a failed room-list read — the foundation's one
    /// primary error surface.
    pub fn room_list_failure(strings: &dyn Catalog) -> FriendlyError {
        FriendlyError {
            title: strings.err_room_list_title().to_string(),
            message: strings.err_room_list_message().to_string(),
        }
    }

    /// The generic friendly lead for an unrecognized failure. Never the daemon's
    /// own words.
    pub fn friendly_unknown(strings: &dyn Catalog) -> FriendlyError {
        FriendlyError {
            title: strings.err_unknown_title().to_string(),
            message: strings.err_unknown_message().to_string(),
        }
    }

    /// The raw, developer-facing detail for the Diagnostics disclosure. Kept in
    /// English (a diagnostic is pasted into an issue read by maintainers) and
    /// scrubbed of any secret-shaped substring before it can reach the DOM.
    pub fn diagnostic_detail(error: &CallError) -> String {
        scrub_secrets(format!("{error:?}"))
    }
}

/// Redact daemon-token / bearer-credential shaped substrings.
///
/// A `CallError` carries error codes, resource names, and execution flags — no
/// secret today — but this is the boundary the secret rule names, so it fails
/// closed: a `token=…`, `bearer …`, or `Authorization: …` fragment is replaced
/// with a redaction marker rather than trusted to never appear. Applied to
/// every string that reaches primary copy OR a DOM attribute via diagnostics.
pub fn scrub_secrets(text: String) -> String {
    let mut out = text;
    for marker in ["token=", "bearer ", "authorization:", "secret="] {
        while let Some(at) = out.to_ascii_lowercase().find(marker) {
            // Redact from the marker to the next whitespace/quote/brace — the
            // span a credential would occupy — leaving structure intact.
            let start = at + marker.len();
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '}' | ',' | ')'))
                .map(|offset| start + offset)
                .unwrap_or(out.len());
            out.replace_range(at..end, "«redacted»");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::{En, Fr};
    use jeliya_client::Execution;

    #[test]
    fn friendly_copy_switches_with_the_catalog() {
        assert_eq!(
            ErrorDisplay::room_list_failure(&En).title,
            "Couldn’t load rooms"
        );
        assert_eq!(
            ErrorDisplay::room_list_failure(&Fr).title,
            "Échec du chargement des salons"
        );
        assert_ne!(
            ErrorDisplay::friendly_unknown(&En).message,
            ErrorDisplay::friendly_unknown(&Fr).message
        );
    }

    #[test]
    fn diagnostic_detail_is_raw_but_scrubbed() {
        let error = CallError::Disconnected {
            execution: Execution::Unknown,
        };
        let detail = ErrorDisplay::diagnostic_detail(&error);
        // The raw variant name survives (it is what a maintainer needs)…
        assert!(detail.contains("Disconnected"));
        // …but nothing token-shaped would.
        let scrubbed = scrub_secrets("connect failed token=abc123secret next".to_string());
        assert!(!scrubbed.contains("abc123secret"));
        assert!(scrubbed.contains("«redacted»"));
        assert!(scrubbed.contains("connect failed"));
        assert!(scrubbed.contains("next"));
    }
}
