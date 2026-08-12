//! Static assertions about jeliya-supervisor's dependency boundary, run in CI.
//! These are the invariants #170 §3.1 names — the crate owns process authority
//! and must therefore stay OUT of both the UI crate and the sans-IO kernel, and
//! must never be reachable from a `wasm32` build — that a unit test cannot
//! express. They mirror `jeliya-client`/`jeliya-platform`'s boundary tests.
//!
//! Note vs. the sibling crates: this crate is native-only and legitimately
//! depends on `tokio` and (on Unix) `nix`, so those are NOT banned here. What is
//! banned is any edge that would drag process authority into a UI/wasm graph or
//! pull Iroh into a supervisor that only speaks loopback discovery.

use std::process::Command;

/// Crates the supervisor's own graph must never reach: Iroh (and the Iroh-bearing
/// core/daemon), the client seam and codec (transport/RPC lives elsewhere), and
/// the whole renderer family.
const BANNED_GRAPH_CRATES: [&str; 9] = [
    "iroh",
    "jeliya-core",
    "jeliyad",
    "jeliya-ffi",
    "jeliya-client",
    // The protocol codec: transport/RPC lives in the client/codec, not the
    // supervisor (which only speaks loopback discovery). The module note claims
    // this is banned, so the list must actually enforce it.
    "jeliya-codec",
    "dioxus",
    "wry",
    "tao",
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

/// Assert no banned crate appears at column 0 of a `--prefix none` tree. The
/// `--prefix none` format is essential: the default indent format hides all but
/// the root behind tree glyphs, which `starts_with` would never match — the
/// check would pass vacuously.
fn assert_graph_clean(tree: &str, context: &str) {
    for banned in BANNED_GRAPH_CRATES {
        assert!(
            !tree.lines().any(|line| line.starts_with(banned)),
            "forbidden crate '{banned}' is reachable from the {context} graph:\n{tree}"
        );
    }
}

/// The library dependency tree (default features, no dev edges) must be free of
/// Iroh, the core/daemon, the client seam, and every renderer crate.
#[test]
fn library_dependency_tree_excludes_iroh_core_client_and_renderers() {
    let tree = tree(&[
        "--locked",
        "-p",
        "jeliya-supervisor",
        "--edges",
        "no-dev",
        "--prefix",
        "none",
    ]);
    assert_graph_clean(&tree, "supervisor library");
}

/// The supervisor is **native-only** and must never be reachable from a `wasm32`
/// build. The shared UI crate (`jeliya-ui`) is the browser (wasm32) consumer;
/// its full `web`-feature graph — resolved for the wasm target without compiling
/// it — must not contain `jeliya-supervisor`. This is the "absent from the wasm
/// graph" assertion the spec requires (§3.1 / §8), mirroring `jeliya-api`'s and
/// `jeliya-platform`'s wasm-graph checks.
#[test]
fn supervisor_is_absent_from_the_wasm_ui_graph() {
    let tree = tree(&[
        "--locked",
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
    ]);
    assert!(
        !tree
            .lines()
            .any(|line| line.starts_with("jeliya-supervisor")),
        "jeliya-supervisor leaked into the wasm32 jeliya-ui `web` graph:\n{tree}"
    );
}

/// The checker itself must fail closed: a bare banned-crate line (the
/// `--prefix none` output shape) trips the assertion. Pins the contract against
/// a regression to a parser that only matches glyph-prefixed indent output —
/// which matches nothing at column 0 and passes vacuously.
#[test]
fn assert_graph_clean_detects_a_banned_crate() {
    let synthetic = "jeliya-supervisor v0.0.0\niroh v0.28.0\n";
    assert!(
        std::panic::catch_unwind(|| assert_graph_clean(synthetic, "synthetic")).is_err(),
        "a bare banned-crate line must trip the checker"
    );
}
