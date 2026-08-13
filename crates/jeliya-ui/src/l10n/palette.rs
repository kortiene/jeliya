//! The Rust-facing identity-palette token source (§4-D3, §5.5).
//!
//! Dioxus renders every colour, radius, and elevation through CSS custom
//! properties in the byte-identical `ui/src/styles.css`, so this crate needs
//! **no** Rust duplicate of those values — that would be the exact drift
//! `assets/design-tokens.json` exists to prevent. The one token concept CSS
//! cannot express is the deterministic identity-palette **hash**: an opaque id
//! or filename in, a colour out, and the colour must be byte-identical across
//! clients "or the same person gets a different avatar colour per device"
//! (`docs/design-tokens.md`).
//!
//! These are pure ports of `ui/src/lib/format.ts`. They are pinned by the shared
//! `assets/identity-palette-fixture.json` (read by the test below and, while
//! React exists, by a TS mirror) so the two clients cannot drift. Wiring these
//! into avatar surfaces is a later slice; the foundation ships the function, the
//! fixture, and the parity test.

/// The avatar colour palette. Order is load-bearing — the hash indexes into it —
/// and must match `AVATAR_PALETTE` in `ui/src/lib/format.ts` exactly.
const AVATAR_PALETTE: [&str; 6] = [
    "#2fd6a4", "#6aa8f7", "#a78bfa", "#fb923c", "#f472b6", "#22d3ee",
];

// File-tint colours. Shared with CSS tokens where one exists; `#a78bfa` (violet)
// has no CSS token and is shared here, exactly as the TS comment notes.
const TINT_RED: &str = "#f26d6d";
const TINT_BLUE: &str = "#6aa8f7";
const TINT_ACCENT: &str = "#2fd6a4";
const TINT_VIOLET: &str = "#a78bfa";
const TINT_DIM: &str = "#8aa39d";

/// A deterministic avatar colour for an opaque id.
///
/// A byte-for-byte port of the JavaScript: a 32-bit `h = h*31 + code` FNV-ish
/// roll over the string's **UTF-16 code units** (JavaScript `charCodeAt`), then
/// `h % palette.len()`. Iterating UTF-16 (not bytes or `char`s) is what keeps a
/// non-ASCII id resolving to the same colour as the browser client.
pub fn color_for_id(id: &str) -> &'static str {
    let mut h: u32 = 0;
    for unit in id.encode_utf16() {
        // `Math.imul(h, 31)` is a 32-bit multiply keeping the low bits, and
        // `>>> 0` makes the sum an unsigned 32-bit int — `wrapping_*` on `u32`
        // reproduces both exactly.
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    AVATAR_PALETTE[(h % AVATAR_PALETTE.len() as u32) as usize]
}

/// The lowercased extension of a filename, or `""` when there is none.
fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(index) => name[index + 1..].to_ascii_lowercase(),
        None => String::new(),
    }
}

/// A tint colour for a filename, keyed on its extension. Unknown extensions get
/// the dim neutral — never a fabricated accent.
pub fn file_tint(name: &str) -> &'static str {
    match ext_of(name).as_str() {
        "pdf" => TINT_RED,
        "md" | "txt" | "doc" | "docx" => TINT_BLUE,
        "json" | "js" | "ts" => TINT_ACCENT,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => TINT_VIOLET,
        _ => TINT_DIM,
    }
}

/// The avatar-surface BACKGROUND for an id: its identity colour ([`color_for_id`])
/// at the avatar alpha `0x26`, as an `#rrggbbaa` string. This is the canonical
/// derivation of `${colorForId(id)}26` inlined by the React Sidebar
/// (`ui/src/components/Sidebar.tsx` — `background: `${colorForId(identityId)}26``),
/// so a Dioxus avatar surface reads ONE token source instead of recreating the
/// alpha at every call site (the cross-client seam this module exists to hold).
pub fn avatar_bg(id: &str) -> String {
    format!("{}26", color_for_id(id))
}

/// The room-tile BACKGROUND for an id: its identity colour ([`color_for_id`]) at
/// the tile alpha `0x1f`, as an `#rrggbbaa` string. The canonical derivation of
/// `${colorForId(id)}1f` inlined by the React Sidebar's `room-hex`
/// (`ui/src/components/Sidebar.tsx` — `background: `${tint}1f``), so a Dioxus room
/// tile derives it from the same token source rather than at the call site.
pub fn tile_bg(id: &str) -> String {
    format!("{}1f", color_for_id(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Read the shared fixture as text and extract one `"section": { "k":"v" }`
    /// map. A purpose-built parser for this file's controlled shape — the crate
    /// forbids `serde_json` (the raw-JSON boundary, asserted by
    /// `tests/boundaries.rs`), so the palette parity test cannot pull a JSON
    /// crate to read its own fixture. The fixture's keys and values are plain
    /// strings (raw UTF-8, no `\u` escapes), so a string-delimited scan is
    /// exact; `$comment` is skipped because it is not one of the two sections
    /// this reads.
    fn read_string_map(source: &str, section: &str) -> Vec<(String, String)> {
        let chars: Vec<char> = source.chars().collect();
        let needle: Vec<char> = format!("\"{section}\"").chars().collect();
        let mut i = find(&chars, &needle, 0)
            .unwrap_or_else(|| panic!("section {section:?} not found in fixture"))
            + needle.len();
        while chars[i] != '{' {
            i += 1;
        }
        i += 1;
        let mut out = Vec::new();
        loop {
            while chars[i].is_whitespace() || chars[i] == ',' {
                i += 1;
            }
            if chars[i] == '}' {
                break;
            }
            let (key, next) = parse_string(&chars, i);
            i = next;
            while chars[i].is_whitespace() {
                i += 1;
            }
            assert_eq!(chars[i], ':', "expected ':' after key in {section}");
            i += 1;
            while chars[i].is_whitespace() {
                i += 1;
            }
            let (value, next) = parse_string(&chars, i);
            i = next;
            out.push((key, value));
        }
        out
    }

    /// A JSON string beginning at `chars[start] == '"'`; returns the decoded
    /// value and the index past the closing quote.
    fn parse_string(chars: &[char], start: usize) -> (String, usize) {
        assert_eq!(chars[start], '"', "expected a JSON string");
        let mut i = start + 1;
        let mut out = String::new();
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    out.push(match chars[i + 1] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    i += 2;
                }
                '"' => return (out, i + 1),
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        panic!("unterminated string in fixture");
    }

    fn find(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
        (from..=haystack.len().saturating_sub(needle.len()))
            .find(|&start| haystack[start..start + needle.len()] == *needle)
    }

    fn fixture() -> String {
        // The fixture lives at the repository root, two levels up from the crate
        // manifest. Reading the shared file (not a copy) is the point: both
        // clients answer to the same bytes.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/identity-palette-fixture.json");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn avatar_colours_match_the_shared_fixture() {
        let source = fixture();
        let avatars = read_string_map(&source, "avatars");
        assert!(!avatars.is_empty(), "fixture must pin at least one avatar");
        for (id, expected) in avatars {
            assert_eq!(
                color_for_id(&id),
                expected,
                "avatar colour drift for id {id:?} — the browser client and this \
                 port would render different colours"
            );
        }
    }

    /// The avatar/tile BACKGROUNDS are the identity colour at fixed alphas
    /// (`0x26` avatar, `0x1f` tile), so pin them to the SAME shared fixture: each
    /// id's background must be its pinned avatar colour with the React alpha
    /// suffix. A drift in either the base colour or the alpha (recreated at a call
    /// site instead of derived here) fails, so downstream Dioxus surfaces have one
    /// canonical, cross-client-checked derivation.
    #[test]
    fn avatar_and_tile_backgrounds_match_the_shared_fixture() {
        let source = fixture();
        let avatars = read_string_map(&source, "avatars");
        assert!(!avatars.is_empty(), "fixture must pin at least one avatar");
        for (id, colour) in avatars {
            assert_eq!(
                avatar_bg(&id),
                format!("{colour}26"),
                "avatar_bg drift for id {id:?}"
            );
            assert_eq!(
                tile_bg(&id),
                format!("{colour}1f"),
                "tile_bg drift for id {id:?}"
            );
        }
    }

    /// The palette colours that ARE canonical design tokens must equal
    /// `assets/design-tokens.json` — otherwise a change to `accent`/`blue`/`red`/
    /// `ink-dim` there leaves these hardcoded copies (and the identity fixture,
    /// which validates only against them) silently divergent, exactly the drift
    /// the single-source token file exists to prevent. Read the shared tokens
    /// with the same dependency-free parser (the crate forbids `serde_json`).
    #[test]
    fn shared_colours_match_the_canonical_design_tokens() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/design-tokens.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let colors: std::collections::HashMap<String, String> =
            read_string_map(&source, "color").into_iter().collect();
        let token = |name: &str| -> &str {
            colors
                .get(name)
                .unwrap_or_else(|| panic!("design-tokens.json color.{name} missing"))
                .as_str()
        };
        // Each tint that shares a CSS token must equal that token's value.
        assert_eq!(TINT_ACCENT, token("accent"), "TINT_ACCENT vs color.accent");
        assert_eq!(TINT_BLUE, token("blue"), "TINT_BLUE vs color.blue");
        assert_eq!(TINT_RED, token("red"), "TINT_RED vs color.red");
        assert_eq!(TINT_DIM, token("ink-dim"), "TINT_DIM vs color.ink-dim");
        // The avatar palette shares accent and blue with the tokens too.
        assert!(
            AVATAR_PALETTE.contains(&token("accent")),
            "avatar palette must carry the canonical accent"
        );
        assert!(
            AVATAR_PALETTE.contains(&token("blue")),
            "avatar palette must carry the canonical blue"
        );
    }

    #[test]
    fn file_tints_match_the_shared_fixture() {
        let source = fixture();
        let tints = read_string_map(&source, "fileTints");
        assert!(!tints.is_empty(), "fixture must pin at least one file tint");
        for (name, expected) in tints {
            assert_eq!(file_tint(&name), expected, "file tint drift for {name:?}");
        }
    }
}
