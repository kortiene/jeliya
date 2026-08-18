//! The served maximum-file-size policy (#92, spec D3).
//!
//! The limit is a **served fact** — `Hello.limits.max_shared_file_bytes` — never
//! a compiled-in constant, a catalog number, or a baked catalog figure. No
//! `104857600` / `100 MiB` / `100 MB` literal appears anywhere in this stack;
//! the over-limit surface *interpolates* the served integer (rendered through
//! [`crate::l10n::format::Formats::bytes`] at the edge).
//!
//! Two things live here, both pure and host-testable:
//!
//! - [`SizeLimit`] — the served ceiling, with a **preflight** that refuses a
//!   known-oversize file *before any copy* (the client's preflight point, the
//!   sixth of the six enforcement points the size policy names; the daemon
//!   re-enforces at `stage_declared`/`stage_stream`).
//! - [`ServedLimit`] — the honest **absence** of a served limit (the seam does
//!   not yet surface it — spec R1). When absent, the UI runs **no** client
//!   preflight and lets the daemon re-enforce; it *never* substitutes a
//!   constant, because a hardcoded ceiling is the exact defect #92 closes.

/// The served shared-file size ceiling, read from
/// `jeliya_api::push::Limits::max_shared_file_bytes`.
///
/// A newtype so a component cannot confuse it with an arbitrary byte count, and
/// so the "served, never assumed" invariant is carried by construction: the only
/// way to obtain one is [`SizeLimit::new`] from a value the daemon served.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SizeLimit {
    max_shared_file_bytes: u64,
}

/// A file exceeds the served limit. Carries only byte counts (both safe to
/// render); no name, path, or id — the over-limit surface says "N of M", never
/// which file (§K1 / spec D7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OverLimit {
    /// The offending size (or the running total, for a streamed source).
    pub size: u64,
    /// The served ceiling the size exceeded.
    pub limit: u64,
}

impl SizeLimit {
    /// Wrap the served `max_shared_file_bytes`.
    pub fn new(max_shared_file_bytes: u64) -> Self {
        Self {
            max_shared_file_bytes,
        }
    }

    /// The served ceiling, for interpolation into the localized over-limit copy.
    pub fn max_bytes(self) -> u64 {
        self.max_shared_file_bytes
    }

    /// Preflight a known size against the served ceiling **before any copy**.
    ///
    /// A size equal to the limit is accepted (the ceiling is inclusive); a size
    /// strictly above it is refused with the served integers, so the surface can
    /// explain the limit rather than accuse the file of corruption (spec D3, "no
    /// false accusation").
    pub fn preflight(self, size: u64) -> Result<(), OverLimit> {
        if size > self.max_shared_file_bytes {
            Err(OverLimit {
                size,
                limit: self.max_shared_file_bytes,
            })
        } else {
            Ok(())
        }
    }
}

/// The served limit, or its **honest absence** (spec R1): the `ClientHandle`
/// seam does not yet surface `max_shared_file_bytes` (it arrives with
/// #171/#270's `Hello` capture). `None` means "no client preflight; the daemon
/// re-enforces" — it is **never** a licence to substitute a constant.
pub type ServedLimit = Option<SizeLimit>;

/// Preflight against a possibly-absent served limit. `Ok(())` when the limit is
/// unknown (defer to the daemon) or the size is within it; `Err` only when a
/// *served* limit is known and the size exceeds it.
pub fn preflight(limit: ServedLimit, size: u64) -> Result<(), OverLimit> {
    match limit {
        Some(limit) => limit.preflight(size),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_refuses_only_strictly_oversize() {
        let limit = SizeLimit::new(100);
        assert!(limit.preflight(99).is_ok());
        assert!(limit.preflight(100).is_ok(), "the ceiling is inclusive");
        assert_eq!(
            limit.preflight(101).unwrap_err(),
            OverLimit {
                size: 101,
                limit: 100
            }
        );
    }

    #[test]
    fn an_absent_served_limit_never_substitutes_a_constant() {
        // No served limit → no client preflight; the daemon re-enforces. A huge
        // size is admitted here rather than refused against an invented ceiling.
        assert!(preflight(None, u64::MAX).is_ok());
        // A served limit is honored.
        assert!(preflight(Some(SizeLimit::new(10)), 11).is_err());
    }

    #[test]
    fn max_bytes_accessor_returns_the_served_ceiling() {
        let limit = SizeLimit::new(1_048_576);
        assert_eq!(limit.max_bytes(), 1_048_576);
    }

    #[test]
    fn zero_size_file_does_not_trip_a_zero_limit() {
        // 0 > 0 is false so a zero-byte pick passes the size preflight.
        // The empty-file guard is a separate platform concern (FileEmpty).
        let limit = SizeLimit::new(0);
        assert!(
            limit.preflight(0).is_ok(),
            "0 bytes with a 0-byte ceiling is within the limit"
        );
    }
}
