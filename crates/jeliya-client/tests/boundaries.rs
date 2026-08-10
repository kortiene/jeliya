//! Static assertions about jeliya-client's own surface, run in CI. These are
//! the boundary invariants #167 §3 names — the dependency boundary and the
//! no-`serde_json::Value`-in-public-source rule — that a unit test cannot
//! express. They mirror `jeliya-api/tests/boundaries.rs`.

use std::process::Command;

/// The **library** dependency tree must stay free of Iroh, any WebSocket
/// crate, native transports, `tao`/`wry`, and Dioxus, so the seam is
/// transport- and UI-framework-independent and the browser client can link it
/// for wasm32. The scan is over the library's normal edges with default
/// features off, so the optional Dioxus example dependency (behind the
/// `example` feature) and the dev-dependencies are excluded — exactly the
/// surface a real build of the library carries.
#[test]
fn library_dependency_tree_is_free_of_transport_and_ui_crates() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "jeliya-client",
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
    let tree = String::from_utf8_lossy(&output.stdout);
    for banned in [
        "iroh",
        "websocket",
        "tokio-tungstenite",
        "tungstenite",
        "dioxus",
        "tao",
        "wry",
        "tokio",
    ] {
        assert!(
            !tree
                .lines()
                .any(|line| line.to_lowercase().contains(banned)),
            "forbidden dependency in jeliya-client library tree: {banned}\n{tree}"
        );
    }
}

/// No `serde_json::Value` may appear in any public source. The erased boundary
/// carries a JSON *text* newtype (`RawJson`), so the token appears nowhere —
/// untrusted values are never hidden in raw JSON here. This scans every source
/// file (including `src/mock/`), skipping comment lines that merely describe
/// the rule, exactly like `jeliya-api`.
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
                let trimmed = line.trim_start();
                // Allow doc/line comments that *describe* the rule.
                if trimmed.starts_with("//") {
                    continue;
                }
                if line.contains("serde_json::Value") || line.contains("serde_json::value::Value") {
                    offenders.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        }
    }
}
