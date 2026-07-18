import { useEffect, useState } from 'react';
import type { Client, DaemonErrorShape, Member } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import {
  buildCombinedInvite,
  EXPIRY_PRESETS,
  inviteState,
  isIdentityId,
  type InviteState,
  type MintedInvite,
} from '../lib/invite';
import { CopyButton, ErrorNote, Modal } from './ui';
import { QrCode } from './QrCode';

/** Friendly labels for the shared EXPIRY_PRESETS keys. The convention lives in
 *  invite.ts (label-free on purpose); each client localizes the key. */
const EXPIRY_LABELS: Record<string, string> = {
  '1h': '1 hour',
  '24h': '24 hours',
  '7d': '7 days',
  never: 'No expiry',
};

/** The default preset when the flow opens fresh — a bounded, single-day ticket
 *  is the safer default than a never-expiring one. */
const DEFAULT_PRESET = '24h';

/** The runtime proof of a pending invitation: a signed `invited` roster row.
 *  Reopening the flow restores from this rather than a blank draft. */
function pendingInvite(members: readonly Member[]): Member | null {
  return members.find((m) => m.status === 'invited') ?? null;
}

function lifecycleText(state: InviteState): string {
  switch (state) {
    case 'joined':
      return 'They have joined the room — the roster confirms an active membership.';
    case 'expired':
      return 'This ticket has expired before they joined. Send a fresh one below.';
    default:
      return 'Waiting for them to join. This updates on its own when the roster changes.';
  }
}

function LifecycleChip({ state }: { state: InviteState }) {
  if (state === 'joined') {
    return (
      <span className="chip chip-label tone-green">
        <span className="dot dot-green" aria-hidden="true" /> Joined
      </span>
    );
  }
  if (state === 'expired') {
    return (
      <span className="chip chip-label tone-red">
        <span className="dot dot-red" aria-hidden="true" /> Expired
      </span>
    );
  }
  return (
    <span className="chip chip-label tone-neutral">
      <span className="dot dot-neutral" aria-hidden="true" /> Waiting
    </span>
  );
}

/** Guided invitation + re-invitation flow (issue #66, P14). Presents
 *  identity → role → expiry → sharing, and after minting tracks the invite's
 *  lifecycle from PROVABLE state only: the chip reads Joined ONLY when the
 *  room's roster shows an active row for the invitee (inviteState guarantees
 *  this). Reopening restores a pending `invited` row's waiting state. */
export function InviteModal({
  client,
  roomId,
  members,
  endpointAddr,
  connected,
  onClose,
}: {
  client: Client;
  roomId: string;
  /** The room's live roster (threaded from App). The lifecycle chip reads
   *  Joined only from an `active` row here, and reopen restores an `invited`
   *  row's waiting state — the roster is the runtime proof, not the ticket. */
  members: Member[];
  endpointAddr: string | null;
  /** Ticket generation is gated on a live daemon connection: a request
   *  queued while disconnected would keep the dialog busy — and
   *  undismissable — for as long as the reconnect takes. */
  connected: boolean;
  onClose(): void;
}) {
  // Derive the initial draft from the roster ONCE, on open: a still-pending
  // `invited` row restores the waiting state (identity, role, and a Waiting
  // chip) instead of a blank form. The mounted-once lazy initializers read the
  // members prop at open; live changes flow through the prop thereafter.
  const restored = pendingInvite(members);
  const [identityId, setIdentityId] = useState(() => restored?.identity_id ?? '');
  const [role, setRole] = useState<'member' | 'agent'>(() =>
    restored?.role === 'agent' ? 'agent' : 'member',
  );
  const [expiryKey, setExpiryKey] = useState<string>(DEFAULT_PRESET);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [customExpiry, setCustomExpiry] = useState('');
  const [busy, setBusy] = useState(false);
  const [ticket, setTicket] = useState<string | null>(null);
  // The invite we are tracking. Restored to a waiting invite from the roster;
  // the restored form has no ticket to re-show, so its expiry is unknown —
  // expiresAtMs null keeps the chip honest (Waiting until the roster proves
  // otherwise) rather than inventing an expiry we cannot know.
  const [minted, setMinted] = useState<MintedInvite | null>(() =>
    restored ? { identityId: restored.identity_id, expiresAtMs: null } : null,
  );
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());

  // Live expiry: while tracking a time-boxed invite, tick so waiting flips to
  // expired without a reload. Joined comes from the members prop (App refreshes
  // the roster on member events), so no timer is needed for that transition.
  useEffect(() => {
    if (!minted || minted.expiresAtMs === null) return;
    const id = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [minted]);

  const invitee = identityId.trim();
  const idValid = isIdentityId(invitee);
  const idTouched = invitee.length > 0;
  const canShare = typeof navigator !== 'undefined' && typeof navigator.share === 'function';
  const lifecycle = minted ? inviteState(minted, members, nowMs) : null;
  // A restored/pending invite that has no in-session ticket to display.
  const restoredWaiting = minted !== null && ticket === null;

  const generate = async () => {
    if (!idValid || busy || !connected) return;
    // Resolve the expiry: an advanced custom value (validated positive integer)
    // overrides the chosen preset; otherwise the preset's seconds (null = none).
    let expirySecs: number | null;
    const customText = customExpiry.trim();
    if (advancedOpen && customText) {
      const n = Number(customText);
      if (!Number.isInteger(n) || n <= 0) {
        setError({
          code: 'invalid_params',
          message: 'custom expiry must be a positive number of seconds',
          hint: 'e.g. 3600 for one hour, or pick a preset instead',
        });
        return;
      }
      expirySecs = n;
    } else {
      expirySecs = EXPIRY_PRESETS.find((p) => p.key === expiryKey)?.seconds ?? null;
    }

    setBusy(true);
    setError(null);
    try {
      const result = await client.call('invite.create', {
        room_id: roomId,
        identity_id: invitee,
        role,
        ...(expirySecs !== null ? { expiry: expirySecs } : {}),
      });
      setTicket(result.ticket);
      setMinted({
        identityId: invitee,
        expiresAtMs: expirySecs !== null ? Date.now() + expirySecs * 1000 : null,
      });
    } catch (e) {
      setError(errorShape(e));
    } finally {
      setBusy(false);
    }
  };

  const newInvite = () => {
    setTicket(null);
    setMinted(null);
    setIdentityId('');
    setError(null);
  };

  const share = async () => {
    if (!ticket) return;
    try {
      await navigator.share({
        title: 'Jeliya room invite',
        text: buildCombinedInvite(ticket, endpointAddr ?? ''),
      });
    } catch {
      // The user cancelled the share sheet, or the platform refused it. The
      // Copy affordance is always present, so there is nothing to recover.
    }
  };

  // ---- sharing pane (after a ticket is minted this session) ----------------
  if (ticket) {
    const combined = buildCombinedInvite(ticket, endpointAddr ?? '');
    return (
      <Modal title="Invite to room" onClose={onClose} wide busy={busy}>
        <div>
          {endpointAddr ? (
            <div className="invite-readiness invite-ready">
              <span className="dot dot-green" aria-hidden="true" />
              <div>
                <strong>Ready to send.</strong>
                <p>Stay in this room until they join. If they still see “couldn't reach inviter,” send a fresh invite and retry.</p>
              </div>
            </div>
          ) : (
            <div className="invite-readiness invite-caution">
              <span className="dot" aria-hidden="true" />
              <div>
                <strong>No dialable address reported yet.</strong>
                <p>Keep this room open. The joiner may still connect via discovery or relay, but a fresh room address is more reliable.</p>
              </div>
            </div>
          )}

          {lifecycle ? (
            <div className="invite-lifecycle" role="status" aria-live="polite">
              <LifecycleChip state={lifecycle} />
              <span className="muted">{lifecycleText(lifecycle)}</span>
            </div>
          ) : null}

          <p className="muted">
            {endpointAddr
              ? 'Send this one paste to the invitee — it is the ticket and your dialable address together. They paste it into “Join with a ticket” and the address fills in automatically.'
              : 'Send this ticket to the invitee. They join with it (room.join).'}
          </p>
          <div className="ticket-box">
            <textarea
              className="mono"
              readOnly
              value={combined}
              rows={4}
              aria-label={endpointAddr ? 'Combined invite (ticket and peer address)' : 'Invite ticket'}
              onFocus={(e) => e.target.select()}
            />
            <CopyButton text={combined} label={endpointAddr ? 'Copy invite' : 'Copy ticket'} />
          </div>

          {/* Platform Share (Web Share API), feature-detected — rendered only
              where navigator.share exists. Copy is always the fallback. */}
          {canShare ? (
            <div className="invite-share-row">
              <button type="button" className="btn" onClick={() => void share()}>
                <span aria-hidden="true">↗</span> Share…
              </button>
            </div>
          ) : null}

          {/* QR of the SAME combined invite the Copy button carries (#103).
              Self-contained encoder (no CDN); returns nothing if the payload is
              too large for any symbol, leaving Copy/Share as the fallback. */}
          <QrCode
            value={combined}
            label="QR code for the room invite — scan on another device to join"
            caption={endpointAddr ? 'Scan to join — this is the same invite as above.' : 'Scan to import this ticket on another device.'}
          />

          {endpointAddr ? (
            <details className="invite-advanced">
              <summary className="muted">Send the ticket and address separately</summary>
              <div className="ticket-box">
                <textarea
                  className="mono"
                  readOnly
                  value={ticket}
                  rows={4}
                  aria-label="Invite ticket"
                  onFocus={(e) => e.target.select()}
                />
                <CopyButton text={ticket} label="Copy ticket" />
              </div>
              <div className="ticket-box">
                <code className="mono addr-code">{endpointAddr}</code>
                <CopyButton text={endpointAddr} label="Copy address" />
              </div>
            </details>
          ) : (
            <p className="muted">
              This daemon has not reported a dialable address — the joiner may connect via relay or discovery.
            </p>
          )}

          {lifecycle === 'expired' ? (
            <button
              type="button"
              className="btn btn-primary invite-again"
              onClick={() => void generate()}
              disabled={busy || !connected}
            >
              {busy ? 'Minting…' : connected ? 'Invite again' : 'Reconnecting…'}
            </button>
          ) : null}

          <button type="button" className="btn btn-ghost" onClick={newInvite}>
            <span aria-hidden="true">←</span> New invite
          </button>

          <ErrorNote error={error} />
        </div>
      </Modal>
    );
  }

  // ---- draft pane (identity → role → expiry) -------------------------------
  return (
    <Modal title="Invite to room" onClose={onClose} wide busy={busy}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void generate();
        }}
      >
        {restoredWaiting && lifecycle ? (
          // Reopened over a still-pending invitation: restore the proven waiting
          // state instead of a blank draft. Sending again mints a fresh ticket.
          <div className="invite-lifecycle" role="status" aria-live="polite">
            <LifecycleChip state={lifecycle} />
            <span className="muted">
              You have already invited this identity and they have not joined yet. Send a fresh invite below.
            </span>
          </div>
        ) : (
          <>
            <p className="muted">
              Tickets are bound to one identity. Ask the invitee for their identity id — it is shown on their onboarding
              screen and in their sidebar footer, with a copy button.
            </p>
            <div className="invite-readiness">
              <span className="dot dot-green" aria-hidden="true" />
              <div>
                <strong>This room is open for inviting.</strong>
                <p>Keep it open until the invitee finishes joining. Jeliya can only bootstrap them while an owner is reachable.</p>
              </div>
            </div>
          </>
        )}

        {/* 1. Identity — validated inline against isIdentityId (bare 64-hex).
            Submit is disabled until it is valid, so an obvious typo fails in
            the form, never as a daemon invalid_params error. */}
        <label className="field">
          <span>Invitee identity id</span>
          <input
            value={identityId}
            onChange={(e) => setIdentityId(e.target.value)}
            placeholder="64-hex identity id"
            className="mono"
            spellCheck={false}
            autoFocus
            aria-invalid={idTouched && !idValid}
            aria-describedby="invite-id-hint"
          />
          <span
            id="invite-id-hint"
            className={idTouched && !idValid ? 'field-hint field-error' : 'field-hint muted'}
          >
            {idTouched && !idValid
              ? 'That is not a valid identity id — it must be exactly 64 hexadecimal characters.'
              : 'Paste the invitee’s 64-hex identity id, shown on their onboarding screen and sidebar footer.'}
          </span>
        </label>

        {/* 2. Role — with a one-line consequence for each, and a security
            warning when agent is selected (matching the Add-Agent modal). */}
        <fieldset className="field invite-roles">
          <legend>Role</legend>
          <label className="role-option">
            <input
              type="radio"
              name="invite-role"
              checked={role === 'member'}
              onChange={() => setRole('member')}
            />
            <span>
              <strong>Member</strong> — a person in the room: reads and posts, shares files. No command execution.
            </span>
          </label>
          <label className="role-option">
            <input
              type="radio"
              name="invite-role"
              checked={role === 'agent'}
              onChange={() => setRole('agent')}
            />
            <span>
              <strong>Agent</strong> — an automated participant that can act on this room’s allowlisted messages.
            </span>
          </label>
        </fieldset>
        {role === 'agent' ? (
          <p className="error-note" role="alert">
            WARNING — an agent invite authorizes an automated participant. Minting the ticket{' '}
            <strong>does not start anything</strong>: a human must run the agent on its own machine, where it can
            execute this room’s allowlisted commands — arbitrary code / file execution on that host. Only invite an
            agent for a room and senders you trust.
          </p>
        ) : null}

        {/* 3. Expiry — presets over EXPIRY_PRESETS, plus an advanced custom
            seconds field behind a disclosure. The custom value, when set,
            overrides the selected preset. */}
        <div className="field">
          <span>Ticket expiry</span>
          <div className="expiry-presets" role="group" aria-label="Ticket expiry">
            {EXPIRY_PRESETS.map((p) => {
              const selected = !(advancedOpen && customExpiry.trim()) && expiryKey === p.key;
              return (
                <button
                  key={p.key}
                  type="button"
                  className={`btn btn-sm${selected ? ' is-selected' : ''}`}
                  aria-pressed={selected}
                  onClick={() => {
                    setExpiryKey(p.key);
                    setCustomExpiry('');
                  }}
                >
                  {EXPIRY_LABELS[p.key] ?? p.key}
                </button>
              );
            })}
          </div>
          <details className="invite-advanced" open={advancedOpen}>
            <summary
              className="muted"
              onClick={(e) => {
                e.preventDefault();
                setAdvancedOpen((open) => !open);
              }}
            >
              Advanced / custom expiry
            </summary>
            <label className="field">
              <span>
                Custom expiry seconds <em className="muted">(overrides the preset above)</em>
              </span>
              <input
                value={customExpiry}
                onChange={(e) => setCustomExpiry(e.target.value)}
                placeholder="3600"
                inputMode="numeric"
                aria-label="Custom expiry seconds"
              />
            </label>
          </details>
        </div>

        <button
          type="submit"
          className="btn btn-primary"
          disabled={busy || !idValid || !connected}
        >
          {busy ? 'Generating…' : !connected ? 'Reconnecting…' : restoredWaiting ? 'Send a fresh invite' : 'Generate ticket'}
        </button>
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}
