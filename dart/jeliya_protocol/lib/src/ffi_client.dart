/// In-process FFI transport for mobile: [FfiClient] drives the Rust engine
/// compiled into the app process (`crates/jeliya-ffi`) behind the same
/// [Client] surface as the WebSocket sidecar transport. Request frames go in
/// through `jeliya_engine_request`; every reply envelope and push frame comes
/// back through ONE Dart `ReceivePort` as UTF-8 JSON bytes — the same
/// envelope frames the daemon speaks, decoded by the shared frame router, so
/// the golden conformance corpus holds for this transport by construction.
///
/// SDK-only imports (`dart:ffi`, `dart:isolate`): the package stays
/// pub-dependency-free, which is what lets the corpus replay against the
/// engine under plain `dart test` on the host.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'frame_router.dart';
import 'protocol.dart';

// Return codes mirrored by value from crates/jeliya-ffi/src/lib.rs.
const int _rcOk = 0; // JELIYA_FFI_OK
const int _rcAdopted = 1; // JELIYA_FFI_ADOPTED (hot restart, same data dir)
const int _rcNotStarted = -2; // JELIYA_FFI_ERR_NOT_STARTED
const int _rcDataDirMismatch = -3; // JELIYA_FFI_ERR_DATA_DIR_MISMATCH
const int _rcEngine = -4; // JELIYA_FFI_ERR_ENGINE
const int _rcConfigMismatch = -6; // JELIYA_FFI_ERR_CONFIG_MISMATCH

// The exported C ABI (crates/jeliya-ffi/src/lib.rs). Request buffers use the
// library's own allocator so this file needs no pub allocator package.
typedef _InitDartApiNative = Int32 Function(Pointer<Void> initData);
typedef _InitDartApiDart = int Function(Pointer<Void> initData);
typedef _EngineStartNative = Int32 Function(
    Pointer<Uint8> dataDirUtf8, Bool loopback, Int64 framesPort);
typedef _EngineStartDart = int Function(Pointer<Uint8> dataDirUtf8, bool loopback, int framesPort);
typedef _EngineRequestNative = Int32 Function(Pointer<Uint8> frameUtf8);
typedef _EngineRequestDart = int Function(Pointer<Uint8> frameUtf8);
typedef _EngineStopNative = Int32 Function(Int64 donePort);
typedef _EngineStopDart = int Function(int donePort);
typedef _AllocNative = Pointer<Uint8> Function(Size len);
typedef _AllocDart = Pointer<Uint8> Function(int len);
typedef _DeallocNative = Void Function(Pointer<Uint8> ptr, Size len);
typedef _DeallocDart = void Function(Pointer<Uint8> ptr, int len);

class _Bindings {
  _Bindings(DynamicLibrary lib)
      : initDartApi = lib.lookupFunction<_InitDartApiNative, _InitDartApiDart>(
            'jeliya_ffi_init_dart_api'),
        engineStart =
            lib.lookupFunction<_EngineStartNative, _EngineStartDart>('jeliya_engine_start'),
        engineRequest =
            lib.lookupFunction<_EngineRequestNative, _EngineRequestDart>('jeliya_engine_request'),
        engineStop = lib.lookupFunction<_EngineStopNative, _EngineStopDart>('jeliya_engine_stop'),
        alloc = lib.lookupFunction<_AllocNative, _AllocDart>('jeliya_ffi_alloc'),
        dealloc = lib.lookupFunction<_DeallocNative, _DeallocDart>('jeliya_ffi_dealloc');

  final _InitDartApiDart initDartApi;
  final _EngineStartDart engineStart;
  final _EngineRequestDart engineRequest;
  final _EngineStopDart engineStop;
  final _AllocDart alloc;
  final _DeallocDart dealloc;
}

/// [Client] over the in-process Rust engine.
///
/// Connection semantics are the engine's lifecycle, reported truthfully:
/// `connecting` while the engine constructs, `connected` while it can serve
/// dispatch, `disconnected` after [stop], a failed [start], or once a [call]
/// observes the engine itself is gone (a `daemon.shutdown` honored for real
/// — rendering `connected` against a dead engine would be a fabricated
/// state). NEVER `reconnecting` — no transport exists that can drop
/// independently of the process, and the state renders in Settings.
///
/// AT MOST ONE live FfiClient per process: the engine is a process
/// singleton, and its hot-restart adoption cannot distinguish a restarted
/// isolate from a second coexisting client — a second start() over the same
/// data dir silently reroutes the first client's replies to its own port.
class FfiClient implements Client {
  /// [open] loads the engine library; injectable because every platform
  /// resolves it differently (Android soname, iOS staticlib in-process, host
  /// tests an absolute path to `target/debug/libjeliya_ffi.{dylib,so}`).
  FfiClient({required this.dataDir, this.loopback = false, DynamicLibrary Function()? open})
      : _open = open ?? _defaultOpen;

  /// The engine's data dir. Owned exclusively by this engine while it runs —
  /// two engines (or an engine and a daemon) over one data dir is documented
  /// state corruption.
  final String dataDir;

  /// Restrict the engine's endpoint to loopback discovery (conformance runs);
  /// production mobile uses the real network path.
  final bool loopback;

  final DynamicLibrary Function() _open;
  _Bindings? _bindings;

  /// The one port carrying every reply envelope AND push frame from the
  /// engine (mirroring the single WS text-frame stream); replies correlate
  /// back to their calls by envelope id.
  ReceivePort? _frames;

  bool _stopped = true;
  int _nextId = 1;
  Completer<void>? _starting;
  final Map<int, Completer<dynamic>> _pending = {};

  ConnectionState _state = ConnectionState.disconnected;
  final StreamController<ConnectionState> _states = StreamController.broadcast();
  final StreamController<Push> _pushes = StreamController.broadcast();

  static DynamicLibrary _defaultOpen() {
    // Android resolves the soname against the APK's nativeLibraryDir; iOS
    // links the engine as a staticlib into the app binary itself.
    if (Platform.isAndroid) return DynamicLibrary.open('libjeliya_ffi.so');
    if (Platform.isIOS) return DynamicLibrary.process();
    throw UnsupportedError(
        'no default jeliya_ffi library on ${Platform.operatingSystem}; pass `open:`');
  }

  @override
  ConnectionState get state => _state;
  @override
  Stream<ConnectionState> get states => _states.stream;
  @override
  Stream<Push> get pushes => _pushes.stream;
  @override
  String describe() => 'in-process engine ($dataDir)';

  @override
  Future<void> start() {
    if (!_stopped) return _starting?.future ?? Future<void>.value();
    _stopped = false;
    final starting = _starting = Completer<void>();
    _setState(ConnectionState.connecting);
    try {
      _bindEngine();
    } catch (e, st) {
      // Unlike the WS transport there is no daemon to wait for, so a failed
      // start does not retry: stay disconnected and surface the error to the
      // awaiter (the boot screen renders it).
      _stopped = true;
      _frames?.close();
      _frames = null;
      _setState(ConnectionState.disconnected);
      starting.completeError(e, st);
      return starting.future;
    }
    _setState(ConnectionState.connected);
    starting.complete();
    return starting.future;
  }

  /// Open the library, resolve the Dart API DL stubs, and construct-or-adopt
  /// the engine with [_frames] as its frames port. Synchronous: engine
  /// construction is a sync call in the shim, so no stop() can interleave.
  void _bindEngine() {
    final b = _bindings ??= _Bindings(_open());
    // Idempotent per process; without it every posted frame is silently
    // dropped, so a failure here must fail start(), not limp on.
    if (b.initDartApi(NativeApi.initializeApiDLData) != _rcOk) {
      throw StateError('jeliya_ffi_init_dart_api rejected this Dart VM');
    }
    final frames = ReceivePort('jeliya_ffi frames');
    frames.listen(_onFrameMessage);
    final rc = _withFrameBuffer(b, dataDir,
        (ptr) => b.engineStart(ptr, loopback, frames.sendPort.nativePort));
    // ADOPTED = hot restart over the same data dir: the live engine was kept
    // and the frames port rebound — connected, same as a fresh start.
    if (rc != _rcOk && rc != _rcAdopted) {
      frames.close();
      throw StateError('jeliya_engine_start failed: ${_describeStartRc(rc)}');
    }
    _frames = frames;
  }

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) {
    final completer = Completer<dynamic>();
    if (_stopped) {
      completer.completeError(RequestError('connection_lost', 'client is stopped'));
      return completer.future;
    }
    final b = _bindings!; // non-null whenever !_stopped (set by start())
    final id = _nextId++;
    _pending[id] = completer;
    final frame = jsonEncode({'id': id, 'method': method, 'params': params ?? const {}});
    int rc;
    try {
      rc = _withFrameBuffer(b, frame, b.engineRequest);
    } catch (e) {
      _pending.remove(id);
      completer.completeError(RequestError('internal', 'failed to submit request: $e'));
      return completer.future;
    }
    if (rc != _rcOk) {
      // The frame was never queued, so no reply will ever arrive; fail the
      // call now. NOT_STARTED means the engine tore down under us (a
      // daemon.shutdown honored for real) — the in-process connection_lost.
      _pending.remove(id);
      if (rc == _rcNotStarted) {
        // Engine death is observed here, not via any port signal (the engine
        // tore itself down honoring daemon.shutdown): pending replies will
        // never arrive and dispatch is no longer servable. Fold into the
        // stopped state truthfully — fail everything in flight, release the
        // dead engine's frames port, report disconnected — instead of
        // rendering `connected` against a dead engine; start() can still
        // bring a fresh engine up, exactly as after stop().
        _stopped = true;
        _failInFlight('engine is not running');
        _frames?.close();
        _frames = null;
        _setState(ConnectionState.disconnected);
        completer.completeError(RequestError('connection_lost', 'engine is not running'));
      } else {
        completer.completeError(RequestError('internal', 'jeliya_engine_request failed (rc $rc)'));
      }
    }
    return completer.future;
  }

  /// Stops the client and tears the engine down. Throws a [StateError] when
  /// the engine reports an UNCLEAN teardown (rooms remained open past its
  /// close budget, so their on-disk stores may stay locked until the process
  /// exits) — a clean-looking stop would misreport that. The client itself
  /// is stopped either way.
  @override
  Future<void> stop() async {
    _stopped = true;
    // Resolve a pending start() future so an awaiter never hangs (interface
    // parity with WsClient/MockClient; unreachable today — start() completes
    // synchronously — but the contract must survive an async start()).
    final starting = _starting;
    if (starting != null && !starting.isCompleted) starting.complete();
    _failInFlight('client stopped');
    // Detach and close the frames port BEFORE the first await, mirroring how
    // WsClient captures its socket: a start() interleaved during the
    // done-port wait below binds a FRESH port (the host slot is emptied by
    // engineStop synchronously), and this stop() must never touch it — nor
    // stamp `disconnected` over the restarted client's state.
    final b = _bindings;
    final frames = _frames;
    _frames = null;
    frames?.close();
    _setState(ConnectionState.disconnected);
    if (b == null || frames == null) return;
    final done = ReceivePort('jeliya_ffi stop');
    try {
      // OK means the engine was live and one completion int will be posted
      // once teardown (bounded internally) finishes; NOT_STARTED means it
      // already tore itself down (daemon.shutdown) — nothing to await.
      if (b.engineStop(done.sendPort.nativePort) == _rcOk) {
        // Past the window, fire-and-forget: the singleton is already
        // emptied, teardown keeps running on the engine runtime.
        final result =
            await done.first.timeout(const Duration(seconds: 12), onTimeout: () => null);
        // Done code 1 = rooms remained open; only Node::shutdown releases a
        // room's exclusive on-disk blob lock, so those stores may stay
        // locked for the rest of the process.
        if (result is int && result != 0) {
          throw StateError('engine teardown left rooms open (code $result); '
              'their stores may stay locked until the app exits');
        }
      }
    } finally {
      done.close();
    }
  }

  void _setState(ConnectionState next) {
    if (_state == next) return;
    _state = next;
    if (!_states.isClosed) _states.add(next);
  }

  void _failInFlight(String message) {
    final err = RequestError('connection_lost', message);
    final pending = List.of(_pending.values);
    _pending.clear();
    for (final p in pending) {
      p.completeError(err);
    }
  }

  /// Every frames-port message is one wire frame as UTF-8 bytes
  /// (`Dart_PostCObject_DL` kTypedData). Envelope decoding — push fan-out,
  /// id correlation, drop-malformed — lives in the shared frame router; this
  /// transport reimplements zero of it.
  void _onFrameMessage(dynamic message) {
    if (message is! List<int>) return;
    routeFrame(
      utf8.decode(message, allowMalformed: true),
      onPush: (push) {
        if (!_pushes.isClosed) _pushes.add(push);
      },
      takePending: _pending.remove,
    );
  }

  /// Copy [s] as UTF-8 into a NUL-terminated buffer from the library's own
  /// allocator (zero-filled, so the terminator is free — and `jsonEncode`
  /// escapes control characters, so no interior NUL can truncate a frame),
  /// run [body], and always release the buffer.
  int _withFrameBuffer(_Bindings b, String s, int Function(Pointer<Uint8> ptr) body) {
    final bytes = utf8.encode(s);
    final len = bytes.length + 1;
    final ptr = b.alloc(len);
    if (ptr == nullptr) throw StateError('jeliya_ffi_alloc($len) failed');
    try {
      ptr.asTypedList(len).setAll(0, bytes);
      return body(ptr);
    } finally {
      b.dealloc(ptr, len);
    }
  }

  static String _describeStartRc(int rc) => switch (rc) {
        _rcDataDirMismatch =>
          'an engine is already running over a different data dir (rc $rc); stop it first',
        _rcConfigMismatch =>
          'an engine is already running over this data dir with a different loopback flag '
              '(rc $rc); stop it first',
        _rcEngine => 'engine construction failed over this data dir (rc $rc)',
        _ => 'rc $rc',
      };
}
