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
    // The HTML referencing a URL is not the same as the file being there: a
    // marked-but-incomplete directory (a referenced module or the stylesheet
    // deleted, marker and index intact) must fail the build, not ship a UI
    // that 404s on every load or renders without the canonical design
    // system. "Some non-empty .wasm/.js exists" is not enough either — a
    // stale, differently named file would stand in for the one the shell
    // actually loads — so validate the exact root-relative references
    // index.html makes (they are root-relative by the SPA-fallback contract
    // stated in index.html itself).
    let module_refs: Vec<&str> = {
        let mut refs: Vec<&str> = index
            .split(['"', '\''])
            .filter(|t| {
                t.starts_with('/')
                    && (t.ends_with(".wasm") || t.ends_with(".js") || t.ends_with(".css"))
            })
            .collect();
        refs.sort_unstable();
        refs.dedup();
        refs
    };
    if !module_refs.iter().any(|r| r.ends_with(".wasm")) {
        fail(
            "the embedded index.html references no root-relative .wasm module — it is not the \
             Dioxus shell",
        );
    }
    if !module_refs.iter().any(|r| r.ends_with(".js")) {
        fail(
            "the embedded index.html references no root-relative .js bindgen glue — it is not \
             the Dioxus shell",
        );
    }
    if !module_refs.iter().any(|r| r.ends_with(".css")) {
        fail(
            "the embedded index.html references no root-relative stylesheet — the canonical \
             shell consumes the design system as /styles.css",
        );
    }
    for reference in &module_refs {
        let module = dist.join(reference.trim_start_matches('/'));
        println!("cargo:rerun-if-changed={}", module.display());
        let nonempty = std::fs::metadata(&module)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if !nonempty {
            fail(&format!(
                "the embedded UI is missing or has an empty {reference} — a file index.html \
                 references; the artifact is incomplete"
            ));
        }
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
