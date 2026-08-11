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
/// Cargo has two spellings for the same thing and a boundary check must catch
/// both: the inline form (`jeliya-platform = { …, features = ["implementation"]
/// }`) and the split-table form, where the dependency name is on the `[…]`
/// header line and `features` is on its own line — so the header is tracked
/// rather than each line judged alone. A dependency on the door crate counts
/// too, since that is a transitive door. Inline comments are stripped so prose
/// about the rule (including this crate's own manifest notes) cannot trip it.
fn manifest_opens_the_implementation_door(manifest: &str) -> bool {
    let mut table_names_platform = false;
    for line in manifest.lines() {
        let code = line.split('#').next().unwrap_or("").trim();
        if code.starts_with('[') {
            table_names_platform = code.contains("jeliya-platform");
            if table_names_platform && code.contains("implementation") {
                return true;
            }
            continue;
        }
        if code.contains("jeliya-platform") && code.contains("implementation") {
            return true;
        }
        if table_names_platform && code.contains("implementation") {
            return true;
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
/// literals with escapes, and raw strings with any number of hashes. Character
/// literals are left alone: a `char` cannot hold the token being scanned for,
/// and skipping them avoids mistaking a lifetime for one.
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
/// The third leg — the dependency edge, which unification cannot touch — is
/// [`the_shared_ui_graph_has_no_edge_to_the_implementation_door`].
#[test]
fn only_the_door_crate_reaches_the_implementation_surface() {
    let mut offenders = Vec::new();
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
        // Only crates that consume the contract can spell its factory path.
        // Scanning every member would be wrong, not merely noisy: `from_raw`
        // and the word "implementation" appear legitimately elsewhere in the
        // workspace (jeliyad/src/serve.rs, for one).
        if !manifest.contains("jeliya-platform") {
            continue;
        }
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
    for (label, fixture) in [
        ("inline features", inline),
        ("split dependency table", split_table),
        ("transitive door dependency", transitive),
        ("feature-table forward", feature_table),
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
}

/// The third leg of §K4 under feature unification: a **dependency edge**.
///
/// Unlike a feature, an edge never unifies — so asserting that the shared UI's
/// graph does not reach `jeliya-platform-implementation` is a claim that stays
/// true in every target binary, however the features resolve.
#[test]
fn the_shared_ui_graph_has_no_edge_to_the_implementation_door() {
    let tree = tree(&[
        "--locked",
        "-p",
        "jeliya-ui",
        "--features",
        "ui",
        "--edges",
        "no-dev",
        // Same format note as the default-graph test above.
        "--prefix",
        "none",
    ]);
    assert!(
        !tree
            .lines()
            .any(|line| line.starts_with("jeliya-platform-implementation")),
        "the shared UI graph must carry no edge to the implementation door:\n{tree}"
    );
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
