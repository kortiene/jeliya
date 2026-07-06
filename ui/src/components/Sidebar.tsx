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

/** Primary navigation section — mirrors the left rail in desktop-room.png and
 *  the bottom tab bar in the mobile mockups. */
export type NavKey = 'home' | 'rooms' | 'agents' | 'pipes' | 'files' | 'calls' | 'settings';

const NAV: { key: NavKey; label: string; glyph: string; soon?: boolean }[] = [
  { key: 'home', label: 'Home', glyph: '⌂' },
  { key: 'rooms', label: 'Rooms', glyph: '▦' },
  { key: 'agents', label: 'Agents', glyph: '✦' },
  { key: 'pipes', label: 'Pipes', glyph: '⤳' },
  { key: 'files', label: 'Files', glyph: '▤' },
  { key: 'calls', label: 'Calls', glyph: '☎', soon: true },
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
            disabled={item.soon}
            onClick={() => onNav(item.key)}
          >
            <span className="nav-glyph" aria-hidden="true">
              {item.glyph}
            </span>
            <span className="nav-label">{item.label}</span>
            {item.soon ? <span className="nav-soon">Soon</span> : null}
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
          return (
            <button
              key={room.room_id}
              type="button"
              className={`room-item${active ? ' selected' : ''}`}
              onClick={() => onSelectRoom(room.room_id)}
            >
              <span className="room-hex" style={{ color: tint, background: `${tint}1f` }} aria-hidden="true">
                ⬡
              </span>
              <span className="room-info">
                <span className="room-name">{room.name}</span>
                <span className="room-meta">
                  {room.member_count} member{room.member_count === 1 ? '' : 's'} · {room.open ? 'Active' : 'Idle'}
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
        <span aria-hidden="true">⇥</span> Join with Ticket
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
