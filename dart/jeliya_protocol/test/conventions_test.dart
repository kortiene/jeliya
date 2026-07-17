/// Table-driven tests for the client-convention ports
/// (lib/src/conventions/*). Case tables are lifted from the reference
/// implementations (ui/src/App.tsx, ui/src/lib/{invite,join,format,
/// diagnostics}.ts) so the Dart client tracks the exact same behaviors —
/// including the normative ordering hazards in docs/PROTOCOL.md (Pushes) and
/// the honesty rules the conventions carry.
library;

import 'dart:convert';
import 'dart:io';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:test/test.dart';

/// Walk up from the current dir to the repo root (the dir containing the shared
/// conformance fixtures), so the Dart unread test reads the SAME five-case
/// manifest as the TypeScript one (docs/room-attention.md; issue #63, AC7).
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

RoomSummary _recencyRoom(String roomId, int? lastEventTs, String? lastEventKind) => RoomSummary(
      roomId: roomId,
      memberCount: 0,
      open: false,
      lastEventTs: lastEventTs,
      lastEventKind: lastEventKind,
    );

const _room = 'blake3:1111111111111111111111111111111111111111111111111111111111111111';
const _hex64a = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const _hex64b = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const _hex64c = 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _sender = Sender(identityId: _hex64a, deviceId: _hex64b, role: 'member');

TimelineEvent _event(String id, int ts, {String kind = TimelineKinds.message}) =>
    TimelineEvent(
      eventId: id,
      roomId: _room,
      ts: ts,
      sender: _sender,
      kind: kind,
      body: kind == TimelineKinds.message ? id : null,
    );

List<String> _ids(List<TimelineEvent> events) => events.map((e) => e.eventId).toList();

/// A [Client] whose `room.join` responses are scripted: a [String] resolves
/// as the joined room id, a [RequestError] throws.
class _ScriptedJoinClient implements Client {
  _ScriptedJoinClient(this._script);

  final List<Object> _script;
  final List<Map<String, dynamic>?> calls = [];

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    expect(method, 'room.join');
    calls.add(params);
    final step = _script[calls.length - 1];
    if (step is RequestError) throw step;
    return <String, dynamic>{'room_id': step};
  }

  @override
  Future<void> start() async {}

  @override
  Future<void> stop() async {}

  @override
  ConnectionState get state => ConnectionState.connected;

  @override
  Stream<ConnectionState> get states => const Stream.empty();

  @override
  Stream<Push> get pushes => const Stream.empty();

  @override
  String describe() => 'scripted';
}

void main() {
  group('TimelineFold', () {
    test('folds a shuffled baseline into ts order', () {
      final fold = TimelineFold([_event('b', 200), _event('a', 100), _event('c', 300)]);
      expect(_ids(fold.events), ['a', 'b', 'c']);
      expect(fold.length, 3);
    });

    test('late-backlog splice: an older-ts push lands mid-list, not appended', () {
      final fold = TimelineFold([_event('e1', 100), _event('e2', 200), _event('e3', 300)]);
      expect(fold.insert(_event('late', 150)), isTrue);
      expect(_ids(fold.events), ['e1', 'late', 'e2', 'e3']);
    });

    test('an older-than-everything ts lands at the front', () {
      final fold = TimelineFold([_event('e1', 100), _event('e2', 200)]);
      fold.insert(_event('genesis', 50));
      expect(_ids(fold.events), ['genesis', 'e1', 'e2']);
    });

    test('equal ts appends after existing equal-ts events (stable ties)', () {
      final fold = TimelineFold();
      fold.insertAll([_event('a', 100), _event('b', 100), _event('c', 100)]);
      expect(_ids(fold.events), ['a', 'b', 'c']);
      fold.insert(_event('d', 100));
      expect(_ids(fold.events), ['a', 'b', 'c', 'd']);
    });

    test('equal ts splices after its tie but before later events', () {
      final fold = TimelineFold([_event('a', 100), _event('b', 200)]);
      fold.insert(_event('c', 100));
      expect(_ids(fold.events), ['a', 'c', 'b']);
    });

    test('event_id dedup is idempotent (live pump + reconcile scan)', () {
      final fold = TimelineFold();
      expect(fold.insert(_event('e1', 100)), isTrue);
      // Same event arriving again — even with a different ts — is skipped.
      expect(fold.insert(_event('e1', 50)), isFalse);
      expect(_ids(fold.events), ['e1']);
      expect(fold.contains('e1'), isTrue);
      expect(fold.contains('e2'), isFalse);
      // insertAll counts only the new ones.
      expect(fold.insertAll([_event('e1', 100), _event('e2', 200)]), 1);
      expect(_ids(fold.events), ['e1', 'e2']);
    });

    test('clear() re-baselines for the next room.open', () {
      final fold = TimelineFold([_event('e1', 100)]);
      fold.clear();
      expect(fold.events, isEmpty);
      expect(fold.contains('e1'), isFalse);
      expect(fold.insert(_event('e1', 100)), isTrue);
    });
  });

  group('spliceEventByTs', () {
    test('returns the same list instance on a duplicate event_id', () {
      final timeline = [_event('e1', 100)];
      expect(identical(spliceEventByTs(timeline, _event('e1', 200)), timeline), isTrue);
    });

    test('splices by ts without mutating the input', () {
      final timeline = [_event('e1', 100), _event('e3', 300)];
      final next = spliceEventByTs(timeline, _event('e2', 200));
      expect(_ids(next), ['e1', 'e2', 'e3']);
      expect(_ids(timeline), ['e1', 'e3']);
    });
  });

  group('PendingMessages', () {
    test('beginSend appends a sending message under a client-local id', () {
      final pending = PendingMessages(clock: () => 1000);
      expect(pending.beginSend('hello'), 'pending-1000-0');
      expect(pending.beginSend('again'), 'pending-1000-1');
      final first = pending.messages.first;
      expect(first.phase, PendingPhases.sending);
      expect(first.body, 'hello');
      expect(first.ts, 1000);
      expect(first.eventId, isNull);
      expect(first.error, isNull);
    });

    test('resolveSend moves sending → syncing carrying the wire event_id', () {
      final pending = PendingMessages(clock: () => 1000);
      final clientId = pending.beginSend('hello');
      pending.resolveSend(clientId, 'evt-1', echoAlreadyVisible: false);
      final m = pending.messages.single;
      expect(m.phase, PendingPhases.syncing);
      expect(m.eventId, 'evt-1');
    });

    test('resolveSend drops the message when the echo beat the response', () {
      final pending = PendingMessages(clock: () => 1000);
      final clientId = pending.beginSend('hello');
      pending.resolveSend(clientId, 'evt-1', echoAlreadyVisible: true);
      expect(pending.messages, isEmpty);
    });

    test('failSend moves to failed with the shaped error', () {
      final pending = PendingMessages(clock: () => 1000);
      final a = pending.beginSend('one');
      final b = pending.beginSend('two');
      pending.failSend(a, RequestError(ErrorCodes.connectionLost, 'transport gone'));
      pending.failSend(b, StateError('boom'));
      expect(pending.byClientId(a)?.phase, PendingPhases.failed);
      expect(pending.byClientId(a)?.error?.code, ErrorCodes.connectionLost);
      // Non-RequestError throwables become internal (errorShape fallback).
      expect(pending.byClientId(b)?.error?.code, ErrorCodes.internal);
    });

    test('retry re-enters sending, clears the error, keeps body and id', () {
      var now = 1000;
      final pending = PendingMessages(clock: () => now);
      final clientId = pending.beginSend('hello');
      pending.failSend(clientId, RequestError(ErrorCodes.connectionLost, 'gone'));
      now = 2000;
      expect(pending.beginSend('hello', retryClientId: clientId), clientId);
      final m = pending.messages.single;
      expect(m.clientId, clientId);
      expect(m.phase, PendingPhases.sending);
      expect(m.body, 'hello');
      expect(m.ts, 2000);
      expect(m.error, isNull);
    });

    test('retry of an unknown clientId changes nothing (reference map semantics)', () {
      final pending = PendingMessages(clock: () => 1000);
      pending.beginSend('hello');
      pending.beginSend('other', retryClientId: 'pending-999-9');
      expect(pending.messages.single.body, 'hello');
    });

    test('reconcilePush retires the pending matching a message push by event_id', () {
      final pending = PendingMessages(clock: () => 1000);
      final clientId = pending.beginSend('hello');
      pending.resolveSend(clientId, 'evt-1', echoAlreadyVisible: false);
      expect(pending.reconcilePush(_event('evt-1', 100)), isTrue);
      expect(pending.messages, isEmpty);
    });

    test('reconcilePush ignores non-message kinds and unknown event ids', () {
      final pending = PendingMessages(clock: () => 1000);
      final clientId = pending.beginSend('hello');
      pending.resolveSend(clientId, 'evt-1', echoAlreadyVisible: false);
      expect(
        pending.reconcilePush(_event('evt-1', 100, kind: TimelineKinds.agentStatus)),
        isFalse,
      );
      expect(pending.reconcilePush(_event('evt-2', 100)), isFalse);
      expect(pending.messages, hasLength(1));
    });

    test('reconcileBacklog retires synced ids and keeps unresolved sends', () {
      final pending = PendingMessages(clock: () => 1000);
      final synced = pending.beginSend('synced');
      pending.resolveSend(synced, 'evt-1', echoAlreadyVisible: false);
      pending.beginSend('still in flight');
      expect(
        pending.reconcileBacklog([_event('evt-1', 100), _event('evt-9', 200)]),
        1,
      );
      expect(pending.messages.single.body, 'still in flight');
    });

    test('reconcileBacklog is a no-op while nothing has an event_id yet', () {
      final pending = PendingMessages(clock: () => 1000);
      pending.beginSend('hello');
      expect(pending.reconcileBacklog([_event('evt-1', 100)]), 0);
      expect(pending.messages, hasLength(1));
    });
  });

  group('splitInvite', () {
    const cases = <(String name, String ticket, String peerAddr, String wantTicket, String wantPeerAddr)>[
      ('plain ticket passes through', 'roomtkt1abc', '', 'roomtkt1abc', ''),
      (
        'combined invite splits at #',
        'roomtkt1abc#$_hex64c@203.0.113.7:4242',
        '',
        'roomtkt1abc',
        '$_hex64c@203.0.113.7:4242',
      ),
      (
        'an explicitly provided peer address wins over the embedded one',
        'roomtkt1abc#embedded@1.1.1.1:1',
        'explicit@2.2.2.2:2',
        'roomtkt1abc',
        'explicit@2.2.2.2:2',
      ),
      ('a leading # is not a separator', '#roomtkt1abc', '', '#roomtkt1abc', ''),
      (
        'whitespace is trimmed everywhere',
        '  roomtkt1abc # ep@1.2.3.4:4242  ',
        '   ',
        'roomtkt1abc',
        'ep@1.2.3.4:4242',
      ),
      ('only the first # splits', 'roomtkt1abc#a#b', '', 'roomtkt1abc', 'a#b'),
    ];
    for (final (name, ticket, peerAddr, wantTicket, wantPeerAddr) in cases) {
      test(name, () {
        final split = splitInvite(ticket, peerAddr);
        expect(split.ticket, wantTicket);
        expect(split.peerAddr, wantPeerAddr);
      });
    }
  });

  group('joinRoomWithRetry', () {
    RequestError unreachable() =>
        RequestError(ErrorCodes.peerUnreachable, 'no reachable discovery hint');

    test('returns the room id on first success', () async {
      final client = _ScriptedJoinClient([_room]);
      final progress = <JoinProgress>[];
      final delays = <Duration>[];
      final roomId = await joinRoomWithRetry(
        client,
        ticket: 'roomtkt1abc',
        onProgress: progress.add,
        sleep: (d) async => delays.add(d),
      );
      expect(roomId, _room);
      expect(client.calls, [
        {'ticket': 'roomtkt1abc'},
      ]);
      expect(delays, isEmpty);
      final only = progress.single;
      expect(only.phase, JoinPhases.connecting);
      expect(only.attempt, 1);
      expect(only.maxAttempts, 5);
      expect(only.retryDelay, isNull);
      expect(only.lastError, isNull);
    });

    test('retries only peer_unreachable with the 1.5/2/3/4s ladder', () async {
      final client = _ScriptedJoinClient([unreachable(), unreachable(), _room]);
      final progress = <JoinProgress>[];
      final delays = <Duration>[];
      final roomId = await joinRoomWithRetry(
        client,
        ticket: 'roomtkt1abc',
        onProgress: progress.add,
        sleep: (d) async => delays.add(d),
      );
      expect(roomId, _room);
      expect(client.calls, hasLength(3));
      expect(delays, const [Duration(milliseconds: 1500), Duration(milliseconds: 2000)]);
      expect(progress.map((p) => p.phase).toList(), [
        JoinPhases.connecting,
        JoinPhases.retrying,
        JoinPhases.connecting,
        JoinPhases.retrying,
        JoinPhases.connecting,
      ]);
      expect(progress[1].retryDelay, const Duration(milliseconds: 1500));
      expect(progress[1].lastError?.code, ErrorCodes.peerUnreachable);
      expect(progress[2].attempt, 2);
      expect(progress[2].retryDelay, isNull);
      expect(progress[3].retryDelay, const Duration(milliseconds: 2000));
      expect(progress[4].attempt, 3);
    });

    test('exhausts five attempts, then rethrows peer_unreachable', () async {
      final client = _ScriptedJoinClient(
        [unreachable(), unreachable(), unreachable(), unreachable(), unreachable()],
      );
      final delays = <Duration>[];
      await expectLater(
        joinRoomWithRetry(client, ticket: 'roomtkt1abc', sleep: (d) async => delays.add(d)),
        throwsA(isA<RequestError>()
            .having((e) => e.code, 'code', ErrorCodes.peerUnreachable)),
      );
      expect(client.calls, hasLength(5));
      expect(delays, const [
        Duration(milliseconds: 1500),
        Duration(milliseconds: 2000),
        Duration(milliseconds: 3000),
        Duration(milliseconds: 4000),
      ]);
    });

    test('any other error code throws immediately, no retry', () async {
      final client =
          _ScriptedJoinClient([RequestError(ErrorCodes.badTicket, 'ticket did not parse')]);
      final progress = <JoinProgress>[];
      final delays = <Duration>[];
      await expectLater(
        joinRoomWithRetry(
          client,
          ticket: 'roomtkt1abc',
          onProgress: progress.add,
          sleep: (d) async => delays.add(d),
        ),
        throwsA(isA<RequestError>().having((e) => e.code, 'code', ErrorCodes.badTicket)),
      );
      expect(client.calls, hasLength(1));
      expect(delays, isEmpty);
      expect(progress.single.phase, JoinPhases.connecting);
    });

    test('passes dial hints through to room.join params', () async {
      final client = _ScriptedJoinClient([_room]);
      await joinRoomWithRetry(
        client,
        ticket: 'roomtkt1abc',
        peers: ['$_hex64c@203.0.113.7:4242'],
      );
      expect(client.calls, [
        {
          'ticket': 'roomtkt1abc',
          'peers': ['$_hex64c@203.0.113.7:4242'],
        },
      ]);
    });
  });

  group('labelTone', () {
    const cases = <(String label, LabelTone tone)>[
      // Red: substrings on purpose — a false alarm is the honest direction
      // to err in.
      ('failed', LabelTone.red),
      ('failure', LabelTone.red),
      ('build_failed', LabelTone.red),
      ('network-error', LabelTone.red),
      ('error', LabelTone.red),
      ('blocked', LabelTone.red),
      ('blocker', LabelTone.red),
      ('unblocked', LabelTone.red),
      ('ERROR', LabelTone.red),
      // Red precedence beats blue and green tokens in the same label.
      ('review failed', LabelTone.red),
      ('blocked_on_review', LabelTone.red),
      ('deploy_error_recovered', LabelTone.red),
      // Blue: word-boundary await/review/pend family.
      ('await', LabelTone.blue),
      ('awaiting', LabelTone.blue),
      ('awaiting_review', LabelTone.blue),
      ('review', LabelTone.blue),
      ('reviewing', LabelTone.blue),
      ('reviewed', LabelTone.blue),
      ('pend', LabelTone.blue),
      ('pending', LabelTone.blue),
      ('pending-merge', LabelTone.blue),
      ('needs review', LabelTone.blue),
      // Blue precedence beats green.
      ('review passed', LabelTone.blue),
      // Word-boundary is load-bearing: `review` must not match inside
      // `preview` — preview_ready is a success label, not a waiting one.
      ('preview_ready', LabelTone.green),
      ('preview', LabelTone.neutral),
      // Green is earned: the full healthy/active token set.
      ('done', LabelTone.green),
      ('working', LabelTone.green),
      ('online', LabelTone.green),
      ('ready', LabelTone.green),
      ('pass', LabelTone.green),
      ('passed', LabelTone.green),
      ('tests_passed', LabelTone.green),
      ('success', LabelTone.green),
      ('successful', LabelTone.green),
      ('ok', LabelTone.green),
      ('complete', LabelTone.green),
      ('completed', LabelTone.green),
      ('connected', LabelTone.green),
      ('healthy', LabelTone.green),
      ('active', LabelTone.green),
      ('running', LabelTone.green),
      ('verified', LabelTone.green),
      ('live', LabelTone.green),
      ('DONE', LabelTone.green),
      ('deploy-complete', LabelTone.green),
      // Word boundaries also bound green: pass ≠ passing, ok ≠ okay.
      ('passing', LabelTone.neutral),
      ('okay', LabelTone.neutral),
      // Neutral fallback — unknown and non-English labels never guess green.
      ('échec', LabelTone.neutral),
      ('starting', LabelTone.neutral),
      ('compiling', LabelTone.neutral),
      ('', LabelTone.neutral),
    ];
    for (final (label, tone) in cases) {
      test("'$label' → ${tone.name}", () => expect(labelTone(label), tone));
    }
  });

  group('fetch state fold', () {
    FileEntry file({
      String id = 'file_11111111111111111111111111111111',
      bool fetched = false,
      String? localPath,
      int? localBytes,
      int size = 2048,
    }) =>
        FileEntry(
          fileId: id,
          name: 'notes.md',
          size: size,
          mime: 'text/markdown',
          senderId: _hex64a,
          ts: 1783190000000,
          available: true,
          providers: 1,
          fetched: fetched,
          localPath: localPath,
          localBytes: localBytes,
        );

    test('persistedFetchState is null without a persisted local copy', () {
      expect(persistedFetchState(_room, file()), isNull);
      expect(persistedFetchState(_room, file(fetched: true)), isNull);
      expect(persistedFetchState(_room, file(fetched: true, localPath: '')), isNull);
      expect(persistedFetchState(_room, file(localPath: '/dl/notes.md')), isNull);
    });

    test('persistedFetchState reconstructs the weaker fetched state', () {
      final state = persistedFetchState(
        _room,
        file(fetched: true, localPath: '/dl/notes.md', localBytes: 1024),
        localFileUrl: (roomId, fileId) => '/api/files/local?room_id=$roomId&file_id=$fileId',
      );
      expect(state?.phase, FetchPhases.fetched);
      expect(state?.path, '/dl/notes.md');
      expect(state?.bytes, 1024);
      expect(
        state?.url,
        '/api/files/local?room_id=$_room&file_id=file_11111111111111111111111111111111',
      );
    });

    test('persistedFetchState falls back to size and a null url', () {
      final state =
          persistedFetchState(_room, file(fetched: true, localPath: '/dl/notes.md'));
      expect(state?.bytes, 2048);
      expect(state?.url, isNull);
    });

    test('mergeFetchedFiles never downgrades verified or pending entries', () {
      final previous = <String, FetchState>{
        'file_a': const FetchState.verified(path: '/dl/a', bytes: 10),
        'file_b': const FetchState.pending(),
      };
      final next = mergeFetchedFiles(previous, _room, [
        file(id: 'file_a', fetched: true, localPath: '/dl/a-old'),
        file(id: 'file_b', fetched: true, localPath: '/dl/b-old'),
      ]);
      // Both persisted entries were skipped, so the exact same map comes back.
      expect(identical(next, previous), isTrue);
      expect(next['file_a']?.phase, FetchPhases.verified);
      expect(next['file_b']?.phase, FetchPhases.pending);
    });

    test('mergeFetchedFiles adds, refreshes fetched, and replaces error entries', () {
      final previous = <String, FetchState>{
        'file_a': FetchState.error(RequestError(ErrorCodes.fileUnavailable, 'offline')),
        'file_b': const FetchState.fetched(path: '/dl/b-old', bytes: 1),
      };
      final next = mergeFetchedFiles(previous, _room, [
        file(id: 'file_a', fetched: true, localPath: '/dl/a', localBytes: 11),
        file(id: 'file_b', fetched: true, localPath: '/dl/b', localBytes: 22),
        file(id: 'file_c', fetched: true, localPath: '/dl/c', localBytes: 33),
      ]);
      expect(identical(next, previous), isFalse);
      expect(next['file_a']?.phase, FetchPhases.fetched);
      expect(next['file_a']?.path, '/dl/a');
      expect(next['file_b']?.path, '/dl/b');
      expect(next['file_c']?.bytes, 33);
      // Copy-on-write: the input map is untouched.
      expect(previous['file_a']?.phase, FetchPhases.error);
      expect(previous.containsKey('file_c'), isFalse);
    });

    test('mergeFetchedFiles returns the same map when nothing is persisted', () {
      final previous = <String, FetchState>{'file_a': const FetchState.pending()};
      final next = mergeFetchedFiles(previous, _room, [file(id: 'file_b')]);
      expect(identical(next, previous), isTrue);
    });

    test('hash_mismatch is a hard stop; other errors are not', () {
      expect(
        FetchState.error(RequestError(ErrorCodes.hashMismatch, 'digest differs')).isHardStop,
        isTrue,
      );
      expect(
        FetchState.error(RequestError(ErrorCodes.fileUnavailable, 'offline')).isHardStop,
        isFalse,
      );
      expect(const FetchState.pending().isHardStop, isFalse);
      expect(const FetchState.verified(path: '/dl/a', bytes: 1).isHardStop, isFalse);
    });
  });

  group('buildDiagnostics', () {
    test('empty input renders the full skeleton verbatim', () {
      final report = buildDiagnostics(const DiagnosticsInput(
        generatedAt: '2026-07-08T09:00:00.000Z',
        uiVersion: '0.0.0-test',
        browser: 'dart-test',
        platform: 'macos-test',
        transport: 'ws://127.0.0.1:7420/ws',
        connection: ConnectionState.disconnected,
      ));
      expect(report, '''
# Jeliya Diagnostics

Generated by the Jeliya UI for support. This excludes message bodies, invite tickets, full local paths, file names, pipe targets, and full identity IDs.

## Runtime
- generated_at: 2026-07-08T09:00:00.000Z
- ui_version: 0.0.0-test
- daemon_version: unknown
- daemon_mode: unknown
- connection: disconnected
- transport: ws://127.0.0.1:7420/ws
- browser: dart-test
- platform: macos-test

## Local Node
- identity_present: no
- identity_id: none
- device_id: none
- endpoint_present: no
- endpoint_id: none
- endpoint_address_present: no
- relay_url_present: no
- rooms_open_count: 0

## Rooms
- total: 0
- open: 0
- status_counts: none
- current_room_id: none
- current_room_status: none
- current_room_open: no

## Current Room
- members: 0
- member_status_counts: none
- member_role_counts: none
- peers: 0
- peer_state_counts: none
- peer_path_counts: none
- peer_identity_bound_count: 0
- files_total: 0
- files_available: 0
- files_fetched: 0
- files_fetch_errors: 0
- files_provider_total: 0
- fetch_phase_counts: none
- pipes_total: 0
- pipes_open: 0
- pipes_connected: 0

## Last UI Error
- none_captured: yes
''');
    });

    test('populated input aggregates, sorts, shortens, and redacts', () {
      final input = DiagnosticsInput(
        generatedAt: '2026-07-08T09:00:00.000Z',
        uiVersion: '0.4.3',
        browser: 'dart-test',
        platform: 'macos-test',
        transport: '$_hex64c@127.0.0.1:52731 via ws',
        connection: ConnectionState.connected,
        status: const DaemonStatus(
          version: '0.4.3',
          protocol: 1,
          pid: 4242,
          port: 7420,
          dataDir: '/mock/Jeliya',
          mode: 'loopback',
          identity: Identity(identityId: _hex64a, deviceId: _hex64b),
          endpoint: EndpointInfo(endpointId: _hex64c, addr: '$_hex64c@127.0.0.1:52731'),
          roomsOpen: [_room],
        ),
        rooms: const [
          RoomSummary(roomId: _room, name: 'Ops', memberCount: 3, open: true),
          RoomSummary(
              roomId: 'blake3:$_hex64b', status: 'active', memberCount: 2, open: false),
          RoomSummary(
              roomId: 'blake3:$_hex64c', status: 'left', memberCount: 1, open: false),
        ],
        currentRoomId: _room,
        members: const [
          Member(identityId: _hex64a, role: 'owner', status: 'active'),
          Member(identityId: _hex64b, role: 'member', status: 'active'),
        ],
        files: [
          FileEntry(
            fileId: 'file_a',
            name: 'a.pdf',
            size: 100,
            mime: 'application/pdf',
            senderId: _hex64a,
            ts: 1,
            available: true,
            providers: 2,
            fetched: true,
            localPath: '/dl/a.pdf',
          ),
          FileEntry(
            fileId: 'file_b',
            name: 'b.md',
            size: 50,
            mime: 'text/markdown',
            senderId: _hex64b,
            ts: 2,
            available: false,
            providers: 1,
          ),
        ],
        fetches: {
          'file_a': const FetchState.verified(path: '/dl/a.pdf', bytes: 100),
          'file_b': FetchState.error(RequestError(ErrorCodes.fileUnavailable, 'offline')),
        },
        pipes: const [
          PipeEntry(
            pipeId: 'pipe_1',
            target: '127.0.0.1:3000',
            openedBy: _hex64a,
            state: 'open',
            connected: true,
          ),
          PipeEntry(
            pipeId: 'pipe_2',
            target: '127.0.0.1:5173',
            openedBy: _hex64b,
            state: 'closed',
            connected: false,
          ),
        ],
        peers: const [
          PeerStatus(endpointId: _hex64b, state: 'connected', path: 'direct', identityId: _hex64b),
          PeerStatus(endpointId: _hex64c, state: 'connecting'),
        ],
        lastError: const DiagnosticEvent(
          context: 'file.fetch /Users/sekou/Downloads/a.pdf',
          code: 'file_unavailable',
          message: 'ticket roomtkt1abc23xyz did not help',
          hint: _hex64c,
          at: '2026-07-08T08:59:00.000Z',
        ),
      );
      final report = buildDiagnostics(input);

      // Runtime + local node.
      expect(report, contains('- daemon_version: 0.4.3\n'));
      expect(report, contains('- daemon_mode: loopback\n'));
      expect(report, contains('- connection: connected\n'));
      expect(report, contains('- transport: <endpoint-address> via ws\n'));
      expect(report, contains('- identity_present: yes\n'));
      expect(report, contains('- identity_id: aaaa…aaaa\n'));
      expect(report, contains('- endpoint_address_present: yes\n'));
      expect(report, contains('- relay_url_present: no\n'));
      expect(report, contains('- rooms_open_count: 1\n'));

      // Rooms: a null status counts as active; keys sort; ids shorten.
      expect(report, contains('- total: 3\n'));
      expect(report, contains('- open: 1\n'));
      expect(report, contains('- status_counts: active=2, left=1\n'));
      expect(report, contains('- current_room_id: 1111…1111\n'));
      expect(report, contains('- current_room_status: none\n'));
      expect(report, contains('- current_room_open: yes\n'));

      // Current room aggregates.
      expect(report, contains('- members: 2\n'));
      expect(report, contains('- member_status_counts: active=2\n'));
      expect(report, contains('- member_role_counts: member=1, owner=1\n'));
      expect(report, contains('- peers: 2\n'));
      expect(report, contains('- peer_state_counts: connected=1, connecting=1\n'));
      expect(report, contains('- peer_path_counts: direct=1, unknown=1\n'));
      expect(report, contains('- peer_identity_bound_count: 1\n'));
      expect(report, contains('- files_total: 2\n'));
      expect(report, contains('- files_available: 1\n'));
      expect(report, contains('- files_fetched: 1\n'));
      expect(report, contains('- files_fetch_errors: 1\n'));
      expect(report, contains('- files_provider_total: 3\n'));
      expect(report, contains('- fetch_phase_counts: error=1, verified=1\n'));
      expect(report, contains('- fetch_error_codes: file_unavailable=1\n'));
      expect(report, contains('- pipes_total: 2\n'));
      expect(report, contains('- pipes_open: 1\n'));
      expect(report, contains('- pipes_connected: 1\n'));

      // Last error block is fully redacted: local path, ticket, and raw hash.
      expect(report, contains('- context: file.fetch <local-path>\n'));
      expect(report, contains('- code: file_unavailable\n'));
      expect(report, contains('- message: ticket <ticket> did not help\n'));
      expect(report, contains('- hint: <hash>\n'));
      expect(report, isNot(contains('/Users/')));
      expect(report, isNot(contains('roomtkt1abc23xyz')));
      expect(report, isNot(contains(_hex64c)));
    });

    test('a session-verified fetch counts as fetched without the persisted flag', () {
      final input = DiagnosticsInput(
        generatedAt: 't',
        uiVersion: 'v',
        browser: 'b',
        platform: 'p',
        transport: 'ws',
        connection: ConnectionState.connected,
        files: const [
          FileEntry(
            fileId: 'file_a',
            name: 'a.md',
            size: 10,
            mime: 'text/markdown',
            senderId: _hex64a,
            ts: 1,
            available: true,
            providers: 1,
          ),
        ],
        fetches: {'file_a': const FetchState.verified(path: '/dl/a.md', bytes: 10)},
      );
      final report = buildDiagnostics(input);
      expect(report, contains('- files_fetched: 1\n'));
      expect(report, contains('- files_fetch_errors: 0\n'));
      expect(report, isNot(contains('- fetch_error_codes:')));
    });
  });

  group('roomUnread (device-local unread projection)', () {
    test('is unread when the newest event is past the last-seen mark', () {
      expect(roomUnread(_recencyRoom(_room, 300, 'message'), 100), isTrue);
    });

    test('is not unread when the mark is at or past the newest event', () {
      expect(roomUnread(_recencyRoom(_room, 100, 'message'), 100), isFalse);
      expect(roomUnread(_recencyRoom(_room, 100, 'message'), 300), isFalse);
    });

    test('is not unread with no recency evidence (null lastEventTs)', () {
      expect(roomUnread(_recencyRoom(_room, null, null), 100), isFalse);
    });

    test('is not unread with no baseline (null lastSeen) — no evidence for a dot', () {
      expect(roomUnread(_recencyRoom(_room, 300, 'message'), null), isFalse);
    });

    // The SAME five-case manifest the TypeScript unread test replays, so React
    // and Flutter decide unread from one source (docs/room-attention.md, AC7).
    final fixtureFile = File(
      '${_repoRoot().path}/ui/src/lib/conformance/room-attention.fixtures.json',
    );
    final cases = ((jsonDecode(fixtureFile.readAsStringSync()) as Map<String, dynamic>)['cases']
            as List)
        .cast<Map<String, dynamic>>();

    test('covers the five truthful states exactly once', () {
      final names = cases.map((c) => c['name'] as String).toList()..sort();
      expect(names, ['attention', 'no-data', 'offline', 'stale', 'unread']);
    });

    for (final c in cases) {
      final room = c['room'] as Map<String, dynamic>;
      final expected = (c['expect'] as Map<String, dynamic>)['unread'] as bool;
      test('shared case "${c['name']}" -> unread $expected', () {
        final summary = _recencyRoom(
          room['room_id'] as String,
          room['last_event_ts'] as int?,
          room['last_event_kind'] as String?,
        );
        expect(roomUnread(summary, c['last_seen'] as int?), expected);
      });
    }
  });

  group('fleet attention (Needs Attention closed set)', () {
    AttentionReason? reasonByName(String? name) => switch (name) {
          'failed' => AttentionReason.failed,
          'review' => AttentionReason.review,
          'stale' => AttentionReason.stale,
          'offline' => AttentionReason.offline,
          _ => null,
        };

    test('flags red/blue latest tone, stale, and offline-after-work', () {
      expect(attentionReason('working', 'build_failed'), AttentionReason.failed);
      expect(attentionReason('online-idle', 'awaiting_review'), AttentionReason.review);
      expect(attentionReason('stale', 'working'), AttentionReason.stale);
      expect(attentionReason('offline', 'tests_passed'), AttentionReason.offline);
    });

    test('does not flag offline-never-posted or healthy agents', () {
      expect(attentionReason('offline', null), isNull);
      expect(attentionReason('working', 'working'), isNull);
      expect(attentionReason('online-idle', 'tests_passed'), isNull);
    });

    test('ranks failed < review < stale < offline < none', () {
      expect(attentionRank('working', 'build_failed'), 0);
      expect(attentionRank('online-idle', 'awaiting_review'), 1);
      expect(attentionRank('stale', 'working'), 2);
      expect(attentionRank('offline', 'tests_passed'), 3);
      expect(attentionRank('working', 'working'), attentionOrder.length);
    });

    test('statusUnverified is true only for stale/offline', () {
      expect(statusUnverified('stale'), isTrue);
      expect(statusUnverified('offline'), isTrue);
      expect(statusUnverified('working'), isFalse);
      expect(statusUnverified('online-idle'), isFalse);
    });

    test('hasNumericProgress accepts finite numbers only', () {
      expect(hasNumericProgress(0), isTrue);
      expect(hasNumericProgress(68), isTrue);
      expect(hasNumericProgress(null), isFalse);
      expect(hasNumericProgress(double.nan), isFalse);
      expect(hasNumericProgress(double.infinity), isFalse);
    });

    // The SAME classifier manifest the TypeScript fleet test replays, so React
    // and Flutter group and rank agents from one source (issue #69).
    final fixtureFile = File(
      '${_repoRoot().path}/ui/src/lib/conformance/fleet-attention.fixtures.json',
    );
    final cases = ((jsonDecode(fixtureFile.readAsStringSync()) as Map<String, dynamic>)['cases']
            as List)
        .cast<Map<String, dynamic>>();

    for (final c in cases) {
      final expected = c['expect'] as Map<String, dynamic>;
      test('shared case "${c['name']}" -> reason ${expected['reason']}', () {
        final liveness = c['liveness'] as String;
        final label = c['latest_label'] as String?;
        expect(attentionReason(liveness, label), reasonByName(expected['reason'] as String?));
        expect(needsAttention(liveness, label), expected['reason'] != null);
        expect(statusUnverified(liveness), expected['unverified'] as bool);
      });
    }
  });
}
