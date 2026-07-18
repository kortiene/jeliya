/// Injected local-action failures (issue #71, P27/P28/P30): the four silent
/// paths — prefs write, clipboard copy, issue-link launch, zero-byte share —
/// now each produce actionable, inline feedback and land in diagnostics through
/// the synthetic seam, WITHOUT leaking raw platform text. Copy is asserted via
/// the shared `en`/`fr` catalogs (docs/i18n.md rule 6).
library;

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/l10n/strings_context.dart';
import 'package:jeliya_app/src/l10n/tokens.dart';
import 'package:jeliya_app/src/screens/composer.dart';
import 'package:jeliya_app/src/screens/settings_panel.dart';
import 'package:jeliya_app/src/session/daemon_session.dart';
import 'package:jeliya_app/src/session/prefs_store.dart';
import 'package:jeliya_app/src/theme.dart';
import 'package:jeliya_app/src/widgets/buttons.dart';
import 'package:jeliya_app/src/widgets/copy_button.dart';

import 'helpers.dart';

/// Wire channel names (not user copy).
// i18n-exempt: plugin channel name
const MethodChannel _fileSelectorChannel =
    MethodChannel('plugins.flutter.io/file_selector');
// i18n-exempt: plugin channel name
const MethodChannel _urlLauncherChannel =
    MethodChannel('plugins.flutter.io/url_launcher');

/// A minimal app scaffold: SessionScope over MaterialApp with the real theme
/// and localization delegates, so a leaf widget resolves `context.strings`,
/// `JeliyaTokens.of`, and the ambient session exactly as it does in the app.
Future<void> pumpUnderTest(
    WidgetTester tester, DaemonSession session, Widget child) {
  return tester.pumpWidget(SessionScope(
    session: session,
    child: MaterialApp(
      theme: buildJeliyaTheme(),
      localizationsDelegates: AppStrings.localizationsDelegates,
      supportedLocales: AppStrings.supportedLocales,
      home: Scaffold(body: child),
    ),
  ));
}

void main() {
  // ---- P27: prefs-write honesty ------------------------------------------

  test('a failed prefs write applies in memory but reports lastWriteOk=false',
      () {
    // A path whose parent is a char device — createSync can never make it a
    // directory, so the write throws deterministically on Linux.
    final prefs = PrefsStore('/dev/null/nope/app_prefs.json');
    prefs.textLocale = 'fr';
    expect(prefs.textLocale, 'fr',
        reason: 'the change still takes effect this session');
    expect(prefs.lastWriteOk, isFalse,
        reason: 'the disk write failed — do not imply a saved success');

    prefs.formattingLocale = 'en';
    expect(prefs.formattingLocale, 'en');
    expect(prefs.lastWriteOk, isFalse);
  });

  test('the in-memory prefs no-op path is not a write failure', () {
    final prefs = PrefsStore.inMemory();
    prefs.textLocale = 'fr';
    expect(prefs.lastWriteOk, isTrue);
  });

  testWidgets(
      'Settings shows the session-only note when a locale write fails, and the '
      'pick still applies live', (tester) async {
    // Tall surface so the whole ListView (incl. the language card) builds.
    useDesktopSurface(tester, size: const Size(1440, 3000));
    final session = newSession(newMockClient(),
        prefs: PrefsStore('/dev/null/nope/app_prefs.json'));
    await pumpUnderTest(tester, session, SettingsPanel(onCreateRoom: () {}));
    await tester.pump();

    // Open the UI-language dropdown (the first one) and pick French.
    await tester.tap(find.byType(DropdownButtonFormField<String>).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text(Tokens.langName('fr')!).last);
    await tester.pumpAndSettle();

    // The pick took effect this session even though it could not persist.
    expect(session.prefs.textLocale, 'fr');
    // The surface says so honestly (the harness locale is en, so the note is
    // the English catalog value).
    expect(find.text(en.settingsPrefsSessionOnly), findsOneWidget);
    // The failure was recorded through the synthetic seam — no raw text.
    final err = session.lastDiagnosticError!;
    expect(err.context, 'prefs.write');
    expect(err.code, 'prefs_write_failed');
    expect(err.message, isEmpty);
  });

  // ---- P28: clipboard copy -----------------------------------------------

  testWidgets(
      'a failed clipboard copy shows the manual-copy note, never the false '
      'Copied, and records without leaking the copied text', (tester) async {
    tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
      // i18n-exempt: platform method name
      if (call.method == 'Clipboard.setData') {
        throw PlatformException(code: 'clipboard_unavailable');
      }
      return null;
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null));

    final session = newSession(newMockClient());
    // i18n-exempt: secret fixture payload, not catalog copy
    const secret = 'super-secret-identity-0xdeadbeef';
    await pumpUnderTest(
        tester, session, const Center(child: CopyButton(text: secret)));
    await tester.pump();

    await tester.tap(find.byType(CopyButton));
    await tester.pump();

    expect(find.text(en.commonCopied), findsNothing,
        reason: 'a failed write must never show the false Copied ✓');
    expect(find.text(en.commonCopyFailed), findsOneWidget,
        reason: 'the actionable manual-copy note must appear');

    final err = session.lastDiagnosticError!;
    expect(err.context, 'clipboard.copy');
    expect(err.code, 'clipboard_write_failed');
    expect(err.message, isEmpty);
    // The clipboard payload never rides into diagnostics.
    expect(session.buildDiagnosticsReport(), isNot(contains('secret')));
  });

  // ---- P28: issue-link launch --------------------------------------------

  testWidgets(
      'a failed Report-issue launch shows the on-your-clipboard note and '
      'records the miss', (tester) async {
    // Clipboard succeeds so the copy lands and the note can honestly say the
    // diagnostics are on the clipboard; the browser launch is what fails.
    tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
      // i18n-exempt: platform method name
      if (call.method == 'Clipboard.setData') return null;
      return null;
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null));
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        _urlLauncherChannel,
        (call) async => throw PlatformException(code: 'no_browser'));
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(_urlLauncherChannel, null));

    final session = newSession(newMockClient());
    // Tall surface so the diagnostics card (bottom of the panel) builds.
    useDesktopSurface(tester, size: const Size(1440, 3000));
    await pumpUnderTest(tester, session, SettingsPanel(onCreateRoom: () {}));
    await tester.pump();

    await tester.tap(find.widgetWithText(JeliyaButton, en.settingsReportIssue));
    await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)));
    await tester.pump();
    await tester.pump();

    expect(find.text(en.settingsReportIssueLaunchFailed), findsOneWidget);
    final err = session.lastDiagnosticError!;
    expect(err.context, 'issue.launch');
    expect(err.code, 'url_launch_failed');
    expect(err.message, isEmpty);
  });

  // ---- P28: zero-byte share ----------------------------------------------

  testWidgets(
      'sharing a zero-byte file shows the empty-file note, keeps send enabled, '
      'and leaks no path', (tester) async {
    final empty = File(
        '${Directory.systemTemp.createTempSync('jeliya_empty').path}/empty.bin')
      ..createSync();
    addTearDown(() {
      if (empty.existsSync()) empty.deleteSync();
    });
    expect(empty.lengthSync(), 0);

    tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(_fileSelectorChannel, (call) async {
      // i18n-exempt: plugin method name
      if (call.method == 'openFile') return <String>[empty.path];
      return null;
    });
    addTearDown(() => tester.binding.defaultBinaryMessenger
        .setMockMethodCallHandler(_fileSelectorChannel, null));

    final session = await pumpReadyApp(tester, newMockClient());

    // A non-blank draft so the send button's enabled state is observable.
    final field = find.descendant(
        of: find.byType(Composer), matching: find.byType(TextField));
    // i18n-exempt: draft body fixture, not catalog copy
    await tester.enterText(field, 'ready to send');
    await tester.pump();

    await tester.tap(find.bySemanticsLabel(en.composerShareAFile).first);
    // The picker channel roundtrip AND the real File.length() I/O both need
    // the real event loop to complete — flush it before rendering.
    await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 100)));
    await tester.pump();

    // The empty pick is surfaced inline instead of silently dropped.
    expect(find.text(en.composerShareEmptyFile), findsOneWidget);
    // Share failure never blocks send — the arrow button stays enabled.
    final send = tester.widget<JeliyaButton>(
        find.widgetWithText(JeliyaButton, Tokens.composerSendGlyph));
    expect(send.onPressed, isNotNull,
        reason: 'a failed share must not disable send');

    final err = session.lastDiagnosticError!;
    expect(err.context, 'file.share');
    expect(err.code, 'file_empty');
    expect(err.message, isEmpty);
    // The picked file's path never rides into diagnostics.
    expect(session.buildDiagnosticsReport(), isNot(contains(empty.path)));
  });
}
