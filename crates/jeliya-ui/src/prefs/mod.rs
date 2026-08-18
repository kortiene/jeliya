//! The fresh, namespaced, versioned browser preference schema (#178 §6, D5).
//!
//! This is the pure "decision" half of `WebPreferences`: the namespace, the
//! schema version, the per-key envelope, the typed [`PreferenceKey`] →
//! backend-string derivation, and the corrupt/unsupported-version recovery
//! policy. The browser binding (`jeliya-platform-web`) is a thin
//! [`jeliya_platform::Preferences`] impl = this schema over an
//! [`backend::InMemoryBackend`], plus the boot-time legacy purge (§6.5).
//!
//! Clean-slate (D5): the namespace [`NAMESPACE`] is a prefix **no retiring
//! client ever wrote** (the React keys were bare `jeliya.lastRoom`,
//! `jeliya.aliases.v1`, …; the Flutter store was `app_prefs.json`), asserted
//! disjoint from [`legacy::LEGACY_WEB_KEYS`]. An unknown / malformed /
//! unsupported-version envelope is **never** interpreted as state — it triggers
//! a documented reset to the deterministic default with a visible recovery
//! affordance (§6.4).
//!
//! ## Deterministic first-run defaults (§6.6, AC-4)
//!
//! On a fresh backend every key is unset, so [`PreferenceSchema::get`] returns
//! `None` and each consumer applies its documented default:
//!
//! | Preference | Fresh default |
//! |---|---|
//! | `LastRoom` | unset → no restore |
//! | `Draft{room}` | `""` (empty composer) |
//! | `Aliases` | unset → self renders localized "You", peers render `shortId` |
//! | `SelfLabel` | unset → "You" |
//! | `Pinned{room}` / `Archived{room}` | `false` |
//! | `LastSeen{room}` | unset → seeded to room recency on first appearance |
//! | `TextLocale` | unset → platform preferred languages → English |
//! | `FormattingLocale` | unset → platform locale → resolved text locale |
//!
//! (The locale rows resolve through `LocaleState::resolve`, already implemented;
//! #178 adds only the switch UI that writes the two keys.)

pub mod backend;
pub mod legacy;

use std::cell::RefCell;

use jeliya_platform::PreferenceKey;

use backend::KeyValueBackend;

/// The browser preference namespace — a generation-marked prefix (`.v1`) no
/// retiring client ever wrote, so a legacy key can never collide with a fresh
/// one. The `.v1` is a **hard reset lever** (a breaking generation bumps it);
/// the finer-grained [`SCHEMA_VERSION`] lives in each value's envelope.
pub const NAMESPACE: &str = "jeliya.dx.v1";

/// The namespace with its trailing dot — every backend key starts with this.
pub const NAMESPACE_PREFIX: &str = "jeliya.dx.v1.";

/// The envelope schema version. Gates the envelope, not the namespace: a future
/// breaking schema bumps this and either migrates within the generation or
/// resets (never reads a foreign version as valid).
pub const SCHEMA_VERSION: u32 = 1;

/// Why a stored value could not be interpreted as state (#178 §6.4). Surfaced to
/// the boot layer, which renders the visible recovery banner. Carries the
/// derived backend key (not the raw value — §K1) and the reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SchemaAnomaly {
    /// The backend key whose value was rejected and removed.
    pub key: String,
    /// Why it was rejected.
    pub reason: AnomalyReason,
}

/// Why a stored envelope was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnomalyReason {
    /// The raw value did not parse as a versioned envelope.
    Corrupt,
    /// The envelope's version is newer than this build understands
    /// (`v > SCHEMA_VERSION`) — written by a future app the user downgraded from.
    UnsupportedNewer,
    /// The envelope's version is older than this build and no migration is
    /// defined (`v < SCHEMA_VERSION`).
    UnsupportedOlder,
}

/// The fresh, versioned preference schema over a raw [`KeyValueBackend`].
pub struct PreferenceSchema<B: KeyValueBackend> {
    backend: B,
    anomalies: RefCell<Vec<SchemaAnomaly>>,
}

impl<B: KeyValueBackend> PreferenceSchema<B> {
    /// Wrap `backend`. Does not scan; call [`PreferenceSchema::boot_scan`] once
    /// at startup to proactively surface corruption for the recovery banner
    /// before any read.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            anomalies: RefCell::new(Vec::new()),
        }
    }

    /// The raw backend, for the binding's boot-time legacy purge and tests.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The backend key a typed [`PreferenceKey`] derives to, under the
    /// namespace. Total and closed over the enum — a component cannot express a
    /// legacy or arbitrary key. Room ids are percent-encoded exactly as routes
    /// are (`encodeURIComponent`-identical), so an exotic id cannot break the
    /// key or collide with another.
    pub fn backend_key(key: &PreferenceKey) -> String {
        match key {
            PreferenceKey::LastRoom => format!("{NAMESPACE}.lastRoom"),
            PreferenceKey::Aliases => format!("{NAMESPACE}.aliases"),
            PreferenceKey::SelfLabel => format!("{NAMESPACE}.selfLabel"),
            PreferenceKey::TextLocale => format!("{NAMESPACE}.textLocale"),
            PreferenceKey::FormattingLocale => format!("{NAMESPACE}.formattingLocale"),
            PreferenceKey::Draft { room_id } => {
                format!("{NAMESPACE}.draft.{}", encode_component(room_id.as_str()))
            }
            PreferenceKey::Pinned { room_id } => {
                format!("{NAMESPACE}.pinned.{}", encode_component(room_id.as_str()))
            }
            PreferenceKey::Archived { room_id } => {
                format!(
                    "{NAMESPACE}.archived.{}",
                    encode_component(room_id.as_str())
                )
            }
            PreferenceKey::LastSeen { room_id } => {
                format!(
                    "{NAMESPACE}.lastSeen.{}",
                    encode_component(room_id.as_str())
                )
            }
            PreferenceKey::PipeConnections { room_id } => {
                format!(
                    "{NAMESPACE}.pipeConnections.{}",
                    encode_component(room_id.as_str())
                )
            }
        }
    }

    /// Read a preference value, applying per-key recovery: a corrupt or
    /// unsupported-version envelope is **not** interpreted — the offending key
    /// is removed (so it cannot poison a later read), a [`SchemaAnomaly`] is
    /// recorded, and the deterministic default (`None`) is returned.
    pub fn get(&self, key: &PreferenceKey) -> Option<String> {
        let backend_key = Self::backend_key(key);
        let raw = self.backend.get(&backend_key)?;
        match parse_envelope(&raw) {
            EnvelopeParse::Ok(value) => Some(value),
            EnvelopeParse::Anomaly(reason) => {
                self.backend.remove(&backend_key);
                self.record(SchemaAnomaly {
                    key: backend_key,
                    reason,
                });
                None
            }
        }
    }

    /// Write a preference value as a versioned envelope.
    pub fn set(&self, key: &PreferenceKey, value: &str) {
        self.backend
            .set(&Self::backend_key(key), &write_envelope(value));
    }

    /// Remove a preference.
    pub fn remove(&self, key: &PreferenceKey) {
        self.backend.remove(&Self::backend_key(key));
    }

    /// Proactively validate every namespace key at startup: remove and record
    /// each corrupt/unsupported one, so the recovery banner appears on boot
    /// without waiting for a read. Returns the anomalies found this scan (also
    /// accumulated in [`PreferenceSchema::take_anomalies`]).
    pub fn boot_scan(&self) -> Vec<SchemaAnomaly> {
        let mut found = Vec::new();
        for backend_key in self.backend.keys() {
            if !backend_key.starts_with(NAMESPACE_PREFIX) {
                continue;
            }
            let Some(raw) = self.backend.get(&backend_key) else {
                continue;
            };
            if let EnvelopeParse::Anomaly(reason) = parse_envelope(&raw) {
                self.backend.remove(&backend_key);
                let anomaly = SchemaAnomaly {
                    key: backend_key,
                    reason,
                };
                self.record(anomaly.clone());
                found.push(anomaly);
            }
        }
        found
    }

    /// Drain the anomalies recorded so far (by [`PreferenceSchema::get`] and
    /// [`PreferenceSchema::boot_scan`]). The boot layer drains these to decide
    /// whether to show the recovery banner.
    pub fn take_anomalies(&self) -> Vec<SchemaAnomaly> {
        std::mem::take(&mut self.anomalies.borrow_mut())
    }

    /// Whether any anomaly has been recorded (without draining).
    pub fn has_anomaly(&self) -> bool {
        !self.anomalies.borrow().is_empty()
    }

    /// Reset the whole namespace to defaults: remove every key under
    /// [`NAMESPACE_PREFIX`]. The explicit "reset local preferences" action the
    /// recovery banner offers (§6.4). Only touches namespace keys — never
    /// arbitrary storage.
    pub fn reset_all(&self) {
        for backend_key in self.backend.keys() {
            if backend_key.starts_with(NAMESPACE_PREFIX) {
                self.backend.remove(&backend_key);
            }
        }
        self.anomalies.borrow_mut().clear();
    }

    fn record(&self, anomaly: SchemaAnomaly) {
        self.anomalies.borrow_mut().push(anomaly);
    }
}

/// The outcome of parsing one stored envelope.
enum EnvelopeParse {
    /// A well-formed, current-version envelope: its `value`.
    Ok(String),
    /// A rejected envelope and why.
    Anomaly(AnomalyReason),
}

/// Parse `{ "v": <n>, "value": "<string>" }`, gating on [`SCHEMA_VERSION`].
fn parse_envelope(raw: &str) -> EnvelopeParse {
    let Some((version, value)) = decode_envelope(raw) else {
        return EnvelopeParse::Anomaly(AnomalyReason::Corrupt);
    };
    match version.cmp(&SCHEMA_VERSION) {
        std::cmp::Ordering::Equal => EnvelopeParse::Ok(value),
        std::cmp::Ordering::Greater => EnvelopeParse::Anomaly(AnomalyReason::UnsupportedNewer),
        std::cmp::Ordering::Less => EnvelopeParse::Anomaly(AnomalyReason::UnsupportedOlder),
    }
}

/// Encode one value as the versioned envelope string.
///
/// A hand-rolled two-field JSON object (the crate carries no `serde_json`, and
/// one is not worth a new edge in the audited wasm graph for a fixed shape): the
/// version is an integer, the value is a JSON-escaped string.
pub fn write_envelope(value: &str) -> String {
    format!(
        "{{\"v\":{SCHEMA_VERSION},\"value\":{}}}",
        json_string(value)
    )
}

/// Decode a versioned envelope into `(version, value)`, or `None` if it is not a
/// well-formed `{ "v": <int>, "value": "<string>" }` object.
///
/// A minimal, fixed-shape parser (no general JSON): it accepts insignificant
/// whitespace, requires exactly the two known fields in that order, and rejects
/// anything else — which is precisely the corruption gate the recovery policy
/// needs (an unknown or reordered shape is treated as corrupt and reset, never
/// guessed).
fn decode_envelope(raw: &str) -> Option<(u32, String)> {
    let raw = raw.trim();
    let inner = raw.strip_prefix('{')?.strip_suffix('}')?;
    let inner = inner.trim();
    let inner = inner.strip_prefix("\"v\"")?.trim_start();
    let inner = inner.strip_prefix(':')?.trim_start();
    // The version digits run up to the next comma.
    let comma = inner.find(',')?;
    let version: u32 = inner[..comma].trim().parse().ok()?;
    let inner = inner[comma + 1..].trim_start();
    let inner = inner.strip_prefix("\"value\"")?.trim_start();
    let inner = inner.strip_prefix(':')?.trim_start();
    let value = parse_json_string(inner)?;
    Some((version, value))
}

/// JSON-escape `s` into a quoted string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parse a leading JSON string literal from `s`, requiring it to be the whole
/// remaining (trimmed) input. Returns `None` on any malformed escape or trailing
/// junk — the corruption gate.
fn parse_json_string(s: &str) -> Option<String> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0c}'),
                'u' => {
                    let mut code = 0u32;
                    for _ in 0..4 {
                        code = code * 16 + chars.next()?.to_digit(16)?;
                    }
                    out.push(char::from_u32(code)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    // Nothing but the closing quote may remain.
    if chars.as_str().trim().is_empty() {
        Some(out)
    } else {
        None
    }
}

/// Percent-encode one key component with **exactly** `encodeURIComponent`'s
/// unreserved set (`A–Z a–z 0–9 - _ . ! ~ * ' ( )`) and uppercase hex — the same
/// discipline routes use, so a room id spells identically in a route and a
/// preference key and an exotic id cannot break or collide the key.
fn encode_component(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            other => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[usize::from(other >> 4)] as char);
                out.push(HEX[usize::from(other & 0xF)] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use backend::InMemoryBackend;
    use jeliya_api::RoomId;

    fn schema() -> PreferenceSchema<InMemoryBackend> {
        PreferenceSchema::new(InMemoryBackend::new())
    }

    #[test]
    fn namespace_is_disjoint_from_every_legacy_key() {
        for legacy in legacy::LEGACY_WEB_KEYS {
            assert!(
                !legacy.starts_with(NAMESPACE_PREFIX),
                "legacy key {legacy} collides with the fresh namespace"
            );
            assert!(!legacy::is_legacy_web_key(&format!("{NAMESPACE}.x")));
        }
    }

    #[test]
    fn key_derivation_is_total_and_namespaced() {
        let room = RoomId::new("blake3:abc");
        for key in [
            PreferenceKey::LastRoom,
            PreferenceKey::Aliases,
            PreferenceKey::SelfLabel,
            PreferenceKey::TextLocale,
            PreferenceKey::FormattingLocale,
            PreferenceKey::Draft {
                room_id: room.clone(),
            },
            PreferenceKey::Pinned {
                room_id: room.clone(),
            },
            PreferenceKey::Archived {
                room_id: room.clone(),
            },
            PreferenceKey::LastSeen { room_id: room },
        ] {
            let derived = PreferenceSchema::<InMemoryBackend>::backend_key(&key);
            assert!(derived.starts_with(NAMESPACE_PREFIX), "{derived}");
        }
    }

    #[test]
    fn room_id_is_percent_encoded_in_keys_like_routes() {
        let key = PreferenceKey::Draft {
            room_id: RoomId::new("blake3:abc"),
        };
        assert_eq!(
            PreferenceSchema::<InMemoryBackend>::backend_key(&key),
            "jeliya.dx.v1.draft.blake3%3Aabc"
        );
    }

    #[test]
    fn envelope_round_trips_arbitrary_values() {
        let schema = schema();
        for value in ["", "hello", "quote\"and\\slash", "line\nbreak", "é🚀"] {
            schema.set(&PreferenceKey::SelfLabel, value);
            assert_eq!(
                schema.get(&PreferenceKey::SelfLabel).as_deref(),
                Some(value),
                "value {value:?} did not round-trip"
            );
        }
    }

    #[test]
    fn fresh_backend_returns_none_for_every_key() {
        let schema = schema();
        assert_eq!(schema.get(&PreferenceKey::LastRoom), None);
        assert_eq!(schema.get(&PreferenceKey::TextLocale), None);
        assert!(!schema.has_anomaly());
    }

    #[test]
    fn a_corrupt_value_resets_to_default_removes_and_records() {
        let key = PreferenceKey::SelfLabel;
        let backend_key = PreferenceSchema::<InMemoryBackend>::backend_key(&key);
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([(
            backend_key.clone(),
            "not an envelope".to_owned(),
        )]));
        // Not interpreted as state (default None), removed, and recorded.
        assert_eq!(schema.get(&key), None);
        assert_eq!(schema.backend().get(&backend_key), None);
        let anomalies = schema.take_anomalies();
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].reason, AnomalyReason::Corrupt);
    }

    #[test]
    fn an_unsupported_newer_version_is_reset() {
        let key = PreferenceKey::TextLocale;
        let backend_key = PreferenceSchema::<InMemoryBackend>::backend_key(&key);
        let future = format!("{{\"v\":{},\"value\":\"fr\"}}", SCHEMA_VERSION + 1);
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([(backend_key, future)]));
        assert_eq!(schema.get(&key), None);
        assert_eq!(
            schema.take_anomalies()[0].reason,
            AnomalyReason::UnsupportedNewer
        );
    }

    #[test]
    fn boot_scan_surfaces_and_purges_corruption_before_any_read() {
        let key = PreferenceKey::LastRoom;
        let backend_key = PreferenceSchema::<InMemoryBackend>::backend_key(&key);
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([(
            backend_key.clone(),
            "{ bad ".to_owned(),
        )]));
        let found = schema.boot_scan();
        assert_eq!(found.len(), 1);
        assert_eq!(schema.backend().get(&backend_key), None);
    }

    #[test]
    fn reset_all_clears_only_the_namespace() {
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([(
            "some.other.app".to_owned(),
            "keepme".to_owned(),
        )]));
        schema.set(&PreferenceKey::SelfLabel, "Ada");
        schema.set(&PreferenceKey::TextLocale, "fr");
        schema.reset_all();
        assert_eq!(schema.get(&PreferenceKey::SelfLabel), None);
        assert_eq!(schema.get(&PreferenceKey::TextLocale), None);
        // A non-namespace key is untouched.
        assert_eq!(
            schema.backend().get("some.other.app"),
            Some("keepme".to_owned())
        );
    }

    #[test]
    fn an_unsupported_older_version_is_reset() {
        // v=0 with SCHEMA_VERSION=1 (no migration defined) → UnsupportedOlder.
        let key = PreferenceKey::SelfLabel;
        let backend_key = PreferenceSchema::<InMemoryBackend>::backend_key(&key);
        let old = "{\"v\":0,\"value\":\"legacy\"}".to_string();
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([(backend_key, old)]));
        assert_eq!(
            schema.get(&key),
            None,
            "an older-version value must not be interpreted"
        );
        assert_eq!(
            schema.take_anomalies()[0].reason,
            AnomalyReason::UnsupportedOlder,
        );
    }

    #[test]
    fn multiple_anomalies_accumulate_in_take_anomalies() {
        // Three distinct corrupt keys → three anomaly records drained at once.
        let room = RoomId::new("r-1");
        let k1 = PreferenceSchema::<InMemoryBackend>::backend_key(&PreferenceKey::SelfLabel);
        let k2 = PreferenceSchema::<InMemoryBackend>::backend_key(&PreferenceKey::TextLocale);
        let k3 = PreferenceSchema::<InMemoryBackend>::backend_key(&PreferenceKey::Draft {
            room_id: room.clone(),
        });
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([
            (k1, "not-json".to_owned()),
            (k2, "also-bad".to_owned()),
            (k3, "{}".to_owned()), // valid JSON, missing the required fields
        ]));
        // Trigger recovery for each key.
        let _ = schema.get(&PreferenceKey::SelfLabel);
        let _ = schema.get(&PreferenceKey::TextLocale);
        let _ = schema.get(&PreferenceKey::Draft { room_id: room });
        let anomalies = schema.take_anomalies();
        assert_eq!(anomalies.len(), 3, "all three anomalies must be recorded");
        // Drain is destructive — a second drain sees nothing.
        assert!(schema.take_anomalies().is_empty());
    }

    #[test]
    fn boot_scan_ignores_non_namespace_keys() {
        // A corrupt value under a foreign key must survive a boot scan untouched.
        let schema = PreferenceSchema::new(InMemoryBackend::seeded([
            ("some.other.app".to_owned(), "bad-envelope".to_owned()),
            ("jeliya.lastRoom".to_owned(), "legacy".to_owned()),
        ]));
        let found = schema.boot_scan();
        assert!(
            found.is_empty(),
            "non-namespace keys must not be touched by boot_scan"
        );
        // Both foreign keys are still present.
        assert_eq!(
            schema.backend().get("some.other.app"),
            Some("bad-envelope".to_owned())
        );
        assert_eq!(
            schema.backend().get("jeliya.lastRoom"),
            Some("legacy".to_owned())
        );
    }

    #[test]
    fn decode_envelope_tolerates_insignificant_whitespace() {
        // The hand-rolled parser must accept spaces around fields —
        // no external JSON library is in scope.
        let padded = "{ \"v\" : 1 , \"value\" : \"hello world\" }";
        match parse_envelope(padded) {
            EnvelopeParse::Ok(v) => assert_eq!(v, "hello world"),
            EnvelopeParse::Anomaly(r) => panic!("expected Ok, got anomaly {r:?}"),
        }
    }

    #[test]
    fn encode_component_percent_encodes_special_bytes() {
        // Every unreserved character in encodeURIComponent passes through as-is.
        assert_eq!(encode_component("AZaz09-_.!~*'()"), "AZaz09-_.!~*'()");
        // Reserved/special bytes are uppercased hex-escaped, matching the web shell.
        assert_eq!(encode_component("a:b"), "a%3Ab");
        assert_eq!(encode_component("a/b"), "a%2Fb");
        assert_eq!(encode_component("hello world"), "hello%20world");
        // Hash symbol (common in ids from other tools).
        assert_eq!(encode_component("a#b"), "a%23b");
        // A multibyte UTF-8 scalar is encoded byte-by-byte, matching JS behaviour.
        // '©' is U+00A9 → UTF-8 [0xC2, 0xA9] → %C2%A9.
        assert_eq!(encode_component("©"), "%C2%A9");
    }

    #[test]
    fn write_envelope_produces_valid_json_that_decode_accepts() {
        // Round-trip through the raw encode/decode pair for values with special chars.
        for value in [
            "",
            "simple",
            "quote\"here",
            "back\\slash",
            "\nnewline",
            "é🚀",
        ] {
            let encoded = write_envelope(value);
            match parse_envelope(&encoded) {
                EnvelopeParse::Ok(decoded) => assert_eq!(decoded, value, "failed for {value:?}"),
                EnvelopeParse::Anomaly(r) => {
                    panic!("write_envelope produced anomaly for {value:?}: {r:?}")
                }
            }
        }
    }
}
