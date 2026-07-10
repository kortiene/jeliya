/// The in-process FFI transport: [FfiClient] runs the Rust engine inside the
/// app process (mobile — iOS forbids sidecar subprocesses) behind the same
/// `Client` interface as the WebSocket transport.
///
/// A separate entry point so `dart:ffi`/`dart:isolate` stay out of the main
/// `package:jeliya_protocol/jeliya_protocol.dart` barrel; import this only
/// where the engine library is actually linked or loadable.
library;

export 'src/ffi_client.dart';
