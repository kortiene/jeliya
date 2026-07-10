//! Hand-written bindings to the Dart API Dynamic Linking surface — the
//! minimal subset this shim needs: one-time initialization plus posting
//! bytes / ints to a Dart `ReceivePort` from any thread. `build.rs` compiles
//! the SDK's `dart_api_dl.c` into this library; that object file defines
//! `Dart_InitializeApiDL` and the `Dart_PostCObject_DL` function-pointer slot
//! these externs bind against.
//!
//! Declarations mirror `dart_api_dl.h` / `dart_native_api.h` / `dart_api.h`
//! of the pinned Flutter SDK verbatim (names included, hence the allows).
//! The enum values and the `Dart_CObject` layout are versioned by
//! `DART_API_DL_MAJOR_VERSION`: an SDK upgrade that bumps it must be checked
//! against this file.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::ffi::{c_char, c_void};
use std::sync::{Mutex, PoisonError};

/// `typedef int64_t Dart_Port_DL;` (dart_api_dl.h).
pub(crate) type Dart_Port_DL = i64;

// `Dart_CObject_Type` (dart_native_api.h). A C enum is `int` on every target
// this crate ships (macOS, iOS, Android arm32/arm64/x86_64).
const Dart_CObject_kInt64: i32 = 3;
const Dart_CObject_kTypedData: i32 = 7;
// `Dart_TypedData_Type` (dart_api.h).
const Dart_TypedData_kUint8: i32 = 2;

/// `struct _Dart_CObject` (dart_native_api.h). Only the `kInt64` and
/// `kTypedData` arms are ever constructed here; the rest exist so the union
/// (and therefore the struct the VM reads) has the exact C size/alignment.
#[repr(C)]
struct Dart_CObject {
    r#type: i32,
    value: Dart_CObject_Value,
}

#[repr(C)]
union Dart_CObject_Value {
    as_bool: bool,
    as_int32: i32,
    as_int64: i64,
    as_double: f64,
    as_string: *const c_char,
    as_send_port: Dart_CObject_SendPort,
    as_capability: Dart_CObject_Capability,
    as_array: Dart_CObject_Array,
    as_typed_data: Dart_CObject_TypedData,
    as_external_typed_data: Dart_CObject_ExternalTypedData,
    as_native_pointer: Dart_CObject_NativePointer,
}

// Layout-only union arms (never constructed or read from Rust).
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Dart_CObject_SendPort {
    id: Dart_Port_DL,
    origin_id: Dart_Port_DL,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Dart_CObject_Capability {
    id: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Dart_CObject_Array {
    length: isize,
    values: *mut *mut Dart_CObject,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Dart_CObject_TypedData {
    r#type: i32,
    /// In elements, not bytes (same thing for Uint8).
    length: isize,
    values: *const u8,
}

/// `void (*Dart_HandleFinalizer)(void*, void*)` — pointer-sized, layout only.
type Dart_HandleFinalizer = Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>;

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Dart_CObject_ExternalTypedData {
    r#type: i32,
    length: isize,
    data: *mut u8,
    peer: *mut c_void,
    callback: Dart_HandleFinalizer,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Dart_CObject_NativePointer {
    ptr: isize,
    size: isize,
    callback: Dart_HandleFinalizer,
}

type Dart_PostCObject_Type = Option<unsafe extern "C" fn(Dart_Port_DL, *mut Dart_CObject) -> bool>;

extern "C" {
    /// Defined in `dart_api_dl.c` (compiled by build.rs). Resolves every
    /// `*_DL` stub from `NativeApi.initializeApiDLData`; returns 0 on success.
    fn Dart_InitializeApiDL(data: *mut c_void) -> isize;

    /// The function-pointer slot `dart_api_dl.c` defines: NULL until
    /// `Dart_InitializeApiDL` succeeds, then the VM's `Dart_PostCObject`.
    static mut Dart_PostCObject_DL: Dart_PostCObject_Type;
}

/// Whether `Dart_InitializeApiDL` already succeeded in this process. The
/// guard exists because the resolver REWRITES every `*_DL` slot with plain
/// (non-atomic) stores on each call, while engine-runtime threads read
/// `Dart_PostCObject_DL` unsynchronized inside every post — and the Dart
/// side re-runs init on every `start()` (hot restart, restart-after-stop).
/// First success wins; later calls become validated no-ops.
static INITIALIZED: Mutex<bool> = Mutex::new(false);

/// Resolve the DL stubs; true when `Dart_InitializeApiDL` returned 0 (now or
/// on an earlier call — the slots are only ever written once per process).
///
/// # Safety
/// `data` must be the value of `NativeApi.initializeApiDLData` from the Dart
/// VM hosting this library, passed through unmodified.
pub(crate) unsafe fn initialize(data: *mut c_void) -> bool {
    let mut done = INITIALIZED.lock().unwrap_or_else(PoisonError::into_inner);
    if *done {
        return true;
    }
    // SAFETY: contract forwarded from the caller.
    *done = (unsafe { Dart_InitializeApiDL(data) }) == 0;
    *done
}

fn post_cobject() -> Dart_PostCObject_Type {
    // SAFETY: a by-value read of the slot `dart_api_dl.c` defines. The
    // INITIALIZED guard above ensures it is written at most once per process
    // (a successful Dart_InitializeApiDL never re-runs), the Dart side must
    // complete that init before any engine export can trigger a post, and
    // the slot is only read from then on.
    unsafe { Dart_PostCObject_DL }
}

/// Post `bytes` to `port` as a Dart `Uint8List` (the VM copies the buffer
/// before returning, so the caller's ownership is untouched). False when the
/// DL stubs were never initialized or the port is closed/illegal.
pub(crate) fn post_bytes(port: Dart_Port_DL, bytes: &[u8]) -> bool {
    let Some(post) = post_cobject() else {
        return false;
    };
    let mut message = Dart_CObject {
        r#type: Dart_CObject_kTypedData,
        value: Dart_CObject_Value {
            as_typed_data: Dart_CObject_TypedData {
                r#type: Dart_TypedData_kUint8,
                length: isize::try_from(bytes.len()).unwrap_or(isize::MAX),
                values: bytes.as_ptr(),
            },
        },
    };
    // SAFETY: `post` is the VM's Dart_PostCObject (non-NULL only after a
    // successful Dart_InitializeApiDL, callable from any thread); `message`
    // and the byte buffer it points at outlive the call, and the VM copies
    // the object graph before returning.
    unsafe { post(port, &mut message) }
}

/// Post one `int` to `port`. False when uninitialized or the port is
/// closed/illegal.
pub(crate) fn post_int(port: Dart_Port_DL, value: i64) -> bool {
    let Some(post) = post_cobject() else {
        return false;
    };
    let mut message = Dart_CObject {
        r#type: Dart_CObject_kInt64,
        value: Dart_CObject_Value { as_int64: value },
    };
    // SAFETY: as in `post_bytes`; an int carries no out-of-struct data.
    unsafe { post(port, &mut message) }
}
