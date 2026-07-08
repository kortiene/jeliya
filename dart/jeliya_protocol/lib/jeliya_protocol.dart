/// Transport-agnostic Dart client for the Jeliya daemon protocol
/// (docs/PROTOCOL.md). The desktop-sidecar WebSocket transport lives here;
/// a future mobile in-process FFI transport plugs in behind the same [Client].
library;

export 'src/protocol.dart';
export 'src/models.dart';
export 'src/methods.dart';
export 'src/ws_client.dart';
export 'src/supervisor.dart';
export 'src/daemon_http.dart';
export 'src/conventions.dart';
export 'src/conformance.dart';
// Test fixtures live in `package:jeliya_protocol/testing.dart` — deliberately
// NOT exported here so production code never depends on them.
