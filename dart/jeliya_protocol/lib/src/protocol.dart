/// The wire envelope and client surface for the Jeliya daemon protocol
/// (docs/PROTOCOL.md). Kept transport-agnostic: [Client] is implemented by the
/// WebSocket sidecar transport today and by an in-process FFI transport later
/// — nothing above this layer knows which.
library;

import 'dart:async';

/// A push frame (`room.event`, `peers.changed`) delivered out of band.
class Push {
  const Push(this.name, this.data);

  /// The push name, e.g. `room.event`.
  final String name;

  /// The `data` object of the push.
  final Map<String, dynamic> data;
}

/// The four client-observed connection states (docs/PROTOCOL.md, Connection
/// lifecycle). These are a client concern, never a wire message.
enum ConnectionState { connecting, connected, reconnecting, disconnected }

/// A failed response, or a client-synthesized transport error. `code` mirrors
/// the daemon taxonomy plus the reserved client-synthesized `connection_lost`.
class RequestError implements Exception {
  RequestError(this.code, this.message, {this.hint});

  /// Build from a daemon `error` object.
  factory RequestError.fromWire(Map<String, dynamic> error) => RequestError(
        (error['code'] as String?) ?? 'internal',
        (error['message'] as String?) ?? 'request failed',
        hint: error['hint'] as String?,
      );

  final String code;
  final String message;
  final String? hint;

  @override
  String toString() => 'RequestError($code): $message${hint != null ? ' — $hint' : ''}';
}

/// The transport-agnostic client surface. Mirrors `ui/src/lib/protocol.ts`
/// `Client`, so the reference TypeScript client, this Dart client, and a future
/// FFI client all satisfy the same conformance corpus.
abstract class Client {
  /// Begin connecting (idempotent). Completes when the first connection is
  /// established, or after the first failed attempt (it keeps retrying).
  Future<void> start();

  /// Stop and release the transport. In-flight and queued calls reject with
  /// `connection_lost`.
  Future<void> stop();

  ConnectionState get state;

  /// Connection-state transitions.
  Stream<ConnectionState> get states;

  /// All push frames. Filter by [Push.name].
  Stream<Push> get pushes;

  /// Issue one request; resolves with the `result` object or throws
  /// [RequestError]. `params` defaults to `{}`.
  Future<dynamic> call(String method, [Map<String, dynamic>? params]);

  /// Human-readable transport description.
  String describe();
}
