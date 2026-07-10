/// Composer — ported from ui/src/components/Composer.tsx per
/// phase3-features.json "Composer": auto-growing input (max 150px), Enter
/// sends / Shift+Enter newline, primary send button ('➤', '…' while
/// sending), per-room draft persistence (PrefsStore — loaded on room switch,
/// saved per keystroke, removed when emptied), optimistic clear before send
/// with restore-on-throw + ErrorNote, and file sharing. Desktop adaptation:
/// the web's paste/drop file sharing becomes an explicit picker button
/// (file_selector → RoomStore.shareUserFile, the package staging upload) —
/// zero-byte files are filtered out, exactly like the reference.
///
/// Disabled while conn != connected; pending-message failures render in the
/// timeline, not here (only pre-pending throws surface in this ErrorNote).
library;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart' hide ConnectionState;
import 'package:flutter/services.dart';
import 'package:jeliya_protocol/jeliya_protocol.dart'
    show ConnectionState, RequestError, errorShape;

import '../l10n/strings_context.dart';
import '../l10n/tokens.dart';
import '../session/daemon_session.dart';
import '../theme.dart';
import '../widgets/buttons.dart';
import '../widgets/error_note.dart';

class Composer extends StatefulWidget {
  const Composer({super.key});

  @override
  State<Composer> createState() => _ComposerState();
}

class _ComposerState extends State<Composer> {
  final TextEditingController _controller = TextEditingController();
  final FocusNode _focusNode = FocusNode();

  DaemonSession? _session;
  String? _roomId;
  bool _sending = false;
  bool _sharing = false;
  bool _focused = false;
  RequestError? _error;

  /// True while programmatically syncing the controller from prefs (a room
  /// switch must not be persisted back as a keystroke).
  bool _syncingDraft = false;

  @override
  void initState() {
    super.initState();
    _controller.addListener(_onDraftChanged);
    _focusNode.addListener(_onFocusChanged);
  }

  @override
  void dispose() {
    _controller.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Room switch: load the new room's persisted draft (reference behavior —
    // drafts are read on room change only).
    final session = SessionScope.of(context);
    _session = session;
    final roomId = session.currentRoomId;
    if (roomId != _roomId) {
      _roomId = roomId;
      _syncingDraft = true;
      _controller.text =
          roomId == null ? '' : (session.prefs.draftFor(roomId) ?? '');
      _syncingDraft = false;
      _error = null;
    }
  }

  void _onFocusChanged() {
    if (mounted) setState(() => _focused = _focusNode.hasFocus);
  }

  /// Saved on each keystroke; emptied drafts are removed (PrefsStore).
  void _onDraftChanged() {
    if (_syncingDraft) return;
    final roomId = _roomId;
    if (roomId != null) {
      _session?.prefs.setDraft(roomId, _controller.text);
    }
    // Rebuild for the send button's blank-draft disabled state.
    if (mounted) setState(() {});
  }

  /// Sets the draft text through the normal persistence path.
  void _setDraft(String value) {
    _controller.text = value;
  }

  Future<void> _send() async {
    final session = SessionScope.of(context);
    final disabled = session.conn != ConnectionState.connected;
    final body = _controller.text.trim();
    if (body.isEmpty || _sending || disabled) return;
    final previousDraft = _controller.text;
    // Optimistic clear BEFORE the call; restored only on throw (pending
    // message failures are the timeline's job).
    _setDraft('');
    setState(() {
      _sending = true;
      _error = null;
    });
    try {
      final room = session.room;
      if (room == null) {
        throw StateError('no room open');
      }
      await room.sendMessage(body);
    } catch (e) {
      _setDraft(previousDraft);
      if (mounted) setState(() => _error = errorShape(e));
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  Future<void> _shareFile() async {
    final session = SessionScope.of(context);
    final room = session.room;
    final disabled = session.conn != ConnectionState.connected;
    if (room == null || disabled || _sharing) return;
    final file = await openFile();
    if (file == null || !mounted) return;
    setState(() {
      _sharing = true;
      _error = null;
    });
    try {
      // Zero-byte files are filtered out, like the reference paste/drop path.
      final length = await file.length();
      if (length > 0) {
        await room.shareUserFile(file.path, name: file.name, mime: file.mimeType);
      }
    } catch (e) {
      if (mounted) setState(() => _error = errorShape(e));
    } finally {
      if (mounted) setState(() => _sharing = false);
    }
  }

  /// Enter sends; Shift+Enter falls through and inserts a newline. Repeat
  /// events from a HELD Enter are swallowed too — letting them through would
  /// bypass the send and pour newlines into the just-cleared field (which
  /// then persist as the room draft).
  KeyEventResult _onKeyEvent(FocusNode node, KeyEvent event) {
    if ((event is KeyDownEvent || event is KeyRepeatEvent) &&
        (event.logicalKey == LogicalKeyboardKey.enter ||
            event.logicalKey == LogicalKeyboardKey.numpadEnter) &&
        !HardwareKeyboard.instance.isShiftPressed) {
      if (event is KeyDownEvent) _send();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final session = SessionScope.of(context);
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final disabled = session.conn != ConnectionState.connected;
    final draftBlank = _controller.text.trim().isEmpty;

    String roomName = s.shellUntitledRoom;
    final roomId = session.currentRoomId;
    for (final r in session.rooms) {
      if (r.roomId == roomId) {
        roomName = r.name ?? s.shellUntitledRoom;
        break;
      }
    }
    final placeholder = s.composerMessagePlaceholder(roomName);

    return Container(
      padding: const EdgeInsets.fromLTRB(
          JeliyaSpacing.page, JeliyaSpacing.x12, JeliyaSpacing.page, JeliyaSpacing.x14),
      decoration: BoxDecoration(
        color: tokens.bgRaise,
        border: Border(top: BorderSide(color: tokens.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(bottom: JeliyaSpacing.x10),
              child: ErrorNote(error: _error),
            ),
          Container(
            padding: const EdgeInsets.symmetric(
                horizontal: JeliyaSpacing.x10, vertical: JeliyaSpacing.x8),
            decoration: BoxDecoration(
              color: tokens.bgInput,
              borderRadius: BorderRadius.circular(JeliyaRadii.composer),
              border: Border.all(
                  color: _focused ? tokens.accentLine : tokens.borderStrong),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                _ShareFileButton(
                  busy: _sharing,
                  onPressed: (disabled || _sharing) ? null : _shareFile,
                ),
                const SizedBox(width: JeliyaSpacing.x8),
                Expanded(
                  child: Focus(
                    onKeyEvent: _onKeyEvent,
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxHeight: 150),
                      child: TextField(
                        controller: _controller,
                        focusNode: _focusNode,
                        enabled: !disabled,
                        minLines: 1,
                        maxLines: null,
                        keyboardType: TextInputType.multiline,
                        // Soft keyboards keep a newline key, explicitly —
                        // sending stays on the ➤ button (hardware Enter
                        // sends via the Focus handler above).
                        textInputAction: TextInputAction.newline,
                        cursorColor: tokens.accent,
                        style: JeliyaText.body,
                        decoration: InputDecoration(
                          isCollapsed: true,
                          filled: false,
                          border: InputBorder.none,
                          enabledBorder: InputBorder.none,
                          focusedBorder: InputBorder.none,
                          disabledBorder: InputBorder.none,
                          contentPadding: const EdgeInsets.symmetric(
                              horizontal: JeliyaSpacing.x4,
                              vertical: JeliyaSpacing.x6),
                          hintText: placeholder,
                          hintStyle:
                              TextStyle(fontSize: 14, color: tokens.textMute),
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: JeliyaSpacing.x10),
                JeliyaButton(
                  label: _sending
                      ? Tokens.composerSendingGlyph
                      : Tokens.composerSendGlyph,
                  semanticLabel: s.composerSendMessage,
                  variant: JeliyaButtonVariant.primary,
                  onPressed:
                      (disabled || _sending || draftBlank) ? null : _send,
                ),
              ],
            ),
          ),
          const SizedBox(height: JeliyaSpacing.x6),
          Align(
            alignment: Alignment.centerRight,
            child: Text(
              _sharing ? s.composerSharingFile : s.composerHint,
              style: TextStyle(fontSize: 11.5, color: tokens.textDim),
            ),
          ),
        ],
      ),
    );
  }
}

/// The quiet 26px icon-btn file-share affordance ('⎘') — dim ink, accent on
/// hover, spinner while the staging upload is in flight.
class _ShareFileButton extends StatelessWidget {
  const _ShareFileButton({required this.busy, required this.onPressed});

  final bool busy;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return Tooltip(
      message: s.composerShareAFile,
      child: Semantics(
        label: s.composerShareAFile,
        button: true,
        child: TextButton(
          onPressed: onPressed,
          style: TextButton.styleFrom(
            foregroundColor: tokens.textDim,
            disabledForegroundColor: tokens.textDim.withValues(alpha: 0.55),
            padding: EdgeInsets.zero,
            minimumSize: const Size(26, 26),
            fixedSize: const Size(26, 26),
            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(JeliyaRadii.iconBtn),
            ),
          ),
          child: busy
              ? SizedBox(
                  width: 11,
                  height: 11,
                  child: CircularProgressIndicator(
                      strokeWidth: 2, color: tokens.textDim),
                )
              : const Text(Tokens.composerShareGlyph,
                  style: TextStyle(fontSize: 15)),
        ),
      ),
    );
  }
}
