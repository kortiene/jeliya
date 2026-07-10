/// Mobile Rooms tab home (issue #17 polish): the fixture rooms render as
/// rows with the identity-hash tile tint (theme colorForId), member-count +
/// state meta whose state segment is a dot + label (status never color
/// alone; the open room reads Active, closed rooms Idle), create/join
/// affordances and every row at the 44dp touch floor, and the identity
/// footer (mono id + copy + connection badge) — at 360x800 AND 360x640, in
/// English AND French (the #14 lesson), textScale 1.0, DPR 1.0, with ZERO
/// recorded overflows. Copy is asserted via the shared catalog instances
/// (docs/i18n.md rule 6); room names are fixture data.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/mobile_rooms.dart';
import 'package:jeliya_app/src/theme.dart';
import 'package:jeliya_app/src/widgets/template_text.dart';

import 'helpers.dart';

Future<void> _expectRoomsScreenAt(WidgetTester tester, Size size,
    {required bool french}) async {
  final ready = await pumpReadyMobileApp(tester, newMockClient(), size: size);
  final session = ready.session;
  if (french) {
    // The live-switch idiom (panel_fr_layout_test): flip the pref, repump.
    session.prefs.textLocale = 'fr';
    await pumpSteps(tester, steps: 3);
  }
  final s = french ? fr : en;

  final screen = find.byType(MobileRoomsScreen);
  expect(screen, findsOneWidget);

  // Section label is uppercased at render time (i18n rule 7).
  expect(find.text(s.sidebarYourRooms.toUpperCase()), findsOneWidget);

  // Every fixture room row renders, is tappable, and clears the 44dp floor.
  final list = find.descendant(of: screen, matching: find.byType(Scrollable));
  expect(session.rooms, isNotEmpty);
  for (final room in session.rooms) {
    final row = find.widgetWithText(TextButton, room.name!);
    await tester.scrollUntilVisible(row, 56, scrollable: list.first);
    expect(row.hitTestable(), findsOneWidget,
        reason: "room row '${room.name}' must be tappable");
    expect(tester.getSize(row).height, greaterThanOrEqualTo(44),
        reason: "room row '${room.name}' is under the 44dp touch floor");
  }

  // State = dot + label: the boot-opened room reads Active, the other four
  // fixture rooms Idle (the meta stays ONE message — sidebarRoomMeta).
  expect(find.text(s.sidebarStateActive), findsOneWidget);
  expect(find.text(s.sidebarStateIdle), findsNWidgets(4));
  final open = session.rooms.firstWhere((r) => r.open);
  final openRow = find.widgetWithText(TextButton, open.name!);
  final dot = find.descendant(
      of: openRow,
      matching: find.byWidgetPredicate((w) =>
          w is Tooltip && w.message == s.sidebarSessionOpen));
  expect(dot, findsOneWidget,
      reason: 'the open room state carries the session-open dot');

  // The member-count segment of the meta template renders literally.
  final review = session.rooms.firstWhere((r) => r.name == 'Product Review');
  final reviewRow = find.widgetWithText(TextButton, 'Product Review');
  for (final part
      in templateParts(s.sidebarRoomMeta(review.memberCount, '{state}'))) {
    final text = part.text;
    if (text == null) continue;
    expect(find.descendant(of: reviewRow, matching: find.text(text)),
        findsOneWidget,
        reason: "meta segment '$text' missing from the Product Review row");
  }

  // Identity-hash tile tint comes from the theme helpers.
  final tokens = JeliyaTokens.of(tester.element(screen));
  final glyph = tester.widget<Text>(find
      .descendant(
          of: reviewRow, matching: find.text(Tokens.sidebarRoomHexGlyph))
      .first);
  expect(glyph.style?.color, tokens.colorForId(review.roomId),
      reason: 'room tile glyph must be tinted by colorForId');

  // Create/join affordances: present, labeled from the shared catalog, 44dp.
  for (final label in [s.modalCreateRoom, s.modalJoinRoomTitle]) {
    final row = find.widgetWithText(InkWell, label);
    expect(row.hitTestable(), findsOneWidget,
        reason: "affordance '$label' must be tappable");
    expect(tester.getSize(row).height, greaterThanOrEqualTo(44),
        reason: "affordance '$label' is under the 44dp touch floor");
  }

  // Identity footer: P2P identity label (uppercased at render), truncated
  // mono id line, copy affordance, and the connection badge (dot + label).
  expect(find.text(s.sidebarP2pIdentity.toUpperCase()), findsOneWidget);
  expect(find.bySemanticsLabel(s.commonCopyIdentityId), findsOneWidget);
  expect(find.text(s.shellConnConnected), findsOneWidget);

  expect(ready.overflows, isEmpty,
      reason: 'zero overflows expected at ${size.width}x${size.height} '
          '(${french ? 'fr' : 'en'}):\n${ready.overflows.join('\n')}');
}

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    testWidgets(
        'rooms home at ${size.width.toInt()}x${size.height.toInt()}, en: '
        'rows, dot+label state, affordances, footer, zero overflows',
        (tester) async {
      await _expectRoomsScreenAt(tester, size, french: false);
    });

    testWidgets(
        'rooms home at ${size.width.toInt()}x${size.height.toInt()}, fr: '
        'rows, dot+label state, affordances, footer, zero overflows',
        (tester) async {
      await _expectRoomsScreenAt(tester, size, french: true);
    });
  }
}
