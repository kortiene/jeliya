/// Jeliya desktop (Phase 3 scaffold) — thin entry: theme + session provider
/// + bootstrap-phase routing. All real state lives in [DaemonSession]
/// (lib/src/session/); screens observe it through [SessionScope].
library;

// Hide Flutter's own ConnectionState (async.dart) — we use the protocol's.
import 'dart:io' show Platform;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:intl/date_symbol_data_local.dart';
import 'package:jeliya_protocol/testing.dart' show MockClient;
import 'package:path_provider/path_provider.dart';

import 'src/ffi/jeliya_ffi_smoke.dart';
import 'src/format.dart';
import 'src/l10n/tokens.dart';
import 'src/l10n/strings_context.dart';
import 'src/screens/boot_screen.dart';
import 'src/screens/onboarding_identity.dart';
import 'src/screens/onboarding_rooms.dart';
import 'src/screens/shell.dart';
import 'src/session/daemon_session.dart';
import 'src/session/prefs_store.dart';
import 'src/theme.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // intl date symbols (docs/i18n.md decision 4) — with bundled data ONE call
  // loads every locale, so a live formatting-locale switch never re-inits.
  await initializeDateFormatting();
  // Desktop spawns/adopts the jeliyad sidecar; mobile has no subprocess (iOS
  // forbids it, Android 13 SELinux blocks exec from writable dirs) and runs an
  // INTERIM fixture transport until the in-process FfiClient lands.
  if (Platform.isAndroid || Platform.isIOS) {
    runApp(JeliyaApp(session: await _buildMobileSession()));
  } else {
    runApp(const JeliyaApp());
  }
}

/// INTERIM mobile bring-up (Phase 4 scaffold). The production transport is an
/// `FfiClient implements Client` over an expanded jeliya-ffi (request/response
/// + a push StreamSink via flutter_rust_bridge) — the next milestone. Until
/// then mobile injects the in-memory fixture [MockClient] so the full UI is
/// exercisable on-device, reusing the same injection seam widget tests use.
/// Also fires the FFI beachhead: proving libjeliya_ffi.so loads and runs
/// inside the Flutter process (the last de-risking step before FfiClient).
Future<DaemonSession> _buildMobileSession() async {
  final dataDir = (await getApplicationSupportDirectory()).path;
  if (Platform.isAndroid) {
    try {
      debugPrint('FFI-SMOKE: ${runFfiIdentitySmoke('$dataDir/ffi-smoke')}');
    } catch (e) {
      debugPrint('FFI-SMOKE: FAILED to load libjeliya_ffi.so — $e');
    }
  }
  return DaemonSession(
    client: MockClient(),
    dataDir: dataDir,
    prefs: PrefsStore('$dataDir/app_prefs.json'),
  );
}

class JeliyaApp extends StatefulWidget {
  const JeliyaApp({super.key, this.session});

  /// Test seam: inject a session built over a mock client
  /// (`DaemonSession(client: MockClient(), prefs: PrefsStore.inMemory())`);
  /// null spawns/adopts the real jeliyad sidecar.
  final DaemonSession? session;

  @override
  State<JeliyaApp> createState() => _JeliyaAppState();
}

class _JeliyaAppState extends State<JeliyaApp> with WidgetsBindingObserver {
  late final DaemonSession _session = widget.session ?? DaemonSession();
  late final bool _ownsSession = widget.session == null;

  @override
  void initState() {
    super.initState();
    _session.start();
    // Locale prefs drive MaterialApp.locale and FormatsScope below; the
    // observer catches OS-level locale changes for the system-follow paths.
    _session.prefs.addListener(_onPrefsChanged);
    WidgetsBinding.instance.addObserver(this);
  }

  void _onPrefsChanged() => setState(() {});

  @override
  void didChangeLocales(List<Locale>? locales) => setState(() {});

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _session.prefs.removeListener(_onPrefsChanged);
    if (_ownsSession) _session.dispose();
    super.dispose();
  }

  /// A stored BCP-47/underscore tag → [Locale]; null means follow-system
  /// (MaterialApp resolves PlatformDispatcher.locales against
  /// supportedLocales via basicLocaleListResolution).
  static Locale? _parseTag(String? tag) {
    if (tag == null) return null;
    // Drop empty segments: hand-edited prefs like 'fr-CA-' or '_' must not
    // reach the Locale constructors (they assert non-empty subtags).
    final parts = tag
        .replaceAll('-', '_')
        .split('_')
        .where((p) => p.isNotEmpty)
        .toList();
    if (parts.isEmpty) return null;
    return switch (parts.length) {
      1 => Locale(parts[0]),
      2 => parts[1].length == 4
          ? Locale.fromSubtags(languageCode: parts[0], scriptCode: parts[1])
          : Locale(parts[0], parts[1]),
      _ => Locale.fromSubtags(
          languageCode: parts[0],
          scriptCode: parts[1].length == 4 ? parts[1] : null,
          countryCode: parts.last,
        ),
    };
  }

  @override
  Widget build(BuildContext context) {
    final prefs = _session.prefs;
    // Formatting convention: the pref, else the system locale — clamped to
    // what intl has data for. Read through the binding so widget tests'
    // platformDispatcher overrides apply.
    final formattingLocale = JeliyaFormats.verify(prefs.formattingLocale ??
        WidgetsBinding.instance.platformDispatcher.locale.toLanguageTag());
    return SessionScope(
      session: _session,
      child: FormatsScope(
        locale: formattingLocale,
        child: MaterialApp(
          // The brand wordmark (non-migrating) — but sourced from the strings
          // layer like every other user-visible string.
          title: Tokens.wordmark,
          debugShowCheckedModeBanner: false,
          theme: buildJeliyaTheme(),
          locale: _parseTag(prefs.textLocale),
          localizationsDelegates: AppStrings.localizationsDelegates,
          supportedLocales: AppStrings.supportedLocales,
          home: const _PhaseRouter(),
        ),
      ),
    );
  }
}

/// Routes on the protocol bootstrap phase: boot → onboarding (identity,
/// rooms) → the app shell. Rebuilds on every session notification via
/// [SessionScope].
class _PhaseRouter extends StatelessWidget {
  const _PhaseRouter();

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    return switch (session.phase) {
      BootstrapPhase.boot => const BootScreen(),
      BootstrapPhase.noIdentity => const OnboardingIdentityScreen(),
      BootstrapPhase.noRooms => const OnboardingRoomsScreen(),
      BootstrapPhase.ready => const ShellScreen(),
    };
  }
}
