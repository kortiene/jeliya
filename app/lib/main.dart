// Jeliya desktop walking skeleton (Phase 2).
//
// The thinnest real app that proves the whole seam end-to-end: it spawns (or
// adopts) the jeliyad sidecar via the Phase 0 supervision contract, connects
// the transport-agnostic Dart client over WebSocket, and lets you create/open a
// room and send/receive live messages. UI is deliberately minimal — parity is
// Phase 3; this exists to burn down integration risk.
//
// The daemon binary is located via the JELIYAD_BIN env var, falling back to the
// repo's debug build. Loopback network mode keeps the dev skeleton runnable on
// one machine; real-network multi-peer exchange is the cross-machine gate.

import 'dart:async';
import 'dart:io';

// Hide Flutter's own ConnectionState (async.dart) — we use the protocol's.
import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart';

void main() => runApp(const JeliyaApp());

String _jeliyadBinary() {
  final env = Platform.environment['JELIYAD_BIN'];
  if (env != null && env.isNotEmpty) return env;
  // Dev fallback: the repo debug build relative to a typical checkout.
  final home = Platform.environment['HOME'] ?? '.';
  for (final candidate in [
    '$home/TAC/bantaba/target/debug/jeliyad',
    '${Directory.current.path}/../target/debug/jeliyad',
  ]) {
    if (File(candidate).existsSync()) return candidate;
  }
  return 'jeliyad';
}

String _dataDir() {
  final home = Platform.environment['HOME'] ?? Directory.systemTemp.path;
  return '$home/Library/Application Support/JeliyaAppDev';
}

class JeliyaApp extends StatelessWidget {
  const JeliyaApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Jeliya',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark(useMaterial3: true).copyWith(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF2FD6A4),
          brightness: Brightness.dark,
        ),
      ),
      home: const BootScreen(),
    );
  }
}

/// Boot phases mirror the reference client's state machine.
enum Boot { starting, spawning, connecting, ready, failed }

class BootScreen extends StatefulWidget {
  const BootScreen({super.key});

  @override
  State<BootScreen> createState() => _BootScreenState();
}

class _BootScreenState extends State<BootScreen> {
  Boot _boot = Boot.starting;
  String _detail = '';
  SidecarSupervisor? _supervisor;
  WsClient? _client;
  ConnectionState _conn = ConnectionState.disconnected;

  List<Map<String, dynamic>> _rooms = const [];
  String? _identityId;

  @override
  void initState() {
    super.initState();
    _bringUp();
  }

  Future<void> _bringUp() async {
    try {
      setState(() {
        _boot = Boot.spawning;
        _detail = 'starting the daemon…';
      });
      final supervisor = SidecarSupervisor(
        binaryPath: _jeliyadBinary(),
        dataDir: _dataDir(),
        loopback: true,
      );
      final ready = await supervisor.start(port: 0);
      _supervisor = supervisor;

      setState(() {
        _boot = Boot.connecting;
        _detail = ready.adopted
            ? 'adopted a running daemon (pid ${ready.pid}) on :${ready.port}'
            : 'daemon up (pid ${ready.pid}) on :${ready.port}, connecting…';
      });

      final client = WsClient(supervisor.wsUrl);
      client.states.listen((s) {
        if (mounted) setState(() => _conn = s);
      });
      _client = client;
      await client.start().timeout(const Duration(seconds: 10));

      final status = await client.call('daemon.status') as Map<String, dynamic>;
      _identityId = (status['identity'] as Map?)?['identity_id'] as String?;
      _identityId ??= ((await client.call('identity.create')) as Map)['identity_id'] as String?;

      await _refreshRooms();
      setState(() {
        _boot = Boot.ready;
        _detail = '';
      });
    } catch (e) {
      setState(() {
        _boot = Boot.failed;
        _detail = '$e';
      });
    }
  }

  Future<void> _refreshRooms() async {
    final res = await _client!.call('room.list') as Map<String, dynamic>;
    setState(() => _rooms = (res['rooms'] as List).cast<Map<String, dynamic>>());
  }

  Future<void> _createRoom() async {
    final name = 'Room ${_rooms.length + 1}';
    await _client!.call('room.create', {'name': name});
    await _refreshRooms();
  }

  @override
  void dispose() {
    _client?.stop();
    _supervisor?.shutdown();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_boot != Boot.ready) {
      return Scaffold(
        body: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (_boot == Boot.failed)
                const Icon(Icons.error_outline, size: 40)
              else
                const CircularProgressIndicator(),
              const SizedBox(height: 16),
              Text(_boot == Boot.failed ? 'Could not start' : 'Starting Jeliya…'),
              if (_detail.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.all(16),
                  child: Text(_detail,
                      textAlign: TextAlign.center, style: const TextStyle(color: Colors.white54)),
                ),
              if (_boot == Boot.failed)
                TextButton(onPressed: _bringUp, child: const Text('Retry')),
            ],
          ),
        ),
      );
    }
    return Scaffold(
      appBar: AppBar(
        title: const Text('Jeliya'),
        actions: [
          Padding(
            padding: const EdgeInsets.only(right: 12),
            child: Center(
                child: Text(_conn.name, style: const TextStyle(fontSize: 12, color: Colors.white54))),
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton(onPressed: _createRoom, child: const Icon(Icons.add)),
      body: _rooms.isEmpty
          ? const Center(child: Text('No rooms yet — tap + to create one.'))
          : ListView.separated(
              itemCount: _rooms.length,
              separatorBuilder: (_, _) => const Divider(height: 1),
              itemBuilder: (context, i) {
                final room = _rooms[i];
                return ListTile(
                  title: Text((room['name'] as String?) ?? 'Untitled room'),
                  subtitle: Text('${room['member_count']} member(s) · ${room['status'] ?? 'active'}'),
                  onTap: () => Navigator.of(context).push(MaterialPageRoute(
                    builder: (_) => RoomScreen(client: _client!, room: room),
                  )),
                );
              },
            ),
    );
  }
}

class RoomScreen extends StatefulWidget {
  const RoomScreen({super.key, required this.client, required this.room});
  final Client client;
  final Map<String, dynamic> room;

  @override
  State<RoomScreen> createState() => _RoomScreenState();
}

class _RoomScreenState extends State<RoomScreen> {
  final List<Map<String, dynamic>> _timeline = [];
  final TextEditingController _composer = TextEditingController();
  final Set<String> _seen = {};
  StreamSubscription<Push>? _pushSub;

  String get _roomId => widget.room['room_id'] as String;

  @override
  void initState() {
    super.initState();
    _pushSub = widget.client.pushes.listen(_onPush);
    _open();
  }

  Future<void> _open() async {
    final res = await widget.client.call('room.open', {'room_id': _roomId}) as Map<String, dynamic>;
    if (!mounted) return;
    setState(() {
      for (final e in (res['timeline'] as List).cast<Map<String, dynamic>>()) {
        _insert(e);
      }
    });
  }

  void _onPush(Push push) {
    if (push.name != 'room.event') return;
    if (push.data['room_id'] != _roomId) return;
    if (!mounted) return;
    setState(() => _insert((push.data['event'] as Map).cast<String, dynamic>()));
  }

  // Insert-by-ts + event_id dedup (docs/PROTOCOL.md Pushes contract).
  void _insert(Map<String, dynamic> event) {
    final id = event['event_id'] as String;
    if (!_seen.add(id)) return;
    final ts = (event['ts'] as num).toInt();
    var i = _timeline.length;
    while (i > 0 && (_timeline[i - 1]['ts'] as num).toInt() > ts) {
      i--;
    }
    _timeline.insert(i, event);
  }

  Future<void> _send() async {
    final body = _composer.text.trim();
    if (body.isEmpty) return;
    _composer.clear();
    await widget.client.call('message.send', {'room_id': _roomId, 'body': body});
  }

  @override
  void dispose() {
    _pushSub?.cancel();
    _composer.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text((widget.room['name'] as String?) ?? 'Untitled room')),
      body: Column(
        children: [
          Expanded(
            child: ListView.builder(
              padding: const EdgeInsets.all(12),
              itemCount: _timeline.length,
              itemBuilder: (context, i) {
                final e = _timeline[i];
                final kind = e['kind'] as String;
                if (kind == 'message') {
                  return Align(
                    alignment: Alignment.centerLeft,
                    child: Card(
                      child: Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                        child: Text(e['body'] as String? ?? ''),
                      ),
                    ),
                  );
                }
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 6),
                  child: Center(
                      child: Text(kind, style: const TextStyle(color: Colors.white38, fontSize: 12))),
                );
              },
            ),
          ),
          SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(8),
              child: Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: _composer,
                      decoration:
                          const InputDecoration(hintText: 'Message', border: OutlineInputBorder()),
                      onSubmitted: (_) => _send(),
                    ),
                  ),
                  IconButton(onPressed: _send, icon: const Icon(Icons.send)),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
