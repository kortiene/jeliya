//! Static assertions about jeliya-ui's own surface, run in CI. These are the
//! boundary invariants #176 §3 names — the browser graph exclusion and the
//! no-`serde_json::Value`-in-public-source rule — that a unit test cannot
//! otherwise express. They mirror `jeliya-api`/`jeliya-client`'s boundary
//! tests. The executable AC-2 check `scripts/check-jeliya-ui-wasm-graph.sh`
//! enforces the same wasm-graph exclusion in the dedicated web CI job; this is
//! the belt-and-suspenders in-tree form.

use std::process::Command;

/// The browser (`wasm32`) build graph, with the `web` renderer feature on,
/// must exclude Iroh and every native crate: `jeliya-core`, `jeliyad`,
/// `jeliya-ffi`, `quinn`, `rustls`, `tokio`, `wry`/`tao`, `openssl-sys`, and
/// `native-tls` (architecture Decision 3, #158 AC-1, #176 AC-2). `cargo tree`
/// resolves the graph for the target without compiling it, so this runs even
/// where the wasm std target is not installed.
#[test]
fn wasm_web_graph_is_free_of_iroh_and_native_crates() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "jeliya-ui",
            "--features",
            "web",
            "--target",
            "wasm32-unknown-unknown",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--no-dedupe",
        ])
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout).to_lowercase();
    for banned in [
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
    ] {
        assert!(
            !tree.lines().any(|line| line.starts_with(banned)),
            "forbidden crate '{banned}' is reachable from the wasm32 `web` build:\n{tree}"
        );
    }
}

/// The **default** library graph (no renderer feature) must not pull Dioxus, so
/// the workspace MSRV job compiles this crate renderer-free and OpenSSL-free.
#[test]
fn default_library_graph_pulls_no_renderer() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "jeliya-ui",
            "--no-default-features",
            "--edges",
            "no-dev",
        ])
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout).to_lowercase();
    for banned in ["dioxus", "openssl", "wry", "tao", "tokio", "iroh"] {
        assert!(
            !tree.lines().any(|line| line.contains(banned)),
            "default jeliya-ui graph unexpectedly pulls '{banned}':\n{tree}"
        );
    }
}

/// No `serde_json::Value` may appear in any public source: this crate consumes
/// `jeliya-api` view models, never raw JSON. Scans every `.rs` file, skipping
/// comment lines that merely describe the rule, exactly like
/// `jeliya-api`/`jeliya-client`.
#[test]
fn no_serde_json_value_in_public_source() {
    let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    scan_dir(std::path::Path::new(src_dir), &mut offenders);
    assert!(
        offenders.is_empty(),
        "serde_json::Value found in public source: {offenders:?}"
    );
}

/// Shared `components/` must contain no platform `cfg` forks (architecture
/// Decision 3, §3). Target differences live only in `compose.rs` and
/// per-target `bin/`; components receive platform capabilities as props or
/// injected services and never branch on `cfg(target_...)`. This scans every
/// `.rs` file under `src/components/` and fails on any `cfg` that selects a
/// specific target architecture, OS, family, or pointer width, or branches on
/// the `native`/`web` features by name.
#[test]
fn no_cfg_target_forks_in_shared_components() {
    let components_dir =
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/components"));
    let mut offenders = Vec::new();
    // Recursive: a nested module (src/components/room/mod.rs) is as much a
    // shared component as an immediate file, and an unwalked subdirectory
    // would silently exempt it from the no-fork contract.
    let mut pending = vec![components_dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable components dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for path in files {
        if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("readable component");
            for (index, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                // Platform-discriminating cfg keys that must not appear in
                // shared components. Feature-gating on `ui`/`web`/`native` is
                // forbidden here too: a component cannot self-select its target.
                for pattern in [
                    "cfg(target_arch",
                    "cfg(target_os",
                    "cfg(target_family",
                    "cfg(target_pointer_width",
                    "cfg(target_env",
                    r#"feature = "native""#,
                ] {
                    if line.contains(pattern) {
                        offenders.push(format!(
                            "{}:{} — {}",
                            path.display(),
                            index + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "platform cfg forks found in shared components (Decision 3): {offenders:#?}"
    );
}

/// The crate's `Cargo.toml` `[dependencies]` section must not directly name
/// `jeliya-core`, `jeliyad`, or `jeliya-ffi`. These native crates must never
/// reach `jeliya-ui`'s dependency graph — belt-and-suspenders at the manifest
/// level complementing the `wasm_web_graph_is_free_of_iroh_and_native_crates`
/// graph check.
#[test]
fn crate_manifest_has_no_direct_native_crate_dependency() {
    let manifest_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let manifest = std::fs::read_to_string(manifest_path).expect("readable Cargo.toml");
    // Scan only the [dependencies] and [dev-dependencies] sections.
    let mut in_deps = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]" || trimmed == "[dev-dependencies]";
            continue;
        }
        if !in_deps || trimmed.starts_with('#') {
            continue;
        }
        for banned in ["jeliya-core", "jeliyad", "jeliya-ffi"] {
            // A dependency declaration starts with `<name> =` or `<name>.workspace`.
            if trimmed.starts_with(banned) {
                panic!(
                    "Cargo.toml [dependencies] declares native crate '{banned}' directly — \
                     it must never enter jeliya-ui's graph (line: {line})"
                );
            }
        }
    }
}

/// Recursively scan `.rs` files under `dir`, collecting `file:line` offenders.
fn scan_dir(dir: &std::path::Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("readable src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan_dir(&path, offenders);
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("readable source");
            for (index, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.contains("serde_json::Value") || line.contains("serde_json::value::Value") {
                    offenders.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        }
    }
}
