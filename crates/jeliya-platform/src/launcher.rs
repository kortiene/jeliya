//! The external-URL launcher (#174 §D8, AC-4): an allowlisted, fail-closed safe
//! launcher.
//!
//! [`UrlLauncher::open_external`] opens a URL through the platform (a new
//! browser tab; an external application on native). It accepts only a
//! [`SafeExternalUrl`], a **validated newtype** constructed only via
//! [`SafeExternalUrl::parse`], which **fails closed** on any scheme outside a
//! fixed allowlist. A component cannot call `open_external` with a raw string,
//! so an un-vetted URL cannot reach the platform launcher, and a failed launch
//! is never a success — `open_external` returns a `Result` the UI surfaces.

use crate::error::CapabilityError;

/// The allowlisted URL schemes. `http` is intentionally excluded: the product
/// is loopback-first and external links are `https` / `mailto` / `tel` (Open
/// Question O-2). `javascript:`, `data:`, `file:`, `content:`, and every
/// unknown scheme are rejected.
const ALLOWED_SCHEMES: [&str; 3] = ["https", "mailto", "tel"];

/// A URL whose scheme has been checked against the allowlist and found safe.
///
/// The only constructor is [`SafeExternalUrl::parse`]; there is no way to build
/// one from an un-vetted string, so a component that holds a `SafeExternalUrl`
/// holds a URL the boundary has already vetted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SafeExternalUrl(String);

/// Why a string was rejected as an external URL: its scheme is not on the
/// allowlist. The rejected string is **not** carried, so a rejected
/// `javascript:` payload cannot leak into a log or error string.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("URL scheme is not on the external-launch allowlist")]
pub struct UnsafeUrl;

impl SafeExternalUrl {
    /// Parse and vet an external URL. Returns [`UnsafeUrl`] for any scheme
    /// outside the fixed allowlist (`https`, `mailto`, `tel`), for a
    /// scheme-relative or relative URL, and for an empty string.
    ///
    /// The scheme match is ASCII-case-insensitive (per URL syntax); everything
    /// after the scheme is left to the platform, which is the component that
    /// ultimately opens it.
    pub fn parse(url: &str) -> Result<Self, UnsafeUrl> {
        let scheme = url.split(':').next().unwrap_or("");
        // A URL must actually carry a scheme delimiter; `split(':')` yields the
        // whole string when there is none, so reject when no colon is present.
        if !url.contains(':') || scheme.is_empty() {
            return Err(UnsafeUrl);
        }
        let scheme_lower = scheme.to_ascii_lowercase();
        if ALLOWED_SCHEMES.contains(&scheme_lower.as_str()) {
            Ok(Self(url.to_owned()))
        } else {
            Err(UnsafeUrl)
        }
    }

    /// The vetted URL as a string, for the platform to open.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The external-URL launcher capability.
pub trait UrlLauncher {
    /// Open a vetted external URL through the platform. Returns `Err` (never a
    /// swallowed failure) when the platform could not open it, so the UI can
    /// surface the miss.
    fn open_external(&self, url: SafeExternalUrl) -> Result<(), CapabilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlisted_schemes_parse() {
        for url in [
            "https://example.com/path?q=1",
            "mailto:someone@example.com",
            "tel:+15555550100",
            "HTTPS://EXAMPLE.COM",
        ] {
            assert!(SafeExternalUrl::parse(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn dangerous_and_unknown_schemes_fail_closed() {
        for url in [
            "javascript:alert(1)",
            "data:text/html,<script>",
            "file:///etc/passwd",
            "content://com.example/doc/1",
            "http://insecure.example.com",
            "ftp://example.com",
            "//scheme-relative.example.com",
            "/relative/path",
            "not-a-url",
            "",
        ] {
            assert!(
                SafeExternalUrl::parse(url).is_err(),
                "{url} must be rejected"
            );
        }
    }

    #[test]
    fn the_error_does_not_carry_the_rejected_string() {
        let rendered = format!("{}", UnsafeUrl);
        assert!(!rendered.contains("javascript"));
        assert!(rendered.contains("allowlist"));
    }
}
