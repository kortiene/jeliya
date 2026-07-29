//! Static assertions about jeliya-api's own surface, run in CI. These are
//! the checks #163 names as acceptance criteria that a unit test cannot
//! express: the dependency boundary and the no-`serde_json::Value` rule.

use std::process::Command;

/// The dependency tree must stay free of Iroh, WebSocket, Dioxus, and any
/// platform crate — the browser client links this crate for wasm32.
#[test]
fn dependency_tree_is_free_of_transport_and_platform_crates() {
    let output = Command::new(env!("CARGO"))
        .args(["tree", "-p", "jeliya-api"])
        .output()
        .expect("cargo tree runs");
    let tree = String::from_utf8_lossy(&output.stdout);
    for banned in [
        "iroh",
        "websocket",
        "tokio-tungstenite",
        "dioxus",
        "tao",
        "wry",
    ] {
        assert!(
            !tree.lines().any(|l| l.to_lowercase().contains(banned)),
            "forbidden dependency in jeliya-api tree: {banned}\n{tree}"
        );
    }
}

/// No `serde_json::Value` may appear in any public type. JSON exists only
/// at the WebSocket edge (#164's codec); untrusted values are never hidden
/// in raw JSON here. This scans the crate source for the type.
#[test]
fn no_serde_json_value_in_public_source() {
    let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(src_dir).expect("src dir") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            for (i, line) in text.lines().enumerate() {
                // allow doc comments that *describe* the rule
                let trimmed = line.trim_start();
                if trimmed.starts_with("//!")
                    || trimmed.starts_with("///")
                    || trimmed.starts_with("//")
                {
                    continue;
                }
                if line.contains("serde_json::Value") || line.contains("serde_json::value::Value") {
                    offenders.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "serde_json::Value found in public source: {offenders:?}"
    );
}

