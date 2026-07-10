/// Cross-cutting CONNECTION BANNER: role='status', hangs from the top edge
/// (radius 0 0 10 10), amber while connecting/reconnecting, red when
/// disconnected. Rendered above everything by BOTH shells (the desktop
/// three-pane layout and the mobile bottom-tab layout) whenever
/// conn != connected.
library;

import 'package:flutter/material.dart' hide ConnectionState;
import 'package:jeliya_protocol/jeliya_protocol.dart' show ConnectionState;

import '../l10n/strings_context.dart';
import '../theme.dart';

class ConnectionBanner extends StatelessWidget {
  const ConnectionBanner({super.key, required this.conn, required this.wsUrl});

  final ConnectionState conn;
  final String wsUrl;

  @override
  Widget build(BuildContext context) {
    final s = context.strings;
    final tokens = JeliyaTokens.of(context);
    final disconnected = conn == ConnectionState.disconnected;
    final text = disconnected
        ? s.shellBannerDisconnected
        : s.shellBannerReconnecting(wsUrl);
    final fg = disconnected ? tokens.red : tokens.amber;
    final bg = disconnected ? tokens.bannerDisconnectBg : tokens.bannerReconnectBg;
    final borderColor =
        disconnected ? tokens.redLine : tokens.bannerReconnectBorder;
    return Semantics(
      liveRegion: true, // role="status"
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
        decoration: BoxDecoration(
          color: bg,
          // Uniform border (a rounded box requires one); the top edge sits on
          // the window edge, matching the reference's "no top border" look.
          border: Border.all(color: borderColor),
          borderRadius: const BorderRadius.only(
            bottomLeft: Radius.circular(10),
            bottomRight: Radius.circular(10),
          ),
        ),
        child: Text(text, style: TextStyle(fontSize: 12.5, color: fg)),
      ),
    );
  }
}
