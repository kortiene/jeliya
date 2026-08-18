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

/// The kernel core (#168) is **sans-IO**: it takes logical time as an input and
/// emits "arm a timer" as an action; it never reads a wall clock and never
/// makes an RNG syscall, so its behaviour is deterministic and identical on
/// wasm and native. This scan asserts the discipline structurally — no
/// `std::time`, `Instant::now`, `SystemTime`, `getrandom`, `rand::`, or `tokio`
/// token appears in any `src/kernel/**` source line (comment lines that merely
/// describe the rule are skipped, exactly like the `serde_json::Value` scan).
#[test]
fn kernel_source_has_no_wall_clock_rng_or_runtime() {
    let kernel_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/kernel");
    // Substrings chosen to name the banned APIs without matching innocent words
    // (for example `rand::`, not a bare `rand` that also occurs in "strands").
    const BANNED: [&str; 6] = [
        "std::time",
        "Instant::now",
        "SystemTime",
        "getrandom",
        "rand::",
        "tokio",
    ];
    let mut offenders = Vec::new();
    for path in rust_sources(std::path::Path::new(kernel_dir)) {
        let text = std::fs::read_to_string(&path).expect("readable kernel source");
        for (index, line) in text.lines().enumerate() {
            // Skip comment lines that describe the boundary itself.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for banned in BANNED {
                if line.contains(banned) {
                    offenders.push(format!("{}:{} ({banned})", path.display(), index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "kernel source must carry no wall clock, RNG, or runtime token: {offenders:?}"
    );
}

/// The reconciler core (#169) is **sans-IO**: ordering and dedup are driven by
/// the `pos`/`event_id`/`at` carried on inputs, never by a read of the wall
/// clock, and it makes no RNG syscall and pulls in no runtime — so its behaviour
/// is deterministic and identical on wasm and native. This scan asserts the
/// discipline structurally over every `src/reconcile/**` source line — no
/// `std::time`, `Instant::now`, `SystemTime`, `getrandom`, `rand::`, or `tokio`
/// token (comment lines that merely describe the rule are skipped, exactly like
/// the kernel scan). The signed `at` is *data on an event*, not a clock read.
#[test]
fn reconciler_source_has_no_wall_clock_rng_or_runtime() {
    let reconcile_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/reconcile");
    const BANNED: [&str; 6] = [
        "std::time",
        "Instant::now",
        "SystemTime",
        "getrandom",
        "rand::",
        "tokio",
    ];
    let mut offenders = Vec::new();
    for path in rust_sources(std::path::Path::new(reconcile_dir)) {
        let text = std::fs::read_to_string(&path).expect("readable reconcile source");
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for banned in BANNED {
                if line.contains(banned) {
                    offenders.push(format!("{}:{} ({banned})", path.display(), index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "reconciler source must carry no wall clock, RNG, or runtime token: {offenders:?}"
    );
}

/// The `direct` feature's optional native deps (`tokio`, `jeliya-core`,
/// `jeliya-codec`) and their transitive `iroh-rooms` must not appear in the
/// `jeliya-ui` browser (`wasm32`) build graph: the `direct` feature is
/// native-only and must never leak into the browser artifact.
///
/// `cargo tree --target wasm32-unknown-unknown` resolves the wasm32 dependency
/// graph without compiling it, so this assertion runs even where the wasm32
/// std target is not installed — exactly as `jeliya-platform`'s twin test does.
///
/// This is the Rust counterpart of `scripts/check-jeliya-ui-wasm-graph.sh`
/// scoped to the `direct` feature's contribution (§11).
#[test]
fn direct_feature_deps_absent_from_jeliya_ui_wasm_graph() {
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
    // The direct feature's three optional deps plus the iroh family they
    // transitively carry must never appear in the browser build. `jeliya-core`
    // and `jeliya-codec` are cfg-guarded `cfg(not(wasm32))` + optional, so a
    // misconfiguration (e.g. moving them to base deps or removing the cfg)
    // would fail this check before it could ship.
    for banned in ["tokio", "jeliya-core", "jeliya-codec", "iroh-rooms"] {
        assert!(
            !tree
                .lines()
                .any(|line| line.to_lowercase().contains(banned)),
            "forbidden dep '{banned}' is reachable from the jeliya-ui wasm32 'web' build — \
             the 'direct' feature's native deps must not leak into the browser graph:\n{tree}"
        );
    }
}

/// The `src/direct/**` module must contain no retired v1 seam, Dart, or C-ABI
/// tokens (§10.5, §13 AC-1). The direct data path is in-process and typed;
/// these strings have no legitimate use in it.
///
/// Banned patterns:
/// - `handle_frame` — the retired v1 JSON-envelope seam (`Engine::handle_frame(String)`).
/// - `nativePort` / `SendPort` — Dart `SendPort` API; gone with `jeliya-ffi`.
/// - `portfile` — daemon portfile; DirectClient uses no daemon.
/// - `extern "C"` — C-ABI export; gone with `jeliya-ffi`.
#[test]
fn direct_source_has_no_forbidden_v1_or_ffi_tokens() {
    let direct_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/direct"));
    if !direct_dir.exists() {
        // The `direct` module may not be present on all builds; the feature
        // is default-off. If the source directory is absent, the constraint
        // is vacuously satisfied.
        return;
    }
    const BANNED: [&str; 5] = [
        "handle_frame",
        "nativePort",
        "SendPort",
        "portfile",
        r#"extern "C""#,
    ];
    let mut offenders = Vec::new();
    for path in rust_sources(direct_dir) {
        let text = std::fs::read_to_string(&path).expect("readable direct source");
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for banned in BANNED {
                if line.contains(banned) {
                    offenders.push(format!("{}:{} ({banned})", path.display(), index + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "forbidden v1/FFI token found in src/direct — the direct path must contain \
         no handle_frame, Dart port, C-ABI, or daemon portfile: {offenders:?}"
    );
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
