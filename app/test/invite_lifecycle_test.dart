/// Invite-lifecycle contract parity (issue #66): the Dart side of the shared
/// invite helpers, replaying the SAME corpus as ui/src/lib/invite.test.ts from
/// ui/src/lib/conformance/invite-lifecycle.fixtures.json — so React and Flutter
/// validate the invitee identity, build the combined invite, and derive the
/// waiting/expired/joined lifecycle identically. Joined comes ONLY from a signed
/// active roster row.
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart';

Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/invite-lifecycle.fixtures.json')
        .existsSync()) {
      return dir;
    }
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

void main() {
  final root = _repoRoot();
  final fixtures = jsonDecode(
    File('${root.path}/ui/src/lib/conformance/invite-lifecycle.fixtures.json')
        .readAsStringSync(),
  ) as Map<String, dynamic>;

  test('expiryPresets offers 1h/24h/7d/never with the right seconds', () {
    expect([for (final p in expiryPresets) p.key], ['1h', '24h', '7d', 'never']);
    expect([for (final p in expiryPresets) p.seconds],
        [3600, 86400, 604800, null]);
  });

  group('shared invite-lifecycle fixtures (parity with React)', () {
    for (final c in (fixtures['identity'] as List).cast<Map<String, dynamic>>()) {
      test('identity ${jsonEncode(c['value'])} → ${c['valid']}', () {
        expect(isIdentityId(c['value'] as String), c['valid']);
      });
    }

    for (final c in (fixtures['combined'] as List).cast<Map<String, dynamic>>()) {
      test('combined ${c['ticket']}+${jsonEncode(c['addr'])}', () {
        expect(buildCombinedInvite(c['ticket'] as String, c['addr'] as String),
            c['combined']);
      });
    }

    for (final c in (fixtures['lifecycle'] as List).cast<Map<String, dynamic>>()) {
      test('lifecycle: ${c['name']}', () {
        final members = (c['members'] as List)
            .cast<Map<String, dynamic>>()
            .toList();
        expect(
          inviteState(
            c['identity_id'] as String,
            c['expires_at_ms'] as int?,
            members,
            c['now_ms'] as int,
          ),
          c['state'],
        );
      });
    }
  });
}
