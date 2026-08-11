//! Refresh the dev-loop copy of the canonical stylesheet.
//!
//! `Dioxus.toml` declares `/styles.css` as a web resource, so the documented
//! `dx serve --features web` loop serves it from `asset_dir` (`assets/`) —
//! but the only pipeline that materializes the stylesheet is
//! `scripts/build-web.sh`, which `dx serve` does not run. Without this copy
//! the dev server renders an unstyled shell.
//!
//! The single source stays `ui/src/styles.css` (§7): the copy made here is
//! generated on every build, gitignored, and refreshed whenever the source
//! changes, so it cannot drift. The canonical artifact path performs its own
//! injection into `dist/` and never reads this one.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set");
    let manifest = Path::new(&manifest);
    let source = manifest.join("../../ui/src/styles.css");
    println!("cargo:rerun-if-changed={}", source.display());

    // The copy is a dev-loop convenience the canonical build never consumes:
    // warn rather than fail so an exotic checkout (read-only source tree)
    // cannot break the workspace build over it.
    if source.exists() {
        let assets = manifest.join("assets");
        let result = std::fs::create_dir_all(&assets)
            .and_then(|()| std::fs::copy(&source, assets.join("styles.css")).map(|_| ()));
        if let Err(error) = result {
            println!("cargo:warning=jeliya-ui: could not refresh assets/styles.css ({error}); `dx serve` will render unstyled");
        }
    }
}
