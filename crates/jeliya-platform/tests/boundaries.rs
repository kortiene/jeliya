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

/// The shared-component crate must never reach the `implementation` factory
/// surface: `jeliya-ui`'s manifest must not enable the feature, and its source
/// must not name the factory tokens — the compile-time forgery boundary (§K4)
/// only holds if the shared graph stays on default features.
#[test]
fn jeliya_ui_never_enables_the_implementation_feature() {
    let ui_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../jeliya-ui"));
    let manifest =
        std::fs::read_to_string(ui_dir.join("Cargo.toml")).expect("readable jeliya-ui manifest");
    // Scan non-comment manifest lines only, so prose about the rule cannot
    // trip it; any dependency/feature line naming the feature does.
    let enabled = manifest
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| line.contains("implementation"));
    assert!(
        !enabled,
        "jeliya-ui/Cargo.toml must not enable jeliya-platform/implementation"
    );
    let mut offenders = Vec::new();
    for path in rust_sources(&ui_dir.join("src")) {
        let text = std::fs::read_to_string(&path).expect("readable jeliya-ui source");
        for (index, line) in text.lines().enumerate() {
            if line.contains("for_implementation") || line.contains("from_raw") {
                offenders.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "jeliya-ui source names an implementation-factory token: {offenders:?}"
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
