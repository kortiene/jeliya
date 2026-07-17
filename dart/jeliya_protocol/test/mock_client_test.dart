// The mock oracle held to the shared golden corpus: replays the
// mock-compatible scenarios of ui/src/lib/conformance/corpus.json against
// [MockClient] via the same replay harness the daemon oracle uses
// (test/conformance_test.dart), then pins the mock-specific behaviors the
// corpus cannot see — echo-beats-response ordering (PROTOCOL.md ordering
// hazard 2), connection-state transitions, the injectable clock, and the
// fixture-derived fleet liveness rows.

import 'dart:convert';
import 'dart:io';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:jeliya_protocol/testing.dart';
import 'package:test/test.dart';

/// Walk up from the current dir to the repo root (the dir containing
/// `ui/src/lib/conformance/corpus.json`).
Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/corpus.json').existsSync()) return dir;
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

/// Fast, timer-light mock for tests. Simulation defaults OFF here so no
/// multi-second demo timers outlive a test.
MockClient _newClient({bool fresh = false, int Function()? now, bool simulate = false}) =>
    MockClient(
      fresh: fresh,
      now: now,
      simulateLiveActivity: simulate,
      connectLatency: const Duration(milliseconds: 5),
      callLatency: const Duration(milliseconds: 2),
      fetchLatency: const Duration(milliseconds: 5),
    );

Matcher _throwsCode(String code) =>
    throwsA(isA<RequestError>().having((e) => e.code, 'code', code));

void main() {
  group('conformance: mock reference client', () {
    final corpusFile = File('${_repoRoot().path}/ui/src/lib/conformance/corpus.json');
    final corpus = jsonDecode(corpusFile.readAsStringSync()) as Map<String, dynamic>;
    final scenarios = (corpus['scenarios'] as List)
        .cast<Map<String, dynamic>>()
        .where((s) => !((s['tags'] as List?)?.contains('daemonOnly') ?? false))
        .toList();

    late MockClient client;

    setUpAll(() async {
      client = _newClient();
      await client.start();
      // Shared precondition: an identity exists. The mock is seeded with one,
      // so identity.create would error identity_exists — tolerate that.
      try {
        await client.call('identity.create');
      } on RequestError {
        // identity_exists — the precondition already holds.
      }
    });

    tearDownAll(() => client.stop());

    test('corpus has mock-compatible scenarios', () {
      expect(scenarios, isNotEmpty);
    });

    for (final scenario in scenarios) {
      test(scenario['name'] as String, () async {
        final preIdentity = (scenario['tags'] as List?)?.contains('preIdentity') ?? false;
        final oracle = preIdentity ? _newClient(fresh: true) : client;
        if (preIdentity) await oracle.start();
        try {
          final results = await replayScenario(oracle, scenario, pushWaitMs: 1500);
          final failures = results.where((r) => !r.ok).toList();
          expect(
              failures
                  .map((f) => 'step ${f.step} (${f.method}): ${f.detail}')
                  .toList(),
              isEmpty);
        } finally {
          if (preIdentity) await oracle.stop();
        }
      });
    }
  });

  group('echo-beats-response ordering (PROTOCOL.md hazard 2)', () {
    late MockClient client;
    late String rid;

    setUp(() async {
      client = _newClient();
      await client.start();
      rid = await client.roomCreate('Echo Room');
      await client.roomOpen(rid);
    });

    tearDown(() => client.stop());

    test('room.event echo arrives before the message.send response resolves', () async {
      final order = <String>[];
      String? echoedId;
      final sub = client.pushes.listen((p) {
        if (p.name == 'room.event') {
          order.add('echo');
          echoedId ??= ((p.data['event'] as Map?)?['event_id']) as String?;
        }
      });
      final eventId = await client.messageSend(rid, 'ordering probe');
      order.add('response');
      expect(order, ['echo', 'response']);
      // Same event_id on both frames — the only correlation key.
      expect(echoedId, eventId);
      await sub.cancel();
    });

    test('typed roomEvents stream sees the echo first too', () async {
      final order = <String>[];
      RoomEventPush? echoed;
      final sub = client.roomEvents.listen((p) {
        order.add('echo');
        echoed ??= p;
      });
      final eventId = await client.statusPost(roomId: rid, label: 'working', progress: 10);
      order.add('response');
      expect(order, ['echo', 'response']);
      expect(echoed!.roomId, rid);
      expect(echoed!.event.eventId, eventId);
      expect(echoed!.event.kind, TimelineKinds.agentStatus);
      await sub.cancel();
    });

    test('response, echo, and reopened backlog converge on one event_id', () async {
      String? echoedId;
      final sub = client.roomEvents.listen((p) => echoedId ??= p.event.eventId);
      final eventId = await client.messageSend(rid, 'reconcile me');
      expect(echoedId, eventId);
      await client.roomClose(rid);
      final reopened = await client.roomOpen(rid);
      final matches = reopened.timeline.where((e) => e.body == 'reconcile me').toList();
      expect(matches, hasLength(1));
      expect(matches.single.eventId, eventId);
      await sub.cancel();
    });

    test('pushes flow only while the room is open', () async {
      await client.roomClose(rid);
      final pushes = <Push>[];
      final sub = client.pushes.listen(pushes.add);
      await expectLater(
        client.messageSend(rid, 'into the void'),
        _throwsCode(ErrorCodes.roomNotOpen),
      );
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(pushes, isEmpty);
      await sub.cancel();
    });
  });

  group('connection lifecycle', () {
    test('start walks connecting → connected; stop lands disconnected', () async {
      final client = _newClient();
      final seen = <ConnectionState>[];
      final sub = client.states.listen(seen.add);
      expect(client.state, ConnectionState.disconnected);
      final starting = client.start();
      expect(client.state, ConnectionState.connecting);
      await starting;
      expect(client.state, ConnectionState.connected);
      await client.stop();
      expect(client.state, ConnectionState.disconnected);
      await Future<void>.delayed(Duration.zero);
      expect(seen, [
        ConnectionState.connecting,
        ConnectionState.connected,
        ConnectionState.disconnected,
      ]);
      await sub.cancel();
    });

    test('start is idempotent and restartable after stop', () async {
      final client = _newClient();
      await Future.wait([client.start(), client.start()]);
      expect(client.state, ConnectionState.connected);
      await client.stop();
      await client.start();
      expect(client.state, ConnectionState.connected);
      await client.stop();
    });
  });

  group('deterministic fixtures via the injectable clock', () {
    test('two clients on the same clock produce identical fixtures', () async {
      final fixed = DateTime(2026, 7, 7, 12).millisecondsSinceEpoch;
      final a = _newClient(now: () => fixed);
      final b = _newClient(now: () => fixed);
      await a.start();
      await b.start();
      expect(jsonEncode(await a.call('room.list')), jsonEncode(await b.call('room.list')));
      expect(
        jsonEncode(await a.call('room.timeline', {'room_id': MockClient.mainRoomId})),
        jsonEncode(await b.call('room.timeline', {'room_id': MockClient.mainRoomId})),
      );
      expect(jsonEncode(await a.call('daemon.status')), jsonEncode(await b.call('daemon.status')));
      await a.stop();
      await b.stop();
    });

    test('ticket expiry honors the injected clock', () async {
      var nowMs = DateTime(2026, 7, 7, 12).millisecondsSinceEpoch;
      final client = _newClient(now: () => nowMs);
      await client.start();
      final rid = await client.roomCreate('Expiry Room');
      final invitee = 'a' * 64;
      final ticket = await client.inviteCreate(
        roomId: rid,
        identityId: invitee,
        role: Roles.member,
        expiry: '1s',
      );
      expect(ticket, startsWith('roomtkt1'));
      expect(ticket.length, 8 + 96);
      nowMs += 2000; // one-second ticket, two seconds later
      await expectLater(client.roomJoin(ticket), _throwsCode(ErrorCodes.ticketExpired));
      await client.stop();
    });

    test('tickets are single-use and unminted tickets are bad_ticket', () async {
      final client = _newClient();
      await client.start();
      final rid = await client.roomCreate('Ticket Room');
      final ticket = await client.inviteCreate(
        roomId: rid,
        identityId: 'b' * 64,
        role: Roles.member,
      );
      expect(await client.roomJoin(ticket), rid);
      await expectLater(client.roomJoin(ticket), _throwsCode(ErrorCodes.badTicket));
      await expectLater(client.roomJoin('roomtkt1${'z' * 96}'), _throwsCode(ErrorCodes.badTicket));
      await expectLater(client.roomJoin('nonsense'), _throwsCode(ErrorCodes.badTicket));
      await client.stop();
    });
  });

  group('error taxonomy', () {
    test('room_unknown / room_not_open / owner-leave guard', () async {
      final client = _newClient();
      await client.start();
      final ghost = 'blake3:${'0' * 64}';
      await expectLater(client.roomOpen(ghost), _throwsCode(ErrorCodes.roomUnknown));

      final rid = await client.roomCreate('Closed Room');
      await expectLater(client.messageSend(rid, 'too early'), _throwsCode(ErrorCodes.roomNotOpen));

      // mock.ts parity: the fixture cast gives Alex an owner-role member row
      // in EVERY room (person('alex','owner')), so room.leave always hits the
      // owner guard — member-role leave and not_a_member are dead paths for
      // the local identity, exactly as in the reference mock.
      final rooms = await client.roomList();
      final review = rooms.firstWhere((r) => r.name == 'Product Review');
      await expectLater(client.roomLeave(review.roomId), _throwsCode(ErrorCodes.invalidParams));
      await expectLater(
        client.roomLeave(MockClient.mainRoomId),
        _throwsCode(ErrorCodes.invalidParams),
      );
      await client.stop();
    });

    test('fresh mode preserves the protocol-v1 empty room list before identity', () async {
      final client = _newClient(fresh: true);
      await client.start();
      expect(await client.roomList(), isEmpty);
      final identity = await client.identityCreate();
      expect(identity.identityId, MockPeople.alex.identityId);
      expect(await client.roomList(), isEmpty);
      await expectLater(client.identityCreate(), _throwsCode(ErrorCodes.identityExists));
      await client.stop();
    });
  });

  group('fleet liveness (agent-orchestration §1.2)', () {
    test('peer state overrides labels; opening a room brings agents online', () async {
      final client = _newClient();
      await client.start();

      // No room open: the crashed research runner (working label, offline
      // peer) is stale — never working — and so is backend (no live peer).
      final before = await client.agentsFleet();
      final research =
          before.agents.firstWhere((a) => a.identityId == MockPeople.researchAgent.identityId);
      expect(research.liveness, LivenessValues.stale);
      expect(before.working, 0);

      await client.roomOpen(MockClient.mainRoomId);
      final after = await client.agentsFleet();
      String liveness(MockPerson p) =>
          after.agents.firstWhere((a) => a.identityId == p.identityId).liveness;
      // Row 4: connected peer + fresh working label.
      expect(liveness(MockPeople.backendAgent), LivenessValues.working);
      // Row 6: connected peer, idle-class latest label (preview_ready).
      expect(liveness(MockPeople.frontendAgent), LivenessValues.onlineIdle);
      // Rows 1–3: QA's peer is offline; awaiting_review is idle-class.
      expect(liveness(MockPeople.qaAgent), LivenessValues.offline);
      expect(liveness(MockPeople.researchAgent), LivenessValues.stale);
      // Deploy is offline (its Agent Workspace room stays closed) with a failed
      // latest status — the Needs Attention fixture (#69).
      expect(liveness(MockPeople.deployAgent), LivenessValues.offline);

      expect(after.working, 1);
      expect(after.active, 2);
      expect(after.total, 5);
      expect(after.roomsTotal, 5);
      expect(after.roomsCovered, 5);
      // Strongest presence sorts first.
      expect(after.agents.first.identityId, MockPeople.backendAgent.identityId);
      await client.stop();
    });

    test('agent.history is one point per real agent_status, chronological', () async {
      final client = _newClient();
      await client.start();
      final points = await client.agentHistory(
        roomId: MockClient.mainRoomId,
        identityId: MockPeople.backendAgent.identityId,
      );
      expect(points, hasLength(4));
      expect([for (final p in points) p.progress], [15, 35, 60, 68]);
      expect(points.map((p) => p.ts).toList(), points.map((p) => p.ts).toList()..sort());
      final limited = await client.agentHistory(
        roomId: MockClient.mainRoomId,
        identityId: MockPeople.backendAgent.identityId,
        limit: 2,
      );
      expect([for (final p in limited) p.progress], [60, 68]);
      await client.stop();
    });
  });

  group('files and pipes fixtures', () {
    late MockClient client;

    setUp(() async {
      client = _newClient();
      await client.start();
      await client.roomOpen(MockClient.mainRoomId);
    });

    tearDown(() => client.stop());

    test('file.fetch verifies available files and rejects offline providers', () async {
      final files = await client.fileList(MockClient.mainRoomId);
      final release = files.firstWhere((f) => f.name == 'release-notes.txt');
      expect(release.available, isFalse);
      await expectLater(
        client.fileFetch(roomId: MockClient.mainRoomId, fileId: release.fileId),
        _throwsCode(ErrorCodes.fileUnavailable),
      );

      final doc = files.firstWhere((f) => f.name == 'room-protocol.md');
      final fetched = await client.fileFetch(roomId: MockClient.mainRoomId, fileId: doc.fileId);
      expect(fetched.path, '/mock/Jeliya/downloads/room-protocol.md');
      expect(fetched.bytes, doc.size);
      expect(fetched.verified, isTrue);

      final again = await client.fileList(MockClient.mainRoomId);
      final local = again.firstWhere((f) => f.fileId == doc.fileId);
      expect(local.fetched, isTrue);
      expect(local.localPath, fetched.path);
      expect(local.localBytes, doc.size);
    });

    test('pipe.connect honors authorization; pipe.close nulls the pipe ref', () async {
      final pipes = await client.pipeList(MockClient.mainRoomId);
      final preview = pipes.firstWhere((p) => p.target == '127.0.0.1:3000');
      // Alex is the authorized peer of both fixture pipes.
      final addr = await client.pipeConnect(roomId: MockClient.mainRoomId, pipeId: preview.pipeId);
      expect(addr, '127.0.0.1:41733');

      final eventId = await client.pipeClose(roomId: MockClient.mainRoomId, pipeId: preview.pipeId);
      final timeline = await client.roomTimeline(MockClient.mainRoomId);
      final closed = timeline.lastWhere((e) => e.kind == TimelineKinds.pipeClosed);
      expect(closed.eventId, eventId);
      expect(closed.pipe!.pipeId, preview.pipeId);
      expect(closed.pipe!.target, isNull);
      expect(closed.pipe!.authorizedPeer, isNull);

      await expectLater(
        client.pipeConnect(roomId: MockClient.mainRoomId, pipeId: preview.pipeId),
        _throwsCode(ErrorCodes.pipeDenied),
      );
      final after = await client.pipeList(MockClient.mainRoomId);
      final closedRow = after.firstWhere((p) => p.pipeId == preview.pipeId);
      expect(closedRow.state, PipeStates.closed);
      expect(closedRow.connected, isFalse);
    });
  });
}
