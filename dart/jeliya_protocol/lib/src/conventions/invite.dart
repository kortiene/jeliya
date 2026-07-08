/// Combined-invite splitting, ported 1:1 from ui/src/lib/invite.ts.
library;

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
