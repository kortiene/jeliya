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
import type { Catalog } from '../l10n/catalog';
import { useStrings } from '../l10n/strings';
import { Template } from '../l10n/template';
import { Example, Glyph } from '../l10n/tokens';
import { rolePill } from '../l10n/wireDisplay';
import { CopyButton, ErrorNote, Modal } from './ui';
import { QrCode } from './QrCode';

/** Friendly labels for the shared EXPIRY_PRESETS keys. The convention lives in
 *  invite.ts (label-free on purpose); each client localizes the key. */
function expiryLabel(s: Catalog, key: string): string {
  switch (key) {
    case '1h':
      return s.inviteExpiry1h;
    case '24h':
      return s.inviteExpiry24h;
    case '7d':
      return s.inviteExpiry7d;
    case 'never':
      return s.inviteExpiryNever;
    default:
      return key;
  }
}

/** The default preset when the flow opens fresh — a bounded, single-day ticket
 *  is the safer default than a never-expiring one. */
const DEFAULT_PRESET = '24h';

/** The runtime proof of a pending invitation: a signed `invited` roster row.
 *  Reopening the flow restores from this rather than a blank draft. */
function pendingInvite(members: readonly Member[]): Member | null {
  return members.find((m) => m.status === 'invited') ?? null;
}

function lifecycleText(s: Catalog, state: InviteState): string {
  switch (state) {
    case 'joined':
      return s.inviteLifecycleJoinedCopy;
    case 'expired':
      return s.inviteLifecycleExpiredCopy;
    default:
      return s.inviteLifecycleWaitingCopy;
  }
}

function LifecycleChip({ state }: { state: InviteState }) {
  const s = useStrings();
  if (state === 'joined') {
    return (
      <span className="chip chip-label tone-green">
        <span className="dot dot-green" aria-hidden="true" /> {s.inviteLifecycleJoined}
      </span>
    );
  }
  if (state === 'expired') {
    return (
      <span className="chip chip-label tone-red">
        <span className="dot dot-red" aria-hidden="true" /> {s.inviteLifecycleExpired}
      </span>
    );
  }
  return (
    <span className="chip chip-label tone-neutral">
      <span className="dot dot-neutral" aria-hidden="true" /> {s.inviteLifecycleWaiting}
    </span>
  );
}

/** A client-local validation error is a designed form state, not a daemon
 * diagnostic. Keep only the boolean in state so switching locale while the
 * error is visible re-resolves all three lines on the next render. */
function InviteErrorNote({
  error,
  expiryInvalid,
}: {
  error: DaemonErrorShape | null;
  expiryInvalid: boolean;
}) {
  const s = useStrings();
  if (expiryInvalid) {
    return (
      <div className="error-note" role="alert">
        <strong className="error-title">{s.inviteExpiryErrorTitle}</strong>
        <span>{s.inviteExpiryErrorMessage}</span>
        <div className="error-hint">{s.inviteExpiryErrorHint}</div>
      </div>
    );
  }
  return <ErrorNote error={error} />;
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
  const s = useStrings();
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
  const [expiryInvalid, setExpiryInvalid] = useState(false);
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
        setExpiryInvalid(true);
        return;
      }
      expirySecs = n;
    } else {
      expirySecs = EXPIRY_PRESETS.find((p) => p.key === expiryKey)?.seconds ?? null;
    }

    setBusy(true);
    setError(null);
    setExpiryInvalid(false);
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
    setExpiryInvalid(false);
  };

  const share = async () => {
    if (!ticket) return;
    try {
      await navigator.share({
        title: s.inviteShareTitle,
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
      <Modal title={s.inviteTitle} onClose={onClose} wide busy={busy}>
        <div>
          {endpointAddr ? (
            <div className="invite-readiness invite-ready">
              <span className="dot dot-green" aria-hidden="true" />
              <div>
                <strong>{s.inviteReadyToSend}</strong>
                <p>{s.inviteReadyToSendCopy}</p>
              </div>
            </div>
          ) : (
            <div className="invite-readiness invite-caution">
              <span className="dot" aria-hidden="true" />
              <div>
                <strong>{s.inviteNoDialableAddress}</strong>
                <p>{s.inviteNoDialableAddressCopy}</p>
              </div>
            </div>
          )}

          {lifecycle ? (
            <div className="invite-lifecycle" role="status" aria-live="polite">
              <LifecycleChip state={lifecycle} />
              <span className="muted">{lifecycleText(s, lifecycle)}</span>
            </div>
          ) : null}

          <p className="muted">
            {endpointAddr
              ? s.inviteCombinedCopy
              : s.inviteTicketOnlyCopy}
          </p>
          <div className="ticket-box">
            <textarea
              className="mono"
              readOnly
              value={combined}
              rows={4}
              aria-label={endpointAddr ? s.inviteCombinedInviteLabel : s.inviteInviteTicketLabel}
              onFocus={(e) => e.target.select()}
            />
            <CopyButton text={combined} label={endpointAddr ? s.inviteCopyInvite : s.inviteCopyTicket} />
          </div>

          {/* Platform Share (Web Share API), feature-detected — rendered only
              where navigator.share exists. Copy is always the fallback. */}
          {canShare ? (
            <div className="invite-share-row">
              <button type="button" className="btn" onClick={() => void share()}>
                <span aria-hidden="true">{Glyph.share}</span>{' '}
                {endpointAddr ? s.inviteShareInvite : s.inviteShareTicket}
              </button>
            </div>
          ) : null}

          {/* QR of the SAME combined invite the Copy button carries (#103).
              Self-contained encoder (no CDN); returns nothing if the payload is
              too large for any symbol, leaving Copy/Share as the fallback. */}
          <QrCode
            value={combined}
            label={s.inviteQrLabel}
            caption={endpointAddr ? s.inviteQrCombinedCaption : s.inviteQrTicketCaption}
          />

          {endpointAddr ? (
            <details className="invite-advanced">
              <summary className="muted">{s.inviteSeparatelySummary}</summary>
              <div className="ticket-box">
                <textarea
                  className="mono"
                  readOnly
                  value={ticket}
                  rows={4}
                  aria-label={s.inviteInviteTicketLabel}
                  onFocus={(e) => e.target.select()}
                />
                <CopyButton text={ticket} label={s.inviteCopyTicket} />
              </div>
              <div className="ticket-box">
                <code className="mono addr-code">{endpointAddr}</code>
                <CopyButton text={endpointAddr} label={s.inviteCopyAddress} />
              </div>
            </details>
          ) : (
            <p className="muted">
              {s.inviteNoDialableAddressNote}
            </p>
          )}

          {lifecycle === 'expired' ? (
            <button
              type="button"
              className="btn btn-primary invite-again"
              onClick={() => void generate()}
              disabled={busy || !connected}
            >
              {busy ? s.inviteGenerating : connected ? s.inviteAgain : s.commonReconnecting}
            </button>
          ) : null}

          <button type="button" className="btn btn-ghost" onClick={newInvite}>
            <span aria-hidden="true">{Glyph.previous}</span>
            {s.inviteNewInvite}
          </button>

          <InviteErrorNote error={error} expiryInvalid={expiryInvalid} />
        </div>
      </Modal>
    );
  }

  // ---- draft pane (identity → role → expiry) -------------------------------
  return (
    <Modal title={s.inviteTitle} onClose={onClose} wide busy={busy}>
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
              {s.inviteAlreadyInvited}
            </span>
          </div>
        ) : (
          <>
            <p className="muted">
              {s.inviteIntro}
            </p>
            <div className="invite-readiness">
              <span className="dot dot-green" aria-hidden="true" />
              <div>
                <strong>{s.inviteRoomOpenForInviting}</strong>
                <p>{s.inviteRoomOpenForInvitingCopy}</p>
              </div>
            </div>
          </>
        )}

        {/* 1. Identity — validated inline against isIdentityId (bare 64-hex).
            Submit is disabled until it is valid, so an obvious typo fails in
            the form, never as a daemon invalid_params error. */}
        <label className="field">
          <span>{s.inviteInviteeIdentityId}</span>
          <input
            value={identityId}
            onChange={(e) => setIdentityId(e.target.value)}
            placeholder={s.inviteInviteePlaceholder}
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
              ? s.inviteIdentityInvalid
              : s.inviteIdentityHint}
          </span>
        </label>

        {/* 2. Role — with a one-line consequence for each, and a security
            warning when agent is selected (matching the Add-Agent modal). */}
        <fieldset className="field invite-roles">
          <legend>{s.inviteRoleLabel}</legend>
          <label className="role-option">
            <input
              type="radio"
              name="invite-role"
              checked={role === 'member'}
              onChange={() => setRole('member')}
            />
            <span>
              <Template
                template={s.inviteRoleMemberConsequence}
                slots={{ role: <strong>{rolePill(s, 'member')}</strong> }}
              />
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
              <Template
                template={s.inviteRoleAgentConsequence}
                slots={{ role: <strong>{rolePill(s, 'agent')}</strong> }}
              />
            </span>
          </label>
        </fieldset>
        {role === 'agent' ? (
          <p className="error-note" role="alert">{s.inviteAgentWarning}</p>
        ) : null}

        {/* 3. Expiry — presets over EXPIRY_PRESETS, plus an advanced custom
            seconds field behind a disclosure. The custom value, when set,
            overrides the selected preset. */}
        <div className="field">
          <span>{s.inviteTicketExpiryLabel}</span>
          <div className="expiry-presets" role="group" aria-label={s.inviteTicketExpiryLabel}>
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
                  {expiryLabel(s, p.key)}
                </button>
              );
            })}
          </div>
          {/* `onToggle` rather than a click handler that preventDefaults the
              native toggle: suppressing the browser's own behaviour also
              suppressed find-in-page auto-expansion, and made the open state
              depend on a React handler firing. The element stays the source of
              truth; state just follows it (issue #72). */}
          <details
            className="invite-advanced"
            open={advancedOpen}
            onToggle={(e) => setAdvancedOpen((e.currentTarget as HTMLDetailsElement).open)}
          >
            <summary className="muted">{s.inviteAdvancedExpiry}</summary>
            <label className="field">
              <span>
                <Template
                  template={s.commonOptionalFieldLabel}
                  slots={{
                    label: s.inviteCustomExpiryLabel,
                    optional: <em className="muted">{s.inviteCustomExpiryOverride}</em>,
                  }}
                />
              </span>
              <input
                value={customExpiry}
                onChange={(e) => setCustomExpiry(e.target.value)}
                placeholder={Example.expirySeconds}
                inputMode="numeric"
                aria-label={s.inviteCustomExpiryLabel}
              />
            </label>
          </details>
        </div>

        <button
          type="submit"
          className="btn btn-primary"
          disabled={busy || !idValid || !connected}
        >
          {busy
            ? s.inviteGenerating
            : !connected
              ? s.commonReconnecting
              : restoredWaiting
                ? s.inviteSendFresh
                : s.inviteGenerateTicket}
        </button>
        <InviteErrorNote error={error} expiryInvalid={expiryInvalid} />
      </form>
    </Modal>
  );
}
