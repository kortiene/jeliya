//! Safe external-preview policy (spec D6): `declared_content_type` is untrusted
//! data, **never** a render authorization.
//!
//! Protocol `file.read` carries `declared_content_type` as an explicitly
//! untrusted field (v1's never-render-inline HTTP headers became *data*). This
//! module is a **pure classifier**: it never renders, never opens, and never
//! auto-loads peer content. It answers one question — may the Files pane offer a
//! *strictly inert, opt-in* preview affordance for this declared type, or is the
//! only safe action export/download? — and the answer is deliberately
//! conservative:
//!
//! - Peer bytes are **never inline-rendered** on the strength of the declared
//!   type. The default is export/download.
//! - A preview affordance is offered **only** for a small allowlist of *inert*
//!   types (plain text, raster images). Active markup (`text/html`, `image/svg+xml`,
//!   anything that can execute or fetch) is never on the allowlist, so a hostile
//!   `declared_content_type` cannot coax the pane into offering an active preview.
//! - Even for an allowlisted type the preview is *opt-in and sandboxed* — this
//!   classifier only decides whether the button exists, not whether bytes render.
//!
//! Any external open goes through `UrlLauncher::open_external(SafeExternalUrl)`
//! (fail-closed on `javascript:`/`data:`/`file:`/`content:`), never a raw string
//! (spec D6) — that is the launcher's job, not this classifier's.

/// What the Files pane may safely offer for a peer-declared content type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewPolicy {
    /// Export / download only. The default for *any* peer content, and the only
    /// option for active or unknown types.
    ExportOnly,
    /// A strictly inert, opt-in, sandboxed preview *may* be offered in addition
    /// to export/download. Never an auto-render, never active markup.
    SandboxedOptional,
}

impl PreviewPolicy {
    /// Whether an opt-in preview affordance may be shown at all.
    pub fn offers_preview(self) -> bool {
        matches!(self, PreviewPolicy::SandboxedOptional)
    }
}

/// The allowlist of *inert* essence types for which an opt-in sandboxed preview
/// may be offered. Deliberately tiny and free of any active/scriptable type:
/// `text/html` and `image/svg+xml` are **excluded** because they can execute or
/// fetch; everything not listed is export-only.
const INERT_ALLOWLIST: [&str; 5] = [
    "text/plain",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
];

/// The lowercased essence (`type/subtype`) of a declared content type, stripped
/// of any `; charset=…` parameter and surrounding whitespace. Untrusted input,
/// so it is only *matched*, never trusted to be well-formed.
fn essence(declared_content_type: &str) -> String {
    declared_content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Classify a peer-declared content type into a [`PreviewPolicy`]. Always safe:
/// an unrecognized, malformed, empty, or active type yields
/// [`PreviewPolicy::ExportOnly`].
pub fn classify(declared_content_type: &str) -> PreviewPolicy {
    if INERT_ALLOWLIST.contains(&essence(declared_content_type).as_str()) {
        PreviewPolicy::SandboxedOptional
    } else {
        PreviewPolicy::ExportOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_types_may_offer_an_optin_preview() {
        for declared in [
            "text/plain",
            "text/plain; charset=utf-8",
            "IMAGE/PNG",
            "  image/jpeg ",
            "image/gif",
            "image/webp",
        ] {
            assert_eq!(
                classify(declared),
                PreviewPolicy::SandboxedOptional,
                "{declared:?} is inert and allowlisted"
            );
        }
    }

    #[test]
    fn active_and_unknown_types_are_export_only() {
        // Active markup and scriptable/unknown types never offer a preview — a
        // hostile declared type cannot coax an active render (spec D6/R7).
        for declared in [
            "text/html",
            "image/svg+xml",
            "application/pdf",
            "application/octet-stream",
            "application/javascript",
            "video/mp4",
            "",
            "not-a-mime",
            "text/html; charset=utf-8",
        ] {
            assert_eq!(
                classify(declared),
                PreviewPolicy::ExportOnly,
                "{declared:?} must be export-only"
            );
        }
    }
}
