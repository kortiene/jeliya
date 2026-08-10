//! Build-time guard for the daemon's embedded web artifact (#176 §9).
//!
//! When (and only when) the packaged `embed-ui` feature is built, the daemon
//! embeds `crates/jeliya-ui/dist`. This guard makes accidental consumption of
//! React (`ui/dist`) output as the canonical artifact **fail closed**: the
//! `embed-ui` build fails unless the embedded folder carries the Dioxus
//! artifact marker and an `index.html` that loads the wasm module and carries
//! **no** Vite/React signature. A plain `cargo build` (no `embed-ui`) does
//! nothing here. #183 later replaces this marker with a content-addressed
//! sealed manifest and adds the runtime legacy-rejection.

use std::path::PathBuf;

fn main() {
    // Only the `embed-ui` build embeds — and therefore guards — the artifact.
    if std::env::var_os("CARGO_FEATURE_EMBED_UI").is_none() {
        return;
    }

    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let dist = manifest.join("../jeliya-ui/dist");
    // Rebuild the guard (and re-embed) whenever the artifact changes.
    println!("cargo:rerun-if-changed={}", dist.display());
    println!(
        "cargo:rerun-if-changed={}",
        dist.join(".dioxus-artifact").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );

    let marker = std::fs::read_to_string(dist.join(".dioxus-artifact")).unwrap_or_default();
    if !marker
        .lines()
        .any(|line| line.trim() == "renderer=dioxus-web")
    {
        fail(
            "the embedded UI is not a Dioxus artifact: crates/jeliya-ui/dist is missing or its \
             .dioxus-artifact marker does not declare renderer=dioxus-web",
        );
    }

    let index = match std::fs::read_to_string(dist.join("index.html")) {
        Ok(text) => text,
        Err(_) => fail("the embedded UI has no index.html"),
    };
    if !index.contains(".wasm") {
        fail("the embedded index.html does not load a wasm module — it is not the Dioxus shell");
    }
    // The HTML referencing a wasm URL is not the same as the module being
    // there: a marked-but-incomplete directory (wasm or JS glue deleted,
    // marker and index intact) must fail the build, not ship a UI that 404s
    // on every load. Require at least one non-empty .wasm and .js file.
    let mut has_wasm = false;
    let mut has_js = false;
    if let Ok(entries) = std::fs::read_dir(&dist) {
        for entry in entries.flatten() {
            let path = entry.path();
            let nonempty = entry.metadata().map(|m| m.len() > 0).unwrap_or(false);
            match path.extension().and_then(|e| e.to_str()) {
                Some("wasm") if nonempty => has_wasm = true,
                Some("js") if nonempty => has_js = true,
                _ => {}
            }
        }
    }
    if !has_wasm {
        fail("the embedded UI has no non-empty .wasm module — the artifact is incomplete");
    }
    if !has_js {
        fail("the embedded UI has no non-empty .js bindgen glue — the artifact is incomplete");
    }
    // Reject a React/Vite signature outright: a Vite dev entry, a Vite HMR
    // client, or a React source module must never be the embedded artifact.
    for signature in [
        "src/main.tsx",
        "src/main.jsx",
        "/@vite/client",
        "/@react-refresh",
        "__vite__",
    ] {
        if index.contains(signature) {
            fail(&format!(
                "the embedded index.html carries a React/Vite signature ({signature}); \
                 React output must not be embedded (#176 §9)"
            ));
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("embed-ui: {message}.");
    eprintln!("embed-ui: run `scripts/build-web.sh` to produce the Dioxus artifact first.");
    std::process::exit(1);
}
