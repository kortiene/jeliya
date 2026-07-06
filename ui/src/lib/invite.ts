/** Split a combined invite (`<ticket>#<peer addr>`) into its parts.
 *
 *  Tickets are base32 (`roomtkt1…`) and peer addresses are
 *  `<endpoint_id>@host:port[,…]` — neither can contain `#`, so the split is
 *  unambiguous. An explicitly provided peer address wins over the embedded
 *  one; a leading `#` is not treated as a separator (the honest failure is
 *  the daemon's bad-ticket error, not a silent empty ticket). */
export function splitInvite(
  ticketInput: string,
  peerAddrInput: string,
): { ticket: string; peerAddr: string } {
  let ticket = ticketInput.trim();
  let peerAddr = peerAddrInput.trim();
  const hash = ticket.indexOf('#');
  if (hash > 0) {
    if (!peerAddr) peerAddr = ticket.slice(hash + 1).trim();
    ticket = ticket.slice(0, hash).trim();
  }
  return { ticket, peerAddr };
}
