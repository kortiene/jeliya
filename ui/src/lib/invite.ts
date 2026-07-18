/** The protocol's invitee identity id: a bare 64-hex string (NOT the `blake3:`
 *  room id). Validated inline before `invite.create` so an obvious typo fails
 *  in the form, not as a daemon `invalid_params` error. Mirrored 1:1 in
 *  dart/jeliya_protocol/lib/src/conventions/invite.dart. */
const IDENTITY_ID_RE = /^[0-9a-f]{64}$/i;

export function isIdentityId(value: string): boolean {
  return IDENTITY_ID_RE.test(value.trim());
}

/** A friendly expiry choice: a stable [key] the UI labels via l10n, and the
 *  [seconds] passed to `invite.create` (null = no expiry / single-use, not
 *  time-boxed). An "advanced custom" option lives outside this table. */
export interface ExpiryPreset {
  key: string;
  seconds: number | null;
}

/** The offered presets, in display order. Label-free on purpose so this stays a
 *  pure convention (each client localizes the key). */
export const EXPIRY_PRESETS: readonly ExpiryPreset[] = [
  { key: '1h', seconds: 60 * 60 },
  { key: '24h', seconds: 24 * 60 * 60 },
  { key: '7d', seconds: 7 * 24 * 60 * 60 },
  { key: 'never', seconds: null },
];

/** Build a combined invite (`<ticket>#<peer addr>`) — the inverse of
 *  [splitInvite]. An empty/blank address yields the bare ticket. */
export function buildCombinedInvite(ticket: string, peerAddr: string): string {
  const addr = peerAddr.trim();
  return addr ? `${ticket}#${addr}` : ticket;
}

/** A minted invite the client is tracking, enough to derive its lifecycle. */
export interface MintedInvite {
  identityId: string;
  /** Wall-clock ms when the ticket expires, or null for no expiry. */
  expiresAtMs: number | null;
}

/** The lifecycle of a minted invite, derived from PROVABLE state only:
 *  - `joined`  — a signed roster row for the invitee is `active` (the ONLY
 *                evidence that admits "Joined"; never inferred from the ticket);
 *  - `expired` — the ticket has an expiry that has passed and the invitee is
 *                not (yet) active, so it must be re-minted (Invite Again);
 *  - `waiting` — minted, not yet joined, not expired.
 *  Joined outranks expired: an invitee who joined before the ticket lapsed is
 *  in, expiry notwithstanding. */
export type InviteState = 'waiting' | 'expired' | 'joined';

export function inviteState(
  invite: MintedInvite,
  members: readonly { identity_id: string; status: string }[],
  nowMs: number,
): InviteState {
  const active = members.some((m) => m.identity_id === invite.identityId && m.status === 'active');
  if (active) return 'joined';
  if (invite.expiresAtMs !== null && nowMs > invite.expiresAtMs) return 'expired';
  return 'waiting';
}

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
