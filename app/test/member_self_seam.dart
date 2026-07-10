/// Leave-flow test seam: the [MockClient] fixtures pin self (alex) as the
/// OWNER on every roster entry and the mock's `room.leave` rejects owners, so
/// the Members-tab Leave affordance never renders against the stock mock.
/// [MemberSelfClient] presents self as a plain member of one room and answers
/// the documented member-leave success shape at the wire seam — the app-side
/// contract under test (Leave button → dialog → pop(true) → leaveCurrentRoom)
/// runs unchanged.
library;

import 'package:jeliya_protocol/jeliya_protocol.dart' show Roles;
import 'package:jeliya_protocol/testing.dart';

import 'helpers.dart';

class MemberSelfClient extends DelegatingClient {
  MemberSelfClient(super.inner);

  /// The room self appears as a plain member of. Set it after boot (fixture
  /// room ids come from `session.rooms`); null bends nothing.
  String? memberRoomId;

  /// Set once the emulated leave succeeds; `room.list` reports 'left' then.
  String? leftRoomId;

  @override
  Future<dynamic> call(String method, [Map<String, dynamic>? params]) async {
    final roomId = memberRoomId;
    // i18n-exempt: wire method names, not copy
    if (method == 'room.leave' && roomId != null && params?['room_id'] == roomId) {
      // The mock would reject its owner fixture; a plain member's leave
      // succeeds on a real daemon. Close the mock session (stops its
      // simulation timers) and answer the documented success shape.
      await inner.call('room.close', {'room_id': roomId});
      leftRoomId = roomId;
      return {'event_id': 'evt-member-left-seam'};
    }
    final result = await inner.call(method, params);
    if (roomId == null) return result;
    switch (method) {
      case 'room.open' when params?['room_id'] == roomId:
        final map = Map<String, dynamic>.of(result as Map<String, dynamic>);
        map['members'] = [
          for (final m
              in (map['members'] as List).cast<Map<String, dynamic>>())
            if (m['identity_id'] == MockPeople.alex.identityId)
              {...m, 'role': Roles.member}
            else
              m,
        ];
        return map;
      case 'room.list':
        final map = Map<String, dynamic>.of(result as Map<String, dynamic>);
        map['rooms'] = [
          for (final r in (map['rooms'] as List).cast<Map<String, dynamic>>())
            if (r['room_id'] == roomId)
              {
                ...r,
                'role': Roles.member,
                // i18n-exempt: wire roster-status value, not copy
                if (leftRoomId != null) 'status': 'left',
                if (leftRoomId != null) 'open': false,
              }
            else
              r,
        ];
        return map;
    }
    return result;
  }
}
