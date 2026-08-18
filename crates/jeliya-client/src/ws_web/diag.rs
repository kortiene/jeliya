//! Browser-safe diagnostics and token redaction (§6.6, AC-4).
//!
//! The driver never builds a diagnostic from the raw URL, the token, the
//! ticket, the `cid`, or any payload bytes. Where a URL would otherwise be
//! rendered, [`RedactedUrl`] stands in and drops the entire query string — the
//! part that carries `token`/`ct`/`cid` — at construction, so no `Debug` /
//! `Display` can ever surface a credential.

/// A URL stripped of its credential-bearing query (and fragment) for
/// diagnostics. Only `scheme://host/path` is retained; the raw query is dropped
/// at construction and never stored in a renderable form.
pub(crate) struct RedactedUrl {
    safe: String,
}

impl RedactedUrl {
    /// Keep only the credential-free `scheme://host/path` prefix of `url`.
    pub(crate) fn new(url: &str) -> Self {
        let end = url.find(['?', '#']).unwrap_or(url.len());
        Self {
            safe: url[..end].to_string(),
        }
    }
}

impl std::fmt::Debug for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.safe)
    }
}

impl std::fmt::Display for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.safe)
    }
}

// Tests for RedactedUrl live here so they compile regardless of where the
// module is declared (the `ws_web` cfg gate does not matter for these because
// the type itself carries no wasm-only dependencies).
#[cfg(test)]
mod tests {
    use super::*;

    // §6.6 AC-4 mandatory test: "A unit test builds a RedactedUrl from a
    // token-bearing URL and asserts the rendering contains neither the token,
    // the ticket, nor the cid."

    #[test]
    fn redacted_url_strips_token_from_query() {
        let url = "ws://127.0.0.1:8080/ws?v=2&sg=1&token=super-secret-token";
        let r = RedactedUrl::new(url);
        let rendered = r.to_string();
        assert_eq!(rendered, "ws://127.0.0.1:8080/ws");
        assert!(
            !rendered.contains("super-secret-token"),
            "token must not appear"
        );
        assert!(
            !rendered.contains("token="),
            "token param name must not appear"
        );
    }

    #[test]
    fn redacted_url_strips_ct_ticket_from_query() {
        // The §6.4 ticket resolver uses `?ct=<ticket>` — must also be redacted.
        let url = "wss://localhost/ws?v=2&sg=3&ct=my-connect-ticket";
        let r = RedactedUrl::new(url);
        let rendered = r.to_string();
        assert_eq!(rendered, "wss://localhost/ws");
        assert!(
            !rendered.contains("my-connect-ticket"),
            "ticket must not appear"
        );
        assert!(!rendered.contains("ct="), "ct param must not appear");
    }

    #[test]
    fn redacted_url_strips_cid_from_query() {
        let url = "ws://127.0.0.1:1234/ws?v=2&sg=0&token=tok&cid=client-id-1";
        let r = RedactedUrl::new(url);
        let rendered = format!("{r}");
        assert!(!rendered.contains("client-id-1"), "cid must not appear");
        assert!(!rendered.contains("cid="), "cid param must not appear");
    }

    #[test]
    fn redacted_url_strips_fragment_only() {
        // A URL with no query but a fragment — the fragment is also stripped.
        let url = "https://host/path#section";
        let r = RedactedUrl::new(url);
        assert_eq!(r.to_string(), "https://host/path");
    }

    #[test]
    fn redacted_url_strips_query_and_fragment() {
        let url = "wss://host/ws?token=abc#fragment";
        let r = RedactedUrl::new(url);
        assert_eq!(r.to_string(), "wss://host/ws");
    }

    #[test]
    fn redacted_url_keeps_scheme_host_path_without_query() {
        let url = "https://example.com/api/health";
        let r = RedactedUrl::new(url);
        assert_eq!(r.to_string(), "https://example.com/api/health");
    }

    #[test]
    fn debug_and_display_both_omit_credential() {
        // Both format traits must redact; a stray {:?} in a log line is as
        // dangerous as Display.
        let url = "ws://127.0.0.1:9000/ws?token=secret&sg=1";
        let r = RedactedUrl::new(url);
        let via_display = format!("{r}");
        let via_debug = format!("{r:?}");
        for rendered in [&via_display, &via_debug] {
            assert!(
                !rendered.contains("secret"),
                "credential must not appear in {rendered:?}"
            );
            assert!(
                rendered.contains("ws://127.0.0.1:9000/ws"),
                "scheme/host/path must be present in {rendered:?}"
            );
        }
    }
}
