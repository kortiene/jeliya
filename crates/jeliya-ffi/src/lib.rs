//! Minimal C-ABI shim over `jeliya-core` for the mobile in-process FFI spike
//! (Phase 2 abort gate). On mobile the Rust core cannot run as a sidecar
//! subprocess (iOS forbids it), so it must run IN-PROCESS behind FFI — this
//! crate is the thinnest possible proof of that path: a function Flutter calls
//! over `dart:ffi` that runs real core logic on the device/emulator.
//!
//! This is a spike, not the production FFI surface (that is a
//! `flutter_rust_bridge` layer later). It exists to answer one question on real
//! Android: does the core actually *run* in-process — CSPRNG, ed25519 keygen,
//! filesystem — not just link?

use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use jeliya_core::identity;

/// Create-or-load a Jeliya identity under `data_dir`, exercising the core's
/// ed25519 keypair generation, OS CSPRNG, and filesystem in-process. Returns a
/// newly-allocated C string (`created …` / `loaded …` / `err: …`) the caller
/// MUST release with [`jeliya_ffi_string_free`].
///
/// # Safety
/// `data_dir` must be a valid, NUL-terminated C string pointer (or null).
#[no_mangle]
pub extern "C" fn jeliya_ffi_identity_smoke(data_dir: *const c_char) -> *mut c_char {
    // A panic must never unwind across the C ABI (UB); contain it.
    let out = std::panic::catch_unwind(|| run_smoke(data_dir))
        .unwrap_or_else(|_| "err: panic in jeliya_ffi_identity_smoke".to_owned());
    CString::new(out)
        .unwrap_or_else(|_| CString::new("err: interior NUL in result").expect("static"))
        .into_raw()
}

fn run_smoke(data_dir: *const c_char) -> String {
    if data_dir.is_null() {
        return "err: null data_dir".to_owned();
    }
    // SAFETY: contract documented on the public fn; contained by catch_unwind.
    let dir = unsafe { CStr::from_ptr(data_dir) }
        .to_string_lossy()
        .into_owned();
    let path = Path::new(&dir);
    match identity::create(path) {
        Ok(p) => format!("created identity={} device={}", p.identity_id, p.device_id),
        // An identity already exists (a re-run) — load it, still proving the
        // core reads its on-disk state in-process.
        Err(_) => match identity::load_profile(path) {
            Ok(Some(p)) => format!("loaded identity={} device={}", p.identity_id, p.device_id),
            Ok(None) => "err: identity create failed and none on disk".to_owned(),
            Err(e) => format!("err: {e}"),
        },
    }
}

/// Free a string returned by [`jeliya_ffi_identity_smoke`].
///
/// # Safety
/// `s` must be a pointer returned by this library and not already freed.
#[no_mangle]
pub unsafe extern "C" fn jeliya_ffi_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: reclaims a CString this library leaked via into_raw.
        unsafe { drop(CString::from_raw(s)) };
    }
}
