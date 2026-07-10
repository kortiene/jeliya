/// Flow surfaces below the breakpoint hold at phone widths (issue #17):
/// the create/leave/rename dialogs and the join/invite full-screen routes at
/// 360x800 AND 360x640, in en AND fr (French runs ~2x wider — the #14
/// lesson), on the strict surface (textScale 1.0, DPR 1.0, overflows
/// recorded). Surfaces reached from the rooms tab assert the recorded list
/// EMPTY outright; surfaces layered over the chat/members routes assert NO
/// NEW overflows from the moment the flow opens, so this file stays
/// decoupled from the chat surface's own pixel budget.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/modals/create_room.dart';
import 'package:jeliya_app/src/screens/modals/invite.dart';
import 'package:jeliya_app/src/screens/modals/join_room.dart';
import 'package:jeliya_app/src/screens/modals/leave_room.dart';
import 'package:jeliya_app/src/screens/modals/rename_peer.dart';
import 'package:jeliya_app/src/screens/right_panel.dart';
import 'package:jeliya_app/src/screens/room_header.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_app/src/widgets/sender_name.dart';

import 'helpers.dart';
import 'member_self_seam.dart';

/// New overflow reports recorded past [mark].
List<String> _newOverflows(List<String> overflows, int mark) =>
    overflows.sublist(mark);

void main() {
  for (final size in const [Size(360, 800), Size(360, 640)]) {
    final label = '${size.width.round()}x${size.height.round()}';

    testWidgets('create room dialog holds at $label in en and fr',
        (tester) async {
      final ready =
          await pumpReadyMobileApp(tester, newMockClient(), size: size);

      await tester.tap(find.text(en.modalCreateRoom).hitTestable());
      await pumpSteps(tester, steps: 3);
      expect(find.byType(CreateRoomModal), findsOneWidget);
      expect(ready.overflows, isEmpty);
      await tester.tap(find.byTooltip(en.commonClose).hitTestable());
      await pumpSteps(tester, steps: 3);

      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      await tester.tap(find.text(fr.modalCreateRoom).hitTestable());
      await pumpSteps(tester, steps: 3);
      expect(find.byType(CreateRoomModal), findsOneWidget);
      expect(find.text(fr.modalRoomNameLabel), findsOneWidget);
      expect(ready.overflows, isEmpty);
    });

    testWidgets('join-with-ticket screen holds at $label in en and fr',
        (tester) async {
      final ready =
          await pumpReadyMobileApp(tester, newMockClient(), size: size);

      await tester.tap(find.text(en.modalJoinRoomTitle).hitTestable());
      await pumpSteps(tester, steps: 3);
      expect(find.byType(JoinRoomModal), findsOneWidget);
      expect(ready.overflows, isEmpty);
      await tester.tap(find.byTooltip(en.commonClose).hitTestable());
      await pumpSteps(tester, steps: 3);

      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      await tester.tap(find.text(fr.modalJoinRoomTitle).hitTestable());
      await pumpSteps(tester, steps: 3);
      expect(find.byType(JoinRoomModal), findsOneWidget);
      expect(find.text(fr.modalTicketLabel), findsOneWidget);
      expect(ready.overflows, isEmpty);
    });

    testWidgets(
        'invite screen (form and combined result) adds no overflows at $label '
        'in en and fr', (tester) async {
      final ready =
          await pumpReadyMobileApp(tester, newMockClient(), size: size);
      await tester.tap(find.text('Product Review').hitTestable());
      await pumpSteps(tester, steps: 6);

      Future<void> exercise(AppStrings s) async {
        final mark = ready.overflows.length;
        // The chat header scrolls internally on short viewports — bring its
        // primary Invite action into view first.
        final invite = find
            .text('${Tokens.roomHeaderInviteGlyph} ${s.roomHeaderInvite}');
        await tester.scrollUntilVisible(invite, 60,
            scrollable: find
                .ancestor(
                    of: find.byType(RoomHeader),
                    matching: find.byType(Scrollable))
                .first);
        await tester.tap(invite.hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.byType(InviteModal), findsOneWidget);
        expect(_newOverflows(ready.overflows, mark), isEmpty);

        await tester.enterText(
            find.widgetWithText(TextField, s.inviteInviteePlaceholder),
            'c' * 64);
        await tester.pump();
        // The stacked fr form outgrows short viewports — bring the submit
        // into view before tapping.
        final generate =
            find.widgetWithText(JeliyaButton, s.inviteGenerateTicket);
        await tester.scrollUntilVisible(generate, 120,
            scrollable: find
                .descendant(
                    of: find.byType(InviteModal),
                    matching: find.byType(Scrollable))
                .first);
        await tester.tap(generate.hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.text(s.inviteReadyToSend), findsOneWidget);
        expect(_newOverflows(ready.overflows, mark), isEmpty);

        await tester.tap(find.byTooltip(s.commonClose).hitTestable());
        await pumpSteps(tester, steps: 3);
      }

      await exercise(en);
      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      await exercise(fr);
    });

    testWidgets('leave room dialog adds no overflows at $label in en and fr',
        (tester) async {
      final client = MemberSelfClient(newMockClient());
      final ready = await pumpReadyMobileApp(tester, client, size: size);
      // Present self as a plain member so the Leave affordance renders.
      client.memberRoomId = ready.session.rooms
          .firstWhere((r) => r.name == 'Product Review')
          .roomId;
      await tester.tap(find.text('Product Review').hitTestable());
      await pumpSteps(tester, steps: 6);
      await tester.tap(find.text(en.panelTabMembers).hitTestable().first);
      await pumpSteps(tester, steps: 6);
      expect(find.byType(RightPanel).hitTestable(), findsOneWidget);

      Future<void> exercise(AppStrings s) async {
        final mark = ready.overflows.length;
        await tester.tap(
            find.widgetWithText(JeliyaButton, s.panelLeave).hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.byType(LeaveRoomModal), findsOneWidget);
        expect(find.text(s.modalLeaveRoom), findsWidgets);
        expect(_newOverflows(ready.overflows, mark), isEmpty);
        await tester.tap(find.byTooltip(s.commonClose).hitTestable());
        await pumpSteps(tester, steps: 3);
      }

      await exercise(en);
      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      await exercise(fr);
    });

    testWidgets('rename peer dialog adds no overflows at $label in en and fr',
        (tester) async {
      final ready =
          await pumpReadyMobileApp(tester, newMockClient(), size: size);
      final session = ready.session;
      // Member rows carry tappable SenderNames near the top of the roster
      // (the timeline's are scrolled above its stick-to-bottom viewport).
      await tester.tap(find.text('Product Review').hitTestable());
      await pumpSteps(tester, steps: 6);
      await tester.tap(find.text(en.panelTabMembers).hitTestable().first);
      await pumpSteps(tester, steps: 6);
      expect(find.byType(RightPanel).hitTestable(), findsOneWidget);

      // Any non-self sender opens the local-alias dialog on tap; the
      // non-self rows sit below the fold at 640, so scroll until one shows.
      Future<SenderName> nonSelfSender() async {
        for (var i = 0; i < 10; i++) {
          final match = tester
              .widgetList<SenderName>(find.byType(SenderName).hitTestable())
              .where((w) => w.id != session.selfId)
              .firstOrNull;
          if (match != null) return match;
          await tester.drag(
              find.byType(RightPanel).hitTestable(), const Offset(0, -120));
          await pumpSteps(tester, steps: 2);
        }
        fail('no non-self SenderName came into view');
      }

      Future<void> exercise(AppStrings s) async {
        final sender = await nonSelfSender();
        final mark = ready.overflows.length;
        await tester.tap(find.byWidget(sender).hitTestable());
        await pumpSteps(tester, steps: 3);
        expect(find.byType(RenamePeerModal), findsOneWidget);
        expect(find.text(s.renamePeerSave), findsOneWidget);
        expect(_newOverflows(ready.overflows, mark), isEmpty);
        await tester.tap(find.byTooltip(s.commonClose).hitTestable());
        await pumpSteps(tester, steps: 3);
      }

      await exercise(en);
      ready.session.prefs.textLocale = 'fr';
      await pumpSteps(tester, steps: 3);
      await exercise(fr);
    });
  }
}
