import type { ConnectionState, DaemonStatus, RoomSummary } from '../lib/protocol';
import { colorForId, shortId } from '../lib/format';
import { CopyButton, TreeMark, Wordmark } from './ui';
import { useNames } from './names';

const CONN_LABEL: Record<ConnectionState, string> = {
  connected: 'Connected',
  connecting: 'Connecting…',
  reconnecting: 'Reconnecting…',
  disconnected: 'Disconnected',
};

/** The global destinations — the only three (docs/room-workbench.md,
 *  decision 1). Files and Pipes left this rail because neither can answer a
 *  question without a room_id: they were always secretly about one room,
 *  chosen elsewhere, and now live in the room's own workbench. Home went
 *  because it duplicated Rooms, and Calls because a destination that only
 *  says "Soon" is a promise the product has not earned. */
export type NavKey = 'rooms' | 'fleet' | 'settings';

const NAV: { key: NavKey; label: string; glyph: string }[] = [
  { key: 'rooms', label: 'Rooms', glyph: '▦' },
  { key: 'fleet', label: 'Agent Fleet', glyph: '✦' },
  { key: 'settings', label: 'Settings', glyph: '⚙' },
];

export function Sidebar({
  rooms,
  currentRoomId,
  status,
  conn,
  activeNav,
  onNav,
  onSelectRoom,
  onCreateRoom,
  onJoinRoom,
}: {
  rooms: RoomSummary[];
  currentRoomId: string | null;
  status: DaemonStatus | null;
  conn: ConnectionState;
  activeNav: NavKey;
  onNav(key: NavKey): void;
  onSelectRoom(roomId: string): void;
  onCreateRoom(): void;
  onJoinRoom(): void;
}) {
  const names = useNames();
  const identityId = status?.identity?.identity_id ?? null;
  const endpointId = status?.endpoint?.endpoint_id ?? null;
  const selfName = identityId ? names.display(identityId) : 'You';
  const handle = identityId ? `@${shortId(identityId).replace(/…/g, '')}` : '@—';

  return (
    <aside className="sidebar">
      <div className="brand">
        <TreeMark size={30} />
        <Wordmark className="brand-name" />
      </div>

      <button type="button" className="profile-card" onClick={() => onNav('settings')} title="Profile & settings">
        {identityId ? (
          <span
            className="profile-avatar"
            style={{ color: colorForId(identityId), background: `${colorForId(identityId)}26` }}
            aria-hidden="true"
          >
            {selfName.slice(0, 2).toUpperCase()}
          </span>
        ) : (
          <span className="profile-avatar" aria-hidden="true">
            ··
          </span>
        )}
        <span className="profile-info">
          <span className="profile-name">{selfName}</span>
          <span className="profile-handle mono">{handle}</span>
        </span>
        <span className="profile-chevron" aria-hidden="true">
          ⌄
        </span>
      </button>

      <nav className="nav-list" aria-label="Primary">
        {NAV.map((item) => (
          <button
            key={item.key}
            type="button"
            className={`nav-item${activeNav === item.key ? ' active' : ''}`}
            aria-current={activeNav === item.key ? 'page' : undefined}
            onClick={() => onNav(item.key)}
          >
            <span className="nav-glyph" aria-hidden="true">
              {item.glyph}
            </span>
            <span className="nav-label">{item.label}</span>
          </button>
        ))}
      </nav>

      <div className="rooms-head">
        <span className="rooms-title">Your Rooms</span>
        <button type="button" className="icon-btn" onClick={onCreateRoom} aria-label="Create room" title="Create room">
          +
        </button>
      </div>

      <nav className="rooms-list" aria-label="Rooms">
        {rooms.map((room) => {
          const tint = colorForId(room.room_id);
          const active = room.room_id === currentRoomId;
          const departed = room.status === 'left' || room.status === 'removed';
          // One label, one fact (docs/room-workbench.md, decision 4). `status`
          // is signed membership; `open` is whether this daemon holds a live
          // session. "Active" used to mean the latter here while meaning the
          // former on the wire — so it is gone.
          const stateLabel = departed
            ? room.status === 'left'
              ? 'Left'
              : 'Removed'
            : room.open
              ? 'Open'
              : 'Closed';
          return (
            <button
              key={room.room_id}
              type="button"
              className={`room-item${active ? ' selected' : ''}${departed ? ' departed' : ''}`}
              onClick={() => onSelectRoom(room.room_id)}
              disabled={departed}
              title={departed ? `You ${room.status === 'left' ? 'left' : 'were removed from'} this room` : undefined}
            >
              <span className="room-hex" style={{ color: tint, background: `${tint}1f` }} aria-hidden="true">
                ⬡
              </span>
              <span className="room-info">
                <span className="room-name">{room.name}</span>
                <span className="room-meta">
                  {room.member_count} member{room.member_count === 1 ? '' : 's'} · {stateLabel}
                </span>
              </span>
              {room.open ? <span className="dot dot-green" title="Session open" /> : null}
            </button>
          );
        })}
        {rooms.length === 0 ? <div className="rooms-empty muted">No rooms yet</div> : null}
      </nav>

      <button type="button" className="create-room" onClick={onCreateRoom}>
        <span aria-hidden="true">⊕</span> Create Room
      </button>
      <button type="button" className="create-room join-room" onClick={onJoinRoom}>
        <span aria-hidden="true">⇥</span> Join with a ticket
      </button>

      <footer className="identity-footer">
        <span className="identity-hex" aria-hidden="true">
          <TreeMark size={22} />
        </span>
        <div className="identity-info">
          <span className="identity-label">P2P Identity</span>
          <span className="identity-id mono" title={identityId ?? undefined}>
            {identityId ? shortId(identityId) : '—'}
            {endpointId ? <span className="identity-ep" title={`endpoint ${endpointId}`}> · ep {shortId(endpointId)}</span> : null}
          </span>
        </div>
        {identityId ? <CopyButton text={identityId} label="⧉" ariaLabel="Copy identity ID" /> : null}
        <span className={`conn-badge conn-${conn}`} title={CONN_LABEL[conn]}>
          <span className="dot" /> {CONN_LABEL[conn]}
        </span>
      </footer>
    </aside>
  );
}
