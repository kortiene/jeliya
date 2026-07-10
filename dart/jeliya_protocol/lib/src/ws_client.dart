/// WebSocket transport for the desktop sidecar path: talks to a local jeliyad
/// over `ws://127.0.0.1:<port>/ws`. Behavior is ported 1:1 from the reference
/// client (`ui/src/lib/client.ts`): numeric-id request/response correlation,
/// push fan-out, exponential backoff reconnect with jitter, an offline send
/// queue flushed on open, and a fresh auth token fetched every attempt so a
/// daemon restart (new per-start token) heals through the reconnect loop.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'frame_router.dart';
import 'protocol.dart';

const Duration _backoffBase = Duration(milliseconds: 500);
const Duration _backoffMax = Duration(seconds: 8);

class _Pending {
  _Pending(this.completer);
  final Completer<dynamic> completer;
}

/// Resolves the current `ws://…/ws?token=…` URL. Re-invoked every connect
/// attempt: a daemon restart mints a new token, and re-resolving heals it.
typedef WsUrlResolver = Future<Uri> Function();

class WsClient implements Client {
  WsClient(this._resolveUrl, {Random? random}) : _random = random ?? Random();

  final WsUrlResolver _resolveUrl;
  final Random _random;

  WebSocket? _ws;
  int _nextId = 1;
  int _attempts = 0;
  int _openSeq = 0;
  bool _stopped = true;
  Uri? _lastUrl;

  final Map<int, _Pending> _pending = {};
  final Set<int> _sent = {}; // ids actually written to the current socket
  final List<({int id, String frame})> _queue = []; // waiting for open

  ConnectionState _state = ConnectionState.disconnected;
  final StreamController<ConnectionState> _states = StreamController.broadcast();
  final StreamController<Push> _pushes = StreamController.broadcast();
  Timer? _reconnectTimer;
  Completer<void>? _firstConnect;

  @override
  ConnectionState get state => _state;
  @override
  Stream<ConnectionState> get states => _states.stream;
  @override
  Stream<Push> get pushes => _pushes.stream;
  @override
  String describe() {
    final url = _lastUrl;
    if (url == null) return 'ws (unconnected)';
    // Rebuild without the query: `replace(queryParameters: {})` keeps an empty
    // query and renders a trailing '?'.
    return Uri(scheme: url.scheme, host: url.host, port: url.port, path: url.path).toString();
  }

  @override
  Future<void> start() {
    if (!_stopped) return _firstConnect?.future ?? Future.value();
    _stopped = false;
    _attempts = 0;
    _firstConnect = Completer<void>();
    _open(ConnectionState.connecting);
    return _firstConnect!.future;
  }

  @override
  Future<void> stop() async {
    _stopped = true;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _openSeq++; // invalidate any in-flight open
    // Resolve a pending start() future so a caller awaiting it never hangs
    // after a stop() (and is not orphaned by a later start() replacing it).
    if (_firstConnect != null && !_firstConnect!.isCompleted) _firstConnect!.complete();
    final ws = _ws;
    _ws = null;
    await ws?.close();
    _failInFlight('client stopped');
    _failQueued('client stopped');
    _setState(ConnectionState.disconnected);
  }

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    final completer = Completer<dynamic>();
    if (_stopped) {
      completer.completeError(RequestError('connection_lost', 'client is stopped'));
      return completer.future;
    }
    final id = _nextId++;
    _pending[id] = _Pending(completer);
    final frame = jsonEncode({'id': id, 'method': method, 'params': params ?? const {}});
    final ws = _ws;
    if (ws != null && _state == ConnectionState.connected) {
      ws.add(frame);
      _sent.add(id);
    } else {
      _queue.add((id: id, frame: frame)); // flush on open
    }
    return completer.future;
  }

  void _setState(ConnectionState next) {
    if (_state == next) return;
    _state = next;
    if (!_states.isClosed) _states.add(next);
  }

  Future<void> _open(ConnectionState opening) async {
    _reconnectTimer = null;
    _setState(opening);
    final seq = ++_openSeq;
    Uri url;
    try {
      url = await _resolveUrl(); // fresh token every attempt
    } catch (_) {
      if (_stopped || seq != _openSeq) return; // a stop() during resolve must not reschedule
      _scheduleReconnect();
      return;
    }
    if (_stopped || seq != _openSeq) return;
    _lastUrl = url;
    WebSocket ws;
    try {
      ws = await WebSocket.connect(url.toString());
    } catch (_) {
      if (_stopped || seq != _openSeq) return;
      _scheduleReconnect();
      return;
    }
    if (_stopped || seq != _openSeq) {
      await ws.close();
      return;
    }
    _ws = ws;
    _attempts = 0;
    // Flush the offline queue.
    final queued = List.of(_queue);
    _queue.clear();
    for (final q in queued) {
      ws.add(q.frame);
      _sent.add(q.id);
    }
    _setState(ConnectionState.connected);
    if (_firstConnect != null && !_firstConnect!.isCompleted) _firstConnect!.complete();
    ws.listen(
      (data) {
        if (ws != _ws) return;
        if (data is String) {
          _handleFrame(data);
        } else if (data is List<int>) {
          _handleFrame(utf8.decode(data, allowMalformed: true));
        }
      },
      onDone: () {
        if (ws != _ws) return;
        _ws = null;
        _failInFlight('connection to daemon lost');
        if (!_stopped) _scheduleReconnect();
      },
      onError: (_) {/* onDone follows */},
      cancelOnError: false,
    );
  }

  void _scheduleReconnect() {
    // Honor the documented start() contract: the first attempt has failed, so
    // resolve any pending start() future — the client keeps retrying in the
    // background regardless.
    if (_firstConnect != null && !_firstConnect!.isCompleted) _firstConnect!.complete();
    // Clamp the exponent BEFORE the shift: `1 << _attempts` is a native int64
    // shift, so an unbounded `_attempts` overflows to a negative product and
    // `min(max, product)` returns it — turning the 8s cap into an immediate
    // retry loop. Clamping keeps the cap intact for arbitrarily long downtime.
    final exp = _attempts < 20 ? _attempts : 20;
    final base = min(_backoffMax.inMilliseconds, _backoffBase.inMilliseconds * (1 << exp));
    final delay = base + _random.nextInt(250);
    _attempts++;
    _setState(ConnectionState.reconnecting);
    _reconnectTimer = Timer(Duration(milliseconds: delay), () => _open(ConnectionState.reconnecting));
  }

  /// Reject requests written to a socket that just died; queued (never-sent)
  /// requests stay queued for the next connection.
  void _failInFlight(String message) {
    final err = RequestError('connection_lost', message, hint: 'is jeliyad running?');
    for (final id in _sent) {
      final p = _pending.remove(id);
      p?.completer.completeError(err);
    }
    _sent.clear();
  }

  void _failQueued(String message) {
    final err = RequestError('connection_lost', message, hint: 'is jeliyad running?');
    for (final q in _queue) {
      final p = _pending.remove(q.id);
      p?.completer.completeError(err);
    }
    _queue.clear();
  }

  // Envelope decoding lives in the shared frame router (frame_router.dart);
  // this transport only contributes the closed-controller guard and the
  // `_sent` bookkeeping alongside id correlation.
  void _handleFrame(String raw) {
    routeFrame(
      raw,
      onPush: (push) {
        if (!_pushes.isClosed) _pushes.add(push);
      },
      takePending: (id) {
        final p = _pending.remove(id);
        if (p == null) return null;
        _sent.remove(id);
        return p.completer;
      },
    );
  }
}
