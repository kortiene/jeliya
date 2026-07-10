//! Android runtime smoke: run real `jeliya-core` logic on the device/emulator.
//! Cross-compiled for Android and executed via `adb shell`, it proves the core
//! (with the entire iroh-rooms + QUIC + ring stack LINKED in) actually RUNS on
//! Android — OS CSPRNG (getrandom syscall), ed25519 keygen, and filesystem —
//! not merely that it cross-compiles. This is the Phase 2 abort-gate proof for
//! the mobile in-process path.

use std::path::Path;

use jeliya_core::identity;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/data/local/tmp/jeliya-smoke-data".to_string());
    let path = Path::new(&dir);
    if let Err(e) = std::fs::create_dir_all(path) {
        println!("SMOKE ERR could not create {dir}: {e}");
        std::process::exit(1);
    }
    match identity::create(path) {
        Ok(p) => println!(
            "SMOKE OK created identity={} device={}",
            p.identity_id, p.device_id
        ),
        // A re-run: the identity already exists — load it, still exercising the
        // core's on-disk read path in-process.
        Err(_) => match identity::load_profile(path) {
            Ok(Some(p)) => println!(
                "SMOKE OK loaded identity={} device={}",
                p.identity_id, p.device_id
            ),
            Ok(None) => {
                println!("SMOKE ERR create failed and no identity on disk");
                std::process::exit(1);
            }
            Err(e) => {
                println!("SMOKE ERR {e}");
                std::process::exit(1);
            }
        },
    }
}
