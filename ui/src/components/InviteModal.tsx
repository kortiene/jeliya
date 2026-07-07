import { useState } from 'react';
import type { Client, DaemonErrorShape } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { CopyButton, ErrorNote, Modal } from './ui';

/** Generates a ticket via invite.create and shows it copy-pasteable, along
 *  with our dialable address so the joiner can dial directly. */
export function InviteModal({
  client,
  roomId,
  endpointAddr,
  onClose,
}: {
  client: Client;
  roomId: string;
  endpointAddr: string | null;
  onClose(): void;
}) {
  const [identityId, setIdentityId] = useState('');
  const [role, setRole] = useState<'member' | 'agent'>('member');
  const [expiry, setExpiry] = useState('');
  const [busy, setBusy] = useState(false);
  const [ticket, setTicket] = useState<string | null>(null);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const generate = async () => {
    const invitee = identityId.trim();
    if (!invitee || busy) return;
    setBusy(true);
    setError(null);
    setTicket(null);
    try {
      const expiryText = expiry.trim();
      const expiryValue = expiryText ? Number(expiryText) : undefined;
      if (expiryText && (expiryValue === undefined || !Number.isInteger(expiryValue) || expiryValue <= 0)) {
        setError({
          code: 'invalid_params',
          message: 'expiry must be a positive number of seconds',
          hint: 'leave it blank or use a value like 3600',
        });
        return;
      }
      const expirySecs = expiryValue;
      const result = await client.call('invite.create', {
        room_id: roomId,
        identity_id: invitee,
        role,
        ...(expirySecs !== undefined && Number.isFinite(expirySecs) ? { expiry: expirySecs } : {}),
      });
      setTicket(result.ticket);
    } catch (e) {
      setError(errorShape(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Invite to room" onClose={onClose} wide>
      {!ticket ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void generate();
          }}
        >
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
          <label className="field">
            <span>Invitee identity id</span>
            <input
              value={identityId}
              onChange={(e) => setIdentityId(e.target.value)}
              placeholder="64-hex identity id"
              className="mono"
              spellCheck={false}
              autoFocus
            />
          </label>
          <div className="field-row">
            <label className="field">
              <span>Role</span>
              <select value={role} onChange={(e) => setRole(e.target.value as 'member' | 'agent')}>
                <option value="member">member</option>
                <option value="agent">agent</option>
              </select>
            </label>
            <label className="field">
              <span>
                Expiry seconds <em className="muted">(optional)</em>
              </span>
              <input
                value={expiry}
                onChange={(e) => setExpiry(e.target.value)}
                placeholder="3600"
                inputMode="numeric"
              />
            </label>
          </div>
          <button type="submit" className="btn btn-primary" disabled={busy || !identityId.trim()}>
            {busy ? 'Generating…' : 'Generate ticket'}
          </button>
          <ErrorNote error={error} />
        </form>
      ) : endpointAddr ? (
        <div>
          <div className="invite-readiness invite-ready">
            <span className="dot dot-green" aria-hidden="true" />
            <div>
              <strong>Ready to send.</strong>
              <p>Stay in this room until they join. If they still see “couldn't reach inviter,” copy a fresh invite and retry.</p>
            </div>
          </div>
          <p className="muted">
            Send this one paste to the invitee — it is the ticket and your dialable address together. They paste it
            into “Join with a ticket” and the address fills in automatically.
          </p>
          <div className="ticket-box">
            <textarea
              className="mono"
              readOnly
              value={`${ticket}#${endpointAddr}`}
              rows={4}
              aria-label="Combined invite (ticket and peer address)"
              onFocus={(e) => e.target.select()}
            />
            <CopyButton text={`${ticket}#${endpointAddr}`} label="Copy invite" />
          </div>
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
          <button type="button" className="btn btn-ghost" onClick={() => setTicket(null)}>
            <span aria-hidden="true">←</span> New invite
          </button>
        </div>
      ) : (
        <div>
          <div className="invite-readiness invite-caution">
            <span className="dot" aria-hidden="true" />
            <div>
              <strong>No dialable address reported yet.</strong>
              <p>Keep this room open. The joiner may still connect via discovery or relay, but a fresh room address is more reliable.</p>
            </div>
          </div>
          <p className="muted">Send this ticket to the invitee. They join with it (room.join).</p>
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
          <p className="muted">
            This daemon has not reported a dialable address — the joiner may connect via relay or discovery.
          </p>
          <button type="button" className="btn btn-ghost" onClick={() => setTicket(null)}>
            <span aria-hidden="true">←</span> New invite
          </button>
        </div>
      )}
    </Modal>
  );
}
