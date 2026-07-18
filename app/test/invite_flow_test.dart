/// Guided invitation + re-invitation flow (issue #66, P15) — the Flutter
/// mirror of the React InviteModal contracts. Covers: inline identity
/// validation gating the generate button; a preset AND an Advanced custom
/// expiry both minting; the agent-role security warning; the combined
/// `ticket#address` + Copy; and the honest lifecycle — a Waiting chip after
/// mint, an Expired chip + Invite-again once the (test-controlled) clock passes
/// the expiry, and a Joined chip ONLY once the roster shows an active row (via
/// the mock's acceptInvite hook). Copy is asserted through the shared `en`
/// catalog (docs/i18n.md rule 6). Share layout/desktop-copy-only coverage lives
/// in invite_share_test.dart; the lifecycle math corpus in
/// invite_lifecycle_test.dart — this file owns the modal behavior.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/qr/qr_view.dart';
import 'package:jeliya_app/src/screens/modals/invite.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_app/src/widgets/copy_button.dart';

import 'helpers.dart';

/// Scopes a finder to the invite modal (a Dialog over the shell — the room
/// header behind it can carry lookalike words).
Finder _inModal(Finder f) =>
    find.descendant(of: find.byType(InviteModal), matching: f);

Finder _modalText(String t) => _inModal(find.text(t));

/// The invitee field is the only visible TextField in the form view (the
/// custom-expiry field is hidden until Advanced is opened, and enters the tree
/// after it). Stable across content (the placeholder hint disappears once the
/// field has text, so it cannot be re-found by placeholder).
Finder get _idField => _inModal(find.byType(TextField)).first;

/// Opens Product Review, then the Invite modal (desktop shell).
Future<void> _openInvite(WidgetTester tester) async {
  await tester.tap(find.text('Product Review').hitTestable());
  await pumpSteps(tester, steps: 6);
  final invite =
      find.text('${Tokens.roomHeaderInviteGlyph} ${en.roomHeaderInvite}');
  await tester.tap(invite.hitTestable().first);
  await pumpSteps(tester, steps: 3);
  expect(find.byType(InviteModal), findsOneWidget);
}

JeliyaButton _generateButton(WidgetTester tester) => tester.widget<JeliyaButton>(
    _inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket)));

void main() {
  testWidgets(
      'inline identity validation gates the generate button and shows an '
      'inline error for a non-64-hex entry', (tester) async {
    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    // Empty → disabled (nothing to generate for).
    expect(_generateButton(tester).onPressed, isNull);
    expect(_modalText(en.inviteIdentityHint), findsOneWidget);

    // A too-short entry stays disabled and surfaces the inline error.
    await tester.enterText(_idField, 'abc');
    await tester.pump();
    expect(_generateButton(tester).onPressed, isNull);
    expect(_modalText(en.inviteIdentityInvalid), findsOneWidget);

    // A valid bare 64-hex enables it and clears the error.
    await tester.enterText(_idField, 'a' * 64);
    await tester.pump();
    expect(_generateButton(tester).onPressed, isNotNull);
    expect(_modalText(en.inviteIdentityInvalid), findsNothing);
  });

  testWidgets(
      'a default preset mints the combined ticket#address with Copy and a '
      'Waiting chip', (tester) async {
    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    await tester.enterText(_idField, 'a' * 64);
    await tester.pump();
    // The default 24h preset is selected; just generate.
    await tester
        .tap(_inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket))
            .hitTestable());
    await pumpSteps(tester, steps: 3);

    expect(_modalText(en.inviteReadyToSend), findsOneWidget);
    // Combined built with buildCombinedInvite (ticket#address), not concat.
    final copyPayload = tester
        .widget<CopyButton>(
            _inModal(find.widgetWithText(CopyButton, en.inviteCopyInvite)))
        .text;
    expect(copyPayload, startsWith('roomtkt1'));
    expect(copyPayload, contains('#'));

    // A scannable QR of the SAME combined invite renders alongside Copy (#103):
    // it encodes the copyable payload, and its matrix is well-formed.
    final qr = tester.widget<QrView>(_inModal(find.byType(QrView)));
    expect(qr.value, copyPayload);

    // The lifecycle chip is honest: Waiting (no active roster row yet).
    expect(_modalText(en.inviteLifecycleWaiting), findsOneWidget);
    expect(_modalText(en.inviteLifecycleJoined), findsNothing);
  });

  testWidgets('an Advanced custom expiry also mints a ticket', (tester) async {
    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    await tester.enterText(_idField, 'a' * 64);
    await tester.pump();

    // Reveal the Advanced custom-seconds field and enter a positive integer;
    // it overrides the selected preset.
    await tester.tap(_inModal(find.textContaining(en.inviteAdvancedExpiry)));
    await pumpSteps(tester, steps: 2);
    final custom = _inModal(find.byWidgetPredicate(
        (w) => w is TextField && w.keyboardType == TextInputType.number));
    expect(custom, findsOneWidget);
    await tester.enterText(custom, '90');
    await tester.pump();

    await tester
        .tap(_inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket))
            .hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(_modalText(en.inviteReadyToSend), findsOneWidget);
  });

  testWidgets('selecting the agent role reveals the security warning',
      (tester) async {
    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    expect(_modalText(en.inviteAgentWarning), findsNothing);
    await tester.tap(_modalText(en.panelRoleAgent));
    await tester.pump();
    expect(_modalText(en.inviteAgentWarning), findsOneWidget);

    // Switching back to member hides it again.
    await tester.tap(_modalText(en.panelRoleMember));
    await tester.pump();
    expect(_modalText(en.inviteAgentWarning), findsNothing);
  });

  testWidgets(
      'a time-boxed invite flips waiting → expired on the tick and offers '
      'Invite again, which re-mints back to Waiting', (tester) async {
    // The lifecycle clock is injectable — there is no fake wall clock under
    // pump(); driving it lets the 1s tick cross the expiry deterministically.
    var fakeNow = 1720000000000;
    final savedNow = inviteNowMs;
    inviteNowMs = () => fakeNow;
    addTearDown(() => inviteNowMs = savedNow);

    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    await tester.enterText(_idField, 'a' * 64);
    await tester.pump();
    // The default 24h preset gives a time-boxed ticket (expiresAtMs non-null).
    await tester
        .tap(_inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket))
            .hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(_modalText(en.inviteLifecycleWaiting), findsOneWidget);
    expect(_inModal(find.widgetWithText(JeliyaButton, en.inviteAgain)),
        findsNothing);

    // Advance past the 24h expiry; the periodic tick re-derives the state.
    fakeNow += const Duration(hours: 25).inMilliseconds;
    await tester.pump(const Duration(seconds: 1));
    expect(_modalText(en.inviteLifecycleExpired), findsOneWidget);
    final again =
        _inModal(find.widgetWithText(JeliyaButton, en.inviteAgain));
    expect(again, findsOneWidget);

    // Invite again re-mints a fresh, unexpired ticket → back to Waiting.
    await tester.tap(again.hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(_modalText(en.inviteLifecycleWaiting), findsOneWidget);
    expect(_modalText(en.inviteLifecycleExpired), findsNothing);
  });

  testWidgets(
      'the Joined chip appears ONLY once the roster shows an active row',
      (tester) async {
    final mock = newMockClient();
    final session = await pumpReadyApp(tester, mock);
    final review = session.rooms.firstWhere((r) => r.name == 'Product Review');
    await _openInvite(tester);

    const invitee =
        'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd';
    await tester.enterText(_idField, invitee);
    await tester.pump();
    await tester
        .tap(_inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket))
            .hitTestable());
    await pumpSteps(tester, steps: 3);

    // Minting alone must never read as Joined — only an invited row exists.
    expect(_modalText(en.inviteLifecycleWaiting), findsOneWidget);
    expect(_modalText(en.inviteLifecycleJoined), findsNothing);

    // The invitee accepts: the mock flips the invited row to active and
    // publishes member_joined, exactly as the inviter's daemon observes it.
    mock.acceptInvite(review.roomId, invitee);
    await pumpSteps(tester, steps: 4);

    expect(_modalText(en.inviteLifecycleJoined), findsOneWidget);
    expect(_modalText(en.inviteLifecycleWaiting), findsNothing);
  });

  testWidgets(
      'reopening over a still-pending invitation restores the waiting state, '
      'not a blank draft', (tester) async {
    await pumpReadyApp(tester, newMockClient());
    await _openInvite(tester);

    // Mint an invite — the mock persists an `invited` roster row (the runtime
    // proof the reopen restores from).
    await tester.enterText(_idField, 'a' * 64);
    await tester.pump();
    await tester
        .tap(_inModal(find.widgetWithText(JeliyaButton, en.inviteGenerateTicket))
            .hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(_modalText(en.inviteReadyToSend), findsOneWidget);

    // Close the modal, then reopen it fresh.
    await tester.tap(find.byTooltip(en.commonClose).hitTestable());
    await pumpSteps(tester, steps: 3);
    expect(find.byType(InviteModal), findsNothing);

    final invite =
        find.text('${Tokens.roomHeaderInviteGlyph} ${en.roomHeaderInvite}');
    await tester.tap(invite.hitTestable().first);
    await pumpSteps(tester, steps: 3);
    expect(find.byType(InviteModal), findsOneWidget);

    // Restored: the invitee id is prefilled, the already-invited copy shows,
    // a Waiting chip renders, and the submit offers a fresh invite.
    expect(tester.widget<TextField>(_idField).controller!.text, 'a' * 64);
    expect(_modalText(en.inviteAlreadyInvited), findsOneWidget);
    expect(_modalText(en.inviteLifecycleWaiting), findsOneWidget);
    expect(_inModal(find.widgetWithText(JeliyaButton, en.inviteSendFresh)),
        findsOneWidget);
  });
}
