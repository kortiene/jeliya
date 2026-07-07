import type { Member, PeerStatus } from '../lib/protocol';
import { shortId } from '../lib/format';
import { useNames } from './names';

/** Peer path (direct/relay) and state are shown exactly as reported by the
 *  daemon — relay fallback is never hidden (honesty rule #2). */
function PeerChip({ peer }: { peer: PeerStatus }) {
  const names = useNames();
  const label =
    peer.state === 'connected' ? (peer.path ?? 'connected') : peer.state === 'connecting' ? 'connecting' : 'offline';
  // identity_id is only known once the SDK has bound the device (on admit);
  // fall back to the raw endpoint id until then, but keep the hex around in
  // the tooltip either way.
  const display = peer.identity_id ? names.display(peer.identity_id) : shortId(peer.endpoint_id);
  return (
    <span className={`peer-chip peer-${peer.state} peer-path-${peer.path ?? 'none'}`} title={peer.endpoint_id}>
      <span className="dot" /> {display} <em>{label}</em>
    </span>
  );
}

export function RoomHeader({
  name,
  memberCount,
  members,
  peers,
  onInvite,
  onShareFile,
  onOpenPipe,
}: {
  name: string;
  memberCount: number;
  members: Member[];
  peers: PeerStatus[];
  onInvite(): void;
  onShareFile(): void;
  onOpenPipe(): void;
}) {
  const activeCount = members.length > 0 ? members.filter((m) => m.status === 'active').length : memberCount;
  const invitedCount = members.filter((m) => m.status === 'invited').length;
  const agentCount = members.filter((m) => m.role === 'agent').length;
  const connected = peers.filter((p) => p.state === 'connected');
  const hasDirect = connected.some((p) => p.path === 'direct');
  // Three honest states: nobody here, a live direct link, or relay-only
  // (still connected, just not the path we'd prefer). A room with peers that
  // are merely connecting/offline reads as the same "alone" state — there's
  // no live link to call peer-to-peer yet either way.
  const p2p =
    peers.length === 0
      ? { dot: 'dot-neutral', label: 'Alone in this room' }
      : hasDirect
        ? { dot: 'dot-green', label: 'Peer-to-Peer' }
        : connected.length > 0
          ? { dot: null, label: 'Relay only' } // amber: no dedicated CSS class, see inline style below
          : { dot: 'dot-neutral', label: 'Alone in this room' };
  return (
    <header className="room-header">
      <div className="room-header-top">
        <div className="room-title">
          <h1>{name}</h1>
          <div className="room-subtitle">
            <span>
              {activeCount} active
            </span>
            {agentCount > 0 ? (
              <>
                <span className="sep">|</span>
                <span>
                  {agentCount} agent{agentCount === 1 ? '' : 's'}
                </span>
              </>
            ) : null}
            {invitedCount > 0 ? (
              <>
                <span className="sep">|</span>
                <span className="pending-invites">
                  {invitedCount} invite{invitedCount === 1 ? '' : 's'} pending
                </span>
              </>
            ) : null}
            <span className="sep">|</span>
            <span className="p2p-badge">
              <span
                className={p2p.dot ? `dot ${p2p.dot}` : 'dot'}
                style={p2p.dot ? undefined : { color: 'var(--amber)' }}
              />{' '}
              {p2p.label}
            </span>
          </div>
        </div>
        <div className="room-actions">
          <button type="button" className="btn" onClick={onShareFile}>
            <span aria-hidden="true">⎘</span> Share File
          </button>
          <button type="button" className="btn" onClick={onOpenPipe}>
            <span aria-hidden="true">⤳</span> Open Pipe
          </button>
          <button type="button" className="btn btn-primary" onClick={onInvite}>
            <span aria-hidden="true">⊕</span> Invite
          </button>
        </div>
      </div>
      {peers.length > 0 ? (
        <div className="peer-strip" role="group" aria-label="Peer connections">
          {peers.map((p) => (
            <PeerChip key={p.endpoint_id} peer={p} />
          ))}
        </div>
      ) : null}
    </header>
  );
}
