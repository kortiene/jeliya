import { useState } from 'react';
import { splitInvite } from '../lib/invite';
import type { Client, DaemonErrorShape } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { joinRoomWithRetry } from '../lib/join';
import type { JoinProgress } from '../lib/join';
import type { Catalog } from '../l10n/catalog';
import type { Formats } from '../l10n/formats';
import { useFormats, useStrings } from '../l10n/strings';
import { Template } from '../l10n/template';
import { Example } from '../l10n/tokens';
import { CopyButton, ErrorNote, SelfLabelField, TreeMark, Wordmark } from './ui';

// i18n-exempt: literal combined-invite wire syntax rendered inside localized copy.
const COMBINED_INVITE_SYNTAX = 'ticket#address';

function joinProgressMessage(s: Catalog, formats: Formats, progress: JoinProgress): string {
  if (progress.phase === 'retrying' && progress.retryDelayMs !== undefined) {
    const seconds = Math.round(progress.retryDelayMs / 1000);
    return s.onboardingJoinRetryWait(seconds, formats.count(seconds));
  }
  return progress.attempt === 1
    ? s.onboardingJoinFinding
    : s.onboardingJoinRetryingAttempt(
        progress.attempt,
        progress.maxAttempts,
        formats.count(progress.attempt),
        formats.count(progress.maxAttempts),
      );
}

/** No identity yet → create one. No rooms yet → create or join by ticket.
 *  Mirrors identity.create / room.create / room.join exactly. */
export function Onboarding({
  step,
  client,
  identityId,
  selfLabel,
  onSetSelfLabel,
  onAdvance,
}: {
  step: 'identity' | 'rooms';
  client: Client;
  identityId: string | null;
  selfLabel: string;
  onSetSelfLabel(label: string): void;
  onAdvance(): void;
}) {
  const s = useStrings();
  return (
    // Onboarding is a full-page destination, so its content lives in a page
    // landmark rather than a bare div — every step's copy used to be
    // landmark-orphaned (issue #72). The `h1` stays the wordmark: the rooms
    // step offers TWO equal tasks ("Create a room" and "Join with a ticket"),
    // so there is no single task heading that could honestly be the h1, and
    // promoting either would rank one above the other.
    <main className="onboarding" id="onboarding-main">
      <div className="onboarding-brand">
        <TreeMark size={44} />
        <Wordmark as="h1" />
        <p className="onboarding-tag">{s.onboardingTagline}</p>
      </div>
      {step === 'identity' ? (
        <IdentityStep client={client} onAdvance={onAdvance} />
      ) : (
        <RoomsStep
          client={client}
          identityId={identityId}
          selfLabel={selfLabel}
          onSetSelfLabel={onSetSelfLabel}
          onAdvance={onAdvance}
        />
      )}
    </main>
  );
}

function IdentityStep({ client, onAdvance }: { client: Client; onAdvance(): void }) {
  const s = useStrings();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.call('identity.create', {});
      onAdvance();
    } catch (e) {
      const err = errorShape(e);
      if (err.code === 'identity_exists') {
        onAdvance(); // someone else created it — just re-sync
      } else {
        setError(err);
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="onboarding-card">
      <h2>{s.onboardingIdentityTitle}</h2>
      <p className="muted">{s.onboardingIdentityCopy1}</p>
      <p className="muted">{s.onboardingIdentityCopy2}</p>
      <button type="button" className="btn btn-primary btn-lg" disabled={busy} onClick={() => void create()}>
        {busy ? s.onboardingCreatingIdentity : s.onboardingCreateIdentity}
      </button>
      <ErrorNote error={error} />
    </div>
  );
}

function RoomsStep({
  client,
  identityId,
  selfLabel,
  onSetSelfLabel,
  onAdvance,
}: {
  client: Client;
  identityId: string | null;
  selfLabel: string;
  onSetSelfLabel(label: string): void;
  onAdvance(): void;
}) {
  const s = useStrings();
  const formats = useFormats();
  const [name, setName] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<DaemonErrorShape | null>(null);

  const [ticket, setTicket] = useState('');
  const [peerAddr, setPeerAddr] = useState('');
  const [joining, setJoining] = useState(false);
  const [joinError, setJoinError] = useState<DaemonErrorShape | null>(null);
  const [joinProgress, setJoinProgress] = useState<JoinProgress | null>(null);

  const create = async () => {
    if (!name.trim()) return;
    setCreating(true);
    setCreateError(null);
    try {
      await client.call('room.create', { name: name.trim() });
      onAdvance();
    } catch (e) {
      setCreateError(errorShape(e));
    } finally {
      setCreating(false);
    }
  };

  const join = async () => {
    if (!ticket.trim()) return;
    setJoining(true);
    setJoinError(null);
    setJoinProgress(null);
    try {
      const { ticket: t, peerAddr: addr } = splitInvite(ticket, peerAddr);
      await joinRoomWithRetry(client, {
        ticket: t,
        ...(addr ? { peers: [addr] } : {}),
      }, setJoinProgress);
      onAdvance();
    } catch (e) {
      setJoinError(errorShape(e));
      setJoinProgress(null);
    } finally {
      setJoining(false);
    }
  };

  return (
    <div className="onboarding-rooms">
      {identityId ? (
        <div className="onboarding-identity">
          <div className="onboarding-identity-head">
            <span className="identity-label">{s.onboardingYourIdentityId}</span>
            <CopyButton text={identityId} label={s.commonCopy} ariaLabel={s.identityCopy} />
          </div>
          <code className="mono onboarding-identity-id">{identityId}</code>
          <p className="muted">{s.onboardingIdentityCardCopy1}</p>
          <p className="muted">{s.onboardingIdentityCardCopy2}</p>
          <SelfLabelField value={selfLabel} onChange={onSetSelfLabel} />
        </div>
      ) : null}
      <div className="onboarding-columns">
        <form
          className="onboarding-card"
          onSubmit={(e) => {
            e.preventDefault();
            void create();
          }}
        >
          <h2>{s.modalCreateTitle}</h2>
          <p className="muted">{s.onboardingCreateRoomCopy}</p>
          <label className="field">
            <span>{s.modalRoomNameLabel}</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={s.modalRoomNamePlaceholder}
              autoFocus
            />
          </label>
          <button type="submit" className="btn btn-primary" disabled={creating || !name.trim()}>
            {creating ? s.modalCreating : s.roomsCreate}
          </button>
          <ErrorNote error={createError} />
        </form>

        <form
          className="onboarding-card"
          onSubmit={(e) => {
            e.preventDefault();
            void join();
          }}
        >
          <h2>{s.roomsJoinWithTicket}</h2>
          <p className="muted">
            <Template
              template={s.modalJoinCopy}
              slots={{ combined: <code>{COMBINED_INVITE_SYNTAX}</code> }}
            />
          </p>
          <label className="field">
            <span>{s.modalTicketLabel}</span>
            <textarea
              value={ticket}
              onChange={(e) => setTicket(e.target.value)}
              placeholder={s.modalTicketPlaceholder}
              rows={3}
              spellCheck={false}
            />
          </label>
          <label className="field">
            <span>
              <Template
                template={s.commonOptionalFieldLabel}
                slots={{
                  label: s.modalPeerAddrLabel,
                  optional: <em className="muted">{s.commonOptional}</em>,
                }}
              />
            </span>
            <input
              value={peerAddr}
              onChange={(e) => setPeerAddr(e.target.value)}
              placeholder={Example.peerAddress}
              spellCheck={false}
            />
          </label>
          <button type="submit" className="btn btn-primary" disabled={joining || !ticket.trim()}>
            {joining ? s.modalJoining : s.modalJoinSubmit}
          </button>
          {joinProgress ? (
            <div className="join-progress" role="status">
              <span className="spinner" aria-hidden="true" />
              <span>{joinProgressMessage(s, formats, joinProgress)}</span>
              <em>
                {s.modalJoinAttempt(
                  joinProgress.attempt,
                  joinProgress.maxAttempts,
                  formats.count(joinProgress.attempt),
                  formats.count(joinProgress.maxAttempts),
                )}
              </em>
            </div>
          ) : null}
          <ErrorNote error={joinError} />
        </form>
      </div>
    </div>
  );
}
