//! The production C-ABI surface over `jeliya-core` for the mobile in-process
//! transport. On mobile the Rust core cannot run as a sidecar subprocess
//! (iOS forbids it), so it runs IN-PROCESS behind this hand-rolled bridge:
//! Dart calls the `jeliya_engine_*` exports over `dart:ffi`, and every reply
//! envelope and push frame travels back through one Dart `NativePort`
//! (`Dart_PostCObject_DL`) as UTF-8 JSON bytes — the same envelope frames the
//! WebSocket daemon speaks, so the golden conformance corpus holds for this
//! transport by construction.
//!
//! Hand-rolled rather than flutter_rust_bridge — a decision, not a stopgap.
//! The repo-verified reasons, and when to revisit:
//! - The surface is one string-typed seam (`Engine::handle_frame(&str) ->
//!   String` plus pre-serialized push frames), so FRB's typed codegen buys
//!   nothing here, while a typed fn-per-method bridge would freeze the
//!   24-method list and foreclose the protocol's reserved headroom
//!   (`client_msg_id`, `after_event_id`, `delivery`, `min_protocol`) that
//!   JSON pass-through carries with no bridge change.
//! - Both of FRB's blessed Android build integrations were already rejected
//!   here: cargokit is archived and cargo-ndk 4.1.2 panics under this repo's
//!   asdf-managed Rust, so scripts/build-android-libs.mjs drives the NDK
//!   clang directly — a pipeline a hand-rolled bridge leaves untouched.
//! - Zero new dependencies under the pinned-toolchain posture: `cc` and
//!   `tokio` were already in Cargo.lock and `dart_api_dl.c` ships with the
//!   pinned Flutter SDK, where FRB adds a crates.io dep tree, a pub.dev
//!   package, and a separately installed codegen binary version-locked to
//!   both.
//! - The acceptance gate is the golden corpus replaying under plain
//!   `dart test` on the host, which requires `FfiClient` to live inside
//!   pub-dependency-free `jeliya_protocol`; an FRB-generated client depends
//!   on package:flutter_rust_bridge and cannot.
//!
//! Revisit FRB only if this surface grows typed or streaming APIs beyond
//! JSON frames.
//!
//! Contracts every export upholds:
//! - a panic never crosses the ABI (UB): `catch_unwind` at each entry point;
//! - no export blocks: engine work runs on this crate's own multi-thread
//!   tokio runtime ([`host`]), never on Flutter's UI thread;
//! - Dart owns request buffers ([`jeliya_ffi_alloc`] → write UTF-8+NUL →
//!   call → [`jeliya_ffi_dealloc`]); Rust copies before returning, and all
//!   Rust→Dart data travels by value through the port (no shared pointers).
//!
//! [`jeliya_ffi_identity_smoke`] predates this surface — it is the Phase 2
//! runtime proof (does the core RUN in-process on real Android: CSPRNG,
//! ed25519 keygen, filesystem) and stays exported until the app's last use
//! of it is retired.

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::catch_unwind;
use std::path::Path;

use jeliya_core::identity;

mod dart_api;
mod host;

// Return codes shared by the `jeliya_engine_*` / `jeliya_ffi_init_dart_api`
// exports: 0 success, 1 the one non-error extra state, negatives errors.
// The Dart client mirrors these by value.

/// Success.
pub const JELIYA_FFI_OK: i32 = 0;
/// `jeliya_engine_start` adopted a live engine (hot restart, same data dir).
pub const JELIYA_FFI_ADOPTED: i32 = 1;
/// Null pointer, non-UTF-8 string, or rejected DL init data.
pub const JELIYA_FFI_ERR_INVALID_ARG: i32 = -1;
/// No engine is running (`jeliya_engine_request` / `jeliya_engine_stop`).
pub const JELIYA_FFI_ERR_NOT_STARTED: i32 = -2;
/// An engine is live over a DIFFERENT data dir; stop it first.
pub const JELIYA_FFI_ERR_DATA_DIR_MISMATCH: i32 = -3;
/// Engine construction failed (bad dir, locked store, …).
pub const JELIYA_FFI_ERR_ENGINE: i32 = -4;
/// A panic was caught at the ABI boundary.
pub const JELIYA_FFI_ERR_PANIC: i32 = -5;
/// An engine is live over the SAME data dir but with a different
/// configuration (`loopback`); adopting it would serve the wrong mode.
pub const JELIYA_FFI_ERR_CONFIG_MISMATCH: i32 = -6;

/// One-time per process: hand `NativeApi.initializeApiDLData` over so this
/// library can post to Dart `ReceivePort`s from any thread. Must return 0
/// before [`jeliya_engine_start`] — otherwise every reply and push frame is
/// silently dropped.
///
/// # Safety
/// `init_data` must be the value of `NativeApi.initializeApiDLData` from the
/// Dart VM hosting this library (or null, which yields -1).
#[no_mangle]
pub unsafe extern "C" fn jeliya_ffi_init_dart_api(init_data: *mut c_void) -> i32 {
    catch_unwind(|| {
        if init_data.is_null() {
            return JELIYA_FFI_ERR_INVALID_ARG;
        }
        // SAFETY: contract documented above, forwarded to the DL stub.
        if unsafe { dart_api::initialize(init_data) } {
            JELIYA_FFI_OK
        } else {
            JELIYA_FFI_ERR_INVALID_ARG
        }
    })
    .unwrap_or(JELIYA_FFI_ERR_PANIC)
}

/// Construct-or-rebind the process-singleton engine over `data_dir_utf8`,
/// with every reply envelope and push frame posted to `frames_port` (a Dart
/// `SendPort.nativePort`) as UTF-8 JSON bytes. Requests are dispatched
/// strictly one at a time, like one WebSocket connection's frames.
///
/// Returns [`JELIYA_FFI_OK`] for a fresh engine (push loop, dispatch task,
/// frames drain and `daemon.shutdown` teardown watcher spawned);
/// [`JELIYA_FFI_ADOPTED`] when a live engine on the same canonical data dir
/// with the same `loopback` flag was adopted (hot restart — the frames port
/// is rebound, nothing is rebuilt); [`JELIYA_FFI_ERR_DATA_DIR_MISMATCH`]
/// when an engine is live over a different data dir (a completed
/// [`jeliya_engine_stop`] makes a new dir legal);
/// [`JELIYA_FFI_ERR_CONFIG_MISMATCH`] when the live engine's `loopback`
/// differs (adopting it would serve a mode `daemon.status` contradicts);
/// [`JELIYA_FFI_ERR_INVALID_ARG`] / [`JELIYA_FFI_ERR_ENGINE`] /
/// [`JELIYA_FFI_ERR_PANIC`] on failure.
///
/// Adoption exists for hot restarts and cannot distinguish one from a second
/// coexisting caller: a process must hold AT MOST ONE live client of this
/// API at a time, or the older client's replies silently reroute to the
/// newest frames port.
///
/// # Safety
/// `data_dir_utf8` must be a valid NUL-terminated UTF-8 C string (or null,
/// which yields -1) that stays valid for the duration of this call; the
/// bytes are copied out, the caller keeps ownership.
#[no_mangle]
pub unsafe extern "C" fn jeliya_engine_start(
    data_dir_utf8: *const c_char,
    loopback: bool,
    frames_port: i64,
) -> i32 {
    catch_unwind(|| {
        if data_dir_utf8.is_null() {
            return JELIYA_FFI_ERR_INVALID_ARG;
        }
        // SAFETY: contract documented above; contained by catch_unwind.
        let raw = unsafe { CStr::from_ptr(data_dir_utf8) };
        let Ok(data_dir) = raw.to_str() else {
            return JELIYA_FFI_ERR_INVALID_ARG;
        };
        host::start(data_dir, loopback, frames_port)
    })
    .unwrap_or(JELIYA_FFI_ERR_PANIC)
}

/// Submit one request frame `{id, method, params}` (UTF-8 JSON,
/// NUL-terminated). Non-blocking: the frame is copied, dispatched on the
/// engine runtime, and the reply envelope is posted to the frames port —
/// correlate by envelope `id` on the Dart side. Returns [`JELIYA_FFI_OK`]
/// once queued, [`JELIYA_FFI_ERR_NOT_STARTED`] when no engine is running.
///
/// # Safety
/// `frame_utf8` must be a valid NUL-terminated UTF-8 C string (or null,
/// which yields -1) that stays valid for the duration of this call; the
/// bytes are copied out before returning, so the caller may free it (via
/// [`jeliya_ffi_dealloc`]) as soon as this returns.
#[no_mangle]
pub unsafe extern "C" fn jeliya_engine_request(frame_utf8: *const c_char) -> i32 {
    catch_unwind(|| {
        if frame_utf8.is_null() {
            return JELIYA_FFI_ERR_INVALID_ARG;
        }
        // SAFETY: contract documented above; contained by catch_unwind.
        let raw = unsafe { CStr::from_ptr(frame_utf8) };
        let Ok(frame) = raw.to_str() else {
            return JELIYA_FFI_ERR_INVALID_ARG;
        };
        host::request(frame.to_owned())
    })
    .unwrap_or(JELIYA_FFI_ERR_PANIC)
}

/// Tear the engine down: stop the push loop and the dispatch task, close
/// every room (bounded internally at 10s; each clean close releases that
/// room's rooms.db handles and blob locks via `Node::shutdown`), drop the
/// engine, then post one completion int to `done_port` — `0` for a clean
/// teardown, `1` when rooms remained open past the close budget (their
/// on-disk stores may stay locked until the process exits; the caller must
/// not report that stop as clean). Returns immediately with
/// [`JELIYA_FFI_OK`]; the Dart side awaits `done_port` with its own timeout
/// before it may start an engine over a different data dir. Returns
/// [`JELIYA_FFI_ERR_NOT_STARTED`] when no engine is running (nothing is
/// posted).
#[no_mangle]
pub extern "C" fn jeliya_engine_stop(done_port: i64) -> i32 {
    catch_unwind(|| host::stop(done_port)).unwrap_or(JELIYA_FFI_ERR_PANIC)
}

/// Allocate `len` bytes for a request frame, so the Dart side needs no pub
/// packages (`dart:ffi` core ships no allocator). Zero-filled. Release with
/// [`jeliya_ffi_dealloc`] and the same `len`. Null on failure.
#[no_mangle]
pub extern "C" fn jeliya_ffi_alloc(len: usize) -> *mut u8 {
    catch_unwind(|| {
        // A boxed slice's allocation layout is exactly `len` bytes, so the
        // (ptr, len) pair alone can reconstruct it for deallocation.
        let buf = vec![0u8; len].into_boxed_slice();
        Box::into_raw(buf).cast::<u8>()
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Release a buffer from [`jeliya_ffi_alloc`].
///
/// # Safety
/// `ptr` must have come from [`jeliya_ffi_alloc`] with this exact `len` and
/// not already be deallocated (null is a no-op).
#[no_mangle]
pub unsafe extern "C" fn jeliya_ffi_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let _ = catch_unwind(|| {
        // SAFETY: (ptr, len) reconstruct the boxed slice `jeliya_ffi_alloc`
        // leaked, per the contract above.
        unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len))) };
    });
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn start_with(dir: &Path, loopback: bool, frames_port: i64) -> i32 {
        let c = CString::new(dir.to_str().expect("utf-8 temp path")).expect("no NUL");
        // SAFETY: `c` is a valid NUL-terminated UTF-8 string, live across the call.
        unsafe { jeliya_engine_start(c.as_ptr(), loopback, frames_port) }
    }

    fn start(dir: &Path, frames_port: i64) -> i32 {
        start_with(dir, true, frames_port)
    }

    fn request(frame: &str) -> i32 {
        let c = CString::new(frame).expect("no NUL");
        // SAFETY: `c` is a valid NUL-terminated UTF-8 string, live across the call.
        unsafe { jeliya_engine_request(c.as_ptr()) }
    }

    /// Poll until the engine singleton is gone (teardown runs on the engine
    /// runtime, asynchronously to the caller).
    fn wait_until_stopped() {
        let deadline = Instant::now() + Duration::from_secs(10);
        while request(r#"{"id":0,"method":"daemon.status"}"#) != JELIYA_FFI_ERR_NOT_STARTED {
            assert!(
                Instant::now() < deadline,
                "engine teardown did not complete within 10s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // HOST is process-global state: every lifecycle assertion lives in this
    // ONE sequential test so parallel test threads cannot interleave engines.
    // Posts go to fake ports with no Dart VM attached — Dart_PostCObject_DL
    // is NULL, so every post is a silent no-op by design.
    #[test]
    fn engine_singleton_state_machine() {
        let dir_a = TempDir::new().expect("tempdir");
        let dir_b = TempDir::new().expect("tempdir");

        // Nothing started yet.
        assert_eq!(
            request(r#"{"id":1,"method":"daemon.status"}"#),
            JELIYA_FFI_ERR_NOT_STARTED
        );
        assert_eq!(jeliya_engine_stop(0), JELIYA_FFI_ERR_NOT_STARTED);

        // Null and non-UTF-8 arguments are refused, not UB.
        // SAFETY: null is explicitly part of the exports' contract.
        assert_eq!(
            unsafe { jeliya_engine_start(std::ptr::null(), true, 1) },
            JELIYA_FFI_ERR_INVALID_ARG
        );
        // SAFETY: as above.
        assert_eq!(
            unsafe { jeliya_engine_request(std::ptr::null()) },
            JELIYA_FFI_ERR_INVALID_ARG
        );

        // Fresh start.
        assert_eq!(start(dir_a.path(), 100), JELIYA_FFI_OK);
        // Hot restart: the same dir — even spelled non-canonically — adopts
        // the live engine instead of rebuilding it.
        assert_eq!(start(&dir_a.path().join("."), 101), JELIYA_FFI_ADOPTED);
        // The same dir with a DIFFERENT loopback flag must not be adopted:
        // the live engine's mode would contradict the caller's request.
        assert_eq!(
            start_with(dir_a.path(), false, 101),
            JELIYA_FFI_ERR_CONFIG_MISMATCH
        );
        // A different data dir while an engine is live is refused.
        assert_eq!(start(dir_b.path(), 102), JELIYA_FFI_ERR_DATA_DIR_MISMATCH);

        // Requests queue fire-and-forget (the reply post is dropped without
        // a Dart VM; what is under test is the accept/queue path).
        assert_eq!(
            request(r#"{"id":2,"method":"daemon.status"}"#),
            JELIYA_FFI_OK
        );
        assert_eq!(request("not json at all"), JELIYA_FFI_OK);

        // daemon.shutdown honesty: {shutting_down:true} must be followed by
        // real teardown — the singleton empties WITHOUT an explicit stop.
        assert_eq!(
            request(r#"{"id":3,"method":"daemon.shutdown"}"#),
            JELIYA_FFI_OK
        );
        wait_until_stopped();

        // After a completed teardown, a NEW data dir is legal.
        assert_eq!(start(dir_b.path(), 103), JELIYA_FFI_OK);
        assert_eq!(jeliya_engine_stop(0), JELIYA_FFI_OK);
        // stop() empties the singleton synchronously: a second stop has
        // nothing left even before the async teardown finishes.
        assert_eq!(jeliya_engine_stop(0), JELIYA_FFI_ERR_NOT_STARTED);
        wait_until_stopped();
    }

    #[test]
    fn alloc_dealloc_round_trip() {
        let ptr = jeliya_ffi_alloc(16);
        assert!(!ptr.is_null());
        // SAFETY: 16 bytes were just allocated at `ptr`; dealloc uses the
        // same (ptr, len) pair per the contract.
        unsafe {
            std::ptr::write_bytes(ptr, 0x41, 16);
            jeliya_ffi_dealloc(ptr, 16);
        }

        // Zero-length allocations round-trip too (dangling, never read).
        let empty = jeliya_ffi_alloc(0);
        assert!(!empty.is_null());
        // SAFETY: (empty, 0) is exactly what jeliya_ffi_alloc(0) returned.
        unsafe { jeliya_ffi_dealloc(empty, 0) };

        // Null is a documented no-op.
        // SAFETY: null short-circuits before any deallocation.
        unsafe { jeliya_ffi_dealloc(std::ptr::null_mut(), 8) };
    }
}
