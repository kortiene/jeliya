/// Rename peer modal (App.tsx `RenameModal`) — a LOCAL alias only; names
/// never leave this machine (never wire data). Save stores the trimmed alias
/// via the session prefs (the localStorage 'jeliya.aliases.v1' counterpart);
/// an empty alias — or 'Clear alias' — deletes it. Self is never renameable
/// (SenderName never opens this modal for self).
library;

import 'package:flutter/material.dart';

import '../../l10n/strings_context.dart';
import '../../session/daemon_session.dart';
import '../../theme.dart';
import '../../widgets/buttons.dart';
import '../../widgets/modal_scaffold.dart';

class RenamePeerModal extends StatefulWidget {
  const RenamePeerModal({super.key, required this.identityId});

  /// The peer's full identity id (never self).
  final String identityId;

  @override
  State<RenamePeerModal> createState() => _RenamePeerModalState();
}

class _RenamePeerModalState extends State<RenamePeerModal> {
  TextEditingController? _alias;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // Pre-fill with the current alias (empty against a real daemon when none
    // is stored — there are no seeded suggestions outside mock fixtures).
    _alias ??= TextEditingController(
      text: SessionScope.of(context).prefs.aliasFor(widget.identityId) ?? '',
    );
  }

  @override
  void dispose() {
    _alias?.dispose();
    super.dispose();
  }

  void _save() {
    // setAlias trims and deletes on blank — the reference Save semantics.
    SessionScope.of(context).setAlias(widget.identityId, _alias?.text);
    Navigator.of(context).pop();
  }

  void _clear() {
    SessionScope.of(context).setAlias(widget.identityId, null);
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    return ModalScaffold(
      title: s.renamePeerTitle,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            s.renamePeerCopy,
            style: TextStyle(fontSize: 13, color: tokens.textDim),
          ),
          const SizedBox(height: JeliyaSpacing.x4),
          Text(
            s.renamePeerIdentityLabel,
            style: TextStyle(fontSize: 12.5, color: tokens.textDim),
          ),
          const SizedBox(height: JeliyaSpacing.x2),
          SelectableText(
            widget.identityId,
            style: JeliyaText.mono(fontSize: 11, color: tokens.textMute),
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          Padding(
            padding: const EdgeInsets.only(bottom: 5),
            child: Text(
              s.renamePeerAliasLabel,
              style: TextStyle(fontSize: 12.5, color: tokens.textDim),
            ),
          ),
          TextField(
            controller: _alias,
            autofocus: true,
            decoration:
                InputDecoration(hintText: s.renamePeerAliasPlaceholder),
            onSubmitted: (_) => _save(),
          ),
          const SizedBox(height: JeliyaSpacing.x12),
          // Wrap + scale-down, not Row: at the 360dp dialog width the French
          // 'Supprimer l'alias' no longer fits beside Save (leave_room.dart
          // has the same treatment).
          Wrap(
            spacing: JeliyaSpacing.x8,
            runSpacing: JeliyaSpacing.x8,
            children: [
              FittedBox(
                fit: BoxFit.scaleDown,
                child: JeliyaButton(
                  label: s.renamePeerSave,
                  variant: JeliyaButtonVariant.primary,
                  onPressed: _save,
                ),
              ),
              FittedBox(
                fit: BoxFit.scaleDown,
                child: JeliyaButton(
                  label: s.renamePeerClearAlias,
                  variant: JeliyaButtonVariant.ghost,
                  onPressed: _clear,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
