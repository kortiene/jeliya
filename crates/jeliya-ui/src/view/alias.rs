//! Device-local aliases (§7.4, Q5): the per-identity name map stored under
//! [`PreferenceKey::Aliases`](jeliya_platform::PreferenceKey), and the pure
//! name-resolution the roster/fleet/agents renders every identity through.
//!
//! The map is **caller-serialized** (the storage contract holds one opaque
//! string). This crate declares no `serde` dependency of its own, so rather than
//! pull one into the audited wasm graph the format is a tiny, versioned,
//! escape-safe line codec: a `v1` header, then one `subject\tlabel` line per
//! entry, with `\\`, `\n`, and `\t` backslash-escaped so an opaque subject id or
//! a user label carrying any byte round-trips. A corrupt or unversioned value
//! parses to the **empty** map — never a crash and never a wrong alias — mirroring
//! the #178 corrupt-state-resets discipline.
//!
//! Resolution is deliberately pure and catalog-free: [`AliasMap::resolve`]
//! returns a [`ResolvedName`] whose *fallback wording* ("You" for the unlabelled
//! self, the short id for an unlabelled peer) is chosen by the display seam
//! (`status::self_name` / `status::peer_name`) so the fold stays free of l10n.

use std::collections::BTreeMap;

use jeliya_api::SubjectId;

/// The current alias-map schema version tag. Bumping it (a future migration)
/// makes an old value parse to empty rather than be misread.
const VERSION: &str = "v1";

/// A device-local map from subject id to a user-chosen label.
///
/// `BTreeMap` so serialization is deterministic (stable ordering), which keeps
/// a written-then-read round-trip byte-stable and makes the editor's output
/// diff-friendly.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AliasMap {
    entries: BTreeMap<String, String>,
}

impl AliasMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse the stored value. `None` (unset) or any corrupt/unversioned string
    /// yields the empty map — a wrong alias is worse than no alias.
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::new();
        };
        let mut lines = raw.split('\n');
        // The first line is the version header; anything else is corrupt.
        if lines.next() != Some(VERSION) {
            return Self::new();
        }
        let mut entries = BTreeMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            // Split on the FIRST unescaped tab into key / value, then unescape
            // each half. A line with no unescaped tab is corrupt — skip it.
            let Some((key, value)) = split_unescaped_tab(line) else {
                continue;
            };
            let (Some(key), Some(value)) = (unescape(key), unescape(value)) else {
                continue;
            };
            if !key.is_empty() {
                entries.insert(key, value);
            }
        }
        Self { entries }
    }

    /// Serialize to the stored value (the versioned line codec).
    pub fn serialize(&self) -> String {
        let mut out = String::from(VERSION);
        for (key, value) in &self.entries {
            out.push('\n');
            out.push_str(&escape(key));
            out.push('\t');
            out.push_str(&escape(value));
        }
        out
    }

    /// The alias set for `subject`, if any.
    pub fn get(&self, subject: &SubjectId) -> Option<&str> {
        self.entries.get(subject.as_str()).map(String::as_str)
    }

    /// Set (or, on an empty trimmed label, clear) the alias for `subject`.
    /// Mirrors the self-label rule: trim, empty clears, soft 40-char cap.
    pub fn set(&mut self, subject: &SubjectId, label: &str) {
        let label: String = label.trim().chars().take(40).collect();
        if label.is_empty() {
            self.entries.remove(subject.as_str());
        } else {
            self.entries.insert(subject.as_str().to_owned(), label);
        }
    }

    /// Remove any alias for `subject`.
    pub fn remove(&mut self, subject: &SubjectId) {
        self.entries.remove(subject.as_str());
    }

    /// Whether the map holds no aliases.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `(subject_id, label)` pairs in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Resolve a subject to a [`ResolvedName`], distinguishing self from peer so
    /// the display seam can pick the right fallback wording. `self_id` is the
    /// local identity (from the connection snapshot), `None` while unknown.
    pub fn resolve(&self, subject: &SubjectId, self_id: Option<&SubjectId>) -> ResolvedName {
        let is_self = self_id == Some(subject);
        match (is_self, self.get(subject)) {
            (true, Some(alias)) => ResolvedName::SelfAlias(alias.to_owned()),
            (true, None) => ResolvedName::SelfDefault,
            (false, Some(alias)) => ResolvedName::PeerAlias(alias.to_owned()),
            (false, None) => ResolvedName::PeerShort(short_subject(subject)),
        }
    }
}

/// A resolved identity name, with the fallback wording left to the display seam.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResolvedName {
    /// The local identity, with a device alias set.
    SelfAlias(String),
    /// The local identity with no alias — rendered "You" by the seam.
    SelfDefault,
    /// A peer with a device alias set.
    PeerAlias(String),
    /// A peer with no alias — rendered as its short id by the seam.
    PeerShort(String),
}

impl ResolvedName {
    /// Whether this names the local identity.
    pub fn is_self(&self) -> bool {
        matches!(self, ResolvedName::SelfAlias(_) | ResolvedName::SelfDefault)
    }
}

/// A short, human-scannable rendering of a subject id: the tail after any `:`
/// prefix, truncated — the same disambiguator discipline the room/settings
/// surfaces use.
pub fn short_subject(subject: &SubjectId) -> String {
    let raw = subject.as_str();
    let tail = raw.rsplit(':').next().unwrap_or(raw);
    tail.chars().take(12).collect()
}

/// Escape `\`, `\n`, `\t` so a value round-trips through the line codec.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// Reverse [`escape`]. Returns `None` on a dangling/invalid escape (corrupt).
fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Split `line` on its first **unescaped** tab into `(key, value)` halves (still
/// escaped). A tab preceded by an odd run of backslashes is escaped and does not
/// split. `None` when the line carries no unescaped tab.
fn split_unescaped_tab(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut backslashes = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\\' => backslashes += 1,
            b'\t' if backslashes.is_multiple_of(2) => {
                return Some((&line[..i], &line[i + 1..]));
            }
            _ => backslashes = 0,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(s: &str) -> SubjectId {
        SubjectId::new(s)
    }

    #[test]
    fn empty_and_corrupt_values_parse_to_empty() {
        assert!(AliasMap::parse(None).is_empty());
        assert!(AliasMap::parse(Some("")).is_empty());
        assert!(AliasMap::parse(Some("garbage-no-version")).is_empty());
        // A wrong version is not silently read as v1.
        assert!(AliasMap::parse(Some("v2\nblake3:a\tAda")).is_empty());
    }

    #[test]
    fn set_trims_soft_caps_and_empty_clears() {
        let mut map = AliasMap::new();
        map.set(&sid("blake3:a"), "  Ada  ");
        assert_eq!(map.get(&sid("blake3:a")), Some("Ada"));
        map.set(&sid("blake3:a"), "   ");
        assert_eq!(map.get(&sid("blake3:a")), None);
        let long = "x".repeat(60);
        map.set(&sid("blake3:b"), &long);
        assert_eq!(map.get(&sid("blake3:b")).unwrap().chars().count(), 40);
    }

    #[test]
    fn round_trips_through_serialize_and_parse() {
        let mut map = AliasMap::new();
        map.set(&sid("blake3:a"), "Ada");
        map.set(&sid("blake3:b"), "Grace");
        let encoded = map.serialize();
        let back = AliasMap::parse(Some(&encoded));
        assert_eq!(map, back);
        // Deterministic ordering makes the encoding stable.
        assert_eq!(map.serialize(), back.serialize());
    }

    #[test]
    fn tabs_newlines_and_backslashes_round_trip() {
        // An opaque id or label carrying delimiter-shaped bytes must survive.
        let mut map = AliasMap::new();
        map.set(&sid("weird\tid\\x"), "label\nwith\ttabs\\and\\slash");
        let back = AliasMap::parse(Some(&map.serialize()));
        assert_eq!(
            back.get(&sid("weird\tid\\x")),
            Some("label\nwith\ttabs\\and\\slash")
        );
    }

    #[test]
    fn resolution_distinguishes_self_from_peer() {
        let mut map = AliasMap::new();
        let me = sid("blake3:me");
        let peer = sid("blake3:peerabcdefghijklmnop");
        // No aliases: self is default, peer is its short id.
        assert_eq!(map.resolve(&me, Some(&me)), ResolvedName::SelfDefault);
        assert_eq!(
            map.resolve(&peer, Some(&me)),
            ResolvedName::PeerShort("peerabcdefgh".to_string())
        );
        // With aliases.
        map.set(&me, "Me");
        map.set(&peer, "Bob");
        assert_eq!(
            map.resolve(&me, Some(&me)),
            ResolvedName::SelfAlias("Me".to_string())
        );
        assert_eq!(
            map.resolve(&peer, Some(&me)),
            ResolvedName::PeerAlias("Bob".to_string())
        );
        // Unknown self id: a subject is treated as a peer.
        assert!(!map.resolve(&peer, None).is_self());
    }
}
