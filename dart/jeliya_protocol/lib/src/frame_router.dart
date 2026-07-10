/// Shared routing for daemon→client wire frames. One decoder consumed by
/// every transport (the WebSocket sidecar today, the in-process FFI transport
/// next), so push fan-out, id correlation, and the drop-malformed rule cannot
/// drift between them. Package-private: not exported from the barrel — the
/// wire envelope is a transport implementation detail.
library;

import 'dart:async';
import 'dart:convert';

import 'protocol.dart';

/// Routes one raw frame from the daemon:
///
/// - `{'push': name, 'data': {...}}` → [onPush]. `data` of an unexpected
///   shape still delivers the push (empty payload) rather than throwing — a
///   future daemon may grow non-object payloads.
/// - `{'id': n, ...}` → the completer surrendered by [takePending] (null for
///   a late reply the caller gave up on), completed with `result` when
///   `ok == true`, else rejected with [RequestError.fromWire] (tolerant of
///   malformed error objects).
/// - Everything else is dropped silently. A frame the daemon sends must never
///   be able to kill the client: this runs inside a transport's data
///   callback, where an uncaught throw is an unhandled async error.
///   Forward-compat rule: drop what we cannot read.
void routeFrame(
  String raw, {
  required void Function(Push push) onPush,
  required Completer<dynamic>? Function(int id) takePending,
}) {
  try {
    _routeChecked(raw, onPush, takePending);
  } catch (_) {/* malformed frame — ignored */}
}

void _routeChecked(
  String raw,
  void Function(Push push) onPush,
  Completer<dynamic>? Function(int id) takePending,
) {
  final dynamic msg;
  try {
    msg = jsonDecode(raw);
  } catch (_) {
    return; // not JSON
  }
  if (msg is! Map) return;
  final frame = msg.cast<String, dynamic>();
  final push = frame['push'];
  if (push is String) {
    final data = frame['data'];
    onPush(Push(push, data is Map ? data.cast<String, dynamic>() : const {}));
    return;
  }
  final id = frame['id'];
  if (id is int) {
    final completer = takePending(id);
    if (completer == null) return; // late reply for a call we gave up on
    if (frame['ok'] == true) {
      completer.complete(frame['result']);
    } else {
      final error = frame['error'];
      completer.completeError(
          RequestError.fromWire(error is Map ? error.cast<String, dynamic>() : const {}));
    }
  }
}
