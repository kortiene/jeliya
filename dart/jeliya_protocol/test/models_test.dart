/// Parse tests for the typed models (lib/src/models.dart) and the typed
/// method/push surface (lib/src/methods.dart). Frames are lifted from the
/// shared conformance corpus (ui/src/lib/conformance/corpus.json) and the
/// reference mock fixtures (ui/src/lib/mock.ts), so these assertions track the
/// same wire shapes the TypeScript client is held to.
library;

import 'dart:async';

import 'package:jeliya_protocol/jeliya_protocol.dart';
import 'package:test/test.dart';

const _hex64a = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const _hex64b = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const _hex64c = 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _roomId = 'blake3:1111111111111111111111111111111111111111111111111111111111111111';

Map<String, dynamic> _sender() =>
    {'identity_id': _hex64a, 'device_id': _hex64b, 'role': 'owner'};

Map<String, dynamic> _event(String kind, [Map<String, dynamic> extra = const {}]) => {
      'event_id': _hex64c,
      'room_id': _roomId,
      'ts': 1783190000000,
      'sender': _sender(),
      'kind': kind,
      ...extra,
    };

void main() {
  group('DaemonStatus', () {
    // The corpus "version handshake" scenario shape, values from mock.ts.
    final json = <String, dynamic>{
      'version': '0.4.3',
      'protocol': 1,
      'pid': 4242,
      'port': 7420,
      'data_dir': '/mock/Jeliya',
      'mode': 'loopback',
      'identity': {'identity_id': _hex64a, 'device_id': _hex64b},
      'endpoint': {
        'endpoint_id': _hex64c,
        'addr': '$_hex64c@127.0.0.1:52731',
        'relay_url': null,
      },
      'rooms_open': [_roomId],
    };

    test('parses the full handshake frame', () {
      final s = DaemonStatus.fromJson(json);
      expect(s.version, '0.4.3');
      expect(s.protocol, 1);
      expect(s.pid, 4242);
      expect(s.port, 7420);
      expect(s.dataDir, '/mock/Jeliya');
      expect(s.mode, DaemonModes.loopback);
      expect(s.identity?.identityId, _hex64a);
      expect(s.identity?.deviceId, _hex64b);
      expect(s.endpoint?.endpointId, _hex64c);
      expect(s.endpoint?.addr, '$_hex64c@127.0.0.1:52731');
      expect(s.endpoint?.relayUrl, isNull);
      expect(s.roomsOpen, [_roomId]);
    });

    test('identity and endpoint are null pre-onboarding', () {
      final s = DaemonStatus.fromJson({
        ...json,
        'identity': null,
        'endpoint': null,
        'rooms_open': <String>[],
      });
      expect(s.identity, isNull);
      expect(s.endpoint, isNull);
      expect(s.roomsOpen, isEmpty);
    });

    test('ignores unknown keys (forward-compat rule 1)', () {
      final s = DaemonStatus.fromJson({...json, 'min_protocol': 1, 'future_field': {'x': 1}});
      expect(s.protocol, 1);
    });
  });

  group('RoomSummary', () {
    test('parses a full row', () {
      final r = RoomSummary.fromJson({
        'room_id': _roomId,
        'name': 'Build Iroh Rooms MVP',
        'role': 'owner',
        'status': 'active',
        'member_count': 7,
        'open': true,
      });
      expect(r.roomId, _roomId);
      expect(r.name, 'Build Iroh Rooms MVP');
      expect(r.role, Roles.owner);
      expect(r.status, 'active');
      expect(r.memberCount, 7);
      expect(r.open, isTrue);
    });

    test('name is null until genesis syncs; role null with no identity', () {
      final r = RoomSummary.fromJson({
        'room_id': _roomId,
        'name': null,
        'role': null,
        'status': null,
        'member_count': 1,
        'open': false,
      });
      expect(r.name, isNull);
      expect(r.role, isNull);
      expect(r.status, isNull);
    });
  });

  group('TimelineEvent', () {
    test('message (corpus echo frame)', () {
      final e = TimelineEvent.fromJson(_event('message', {'body': 'hello conformance'}));
      expect(e.eventId, _hex64c);
      expect(e.roomId, _roomId);
      expect(e.ts, 1783190000000);
      expect(e.kind, TimelineKinds.message);
      expect(e.isKnownKind, isTrue);
      expect(e.body, 'hello conformance');
      expect(e.sender.identityId, _hex64a);
      expect(e.sender.role, Roles.owner);
      // Non-message fields stay unset.
      expect(e.file, isNull);
      expect(e.pipe, isNull);
      expect(e.member, isNull);
      expect(e.label, isNull);
    });

    test('agent_status with artifacts (mock QA fixture)', () {
      final e = TimelineEvent.fromJson(_event('agent_status', {
        'label': 'awaiting_review',
        'status_message': 'Completed test suite v1. Summary attached.',
        'artifacts': ['file_00000000000000000000000000000000'],
      }));
      expect(e.label, 'awaiting_review');
      expect(e.statusMessage, 'Completed test suite v1. Summary attached.');
      expect(e.progress, isNull);
      expect(e.artifacts, ['file_00000000000000000000000000000000']);
    });

    test('agent_status without artifacts — absent means [] (normative)', () {
      final e = TimelineEvent.fromJson(_event('agent_status', {
        'label': 'working',
        'status_message': 'Sync convergence suite running (14/24 green).',
        'progress': 68,
      }));
      expect(e.artifacts, isEmpty);
      expect(e.progress, 68);
    });

    test('file_shared (mock PRD fixture)', () {
      final e = TimelineEvent.fromJson(_event('file_shared', {
        'file': {
          'file_id': 'file_00000000000000000000000000000000',
          'name': 'PRD_v0.2.pdf',
          'size': 1887437,
          'mime': 'application/pdf',
        },
      }));
      expect(e.file?.fileId, 'file_00000000000000000000000000000000');
      expect(e.file?.name, 'PRD_v0.2.pdf');
      expect(e.file?.size, 1887437);
      expect(e.file?.mime, 'application/pdf');
    });

    test('pipe_opened carries target and authorized_peer', () {
      final e = TimelineEvent.fromJson(_event('pipe_opened', {
        'pipe': {
          'pipe_id': '00000000000000000000000000000000',
          'target': '127.0.0.1:3000',
          'authorized_peer': _hex64a,
        },
      }));
      expect(e.pipe?.target, '127.0.0.1:3000');
      expect(e.pipe?.authorizedPeer, _hex64a);
    });

    test('pipe_closed nulls both target and authorized_peer (normative)', () {
      final e = TimelineEvent.fromJson(_event('pipe_closed', {
        'pipe': {
          'pipe_id': '00000000000000000000000000000000',
          'target': null,
          'authorized_peer': null,
        },
      }));
      expect(e.pipe?.pipeId, '00000000000000000000000000000000');
      expect(e.pipe?.target, isNull);
      expect(e.pipe?.authorizedPeer, isNull);
    });

    test('member_joined carries role; member_left omits it (normative)', () {
      final joined = TimelineEvent.fromJson(_event('member_joined', {
        'member': {'identity_id': _hex64b, 'role': 'member'},
      }));
      expect(joined.member?.identityId, _hex64b);
      expect(joined.member?.role, Roles.member);

      final left = TimelineEvent.fromJson(_event('member_left', {
        'member': {'identity_id': _hex64b},
      }));
      expect(left.member?.identityId, _hex64b);
      expect(left.member?.role, isNull);
    });

    test('room_created has no kind-specific payload', () {
      final e = TimelineEvent.fromJson(_event('room_created'));
      expect(e.isKnownKind, isTrue);
      expect(e.body, isNull);
      expect(e.member, isNull);
    });

    test('UNKNOWN kind parses without throwing (forward-compat rule 2)', () {
      final frame = _event('reaction_added', {'emoji': '🔥', 'delivery': 'queued'});
      final e = TimelineEvent.fromJson(frame);
      expect(e.kind, 'reaction_added');
      expect(e.isKnownKind, isFalse);
      expect(e.eventId, _hex64c);
      expect(e.ts, 1783190000000);
      // The raw frame stays accessible and round-trips untouched.
      expect(e.raw['emoji'], '🔥');
      expect(e.toJson(), equals(frame));
    });

    test('unknown keys on a known kind are ignored but preserved in raw', () {
      final frame = _event('message', {'body': 'hi', 'delivery': 'live'});
      final e = TimelineEvent.fromJson(frame);
      expect(e.body, 'hi');
      expect(e.raw['delivery'], 'live');
      expect(e.toJson(), equals(frame));
    });

    test('hand-constructed events emit kind-specific keys only when set', () {
      final e = TimelineEvent(
        eventId: _hex64c,
        roomId: _roomId,
        ts: 1,
        sender: const Sender(identityId: _hex64a, deviceId: _hex64b, role: Roles.agent),
        kind: TimelineKinds.agentStatus,
        label: 'working',
      );
      final json = e.toJson();
      expect(json['label'], 'working');
      expect(json.containsKey('body'), isFalse);
      expect(json.containsKey('artifacts'), isFalse, reason: 'empty artifacts stay absent');
      expect(json.containsKey('file'), isFalse);
    });
  });

  group('PeerStatus', () {
    test('connected direct / relay / offline (mock peers fixture)', () {
      final direct = PeerStatus.fromJson(
          {'endpoint_id': _hex64a, 'state': 'connected', 'path': 'direct', 'identity_id': _hex64b});
      expect(direct.state, PeerStates.connected);
      expect(direct.path, PeerPaths.direct);
      expect(direct.identityId, _hex64b);

      final offline = PeerStatus.fromJson(
          {'endpoint_id': _hex64a, 'state': 'offline', 'path': null, 'identity_id': _hex64b});
      expect(offline.state, PeerStates.offline);
      expect(offline.path, isNull);
    });

    test('identity_id is null before/during admission (normative)', () {
      final p = PeerStatus.fromJson(
          {'endpoint_id': _hex64a, 'state': 'connecting', 'path': null, 'identity_id': null});
      expect(p.identityId, isNull);
      expect(p.state, PeerStates.connecting);
    });
  });

  group('FileEntry', () {
    test('unfetched row (mock fixture): persisted fetch fields default', () {
      final f = FileEntry.fromJson({
        'file_id': 'file_00000000000000000000000000000000',
        'name': 'release-notes.txt',
        'size': 8192,
        'mime': 'text/plain',
        'sender_id': _hex64a,
        'ts': 1783190000000,
        'available': false,
        'providers': 1,
      });
      expect(f.available, isFalse);
      expect(f.providers, 1);
      expect(f.fetched, isFalse);
      expect(f.localPath, isNull);
      expect(f.localBytes, isNull);
      expect(f.fetchedAtMs, isNull);
    });

    test('fetched row carries the persisted local-copy fields', () {
      final f = FileEntry.fromJson({
        'file_id': 'file_00000000000000000000000000000000',
        'name': 'PRD_v0.2.pdf',
        'size': 1887437,
        'mime': 'application/pdf',
        'sender_id': _hex64a,
        'ts': 1783190000000,
        'available': true,
        'providers': 4,
        'fetched': true,
        'local_path': '/mock/Jeliya/downloads/PRD_v0.2.pdf',
        'local_bytes': 1887437,
        'fetched_at_ms': 1783190001234,
      });
      expect(f.fetched, isTrue);
      expect(f.localPath, '/mock/Jeliya/downloads/PRD_v0.2.pdf');
      expect(f.localBytes, 1887437);
      expect(f.fetchedAtMs, 1783190001234);
    });
  });

  group('PipeEntry', () {
    test('open pipe with one authorized peer (mock fixture)', () {
      final p = PipeEntry.fromJson({
        'pipe_id': '00000000000000000000000000000000',
        'target': '127.0.0.1:3000',
        'opened_by': _hex64a,
        'authorized_peer': _hex64b,
        'state': 'open',
        'connected': true,
      });
      expect(p.target, '127.0.0.1:3000');
      expect(p.openedBy, _hex64a);
      expect(p.authorizedPeer, _hex64b);
      expect(p.state, PipeStates.open);
      expect(p.connected, isTrue);
    });

    test('authorized_peer: null when unscoped, comma-joined for multi-identity', () {
      final unscoped = PipeEntry.fromJson({
        'pipe_id': '00000000000000000000000000000000',
        'target': '127.0.0.1:4000',
        'opened_by': _hex64a,
        'authorized_peer': null,
        'state': 'closed',
        'connected': false,
      });
      expect(unscoped.authorizedPeer, isNull);
      expect(unscoped.state, PipeStates.closed);

      final multi = PipeEntry.fromJson({
        'pipe_id': '00000000000000000000000000000000',
        'target': '127.0.0.1:4000',
        'opened_by': _hex64a,
        'authorized_peer': '$_hex64b,$_hex64c',
        'state': 'open',
        'connected': false,
      });
      expect(multi.authorizedPeer, '$_hex64b,$_hex64c');
    });
  });

  group('fleet reads', () {
    test('FleetResult with agents (PROTOCOL.md FleetAgent frame)', () {
      final r = FleetResult.fromJson({
        'active': 3,
        'working': 1,
        'total': 4,
        'rooms_total': 5,
        'rooms_covered': 3,
        'agents': [
          {
            'identity_id': _hex64a,
            'rooms': [
              {'room_id': _roomId, 'name': 'Build Iroh Rooms MVP'},
              {'room_id': _roomId, 'name': null},
            ],
            'liveness': 'working',
            'latest': {
              'label': 'working',
              'message': 'Sync convergence suite running (14/24 green).',
              'progress': 68,
              'ts': 1783190000000,
              'room_id': _roomId,
            },
            'last_seen_ts': 1783190000000,
          },
          {
            'identity_id': _hex64b,
            'rooms': <Map<String, dynamic>>[],
            'liveness': 'offline',
            'latest': null,
            'last_seen_ts': null,
          },
        ],
      });
      expect(r.active, 3);
      expect(r.working, 1);
      expect(r.total, 4);
      expect(r.roomsTotal, 5);
      expect(r.roomsCovered, 3);
      expect(r.agents, hasLength(2));

      final working = r.agents[0];
      expect(working.liveness, LivenessValues.working);
      expect(working.rooms[0].name, 'Build Iroh Rooms MVP');
      expect(working.rooms[1].name, isNull, reason: 'unsynced genesis leaves name null');
      expect(working.latest?.label, 'working');
      expect(working.latest?.progress, 68);
      expect(working.lastSeenTs, 1783190000000);

      final silent = r.agents[1];
      expect(silent.latest, isNull, reason: 'never posted an agent_status');
      expect(silent.lastSeenTs, isNull);
      expect(silent.liveness, LivenessValues.offline);
    });

    test('HistoryPoint: progress null is a real wire value', () {
      final p = HistoryPoint.fromJson({'ts': 1783190000000, 'label': 'preview_ready', 'progress': null});
      expect(p.ts, 1783190000000);
      expect(p.label, 'preview_ready');
      expect(p.progress, isNull);
    });
  });

  group('method result shapes', () {
    test('RoomOpenResult (corpus create→open scenario shape)', () {
      final r = RoomOpenResult.fromJson({
        'endpoint': {'endpoint_id': _hex64a, 'addr': '$_hex64a@127.0.0.1:52731'},
        'members': [
          {'identity_id': _hex64a, 'role': 'owner', 'status': 'active'},
          {'identity_id': _hex64b, 'role': 'agent', 'status': 'active'},
        ],
        'timeline': [
          _event('room_created'),
          _event('message', {'body': 'hello conformance'}),
        ],
      });
      expect(r.endpoint.endpointId, _hex64a);
      expect(r.endpoint.addr, '$_hex64a@127.0.0.1:52731');
      expect(r.endpoint.relayUrl, isNull, reason: 'room.open endpoint carries no relay_url');
      expect(r.members, hasLength(2));
      expect(r.members[1].role, Roles.agent);
      expect(r.timeline, hasLength(2));
      expect(r.timeline[1].body, 'hello conformance');
    });

    test('room.open endpoint addr can be null (real mode, not yet known)', () {
      final r = RoomOpenResult.fromJson({
        'endpoint': {'endpoint_id': _hex64a, 'addr': null},
        'members': <Map<String, dynamic>>[],
        'timeline': <Map<String, dynamic>>[],
      });
      expect(r.endpoint.addr, isNull);
      expect(r.members, isEmpty);
      expect(r.timeline, isEmpty);
    });

    test('FileShareResult / FileFetchResult / PipeExposeResult', () {
      final share = FileShareResult.fromJson(
          {'file_id': 'file_00000000000000000000000000000000', 'event_id': _hex64c});
      expect(share.fileId, 'file_00000000000000000000000000000000');
      expect(share.eventId, _hex64c);

      final fetch = FileFetchResult.fromJson(
          {'path': '/mock/Jeliya/downloads/PRD_v0.2.pdf', 'bytes': 1887437, 'verified': true});
      expect(fetch.path, '/mock/Jeliya/downloads/PRD_v0.2.pdf');
      expect(fetch.bytes, 1887437);
      expect(fetch.verified, isTrue);

      final pipe = PipeExposeResult.fromJson(
          {'pipe_id': '00000000000000000000000000000000', 'event_id': _hex64c});
      expect(pipe.pipeId, '00000000000000000000000000000000');
      expect(pipe.eventId, _hex64c);
    });
  });

  group('error taxonomy', () {
    test('the 14 wire codes plus the 2 client-synthesized codes', () {
      expect(ErrorCodes.wire, hasLength(14));
      expect(
        ErrorCodes.wire,
        containsAll([
          'invalid_params',
          'identity_missing',
          'identity_exists',
          'not_a_member',
          'room_unknown',
          'room_not_open',
          'bad_ticket',
          'ticket_expired',
          'file_unavailable',
          'file_unauthorized',
          'hash_mismatch',
          'pipe_denied',
          'peer_unreachable',
          'internal',
        ]),
      );
      expect(ErrorCodes.clientSynthesized, ['connection_lost', 'internal']);
      expect(ErrorCodes.connectionLost, isNot(isIn(ErrorCodes.wire)),
          reason: 'connection_lost never crosses the wire');
    });

    test('errorShape passes a RequestError through unchanged', () {
      final original = RequestError(ErrorCodes.badTicket, 'ticket is not a valid roomtkt1 token',
          hint: 'ask the inviter for a fresh ticket');
      expect(errorShape(original), same(original));
    });

    test('errorShape coerces any other thrown object to internal', () {
      final fromException = errorShape(StateError('boom'));
      expect(fromException.code, ErrorCodes.internal);
      expect(fromException.message, contains('boom'));
      expect(fromException.hint, isNull);

      final fromString = errorShape('plain string throw');
      expect(fromString.code, ErrorCodes.internal);
      expect(fromString.message, 'plain string throw');

      expect(errorShape(null).code, ErrorCodes.internal);
    });

    test('RequestError.fromWire parses a daemon error object', () {
      final e = RequestError.fromWire({
        'code': 'room_unknown',
        'message': 'no room blake3:0000… on this daemon',
        'hint': 'room.list shows known rooms',
      });
      expect(e.code, ErrorCodes.roomUnknown);
      expect(e.hint, 'room.list shows known rooms');
    });
  });

  group('typed pushes', () {
    test('RoomEventPush decodes the corpus message echo push', () {
      final push = RoomEventPush.fromJson({
        'room_id': _roomId,
        'event': _event('message', {'body': 'hello conformance'}),
      });
      expect(push.roomId, _roomId);
      expect(push.event.kind, TimelineKinds.message);
      expect(push.event.body, 'hello conformance');
      expect(push.event.eventId, _hex64c);
    });

    test('RoomEventPush tolerates an unknown event kind', () {
      final push = RoomEventPush.fromJson({
        'room_id': _roomId,
        'event': _event('hologram_shared', {'hologram': {'id': 'h1'}}),
      });
      expect(push.event.isKnownKind, isFalse);
      expect(push.event.raw['hologram'], {'id': 'h1'});
    });

    test('PeersChangedPush decodes a full peer replacement list', () {
      final push = PeersChangedPush.fromJson({
        'room_id': _roomId,
        'peers': [
          {'endpoint_id': _hex64a, 'state': 'connected', 'path': 'relay', 'identity_id': _hex64b},
          {'endpoint_id': _hex64c, 'state': 'connecting', 'path': null, 'identity_id': null},
        ],
      });
      expect(push.roomId, _roomId);
      expect(push.peers, hasLength(2));
      expect(push.peers[0].path, PeerPaths.relay);
      expect(push.peers[1].identityId, isNull);
    });
  });

  group('JeliyaMethods extension', () {
    test('daemonStatus decodes over call()', () async {
      final client = _StubClient({
        'daemon.status': (_) => {
              'version': '0.4.3',
              'protocol': 1,
              'pid': 1,
              'port': 7420,
              'data_dir': '/d',
              'mode': 'real',
              'identity': null,
              'endpoint': null,
              'rooms_open': <String>[],
            },
      });
      final s = await client.daemonStatus();
      expect(s.mode, DaemonModes.real);
      expect(s.identity, isNull);
    });

    test('roomOpen sends room_id and optional peers, decodes the result', () async {
      final client = _StubClient({
        'room.open': (params) {
          expect(params['room_id'], _roomId);
          expect(params['peers'], ['$_hex64a@10.0.0.5:4444']);
          return {
            'endpoint': {'endpoint_id': _hex64a, 'addr': null},
            'members': <Map<String, dynamic>>[],
            'timeline': [_event('room_created')],
          };
        },
      });
      final r = await client.roomOpen(_roomId, peers: ['$_hex64a@10.0.0.5:4444']);
      expect(r.timeline.single.kind, TimelineKinds.roomCreated);
    });

    test('optional params are omitted, not sent as null', () async {
      final client = _StubClient({
        'invite.create': (params) {
          expect(params.containsKey('expiry'), isFalse);
          return {'ticket': 'roomtkt1abcdefghijklmnopqrstuvwx'};
        },
        'room.timeline': (params) {
          expect(params.containsKey('limit'), isFalse);
          return {'events': <Map<String, dynamic>>[]};
        },
      });
      final ticket = await client.inviteCreate(
          roomId: _roomId, identityId: _hex64a, role: Roles.member);
      expect(ticket, startsWith('roomtkt1'));
      expect(await client.roomTimeline(_roomId), isEmpty);
    });

    test('inviteCreate passes expiry through as seconds or duration string', () async {
      final seen = <Object?>[];
      final client = _StubClient({
        'invite.create': (params) {
          seen.add(params['expiry']);
          return {'ticket': 'roomtkt1abcdefghijklmnopqrstuvwx'};
        },
      });
      await client.inviteCreate(
          roomId: _roomId, identityId: _hex64a, role: Roles.agent, expiry: 3600);
      await client.inviteCreate(
          roomId: _roomId, identityId: _hex64a, role: Roles.agent, expiry: '24h');
      expect(seen, [3600, '24h']);
    });

    test('scalar-result wrappers unwrap their key', () async {
      final client = _StubClient({
        'room.create': (p) => {'room_id': _roomId},
        'message.send': (p) => {'event_id': _hex64c},
        'room.join': (p) => {'room_id': _roomId},
        'pipe.connect': (p) => {'local_addr': '127.0.0.1:41733'},
      });
      expect(await client.roomCreate('Conformance Room'), _roomId);
      expect(await client.messageSend(_roomId, 'hello conformance'), _hex64c);
      expect(await client.roomJoin('roomtkt1abcdefghijklmnopqrstuvwx'), _roomId);
      expect(await client.pipeConnect(roomId: _roomId, pipeId: 'p1'), '127.0.0.1:41733');
    });

    test('a failed response surfaces as RequestError with the wire code', () async {
      final client = _StubClient({
        'room.open': (_) => throw RequestError.fromWire({
              'code': 'room_unknown',
              'message': 'no such room',
              'hint': 'room.list shows known rooms',
            }),
      });
      await expectLater(
        client.roomOpen(_roomId),
        throwsA(isA<RequestError>().having((e) => e.code, 'code', ErrorCodes.roomUnknown)),
      );
    });

    test('roomEvents / peersChanged decode and filter by push name', () async {
      final client = _StubClient(const {});
      final events = <RoomEventPush>[];
      final peers = <PeersChangedPush>[];
      final s1 = client.roomEvents.listen(events.add);
      final s2 = client.peersChanged.listen(peers.add);

      client.emit(Push('room.event', {
        'room_id': _roomId,
        'event': _event('message', {'body': 'hi'}),
      }));
      client.emit(Push('room.event', {
        'room_id': _roomId,
        'event': _event('hologram_shared'), // unknown kind still flows
      }));
      client.emit(Push('peers.changed', {
        'room_id': _roomId,
        'peers': [
          {'endpoint_id': _hex64a, 'state': 'connected', 'path': 'direct', 'identity_id': null},
        ],
      }));
      client.emit(Push('totally.new.push', {'x': 1})); // unknown name is ignored
      await Future<void>.delayed(Duration.zero);

      expect(events, hasLength(2));
      expect(events[0].event.body, 'hi');
      expect(events[1].event.isKnownKind, isFalse);
      expect(peers.single.peers.single.identityId, isNull);
      await s1.cancel();
      await s2.cancel();
    });
  });
}

/// Minimal scripted [Client]: `call` dispatches to a handler map, `emit` feeds
/// the push stream — enough to exercise the typed extension end to end.
class _StubClient implements Client {
  _StubClient(this.handlers);

  final Map<String, dynamic Function(Map<String, dynamic> params)> handlers;
  final StreamController<Push> _pushes = StreamController.broadcast(sync: true);

  void emit(Push push) => _pushes.add(push);

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    final handler = handlers[method];
    if (handler == null) {
      throw RequestError(ErrorCodes.invalidParams, 'unknown method $method');
    }
    return handler(params ?? const {});
  }

  @override
  Stream<Push> get pushes => _pushes.stream;

  @override
  ConnectionState get state => ConnectionState.connected;

  @override
  Stream<ConnectionState> get states => const Stream.empty();

  @override
  Future<void> start() async {}

  @override
  Future<void> stop() async {}

  @override
  String describe() => 'stub client (models_test)';
}
