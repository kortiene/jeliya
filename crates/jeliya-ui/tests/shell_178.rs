//! Focused integration tests for #178 (bootstrap, shell, routing, preferences).
//!
//! These tests exercise behaviour that belongs at the integration level —
//! spanning source-file scans, cross-module constants, and the fake's recorded
//! effects — rather than in inline unit tests inside the modules under test.
//!
//! Run with:
//!   cargo test -p jeliya-ui --features ui --test shell_178

// ---- CSS parity: breakpoint constants match the stylesheet ----------------

/// The three responsive-shell media-query constant strings in `shell/mod.rs`
/// must appear verbatim in `ui/src/styles.css` (§8 / the retiring `shell.test.ts`
/// guard).  If a stylesheet edit drifts the breakpoints away from the Rust
/// constants, this test fails before any pixel ships.
#[test]
fn shell_media_query_constants_appear_verbatim_in_the_stylesheet() {
    let css = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/styles.css"
    ))
    .expect("ui/src/styles.css must be readable from the workspace root");

    for (name, query) in [
        ("COMPACT_MEDIA_QUERY", jeliya_ui::shell::COMPACT_MEDIA_QUERY),
        ("MEDIUM_MEDIA_QUERY", jeliya_ui::shell::MEDIUM_MEDIA_QUERY),
        ("WIDE_MEDIA_QUERY", jeliya_ui::shell::WIDE_MEDIA_QUERY),
    ] {
        assert!(
            css.contains(query),
            "the stylesheet does not contain the {name} string {query:?} — \
             breakpoint constant and CSS have drifted apart"
        );
    }
}

// ---- Source-scan: no legacy-key reader (AC-6) ----------------------------

/// No production source file in `src/` may contain a read (`getItem` or any
/// explicit read-path) of a known legacy React localStorage key.  The boot-time
/// purge is write-only (§6.5 / AC-6): it calls `remove_item`, never `getItem`.
///
/// This is the Rust analogue of the spec's "a source-scan test asserts no
/// legacy-key reader exists in the shipped shell" (§10.1, §9, §6.5).
#[test]
fn no_legacy_key_reader_exists_in_shell_source() {
    use jeliya_ui::prefs::legacy::LEGACY_WEB_KEYS;

    let src_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut offenders: Vec<String> = Vec::new();
    scan_for_legacy_readers(src_dir, LEGACY_WEB_KEYS, &mut offenders);

    assert!(
        offenders.is_empty(),
        "legacy-key reads found in shell source (AC-6 — the purge is write-only):\n{}",
        offenders.join("\n")
    );
}

fn scan_for_legacy_readers(
    dir: &std::path::Path,
    legacy_keys: &[&str],
    offenders: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_for_legacy_readers(&path, legacy_keys, offenders);
            continue;
        }
        if path.extension().is_some_and(|e| e == "rs") {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            for (line_num, line) in text.lines().enumerate() {
                // Skip pure comment lines.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                // A `get_item` call with a legacy key literal in the same line
                // is the read pattern we forbid.  Removal via `remove_item` is
                // allowed — the purge is write-only.
                for key in legacy_keys {
                    if line.contains("get_item") && line.contains(key) {
                        offenders.push(format!(
                            "{}:{} — get_item({key:?}) reads a legacy key",
                            path.display(),
                            line_num + 1,
                        ));
                    }
                }
                // Also catch the family prefix being used in a getItem context.
                for prefix in jeliya_ui::prefs::legacy::LEGACY_KEY_PREFIXES {
                    if line.contains("get_item") && line.contains(prefix) {
                        offenders.push(format!(
                            "{}:{} — get_item with legacy prefix {prefix:?}",
                            path.display(),
                            line_num + 1,
                        ));
                    }
                }
            }
        }
    }
}

// ---- navigate_replace records NavigatedReplace, not Navigated (D4) --------

/// The fake's `navigate_replace` must emit `NavigatedReplace` — a distinct
/// `RecordedEffect` variant — not `Navigated`. This keeps the canonicalizing-
/// redirect invariant ("Back never lands on a URL the user never typed") testable
/// by asserting the replace/push distinction in the recorded effect log.
#[test]
fn navigate_replace_records_navigated_replace_not_navigated() {
    use jeliya_platform::fake;
    use jeliya_platform::fake::RecordedEffect;
    use jeliya_platform::navigation::Route;

    let (services, controller) = fake::browser();
    let target = Route::Settings;

    // A `navigate` must record `Navigated`.
    services.navigation().navigate(target.clone());
    // A `navigate_replace` must record `NavigatedReplace`, not `Navigated`.
    services.navigation().navigate_replace(target.clone());

    let effects = controller.effects();
    assert_eq!(effects.len(), 2, "expected exactly two navigation effects");
    assert!(
        matches!(&effects[0], RecordedEffect::Navigated { route } if *route == target),
        "first effect must be Navigated, got {:?}",
        effects[0]
    );
    assert!(
        matches!(&effects[1], RecordedEffect::NavigatedReplace { route } if *route == target),
        "second effect must be NavigatedReplace (not Navigated), got {:?}",
        effects[1]
    );

    // The platform-facing accessors also distinguish the two:
    assert_eq!(controller.navigations(), vec![target.clone()]);
    assert_eq!(controller.replaced_navigations(), vec![target]);
}
