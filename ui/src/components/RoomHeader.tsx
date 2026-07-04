import type { PeerStatus } from '../lib/protocol';
import { shortId } from '../lib/format';

/** Peer path (direct/relay) and state are shown exactly as reported by the
 *  daemon — relay fallback is never hidden (honesty rule #2). */
function PeerChip({ peer }: { peer: PeerStatus }) {
  const label =
    peer.state === 'connected' ? (peer.path ?? 'connected') : peer.state === 'connecting' ? 'connecting' : 'offline';
  return (
    <span className={`peer-chip peer-${peer.state} peer-path-${peer.path ?? 'none'}`} title={peer.endpoint_id}>
      <span className="dot" /> {shortId(peer.endpoint_id)} <em>{label}</em>
    </span>
  );
}

export function RoomHeader({
  name,
  memberCount,
  peers,
  onInvite,
  onShareFile,
  onOpenPipe,
}: {
  name: string;
  memberCount: number;
  peers: PeerStatus[];
  onInvite(): void;
  onShareFile(): void;
  onOpenPipe(): void;
}) {
  return (
    <header className="room-header">
      <div className="room-header-top">
        <div className="room-title">
          <h1>{name}</h1>
          <div className="room-subtitle">
            <span>
              {memberCount} member{memberCount === 1 ? '' : 's'}
            </span>
            <span className="sep">|</span>
            <span className="p2p-badge">
              <span className="dot dot-green" /> Peer-to-Peer
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
