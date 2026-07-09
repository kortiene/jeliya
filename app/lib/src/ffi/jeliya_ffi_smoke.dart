/// The FFI beachhead: load `libjeliya_ffi.so` and run its identity smoke
/// IN-PROCESS inside the Flutter app, proving the native core (iroh-rooms +
/// QUIC + ring linked in) loads and runs in the app's own process — not just
/// as a standalone binary (that was the earlier on-device runtime proof).
///
/// This is the last de-risking step before the real in-process transport: an
/// `FfiClient implements Client` over an expanded jeliya-ffi (request/response
/// + a push StreamSink, via flutter_rust_bridge). It is intentionally tiny —
/// one synchronous call to the existing C ABI — and Android-only for now
/// (iOS links the same core as a staticlib via DynamicLibrary.process()).
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';

typedef _SmokeNative = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _SmokeDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _FreeNative = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

/// Runs `jeliya_ffi_identity_smoke(data_dir)` from the loaded native library
/// and returns its result line (`created identity=… device=…` /
/// `loaded identity=…`). Throws [ArgumentError]/[Exception] if the library
/// cannot be loaded or the symbols are missing.
String runFfiIdentitySmoke(String dataDir) {
  // Android resolves the soname against the APK's nativeLibraryDir.
  final lib = DynamicLibrary.open('libjeliya_ffi.so');
  final smoke =
      lib.lookupFunction<_SmokeNative, _SmokeDart>('jeliya_ffi_identity_smoke');
  final free =
      lib.lookupFunction<_FreeNative, _FreeDart>('jeliya_ffi_string_free');

  final dirPtr = dataDir.toNativeUtf8();
  try {
    final resultPtr = smoke(dirPtr);
    if (resultPtr == nullptr) return 'ffi smoke returned null';
    try {
      // The core panic-guards internally (catch_unwind), so a failure comes
      // back as an 'err: …' string, never an unwind across the C ABI.
      return resultPtr.toDartString();
    } finally {
      free(resultPtr); // reclaim the heap CString the core handed us
    }
  } finally {
    malloc.free(dirPtr);
  }
}
