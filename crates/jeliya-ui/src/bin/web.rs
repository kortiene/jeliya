//! The browser (`wasm32-unknown-unknown`) entry point (§3).
//!
//! Built only under the `web` feature (`required-features = ["web"]`), so the
//! DOM renderer and its `dioxus/web` graph are compiled solely for the
//! dedicated web job — never by the MSRV/clippy `--workspace --all-targets`
//! jobs. It launches [`jeliya_ui::compose::WebRoot`], which selects the browser
//! composition (the deterministic mock behind a `ClientHandle`, the
//! browser-appropriate `PlatformServices`) and renders the shared shell.
//!
//! This binary opens **no socket**: #176 renders against the mock; the real
//! `WsWeb` transport (#168) slots in behind the same handle later.

fn main() {
    dioxus::launch(jeliya_ui::compose::WebRoot);
}
