/// Transport-agnostic Dart client for the Jeliya daemon protocol
/// (docs/PROTOCOL.md). The desktop-sidecar WebSocket transport lives here;
/// a future mobile in-process FFI transport plugs in behind the same [Client].
library;

export 'src/protocol.dart';
export 'src/ws_client.dart';
export 'src/supervisor.dart';
export 'src/conformance.dart';
