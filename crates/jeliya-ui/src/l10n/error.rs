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
    /// primary error surface. `terminal` selects copy that does NOT promise a
    /// recovery the shell will never perform (a refusal / timeout / decode
    /// failure) versus the retryable-disconnect copy that may.
    pub fn room_list_failure(strings: &dyn Catalog, terminal: bool) -> FriendlyError {
        FriendlyError {
            title: strings.err_room_list_title().to_string(),
            message: if terminal {
                strings.err_room_list_terminal_message().to_string()
            } else {
                strings.err_room_list_message().to_string()
            },
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
    // `whole_field` = the credential can contain spaces, so its value must be
    // redacted to a STRUCTURAL boundary, not the first whitespace. `Authorization`
    // is a scheme + payload (`Basic dXNlcjpwYXNz`): stopping at the space after the
    // scheme would leave the payload. `token`/`secret`/`bearer` are single tokens.
    for (marker, whole_field) in [
        ("token=", false),
        ("bearer ", false),
        ("authorization:", true),
        ("secret=", false),
    ] {
        while let Some(at) = out.to_ascii_lowercase().find(marker) {
            let start = at + marker.len();
            // Skip whitespace between the marker and the value (`Authorization:
            // "abc"`), then find the value's end. A QUOTED value must be redacted
            // THROUGH its closing quote: the opening quote is itself a delimiter,
            // so stopping at the first delimiter would strip only the marker and
            // leave the credential (`token="abc"` → the `"abc"` would survive).
            let mut cursor = start;
            while out[cursor..]
                .chars()
                .next()
                .is_some_and(|c| c == ' ' || c == '\t')
            {
                cursor += 1; // space/tab are one byte
            }
            let end = match out[cursor..].chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let after = cursor + quote.len_utf8();
                    // Find the UNESCAPED closing quote: a backslash escapes the next
                    // char, so a `\"` INSIDE the value (Debug output escapes embedded
                    // quotes) does NOT close it — otherwise `token="abc\"SECRET"`
                    // would stop at the escaped quote and leak `SECRET`.
                    let mut end = out.len();
                    let mut chars = out[after..].char_indices();
                    while let Some((offset, c)) = chars.next() {
                        if c == '\\' {
                            chars.next(); // skip the escaped char
                        } else if c == quote {
                            end = after + offset + quote.len_utf8();
                            break;
                        }
                    }
                    end
                }
                _ if whole_field => {
                    // A whole-field credential (a scheme-based `Authorization`) may
                    // contain spaces AND value punctuation: a `Digest` field is
                    // quote- and comma-delimited internally
                    // (`username="a", response="b"`), so stopping at a `"`/`,` would
                    // leave the rest exposed. Redact through the actual FIELD
                    // terminator — a structural container close or end of line/input.
                    // QUOTE-AWARE, ESCAPE-AWARE: a `}`/`)`/newline INSIDE a quoted
                    // parameter (`username="a)"`) is part of the value, not the
                    // terminator; and a BACKSLASH-ESCAPED quote (`"a\""` — Debug
                    // output escapes embedded quotes) does NOT close the quote, so a
                    // following structural char stays inside the value. Track both,
                    // and stop only at an UNQUOTED terminator.
                    let chars: Vec<(usize, char)> = out[cursor..].char_indices().collect();
                    let mut in_quote: Option<char> = None;
                    let mut terminator = None;
                    let mut k = 0;
                    while k < chars.len() {
                        let (offset, c) = chars[k];
                        if let Some(q) = in_quote {
                            if c == '\\' {
                                k += 2; // skip the escaped char (an escaped quote never closes)
                                continue;
                            }
                            if c == q {
                                in_quote = None;
                            }
                        } else if c == '"' || c == '\'' {
                            in_quote = Some(c);
                        } else if matches!(c, '}' | ')' | '\n' | '\r') {
                            terminator = Some(cursor + offset);
                            break;
                        }
                        k += 1;
                    }
                    terminator.unwrap_or(out.len())
                }
                _ => {
                    let is_boundary =
                        |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '}' | ',' | ')');
                    out[cursor..]
                        .find(is_boundary)
                        .map(|offset| cursor + offset)
                        .unwrap_or(out.len())
                }
            };
            // Redact from the marker through the value (leaving surrounding
            // structure intact).
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
            ErrorDisplay::room_list_failure(&En, false).title,
            "Couldn’t load rooms"
        );
        assert_eq!(
            ErrorDisplay::room_list_failure(&Fr, false).title,
            "Échec du chargement des salons"
        );
        assert_ne!(
            ErrorDisplay::friendly_unknown(&En).message,
            ErrorDisplay::friendly_unknown(&Fr).message
        );
    }

    #[test]
    fn terminal_and_retryable_room_list_copy_differ() {
        // The terminal message must NOT promise a retry the shell won't perform;
        // the retryable one may. They must be distinct copy in both locales.
        for cat in [&En as &dyn Catalog, &Fr as &dyn Catalog] {
            let retryable = ErrorDisplay::room_list_failure(cat, false).message;
            let terminal = ErrorDisplay::room_list_failure(cat, true).message;
            assert_ne!(
                retryable, terminal,
                "terminal room-list copy must differ from the retryable copy"
            );
        }
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

    #[test]
    fn scrub_secrets_redacts_quoted_and_spaced_credentials() {
        // A QUOTED value: the opening quote is a delimiter, so a naive
        // stop-at-first-delimiter left the credential behind (`"abc123secret"`).
        for input in [
            r#"token="abc123secret""#,
            r#"secret='abc123secret'"#,
            r#"Authorization: "abc123secret""#,
            "token=abc123secret trailing",
            // Scheme-based (unquoted) Authorization: the WHOLE field is the
            // credential — stopping at the space after `Basic` would leak the payload.
            "Authorization: Basic abc123secret, next",
        ] {
            let scrubbed = scrub_secrets(input.to_string());
            assert!(
                !scrubbed.contains("abc123secret"),
                "credential leaked from {input:?}: {scrubbed}"
            );
            assert!(
                scrubbed.contains("«redacted»"),
                "no redaction marker for {input:?}: {scrubbed}"
            );
        }
        // Surrounding structure is preserved.
        let s = scrub_secrets(r#"connect failed token="abc123secret" then done"#.to_string());
        assert!(s.contains("connect failed") && s.contains("done"), "{s}");
        // An ESCAPED quote inside a quoted value (Debug escapes embedded quotes)
        // does NOT close it — the value must be redacted through the UNESCAPED
        // closing quote, or the tail after `\"` leaks (a P1).
        let escaped = scrub_secrets(r#"token="abc\"VISIBLE_SECRET" then done"#.to_string());
        assert!(
            !escaped.contains("VISIBLE_SECRET"),
            "escaped-quote token leaked: {escaped}"
        );
        assert!(
            escaped.contains("done"),
            "structure after must survive: {escaped}"
        );
    }

    #[test]
    fn scrub_secrets_redacts_the_whole_digest_authorization_field() {
        // Digest credentials are quote- AND comma-delimited within ONE field, so a
        // whole-field redaction that stopped at the first `"`/`,` would leak the
        // username, nonce, and response. The whole field must be redacted through
        // its terminator (here, end of input).
        let s = scrub_secrets(
            r#"Authorization: Digest username="alice", nonce="n0nce", response="deadbeefcafe""#
                .to_string(),
        );
        assert!(!s.contains("alice"), "digest username leaked: {s}");
        assert!(!s.contains("n0nce"), "digest nonce leaked: {s}");
        assert!(!s.contains("deadbeefcafe"), "digest response leaked: {s}");
        assert!(s.contains("«redacted»"), "no redaction marker: {s}");
        // A trailing structural close still bounds the redaction.
        let bounded =
            scrub_secrets(r#"{Authorization: Digest response="deadbeefcafe"} tail"#.to_string());
        assert!(
            !bounded.contains("deadbeefcafe"),
            "bounded digest leaked: {bounded}"
        );
        assert!(
            bounded.contains("tail"),
            "structure after `}}` must survive: {bounded}"
        );
        // A `)` / `}` INSIDE a quoted parameter is part of the VALUE, not the field
        // terminator: the redaction must not stop there and leak the rest.
        let quoted = scrub_secrets(
            r#"Authorization: Digest username="a)}", response="deadbeefcafe""#.to_string(),
        );
        assert!(
            !quoted.contains("deadbeefcafe"),
            "quoted-delimiter digest leaked: {quoted}"
        );
        // A BACKSLASH-ESCAPED quote inside a value (Debug output escapes embedded
        // quotes) does not close the quote, so a following `)` stays inside the
        // value and must not terminate the redaction early.
        let escaped = scrub_secrets(
            r#"Authorization: Digest username="a\")", response="deadbeefcafe""#.to_string(),
        );
        assert!(
            !escaped.contains("deadbeefcafe"),
            "escaped-quote digest leaked: {escaped}"
        );
    }
}
