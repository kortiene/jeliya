//! The version axes the supervisor gates on.
//!
//! A daemon is a valid adoption/dial target iff its `protocol` **and**
//! `storage_generation` both equal the values this supervisor was built
//! against. The retired `schema` field is not consulted — it no longer exists,
//! and a v1 portfile is caught by this generation gate, not by a schema number.

use std::fmt;

/// The `{protocol, storage_generation}` pair this build speaks, injected at
/// construction rather than lifted from a `jeliya-core` constant so "what this
/// build was built against" is explicit and testable (spec §3.1 / OQ-1). The
/// caller reads its expected generation from a single `jeliya-api` source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Generation {
    /// The protocol generation (`2` for protocol-v2).
    pub protocol: u64,
    /// The storage generation.
    pub storage_generation: u64,
}

impl Generation {
    /// Construct an expected generation.
    pub fn new(protocol: u64, storage_generation: u64) -> Self {
        Self {
            protocol,
            storage_generation,
        }
    }

    /// Whether an observed generation matches this one on **both** axes. A
    /// missing axis (`None`) never matches: absence is refusal, never a default
    /// (spec §D1, mirroring protocol-v2 Layer 1's step-3/step-4 rule).
    pub(crate) fn matches(self, seen: SeenGeneration) -> bool {
        seen.protocol == Some(self.protocol)
            && seen.storage_generation == Some(self.storage_generation)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "protocol {} / sg {}",
            self.protocol, self.storage_generation
        )
    }
}

/// What a portfile or health response actually advertised. Either axis may be
/// absent — a pre-v2 daemon omits `storage_generation` entirely — and absence
/// is represented honestly as `None` rather than coerced to a sentinel, so the
/// error a UI shows can tell "declared the wrong generation" from "declared no
/// generation at all". Both are refusals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeenGeneration {
    /// The advertised protocol generation, or `None` if the field was absent.
    pub protocol: Option<u64>,
    /// The advertised storage generation, or `None` if the field was absent.
    pub storage_generation: Option<u64>,
}

impl fmt::Display for SeenGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |v: Option<u64>| v.map_or_else(|| "absent".to_owned(), |n| n.to_string());
        write!(
            f,
            "protocol {} / sg {}",
            show(self.protocol),
            show(self.storage_generation)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(p: u64, sg: u64) -> Generation {
        Generation::new(p, sg)
    }

    fn seen(p: Option<u64>, sg: Option<u64>) -> SeenGeneration {
        SeenGeneration {
            protocol: p,
            storage_generation: sg,
        }
    }

    #[test]
    fn matches_returns_true_when_both_axes_agree() {
        assert!(gen(2, 2).matches(seen(Some(2), Some(2))));
    }

    #[test]
    fn matches_returns_false_for_wrong_protocol() {
        assert!(!gen(2, 2).matches(seen(Some(1), Some(2))));
    }

    #[test]
    fn matches_returns_false_for_wrong_storage_generation() {
        assert!(!gen(2, 2).matches(seen(Some(2), Some(1))));
    }

    // Spec §D1: "absence is refusal, never a default" — a missing axis never matches.
    #[test]
    fn absent_protocol_is_refused_spec_d1() {
        assert!(!gen(2, 2).matches(seen(None, Some(2))));
    }

    #[test]
    fn absent_storage_generation_is_refused_spec_d1() {
        assert!(!gen(2, 2).matches(seen(Some(2), None)));
    }

    #[test]
    fn both_axes_absent_is_refused() {
        assert!(!gen(2, 2).matches(seen(None, None)));
    }

    #[test]
    fn generation_display_format() {
        assert_eq!(format!("{}", gen(2, 3)), "protocol 2 / sg 3");
    }

    #[test]
    fn seen_generation_display_shows_absent_for_none() {
        let s = seen(None, Some(5));
        let d = format!("{s}");
        assert!(
            d.contains("absent"),
            "None protocol should display as 'absent': {d}"
        );
        assert!(d.contains('5'), "present sg should display as '5': {d}");
    }

    #[test]
    fn seen_generation_display_shows_values_when_present() {
        assert_eq!(format!("{}", seen(Some(2), Some(2))), "protocol 2 / sg 2");
    }

    #[test]
    fn seen_generation_both_absent_display() {
        let d = format!("{}", seen(None, None));
        assert_eq!(d, "protocol absent / sg absent");
    }
}
