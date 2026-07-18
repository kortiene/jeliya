import { useState } from 'react';
import type { Member, PeerStatus, RoomSummary } from '../lib/protocol';
import { shortId } from '../lib/format';
import type { Catalog } from '../l10n/catalog';
import { useFormats, useStrings } from '../l10n/strings';
import { Glyph, Punct } from '../l10n/tokens';
import { peerPath } from '../l10n/wireDisplay';
import { useNames } from './names';

/** Peer path (direct/relay) and state are shown exactly as reported by the
 *  daemon — relay fallback is never hidden (honesty rule #2). */
function peerStateLabel(s: Catalog, peer: PeerStatus): string {
  const state: string = peer.state;
  if (state === 'connected') {
    return peer.path === null ? s.roomHeaderPeerStateConnected : peerPath(s, peer.path);
  }
  if (state === 'connecting') return s.roomHeaderPeerStateConnecting;
  if (state === 'offline') return s.roomHeaderPeerStateOffline;
  return state;
}

function PeerChip({ peer }: { peer: PeerStatus }) {
  const s = useStrings();
  const names = useNames();
  const label = peerStateLabel(s, peer);
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

/** What the daemon's peer list actually proves (docs/room-workbench.md,
 *  decision 4). Peer reachability is an observed transport path and nothing
 *  more: it is not presence, and it is not who is in the room.
 *
 *  "Alone in this room" used to render here whenever zero connections were
 *  observed — including in a five-member room whose peers are merely offline.
 *  Absence of an observed connection is not evidence of solitude. */
function peerSummary(s: Catalog, peers: PeerStatus[]): { dot: 'dot-green' | 'dot-neutral' | null; label: string } {
  const connected = peers.filter((p) => p.state === 'connected');
  if (connected.some((p) => p.path === 'direct')) return { dot: 'dot-green', label: s.roomHeaderPeerToPeer };
  // Connected, just not on the path we would prefer. Amber, and never hidden.
  if (connected.some((p) => p.path === 'relay')) return { dot: null, label: s.roomHeaderRelayOnly };
  // `path` is nullable while `state` is already `connected`: the SDK knows the
  // peer is reachable before it knows how. Falling through to Relay here —
  // which is what a bare `connected.length > 0` did — would invent the exact
  // fact (direct vs relay) the honesty rules exist to protect. Green is
  // earned, the link is real; the path is simply not claimed until it is known.
  if (connected.length > 0) return { dot: 'dot-green', label: s.roomHeaderPeerConnected };
  if (peers.some((p) => p.state === 'connecting')) {
    return { dot: 'dot-neutral', label: s.roomHeaderPeerConnecting };
  }
  const futureState = peers.find((p) => !['connected', 'connecting', 'offline'].includes(p.state));
  if (futureState) return { dot: 'dot-neutral', label: futureState.state };
  return { dot: 'dot-neutral', label: s.roomHeaderNoPeersConnected };
}

export function RoomHeader({
  room,
  name,
  members,
  membersLoaded,
  peers,
  compact,
  onBack,
  onInvite,
  onShareFile,
  onOpenPipe,
}: {
  /** The room's list row — its short id disambiguates homonyms, and its
   *  member_count is the only count available before the roster loads. */
  room: RoomSummary;
  name: string;
  members: Member[];
  /** Whether `room.members` has answered. The roster count may not be shown
   *  before it has: the old header substituted the room's *total* count under
   *  an "N active" label, asserting a fact it did not have. */
  membersLoaded: boolean;
  peers: PeerStatus[];
  /** Compact renders the app bar form (#61): Back, a single-line title, the
   *  connectivity summary, Invite, and an overflow disclosure — under 150px
   *  at 320x568 so the timeline keeps at least 180px above the composer. */
  compact?: boolean;
  onBack(): void;
  onInvite(): void;
  onShareFile(): void;
  onOpenPipe(): void;
}) {
  const s = useStrings();
  const formats = useFormats();
  const [infoOpen, setInfoOpen] = useState(false);
  const memberCount = members.filter((m) => m.status === 'active').length;
  const invitedCount = members.filter((m) => m.status === 'invited').length;
  const agentCount = members.filter((m) => m.role === 'agent').length;
  const p2p = peerSummary(s, peers);

  const p2pBadge = (
    <span className="p2p-badge">
      <span className={p2p.dot ? `dot ${p2p.dot}` : 'dot'} style={p2p.dot ? undefined : { color: 'var(--amber)' }} />{' '}
      {p2p.label}
    </span>
  );

  if (compact) {
    return (
      <header className="room-header room-appbar">
        <div className="appbar-row">
          <button type="button" className="icon-btn appbar-back" onClick={onBack} aria-label={s.roomBackToRooms}>
            <span aria-hidden="true">{Glyph.back}</span>
          </button>
          <div className="appbar-title">
            <h1 title={name}>{name}</h1>
            <div className="appbar-sub">
              {membersLoaded ? (
                <span>{s.commonMemberCount(memberCount, formats.count(memberCount))}</span>
              ) : (
                <span className="muted">{s.roomLoadingMembers}</span>
              )}
              <span className="sep">{Punct.metaSep}</span>
              {p2pBadge}
            </div>
          </div>
          <button type="button" className="btn btn-primary appbar-invite" onClick={onInvite}>
            {s.roomHeaderInvite}
          </button>
          {/* Peer paths are diagnostic detail, not app-bar chrome: on a 320px
              phone the chip strip is what pushed the timeline under 180px.
              Behind a disclosure it stays reachable and stops competing. */}
          <button
            type="button"
            className="icon-btn appbar-more"
            aria-expanded={infoOpen}
            aria-label={s.roomInformation}
            onClick={() => setInfoOpen((open) => !open)}
          >
            <span aria-hidden="true">{Glyph.more}</span>
          </button>
        </div>
        {infoOpen ? (
          <div className="appbar-info">
            <dl className="room-info-facts">
              <dt>{s.roomInfoRoom}</dt>
              <dd className="mono">{shortId(room.room_id)}</dd>
              <dt>{s.roomInfoSession}</dt>
              <dd>{room.open ? s.roomsStateOpen : s.roomsStateClosed}</dd>
              {agentCount > 0 ? (
                <>
                  <dt>{s.roomInfoAgents}</dt>
                  <dd>{formats.count(agentCount)}</dd>
                </>
              ) : null}
              {invitedCount > 0 ? (
                <>
                  <dt>{s.roomInfoInvites}</dt>
                  <dd>{s.roomHeaderInvitesPending(invitedCount, formats.count(invitedCount))}</dd>
                </>
              ) : null}
            </dl>
            {peers.length > 0 ? (
              <div className="peer-strip" role="group" aria-label={s.roomHeaderPeerConnections}>
                {peers.map((p) => (
                  <PeerChip key={p.endpoint_id} peer={p} />
                ))}
              </div>
            ) : (
              <p className="muted room-info-empty">{s.roomHeaderNoPeersConnected}</p>
            )}
            <div className="appbar-info-actions">
              <button type="button" className="btn" onClick={onShareFile}>
                <span aria-hidden="true">{Glyph.file}</span> {s.roomHeaderShareFile}
              </button>
              <button type="button" className="btn" onClick={onOpenPipe}>
                <span aria-hidden="true">{Glyph.pipe}</span> {s.roomHeaderOpenPipe}
              </button>
            </div>
          </div>
        ) : null}
      </header>
    );
  }

  return (
    <header className="room-header">
      <div className="room-header-top">
        <div className="room-title">
          <h1>{name}</h1>
          <div className="room-subtitle">
            {membersLoaded ? (
              <span>{s.commonMemberCount(memberCount, formats.count(memberCount))}</span>
            ) : (
              // The roster has not answered. The room's total member_count is a
              // different fact and cannot stand in for it under this label.
              <span className="muted">{s.roomLoadingMembers}</span>
            )}
            {agentCount > 0 ? (
              <>
                <span className="sep">{Punct.metaSep}</span>
                <span>{s.roomHeaderAgentCount(agentCount, formats.count(agentCount))}</span>
              </>
            ) : null}
            {invitedCount > 0 ? (
              <>
                <span className="sep">{Punct.metaSep}</span>
                <span className="pending-invites">
                  {s.roomHeaderInvitesPending(invitedCount, formats.count(invitedCount))}
                </span>
              </>
            ) : null}
            <span className="sep">{Punct.metaSep}</span>
            {p2pBadge}
          </div>
        </div>
        <div className="room-actions">
          <button type="button" className="btn" onClick={onShareFile}>
            <span aria-hidden="true">{Glyph.file}</span> {s.roomHeaderShareFile}
          </button>
          <button type="button" className="btn" onClick={onOpenPipe}>
            <span aria-hidden="true">{Glyph.pipe}</span> {s.roomHeaderOpenPipe}
          </button>
          <button type="button" className="btn btn-primary" onClick={onInvite}>
            <span aria-hidden="true">{Glyph.create}</span> {s.roomHeaderInvite}
          </button>
        </div>
      </div>
      {peers.length > 0 ? (
        <div className="peer-strip" role="group" aria-label={s.roomHeaderPeerConnections}>
          {peers.map((p) => (
            <PeerChip key={p.endpoint_id} peer={p} />
          ))}
        </div>
      ) : null}
    </header>
  );
}
