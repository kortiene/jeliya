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
            "--locked",
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
    let src_dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut offenders = Vec::new();
    // The scan covers the whole target-agnostic root — app.rs and state.rs
    // are as shared as components/, and a platform branch there forks the
    // UI exactly the same way. The EXCEPTIONS are the modules whose job is
    // selection: lib.rs owns the renderer-optional feature-gated module
    // graph, compose.rs and the per-target bin/ own target composition,
    // and services.rs owns service implementations behind the injected
    // seam. Recursive: a nested module is as much shared code as an
    // immediate file, and an unwalked subdirectory would silently exempt
    // it from the no-fork contract.
    let mut pending = vec![src_dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable src dir") {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() {
                if name != "bin" {
                    pending.push(path);
                }
            } else if !matches!(name, "lib.rs" | "compose.rs" | "services.rs") {
                files.push(path);
            }
        }
    }
    for path in files {
        if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("readable component");
            // Platform-discriminating cfg keys that must not appear in
            // shared components. Feature-gating on `ui`/`web`/`native` is
            // forbidden here too: a component cannot self-select its target.
            // Matching runs on a comment-stripped, whitespace-collapsed copy
            // of the WHOLE FILE: per-line matching missed multiline
            // expressions (`#[cfg(any(` on one line, `target_os = "windows"`
            // on the next), and whitespace stripping already covers spaced
            // forms and `cfg!(...)`. The `target_` prefix patterns catch
            // every discriminating key regardless of `any`/`all`/`not`
            // nesting.
            // (Comment stripping is the naive `//` split — a guard, not a
            // parser; components carry no `//` inside string literals.)
            let compact: String = text
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            for pattern in [
                // Every `target_*` cfg key — arch/os/family/env/abi/
                // endian/vendor/feature/pointer_width/has_atomic and any
                // key rustc adds later — discriminates platforms. In the
                // compacted text a cfg operand is always preceded by an
                // opening paren (`cfg(`, `any(`, `all(`, `not(`,
                // `cfg_attr(`, `cfg!(`) or a comma, so two prefixes cover
                // every predicate position without enumerating keys.
                "(target_",
                ",target_",
                // ANY feature gate is a fork a component must not own —
                // `feature="desktop"` self-selects a platform as surely as
                // the renderer features do. Components carry no cfg
                // features at all; composition happens in compose.rs.
                r#"feature=""#,
                // Rust's standalone platform aliases fork on OS/family with
                // no target_* key at all; cover them direct and inside
                // combinators.
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
                "cfg_attr(windows",
                "cfg_attr(unix",
                // An alias needs no leading position: in the compacted
                // `any(feature="experimental",windows)` the alias is a
                // LATER combinator operand, reached only through a comma.
                ",windows)",
                ",windows,",
                ",unix)",
                ",unix,",
            ] {
                if compact.contains(pattern) {
                    offenders.push(format!("{} — contains {pattern:?}", path.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "platform cfg forks found in shared components (Decision 3): {offenders:#?}"
    );
}

/// The crate's `Cargo.toml` dependency tables must not directly name any
/// forbidden native-family crate. The full family list, not just the three
/// Jeliya crates: a dependency restricted to a target CI never resolves
/// (say `iroh` under `[target.'cfg(target_os = "windows")'.dependencies]`)
/// escapes both graph checks, so the manifest scan is the only gate that
/// sees it. Belt-and-suspenders at the manifest level complementing the
/// `wasm_web_graph_is_free_of_iroh_and_native_crates` graph check.
const BANNED_MANIFEST_CRATES: [&str; 13] = [
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
];

#[test]
fn crate_manifest_has_no_direct_native_crate_dependency() {
    let manifest_path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let manifest = std::fs::read_to_string(manifest_path).expect("readable Cargo.toml");
    // Scan only the [dependencies] and [dev-dependencies] sections.
    let mut in_deps = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            // Every dependency table counts, including build-dependencies,
            // target-specific tables like
            // [target.'cfg(not(target_arch = "wasm32"))'.dependencies], and
            // DETAILED tables ([dependencies.name] /
            // [target.'…'.dependencies.name]) — a banned crate declared in
            // any of them is as much a direct dependency as one in
            // [dependencies].
            in_deps = trimmed == "[dependencies]"
                || trimmed == "[dev-dependencies]"
                || trimmed == "[build-dependencies]"
                || trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[dev-dependencies.")
                || trimmed.starts_with("[build-dependencies.")
                || (trimmed.starts_with("[target.")
                    && (trimmed.ends_with("dependencies]") || trimmed.contains(".dependencies.")));
            // A detailed table can name the banned crate in its HEADER
            // (`[dependencies.jeliya-core]`), never reaching the body scan.
            if in_deps {
                for banned in BANNED_MANIFEST_CRATES {
                    // TOML accepts literal (single-quoted) keys and strings
                    // too: `[dependencies.'jeliya-core']` names the same
                    // crate the bare and double-quoted forms do. Quoted
                    // matches are PREFIX-open (`"iroh` not `"iroh"`) so a
                    // family member (`"iroh-base"`) is caught like the
                    // umbrella crate, mirroring the graph gate's semantics.
                    if trimmed.contains(&format!(".{banned}"))
                        || trimmed.contains(&format!("\"{banned}"))
                        || trimmed.contains(&format!("'{banned}"))
                    {
                        panic!(
                            "Cargo.toml dependency table names native crate '{banned}' — \
                             it must never enter jeliya-ui's graph (line: {line})"
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
            // A dependency declaration starts with `<name> =` — or renames
            // the package (`x = { package = "jeliya-core", … }`), so the
            // quoted package name counts wherever it appears in the table,
            // in double-quoted AND single-quoted (TOML literal) form —
            // prefix-open, so family members are caught too.
            if trimmed.starts_with(banned)
                || trimmed.contains(&format!("\"{banned}"))
                || trimmed.contains(&format!("'{banned}"))
            {
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
                // Raw JSON needs no `Value` spelling: the `json!` macro
                // BUILDS a Value, path-position `serde_json::value::…`
                // reaches the module without importing it, and
                // `to_value`/`from_value` traffic in Value at the boundary.
                for door in [
                    "serde_json::json!",
                    "serde_json::value::",
                    "serde_json::to_value",
                    "serde_json::from_value",
                ] {
                    if line.contains(door) {
                        offenders.push(format!(
                            "{}:{} — reaches serde_json::Value via {door}",
                            path.display(),
                            index + 1
                        ));
                    }
                }
            }
            // A grouped, aliased, or MULTILINE import (`use serde_json::{` /
            // `Value,` / `};`) has neither fully qualified spelling on any
            // single line: scan complete `use serde_json::…;` declarations
            // on a comment-stripped, whitespace-collapsed copy of the file.
            let compact: String = text
                .lines()
                .map(|line| line.split("//").next().unwrap_or(""))
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            // Root-qualified imports (`use ::serde_json as json;`) compact
            // to `use::serde_json…`, matching neither predicate below —
            // normalize the optional leading `::` away so both spellings
            // are the same declaration.
            let compact = compact.replace("use::serde_json", "useserde_json");
            // Aliasing the MODULE (`use serde_json as json`) evades both
            // scans while later signatures use json::Value; the alias itself
            // is the breach signal.
            if compact.contains("useserde_jsonas") {
                offenders.push(format!(
                    "{} — aliases the serde_json module (raw-JSON boundary)",
                    path.display()
                ));
            }
            let mut from = 0;
            while let Some(rel) = compact[from..].find("useserde_json::") {
                let at = from + rel;
                let declaration = compact[at..].split(';').next().unwrap_or("");
                // `self` inside the group (`use serde_json::{self as json}`)
                // re-imports the MODULE under an alias — the same breach the
                // top-level alias check catches, through the grouped door.
                // The `value` SUBMODULE is the same breach again: importing
                // it (direct, grouped, or aliased) lets a later signature
                // spell `value::Value` with neither fully qualified form on
                // any line — and importing the `json` MACRO builds a Value
                // with no import of the type at all. The CONVERSION
                // functions are the same breach through a group: `use
                // serde_json::{to_value as encode};` never spells the path
                // the line scan's doors look for, so any `to_value`/
                // `from_value` inside a serde_json use declaration is
                // flagged here, aliased or not.
                if declaration.contains("Value")
                    || declaration.contains("self")
                    || declaration.contains("::value")
                    || declaration.contains("{value")
                    || declaration.contains(",value")
                    || declaration.contains("::json")
                    || declaration.contains("{json")
                    || declaration.contains(",json")
                    || declaration.contains("to_value")
                    || declaration.contains("from_value")
                {
                    offenders.push(format!(
                        "{} — a use declaration imports serde_json Value, its value module, or the json! macro",
                        path.display()
                    ));
                }
                from = at + "useserde_json::".len();
            }
        }
    }
}
