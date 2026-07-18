/// Invite helpers, ported 1:1 from ui/src/lib/invite.ts: the invitee-identity
/// validator, the friendly expiry presets, the combined-invite builder/splitter,
/// and the honest lifecycle state machine (Joined only from signed active
/// membership).
library;

/// The protocol's invitee identity id: a bare 64-hex string (NOT the `blake3:`
/// room id). Validated inline before `invite.create` so an obvious typo fails
/// in the form, not as a daemon `invalid_params` error.
final RegExp _identityIdRe = RegExp(r'^[0-9a-f]{64}$', caseSensitive: false);

bool isIdentityId(String value) => _identityIdRe.hasMatch(value.trim());

/// A friendly expiry choice: a stable [key] the UI labels via l10n, and the
/// [seconds] passed to `invite.create` (null = no expiry / single-use).
class ExpiryPreset {
  const ExpiryPreset(this.key, this.seconds);

  final String key;
  final int? seconds;
}

/// The offered presets, in display order. Label-free on purpose (each client
/// localizes the key).
const List<ExpiryPreset> expiryPresets = [
  ExpiryPreset('1h', 60 * 60),
  ExpiryPreset('24h', 24 * 60 * 60),
  ExpiryPreset('7d', 7 * 24 * 60 * 60),
  ExpiryPreset('never', null),
];

/// Build a combined invite (`<ticket>#<peer addr>`) — the inverse of
/// [splitInvite]. An empty/blank address yields the bare ticket.
String buildCombinedInvite(String ticket, String peerAddr) {
  final addr = peerAddr.trim();
  return addr.isEmpty ? ticket : '$ticket#$addr';
}

/// The lifecycle of a minted invite, one of: `waiting`, `expired`, `joined`.
abstract final class InviteStates {
  static const String waiting = 'waiting';
  static const String expired = 'expired';
  static const String joined = 'joined';
}

/// Derive an invite's lifecycle from PROVABLE state only: `joined` iff a signed
/// roster row for [identityId] is `active` (the ONLY evidence that admits
/// "Joined"); else `expired` iff [expiresAtMs] has passed; else `waiting`.
/// Joined outranks expired. [members] is a list of `{identity_id, status}` maps
/// (the room.members projection); [nowMs] is the wall clock.
String inviteState(
  String identityId,
  int? expiresAtMs,
  List<Map<String, dynamic>> members,
  int nowMs,
) {
  final active = members.any(
      (m) => m['identity_id'] == identityId && m['status'] == 'active');
  if (active) return InviteStates.joined;
  if (expiresAtMs != null && nowMs > expiresAtMs) return InviteStates.expired;
  return InviteStates.waiting;
}

/// The two halves of a combined invite.
class SplitInvite {
  const SplitInvite({required this.ticket, required this.peerAddr});

  final String ticket;
  final String peerAddr;
}

/// Split a combined invite (`<ticket>#<peer addr>`) into its parts.
///
/// Tickets are base32 (`roomtkt1…`) and peer addresses are
/// `<endpoint_id>@host:port[,…]` — neither can contain `#`, so the split is
/// unambiguous. An explicitly provided peer address wins over the embedded
/// one; a leading `#` is not treated as a separator (the honest failure is
/// the daemon's bad-ticket error, not a silent empty ticket).
SplitInvite splitInvite(String ticketInput, String peerAddrInput) {
  var ticket = ticketInput.trim();
  var peerAddr = peerAddrInput.trim();
  final hash = ticket.indexOf('#');
  if (hash > 0) {
    if (peerAddr.isEmpty) peerAddr = ticket.substring(hash + 1).trim();
    ticket = ticket.substring(0, hash).trim();
  }
  return SplitInvite(ticket: ticket, peerAddr: peerAddr);
}
