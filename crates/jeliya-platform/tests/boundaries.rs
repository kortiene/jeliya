//! Static assertions about jeliya-platform's own surface, run in CI. These are
//! the boundary invariants #174 §3 names — the dependency-graph exclusion, the
//! no-`serde_json::Value`-in-public-source rule, the no-platform-`cfg`-fork rule
//! for the contract surface (§K10), and the manifest scan — that a unit test
//! cannot otherwise express. They mirror
//! `jeliya-api`/`jeliya-client`/`jeliya-ui`'s boundary tests.

use std::process::Command;

/// The forbidden crates the contract graph must never reach. The full
/// native/transport/renderer family, exactly as the sibling crates ban it.
const BANNED_GRAPH_CRATES: [&str; 15] = [
    "iroh",
    "jeliya-core",
    "jeliyad",
    "jeliya-ffi",
    "quinn",
    "rustls",
    "tokio",
    "hickory",
    "wry",
    "tao",
    "openssl-sys",
    "native-tls",
    "tungstenite",
    "websocket",
    "dioxus",
];

fn tree(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .args(args)
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_lowercase()
}

fn assert_graph_clean(tree: &str, context: &str) {
    for banned in BANNED_GRAPH_CRATES {
        assert!(
            !tree.lines().any(|line| line.starts_with(banned)),
            "forbidden crate '{banned}' is reachable from the {context} graph:\n{tree}"
        );
    }
}

/// The **default** library graph (types only) must be free of every native and
/// renderer crate — the workspace MSRV `--all-targets` job compiles this crate
/// renderer-free and OpenSSL-free.
#[test]
fn default_library_graph_is_free_of_native_and_renderer_crates() {
    let tree = tree(&[
        "--locked",
        "-p",
        "jeliya-platform",
        "--no-default-features",
        "--edges",
        "no-dev",
        // `--prefix none` puts every package name at column 0; the default
        // indent format hides all but the root behind tree glyphs, which
        // `starts_with` would never match — the check would pass vacuously.
        "--prefix",
        "none",
    ]);
    assert_graph_clean(&tree, "default library");
}

/// The `fake`-feature graph (the surface `jeliya-ui` adopts) must be just as
/// clean: the fakes add no dependencies at all.
#[test]
fn fake_feature_graph_is_free_of_native_and_renderer_crates() {
    let tree = tree(&[
        "--locked",
        "-p",
        "jeliya-platform",
        "--no-default-features",
        "--features",
        "fake",
        "--edges",
        "no-dev",
        // Same format note as the default-graph test above.
        "--prefix",
        "none",
    ]);
    assert_graph_clean(&tree, "fake-feature");
}

/// The browser (`wasm32`) resolution of the `fake` graph — what a wasm UI links
/// — must also exclude every native crate. `cargo tree` resolves the target
/// graph without compiling it, so this runs even where the wasm std target is
/// not installed.
#[test]
fn wasm32_fake_graph_is_free_of_native_and_renderer_crates() {
    let tree = tree(&[
        "--locked",
        "-p",
        "jeliya-platform",
        "--no-default-features",
        "--features",
        "fake",
        "--target",
        "wasm32-unknown-unknown",
        "--edges",
        "normal",
        "--prefix",
        "none",
        "--no-dedupe",
    ]);
    assert_graph_clean(&tree, "wasm32 fake-feature");
}

/// The banned-crate checker itself must fail closed: a bare `tokio vX` line
/// (the `--prefix none` output shape) trips the assertion. Pins the contract
/// against a regression to a parser that only matches glyph-prefixed indent
/// output — which matches nothing at column 0 and passes vacuously.
#[test]
fn assert_graph_clean_detects_banned_crate() {
    let synthetic = "jeliya-platform v0.0.0\ntokio v1.47.1\n";
    assert!(
        std::panic::catch_unwind(|| assert_graph_clean(synthetic, "synthetic")).is_err(),
        "a bare banned-crate line must trip the checker"
    );
}

/// The retired v1 HTTP edge must not be reintroduced anywhere in this crate's
/// source: protocol v2 serves file bytes over the byte-stream framing
/// (`file.read`), so no `/api/files/local` URL — and no token-in-URL format
/// string — may exist for a service to resolve. Comment lines are skipped so
/// the rule may be described where it is enforced.
#[test]
fn no_retired_local_file_url_in_source() {
    let src_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut offenders = Vec::new();
    for path in rust_sources(src_dir) {
        let text = std::fs::read_to_string(&path).expect("readable source");
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("/api/files/local") || line.contains("token=") {
                offenders.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "retired local-file URL edge found in source: {offenders:?}"
    );
}

/// The crates permitted to reach the `implementation` factory surface.
///
/// Adding a name here is the deliberate, reviewable act that admits an M3–M5
/// target crate; nothing outside this list may enable the feature, depend on
/// the door crate, or spell the `implementation` path.
const IMPLEMENTATION_DOOR_CRATES: [&str; 1] = ["jeliya-platform-implementation"];

/// Whether a manifest admits the `implementation` factory surface.
///
/// Cargo has several spellings for the same thing and a boundary check must
/// catch all of them, so this evaluates **table by table** rather than judging
/// lines alone:
///
/// - the inline form, `jeliya-platform = { …, features = ["implementation"] }`;
/// - the split form, where the dependency name is the `[…]` header and
///   `features` is a separate line;
/// - the **renamed** split form, where the header names an alias and
///   `package = "jeliya-platform"` appears somewhere in the body — in either
///   order, which is why the table is buffered before it is judged;
/// - a `[features]` forward, including one through a renamed alias
///   (`ui = ["platform/implementation"]`), which is why aliases are collected
///   in a first pass.
///
/// Inline comments are stripped so prose about the rule cannot trip it. This is
/// a token scan and is deliberately not the only leg: the resolver-truth tests
/// below ask Cargo what it actually resolved and so cannot be out-spelled.
fn manifest_opens_the_implementation_door(manifest: &str) -> bool {
    // Pass 1: gather (header, body) tables of comment-stripped lines.
    let mut tables: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    for line in manifest.lines() {
        let code = line.split('#').next().unwrap_or("").trim().to_string();
        if code.starts_with('[') {
            tables.push((code, Vec::new()));
        } else if !code.is_empty() {
            tables
                .last_mut()
                .expect("a table is always open")
                .1
                .push(code);
        }
    }

    // Pass 2: every alias that resolves to the contract crate or its door.
    let mut aliases: Vec<String> = vec!["jeliya-platform".to_string()];
    for (header, body) in &tables {
        // `[dependencies.alias]` … `package = "jeliya-platform"`
        if let Some(alias) = header
            .trim_start_matches('[')
            .trim_end_matches(']')
            .rsplit('.')
            .next()
            .filter(|_| header.contains("dependencies"))
        {
            if body
                .iter()
                .any(|line| line.starts_with("package") && line.contains("jeliya-platform"))
            {
                aliases.push(alias.to_string());
            }
        }
        // `alias = { package = "jeliya-platform", … }`
        for line in body {
            if line.contains("package") && line.contains("jeliya-platform") {
                if let Some((name, _)) = line.split_once('=') {
                    let name = name.trim();
                    if !name.is_empty() && !name.contains(' ') {
                        aliases.push(name.to_string());
                    }
                }
            }
        }
    }

    for (header, body) in &tables {
        let table_is_platform = aliases.iter().any(|alias| header.contains(alias.as_str()))
            || body
                .iter()
                .any(|line| line.starts_with("package") && line.contains("jeliya-platform"));
        for line in std::iter::once(header).chain(body.iter()) {
            if !line.contains("implementation") {
                continue;
            }
            // A line naming both, an alias-qualified feature forward, or any
            // `implementation` inside a table that resolves to the contract.
            if line.contains("jeliya-platform")
                || table_is_platform
                || aliases
                    .iter()
                    .any(|alias| line.contains(&format!("{alias}/implementation")))
            {
                return true;
            }
        }
    }
    false
}

/// A copy of `text` with comments and string literals blanked out, so a token
/// scan matches **code**, never prose.
///
/// Comment-stripping alone is not enough here: `jeliya-ui`'s composition test
/// says "never forks the implementation" inside an assertion message, and a
/// boundary check that goes red on an English sentence is a check somebody
/// eventually deletes. Handles line and block comments, ordinary string
/// literals with escapes, raw strings with any number of hashes, and character
/// literals.
///
/// Character literals must be consumed even though a `char` cannot hold the
/// scanned token, because a `char` **can** hold a quote: on `'"'` a scanner
/// that skipped char literals would take the inner `"` for the start of a
/// string and blank out every following line up to the next quote — silently
/// hiding real code, including the very import this scan exists to catch. A
/// lifetime (`'a`, no closing quote) is left alone by refusing to run past a
/// newline.
fn code_only(text: &str) -> String {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Line comment.
        if c == '/' && bytes.get(i + 1) == Some(&'/') {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comment (nesting, as Rust allows).
        if c == '/' && bytes.get(i + 1) == Some(&'*') {
            let mut depth = 1;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == '/' && bytes.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == '*' && bytes.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    if bytes[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
            continue;
        }
        // Raw string: r, r#, r##, … followed by a quote.
        if c == 'r' {
            let mut hashes = 0;
            while bytes.get(i + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if bytes.get(i + 1 + hashes) == Some(&'"') {
                i += 2 + hashes;
                let closing: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let closing: Vec<char> = closing.chars().collect();
                while i < bytes.len() {
                    if bytes[i..].starts_with(closing.as_slice()) {
                        i += closing.len();
                        break;
                    }
                    if bytes[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                continue;
            }
        }
        // Character literal — consumed, never emitted. `'\''` and the escape
        // forms `'\n'`, `'\x41'`, `'\u{1f600}'` all end at the first
        // unescaped quote; a lifetime has none, so the scan bails at the
        // newline and leaves it alone.
        if c == '\'' {
            if let Some(end) = char_literal_end(&bytes, i) {
                i = end;
                continue;
            }
        }
        // Ordinary string literal.
        if c == '"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == '\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == '"' {
                    i += 1;
                    break;
                }
                if bytes[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// The index just past a character literal starting at `start`, or `None` when
/// this quote opens no literal — a lifetime, a loop label, or an unterminated
/// run.
///
/// The distinction matters and cannot be made by scanning for the next
/// apostrophe: `'retry: loop { … break 'retry; }` puts two apostrophes on one
/// line with real code between them, so a scan-to-next-quote would strip the
/// body — including an `implementation` import — and the boundary check would
/// pass on code that reaches the factories. A character literal is therefore
/// recognised only by its **shape**: an escape, or exactly one character that
/// closes immediately. Anything else is a label or a lifetime and is left in
/// place.
fn char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    match chars.get(start + 1)? {
        // An escape runs to its own closing quote: `'\''`, `'\n'`, `'\x41'`,
        // `'\u{1f600}'`. Skipping the escaped character first is what keeps
        // `'\''` from terminating on the quote it escapes.
        '\\' => {
            let mut i = start + 3;
            while let Some(c) = chars.get(i) {
                match c {
                    '\'' => return Some(i + 1),
                    '\n' => return None,
                    _ => i += 1,
                }
            }
            None
        }
        // One character, closing immediately — `'a'`, `'"'`, `'é'`.
        _ if chars.get(start + 2) == Some(&'\'') => Some(start + 3),
        // `'a`, `'static`, `'retry:` — a lifetime or a label.
        _ => None,
    }
}

/// Every workspace member directory beside this crate (any sibling holding a
/// `Cargo.toml`).
fn sibling_crate_dirs() -> Vec<std::path::PathBuf> {
    let crates_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(crates_dir).expect("readable crates dir") {
        let path = entry.expect("dir entry").path();
        if path.join("Cargo.toml").is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    dirs
}

/// The `[workspace.dependencies]` (and workspace target-dependency) table
/// bodies of a root manifest, with their headers, and nothing else.
///
/// The root manifest cannot be fed to
/// [`manifest_opens_the_implementation_door`] whole: its `members` line names
/// both `jeliya-platform` and `jeliya-platform-implementation`, so the checker
/// would report the workspace as permanently open. Only the dependency tables
/// can actually grant a feature.
fn workspace_dependency_tables(manifest: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in manifest.lines() {
        let code = line.split('#').next().unwrap_or("").trim();
        if code.starts_with('[') {
            inside = code.starts_with("[workspace.dependencies")
                || (code.starts_with("[workspace.target.") && code.contains(".dependencies"));
            if inside {
                out.push_str(code);
                out.push('\n');
            }
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// **No crate but the blessed door may reach the `implementation` factory
/// surface** (§K4).
///
/// This replaces a narrower check that scanned only `jeliya-ui`'s manifest, and
/// it is the boundary that actually holds under Cargo **feature unification**:
/// once any crate in a target binary enables `implementation`, the module is
/// compiled into the one `jeliya-platform` instance every crate links, so
/// "`jeliya-ui` does not enable the feature" stops meaning "`jeliya-ui` cannot
/// call the factories". Two things still hold, and this test asserts both:
/// only the door crate's manifest may open the door, and no shared crate may
/// spell the `implementation` path — which is why those factories are free
/// functions rather than inherent methods, since a free function cannot be
/// reached without naming its module.
///
/// Two further legs live in their own tests, because a manifest **token** scan
/// cannot see a `package = ` rename or a `[workspace.dependencies]` alias:
/// [`no_shared_ui_selection_reaches_the_implementation_surface`] asserts
/// resolver truth, and
/// [`the_contract_crate_never_enables_its_own_implementation_feature`] covers
/// the one manifest this test must allowlist.
#[test]
fn only_the_door_crate_reaches_the_implementation_surface() {
    let mut offenders = Vec::new();
    // The workspace root is not a sibling and no other test reads it, yet a
    // `[workspace.dependencies]` entry opens the door for every member that
    // writes `{ workspace = true }` — a spelling that carries no token of its
    // own. Slice the root down to its dependency tables first: `members` names
    // both tokens and would otherwise trip the checker forever.
    let root_manifest = std::fs::read_to_string(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Cargo.toml"
    )))
    .expect("readable workspace manifest");
    if manifest_opens_the_implementation_door(&workspace_dependency_tables(&root_manifest)) {
        offenders.push(
            "workspace Cargo.toml [workspace.dependencies] opens the door for every \
                   member"
                .to_string(),
        );
    }
    for dir in sibling_crate_dirs() {
        let crate_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf-8 crate dir");
        // The contract crate itself defines the surface; the door crate is
        // allowlisted by construction.
        if crate_name == "jeliya-platform" || IMPLEMENTATION_DOOR_CRATES.contains(&crate_name) {
            continue;
        }
        let manifest =
            std::fs::read_to_string(dir.join("Cargo.toml")).expect("readable member manifest");
        if manifest_opens_the_implementation_door(&manifest) {
            offenders.push(format!(
                "{crate_name}/Cargo.toml opens the implementation door"
            ));
        }
        // Every member is scanned, not just the ones whose manifest names
        // jeliya-platform: a member can reach the contract through a renamed or
        // inherited dependency whose own line carries neither token, so gating
        // the scan on the manifest text would skip exactly the crate that took
        // the trouble to hide. Comment- and string-stripping makes this free —
        // the token occurs in prose across the workspace and nowhere in code.
        for sub in ["src", "tests", "examples"] {
            for path in rust_sources(&dir.join(sub)) {
                let text = std::fs::read_to_string(&path).expect("readable member source");
                // Code only: the rule is described in prose — in comments and
                // in at least one assertion message — across these crates, and
                // describing it must not trip it.
                for (index, line) in code_only(&text).lines().enumerate() {
                    if line.contains("implementation") {
                        offenders.push(format!(
                            "{}:{} spells the implementation factory path",
                            path.display(),
                            index + 1
                        ));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "only {IMPLEMENTATION_DOOR_CRATES:?} may reach jeliya-platform's implementation \
         surface: {offenders:#?}"
    );
}

/// The manifest checker must fail closed on **both** Cargo spellings, and the
/// split-table form is exactly what a line-at-a-time check misses. Pins the
/// contract against a regression to the previous checker, which read only
/// `jeliya-ui/Cargo.toml` for a bare `implementation` substring and so passed
/// this fixture vacuously.
#[test]
fn the_manifest_checker_detects_both_door_spellings() {
    let inline = r#"
[dependencies]
jeliya-api = { path = "../jeliya-api" }
jeliya-platform = { path = "../jeliya-platform", features = ["implementation"] }
"#;
    let split_table = r#"
[dependencies.jeliya-platform]
path = "../jeliya-platform"
features = ["implementation"]
"#;
    let transitive = r#"
[dependencies]
jeliya-platform-implementation = { path = "../jeliya-platform-implementation" }
"#;
    let feature_table = r#"
[features]
ui = ["jeliya-platform/implementation"]
"#;
    // The renamed split form: the header names an alias, and neither body line
    // carries both tokens. A line-at-a-time or header-only check misses it.
    let renamed_split = r#"
[dependencies.platform]
package = "jeliya-platform"
features = ["implementation"]
"#;
    // …and with the keys in the other order, since the table is judged whole.
    let renamed_split_reversed = r#"
[dependencies.platform]
features = ["implementation"]
package = "jeliya-platform"
"#;
    // A clean renamed dependency whose feature is forwarded from [features].
    let renamed_feature_forward = r#"
[dependencies.platform]
package = "jeliya-platform"

[features]
ui = ["platform/implementation"]
"#;
    let renamed_inline_door = r#"
[dependencies]
door = { package = "jeliya-platform-implementation", path = "../x" }
"#;
    for (label, fixture) in [
        ("inline features", inline),
        ("split dependency table", split_table),
        ("transitive door dependency", transitive),
        ("feature-table forward", feature_table),
        ("renamed split table", renamed_split),
        ("renamed split table, reversed keys", renamed_split_reversed),
        ("renamed feature forward", renamed_feature_forward),
        ("renamed inline door", renamed_inline_door),
    ] {
        assert!(
            manifest_opens_the_implementation_door(fixture),
            "the checker must trip on the {label} spelling"
        );
    }

    // …and must not trip on prose or on an ordinary contract dependency.
    let clean = r#"
# jeliya-ui must never enable jeliya-platform/implementation.
[dependencies]
jeliya-platform = { path = "../jeliya-platform", optional = true } # not implementation

[features]
ui = ["dep:jeliya-platform", "jeliya-platform/fake"]
"#;
    assert!(
        !manifest_opens_the_implementation_door(clean),
        "comments and ordinary contract dependencies must not trip the checker"
    );
}

/// The source scan must see **code and only code**: prose about the boundary —
/// in comments, in an assertion message, in a raw-string fixture — must not
/// trip it, while every import spelling that could actually reach the factories
/// must. The alias and brace-list forms are the ones a naive `implementation::`
/// substring check would miss.
#[test]
fn the_source_scan_sees_code_and_not_prose() {
    let prose = r##"
//! The implementation crate is the only door.
/// A component never forks the implementation.
/* implementation, described in a block comment */
fn main() {
    assert!(true, "prop cloning never forks the implementation");
    let _fixture = r#"features = ["implementation"]"#;
}
"##;
    assert!(
        !code_only(prose).contains("implementation"),
        "prose about the rule must not trip the scan:\n{}",
        code_only(prose)
    );

    for reaching in [
        "use jeliya_platform::implementation::shareable_blob;",
        "use jeliya_platform::implementation as door;",
        "use jeliya_platform::{files, implementation};",
        "let b = jeliya_platform::implementation::blob_token_from_raw(7);",
    ] {
        assert!(
            code_only(reaching).contains("implementation"),
            "a reaching spelling must trip the scan: {reaching}"
        );
    }

    // A `char` holding a QUOTE is the scanner's sharpest hazard: mistake it for
    // the start of a string and everything up to the next quote — real code
    // included — is blanked out, so the scan fails OPEN. The sanitizer arm
    // below is not contrived; `crates/jeliyad/src/serve.rs` already has one,
    // and a file-dialog surface is exactly where the next one will appear.
    for hiding in [
        "fn q(c: char) -> char { if c == '\"' { '_' } else { c } }\n\
         use jeliya_platform::implementation::shareable_blob;\n",
        "const Q: u8 = b'\"';\nuse jeliya_platform::implementation::shareable_blob;\n",
        "let c = '\\'';\nuse jeliya_platform::implementation::shareable_blob;\n",
        "let c = '\\u{1f600}';\nuse jeliya_platform::implementation::shareable_blob;\n",
        "match c { '/' | '\\\\' | ':' | '\"' => '_', o => o };\n\
         use jeliya_platform::implementation::shareable_blob;\n",
    ] {
        assert!(
            code_only(hiding).contains("implementation"),
            "a char literal must not blank out the import that follows it: {hiding:?}\n\
             stripped to: {:?}",
            code_only(hiding)
        );
    }

    // The same desync in the other direction: once falsely "inside" a string,
    // the next real string CLOSES it and its prose is emitted as code — the
    // false positive on English that this function exists to prevent.
    let prose_after_char = "fn q() -> char { '\"' }\n\
                           assert!(true, \"never forks the implementation\");\n";
    assert!(
        !code_only(prose_after_char).contains("implementation"),
        "prose in a string must stay stripped even after a char literal: {:?}",
        code_only(prose_after_char)
    );

    // A lifetime is not a character literal and must not swallow the line.
    let lifetime = "fn f<'a>(s: &'a str) -> &'a str { s }\n\
                    use jeliya_platform::implementation::shareable_blob;\n";
    assert!(
        code_only(lifetime).contains("implementation"),
        "a lifetime must not be mistaken for a char literal: {:?}",
        code_only(lifetime)
    );

    // A loop LABEL is the same hazard with both apostrophes on one line and
    // real code between them: scanning to the next quote would strip the body.
    for labelled in [
        "'retry: loop { jeliya_platform::implementation::blob_token_from_raw(7); break 'retry; }",
        "'a: for _ in 0..1 { use jeliya_platform::implementation::shareable_blob; break 'a; }",
        "'outer: loop { if c == '\"' { break 'outer; } \
         jeliya_platform::implementation::shareable_blob(t, 1); }",
    ] {
        assert!(
            code_only(labelled).contains("implementation"),
            "a loop label must not blank out the code it encloses: {labelled:?}\n\
             stripped to: {:?}",
            code_only(labelled)
        );
    }

    // …while a genuine char literal beside a label is still consumed.
    let label_then_literal = "'outer: loop { let q = '\"'; }\n\
                              assert!(true, \"never forks the implementation\");\n";
    assert!(
        !code_only(label_then_literal).contains("implementation"),
        "a char literal after a label must still strip the string that follows: {:?}",
        code_only(label_then_literal)
    );
}

/// The third leg of §K4 under feature unification: **resolver truth**, over
/// every feature selection the shared UI ships.
///
/// The two textual legs above are token scans, and a token scan cannot see a
/// `package = "jeliya-platform"` rename, a `[workspace.dependencies]` alias
/// named after neither token, or the contract crate enabling the feature on
/// itself. Asking Cargo what it actually resolved is immune to all three: `{f}`
/// prints the **resolved feature list** beside each package, so this asserts
/// both that no edge to the door crate exists and that `jeliya-platform` never
/// resolves with `implementation` on in a shared-UI graph — whatever spelling
/// put it there.
///
/// All three selections are checked, not just `ui`: `web` is what the browser
/// artifact links and `native` is the M4 shell seam, which is precisely the
/// graph this boundary exists to protect.
#[test]
fn no_shared_ui_selection_reaches_the_implementation_surface() {
    for selection in ["ui", "web", "native"] {
        let tree = tree(&[
            "--locked",
            "-p",
            "jeliya-ui",
            "--features",
            selection,
            // ALL edge kinds, not `no-dev`: a dev-dependency enabling the
            // feature unifies it into the same graph when the workspace's
            // tests are built, so excluding dev edges would let a member open
            // the door through `[dev-dependencies]` unseen.
            "--edges",
            "all",
            // Same format note as the default-graph test above.
            "--prefix",
            "none",
            "-f",
            "{p} {f}",
        ]);
        for line in tree.lines() {
            assert!(
                !line.starts_with("jeliya-platform-implementation"),
                "`--features {selection}` carries an edge to the implementation door:\n{tree}"
            );
            if line.starts_with("jeliya-platform v") {
                assert!(
                    !line.contains("implementation"),
                    "`--features {selection}` resolves jeliya-platform with the implementation \
                     feature on:\n{tree}"
                );
            }
        }
    }
}

/// Every workspace member's package name, from the root `members` list.
fn workspace_member_names() -> Vec<String> {
    let root = std::fs::read_to_string(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Cargo.toml"
    )))
    .expect("readable workspace manifest");
    let members = root
        .split_once("members = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list.to_string())
        .expect("a members list");
    let mut names = Vec::new();
    for entry in members.split(',') {
        let path = entry
            .trim()
            .trim_matches(|c| c == '"' || c == '\n' || c == ' ');
        if path.is_empty() {
            continue;
        }
        let manifest = std::fs::read_to_string(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
                .join(path)
                .join("Cargo.toml"),
        )
        .expect("readable member manifest");
        let name = manifest
            .lines()
            .find_map(|line| line.trim().strip_prefix("name = "))
            .map(|name| name.trim().trim_matches('"').to_string())
            .expect("a package name");
        names.push(name);
    }
    names
}

/// **Resolver truth over every workspace member**, with all of that member's
/// own features on.
///
/// The shared-UI test below names the three graphs the product actually ships;
/// this one closes the general claim, because a token scan cannot see a
/// `package = ` rename or a `[workspace.dependencies]` alias, and *any* member
/// reaching the surface violates the sole-door boundary — not only `jeliya-ui`.
/// `--all-features` is what makes it strong: a member that hides the enablement
/// behind one of its own optional features is still caught.
#[test]
fn no_workspace_member_resolves_the_implementation_feature() {
    for name in workspace_member_names() {
        // The contract crate defines the feature; the door crate is the door.
        if name == "jeliya-platform" || IMPLEMENTATION_DOOR_CRATES.contains(&name.as_str()) {
            continue;
        }
        let tree = tree(&[
            "--locked",
            "-p",
            &name,
            "--all-features",
            // ALL edge kinds, not `no-dev`: a dev-dependency enabling the
            // feature unifies it into the same graph when the workspace's
            // tests are built, so excluding dev edges would let a member open
            // the door through `[dev-dependencies]` unseen.
            "--edges",
            "all",
            // Same format note as the default-graph test above.
            "--prefix",
            "none",
            "-f",
            "{p} {f}",
        ]);
        for line in tree.lines() {
            assert!(
                !line.starts_with("jeliya-platform-implementation"),
                "member `{name}` carries an edge to the implementation door:\n{tree}"
            );
            if line.starts_with("jeliya-platform v") {
                assert!(
                    !line.contains("implementation"),
                    "member `{name}` resolves jeliya-platform with the implementation feature \
                     on:\n{tree}"
                );
            }
        }
    }
}

/// Whether a line of `jeliya-platform`'s own `[features]` table turns the
/// `implementation` door on for everybody.
///
/// The token's one legitimate occurrence is the feature's own definition key;
/// naming it in the *value* of any other feature resolves it on in every graph.
/// A continuation line of a multi-line array carries no `=` and so counts as a
/// value.
fn feature_line_opens_the_implementation_door(code: &str) -> bool {
    if !code.contains("implementation") {
        return false;
    }
    !code.split_once('=').is_some_and(|(name, value)| {
        name.trim() == "implementation" && !value.contains("implementation")
    })
}

/// **The contract crate must not open its own door** (§K4, the third spelling).
///
/// [`only_the_door_crate_reaches_the_implementation_surface`] allowlists
/// `jeliya-platform` itself — correctly, since it defines both the feature and
/// the module — which leaves the single most powerful edit in the workspace
/// uncovered: one word in this crate's own `[features]` table (`default`, or
/// `fake`, which `jeliya-ui` itself asks for) resolves the surface on for every
/// consumer, with no consumer manifest touched and no door-crate edge created.
/// The failure would also be silent rather than loud: the tier-1 `compile_fail`
/// doctest is itself gated on the feature being off, so it would stop existing
/// instead of going red.
#[test]
fn the_contract_crate_never_enables_its_own_implementation_feature() {
    let manifest_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let manifest = std::fs::read_to_string(manifest_path).expect("readable Cargo.toml");
    let mut in_features = false;
    let mut offenders = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("").trim();
        if code.starts_with('[') {
            in_features = code == "[features]";
            continue;
        }
        if in_features && feature_line_opens_the_implementation_door(code) {
            offenders.push(format!("Cargo.toml:{}: {code}", index + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "jeliya-platform's own [features] table enables `implementation` for every consumer: \
         {offenders:#?}"
    );

    // The checker itself must fail closed on both global-enablement spellings.
    for opener in [
        "default = [\"implementation\"]",
        "fake = [\"implementation\"]",
        "    \"implementation\",",
    ] {
        assert!(
            feature_line_opens_the_implementation_door(opener),
            "the checker must trip on {opener:?}"
        );
    }
    for clean in ["implementation = []", "default = []", "fake = []"] {
        assert!(
            !feature_line_opens_the_implementation_door(clean),
            "the checker must not trip on {clean:?}"
        );
    }
}

/// No `serde_json::Value` may appear in any public source: this crate consumes
/// `jeliya-api` value types, never raw JSON. Scans every `.rs` file, skipping
/// comment lines that merely describe the rule.
#[test]
fn no_serde_json_value_in_public_source() {
    let src_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut offenders = Vec::new();
    for path in rust_sources(src_dir) {
        let text = std::fs::read_to_string(&path).expect("readable source");
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("serde_json") {
                offenders.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "serde_json token found in public source: {offenders:?}"
    );
}

/// The contract surface — every module except the composition/selection points
/// (`lib.rs`, `services.rs`) and the fakes (`src/fake/`) — must carry **no**
/// platform `cfg` fork (§K10). Target selection and feature gating happen at the
/// crate root and in the fakes, never in the capability traits and types a
/// shared component consumes. Scans a comment-stripped, whitespace-collapsed
/// copy of each file for any `target_*` key, any `windows`/`unix` alias, and any
/// `feature = "…"` gate.
#[test]
fn no_platform_cfg_forks_in_contract_surface() {
    let src_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut offenders = Vec::new();
    for path in rust_sources(src_dir) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let in_fake = path
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new("fake"));
        if in_fake || matches!(name, "lib.rs" | "services.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable source");
        let compact: String = text
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for pattern in [
            "(target_",
            ",target_",
            "feature=\"",
            "cfg(windows",
            "cfg(unix",
            "cfg!(windows",
            "cfg!(unix",
            "any(windows",
            "any(unix",
            "all(windows",
            "all(unix",
            "not(windows",
            "not(unix",
        ] {
            if compact.contains(pattern) {
                offenders.push(format!("{} — contains {pattern:?}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "platform cfg forks found in the contract surface (§K10): {offenders:#?}"
    );
}

/// The crate's `Cargo.toml` dependency tables must not directly name any
/// forbidden native-family crate, nor a renamed `serde_json` door. `dioxus` is
/// permitted only as the OPTIONAL example dependency (never in the default or
/// `fake` graphs, asserted above), so it is not scanned here.
const BANNED_MANIFEST_CRATES: [&str; 14] = [
    "jeliya-core",
    "jeliyad",
    "jeliya-ffi",
    "iroh",
    "quinn",
    "rustls",
    "tokio",
    "hickory",
    "wry",
    "tao",
    "openssl-sys",
    "native-tls",
    "tungstenite",
    "serde_json",
];

#[test]
fn crate_manifest_has_no_direct_native_crate_dependency() {
    let manifest_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let manifest = std::fs::read_to_string(manifest_path).expect("readable Cargo.toml");
    let mut in_deps = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]"
                || trimmed == "[dev-dependencies]"
                || trimmed == "[build-dependencies]"
                || trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[dev-dependencies.")
                || trimmed.starts_with("[build-dependencies.")
                || (trimmed.starts_with("[target.")
                    && (trimmed.ends_with("dependencies]") || trimmed.contains(".dependencies.")));
            if in_deps {
                for banned in BANNED_MANIFEST_CRATES {
                    if trimmed.contains(&format!(".{banned}"))
                        || trimmed.contains(&format!("\"{banned}"))
                        || trimmed.contains(&format!("'{banned}"))
                    {
                        panic!(
                            "Cargo.toml dependency table names forbidden crate '{banned}' \
                             (line: {line})"
                        );
                    }
                }
            }
            continue;
        }
        if !in_deps || trimmed.starts_with('#') {
            continue;
        }
        for banned in BANNED_MANIFEST_CRATES {
            if trimmed.starts_with(banned)
                || trimmed.contains(&format!("\"{banned}"))
                || trimmed.contains(&format!("'{banned}"))
            {
                panic!(
                    "Cargo.toml [dependencies] declares forbidden crate '{banned}' directly \
                     (line: {line})"
                );
            }
        }
    }
}

/// Collect every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut sources = Vec::new();
    if !dir.exists() {
        return sources;
    }
    for entry in std::fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            sources.push(path);
        }
    }
    sources
}
