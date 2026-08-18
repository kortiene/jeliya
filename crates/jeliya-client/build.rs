// Forwards ephemeral jeliyad test-daemon coordinates to the wasm32 browser
// test suite (tests/ws_web_browser.rs) as compile-time constants. The harness
// script (scripts/run-ws-web-browser-tests.sh) starts a supervised daemon,
// reads its portfile, and sets these env vars before invoking cargo.
//
// Only PORT and TOKEN are forwarded. The storage_generation and protocol
// version are discovered at runtime by the adapter itself via GET /api/health.
fn main() {
    for var in [
        "JELIYAD_WEB_TEST_PORT",
        "JELIYAD_WEB_TEST_TOKEN",
        "JELIYAD2_WEB_TEST_PORT",
        "JELIYAD2_WEB_TEST_TOKEN",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
        if let Ok(val) = std::env::var(var) {
            println!("cargo:rustc-env={var}={val}");
        }
    }
}
