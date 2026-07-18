import { useState } from 'react';
import type { ConnectionState, DaemonStatus, RoomSummary } from '../lib/protocol';
import { colorForId, relTime, shortId } from '../lib/format';
import { projectRoomList, type LifecycleFilter, type RoomListRow, type RoomSectionKey } from '../lib/roomList';
import type { RoomFlags } from '../lib/roomFlags';
import { isRoomUnread, type LastSeen } from '../lib/lastSeen';
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

/** The lifecycle filter — separates Active from Left/Removed without dropping a
 *  room from existence (issue #64, docs/room-attention.md, decision 4). */
const FILTERS: { key: LifecycleFilter; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'active', label: 'Active' },
  { key: 'departed', label: 'Left & removed' },
];

/** The two collapsible put-away sections. Pinned and (unheadered) active rooms
 *  are always expanded; these two are disclosures so a long tail of departed or
 *  archived rooms never buries the rooms you actually work in. */
const COLLAPSIBLE: Record<string, boolean> = { departed: true, archived: true };

const SECTION_LABEL: Record<RoomSectionKey, string> = {
  pinned: 'Pinned',
  active: 'Active',
  departed: 'Left & removed',
  archived: 'Archived',
};

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
  query,
  onQueryChange,
  filter,
  onFilterChange,
  flags,
  onTogglePin,
  onToggleArchive,
  lastSeen,
  isPage,
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
  query: string;
  onQueryChange(q: string): void;
  filter: LifecycleFilter;
  onFilterChange(f: LifecycleFilter): void;
  flags: RoomFlags;
  onTogglePin(roomId: string): void;
  onToggleArchive(roomId: string): void;
  lastSeen: LastSeen;
  /** True when the rail IS the destination rather than a column beside it —
   *  the compact `/rooms` screen. It then carries the page's `main` landmark
   *  and its `h1`; otherwise it stays `complementary` under a named region
   *  (issue #72, `lib/landmarks.ts`). */
  isPage: boolean;
}) {
  const names = useNames();
  // Collapsed/expanded state for the two disclosure sections. Local and
  // cosmetic — a room's search/filter/pin state lives in App and survives nav;
  // whether the archive drawer is open does not need to.
  const [open, setOpen] = useState<Record<string, boolean>>({ departed: false, archived: false });

  const identityId = status?.identity?.identity_id ?? null;
  const endpointId = status?.endpoint?.endpoint_id ?? null;
  const selfName = identityId ? names.display(identityId) : 'You';
  const handle = identityId ? `@${shortId(identityId).replace(/…/g, '')}` : '@—';

  // The searched, filtered, ordered, sectioned view — and the disambiguator set
  // recomputed over exactly the rooms this render shows (roomList.ts).
  const view = projectRoomList({ rooms, query, filter, pinned: flags.pinned, archived: flags.archived });

  const clearSearch = () => {
    onQueryChange('');
    onFilterChange('all');
  };

  const renderRow = (row: RoomListRow) => {
    const room = row.room;
    const tint = colorForId(room.room_id);
    const active = room.room_id === currentRoomId;
    const departed = room.status === 'left' || room.status === 'removed';
    // One label, one fact (docs/room-workbench.md, decision 4). `status` is
    // signed membership; `open` is whether this daemon holds a live session.
    const stateLabel = departed
      ? room.status === 'left'
        ? 'Left'
        : 'Removed'
      : room.open
        ? 'Open'
        : 'Closed';
    const unread = isRoomUnread(room, lastSeen);
    const pinned = flags.pinned.has(room.room_id);
    const archived = flags.archived.has(room.room_id);
    const last = room.last_event_ts ?? null;
    return (
      <div
        key={room.room_id}
        className={`room-item${active ? ' selected' : ''}${departed ? ' departed' : ''}${unread ? ' unread' : ''}`}
      >
        <button
          type="button"
          className="room-select"
          onClick={() => onSelectRoom(room.room_id)}
          disabled={departed}
          // `page`, not the generic `true`: a room row inside `<nav>` is a
          // page navigation, matching the rail's own nav items and the compact
          // tab bar.
          aria-current={active ? 'page' : undefined}
          title={departed ? `You ${room.status === 'left' ? 'left' : 'were removed from'} this room` : undefined}
        >
          <span className="room-hex" style={{ color: tint, background: `${tint}1f` }} aria-hidden="true">
            ⬡
          </span>
          <span className="room-info">
            <span className="room-name-line">
              <span className="room-name">{row.displayName}</span>
              {unread ? (
                // Device-local unread (docs/room-attention.md, decision 3): a
                // dot, never a count, and never an implication that anyone
                // received or read anything. Carries a real label + a non-colour
                // weight cue on the row, so it is not colour alone.
                <span className="unread-dot" title="Unread">
                  <span className="visually-hidden">Unread</span>
                </span>
              ) : null}
            </span>
            <span className="room-meta">
              {room.member_count} member{room.member_count === 1 ? '' : 's'} · {stateLabel}
              {row.isHomonym ? (
                // Real text, not aria-hidden, so it lands in the row's accessible
                // name and a screen-reader user can tell the homonyms apart too.
                <>
                  {' · '}
                  <code className="room-disambig mono">{shortId(room.room_id)}</code>
                </>
              ) : null}
              {last != null ? (
                // Last activity is the newest signed event's ts — a daemon
                // projection (decision 2), rendered relative, never the wall
                // clock. Absent (older daemon / not synced) renders nothing, not
                // a fabricated recency.
                <>
                  {' · '}
                  <span className="room-last">{relTime(last)}</span>
                </>
              ) : null}
            </span>
          </span>
          {room.open ? <span className="dot dot-green" title="Session open" /> : null}
        </button>
        <div className="room-row-actions">
          <button
            type="button"
            className={`room-row-action${pinned ? ' on' : ''}`}
            aria-pressed={pinned}
            aria-label={`${pinned ? 'Unpin' : 'Pin'} ${row.displayName}`}
            title={pinned ? 'Unpin' : 'Pin'}
            onClick={() => onTogglePin(room.room_id)}
          >
            {pinned ? '★' : '☆'}
          </button>
          <button
            type="button"
            className={`room-row-action${archived ? ' on' : ''}`}
            aria-pressed={archived}
            aria-label={`${archived ? 'Restore' : 'Archive'} ${row.displayName}`}
            title={archived ? 'Restore from archive' : 'Archive'}
            onClick={() => onToggleArchive(room.room_id)}
          >
            {archived ? '⇧' : '⇩'}
          </button>
        </div>
      </div>
    );
  };

  // The rail is the whole screen on compact `/rooms` and a column beside the
  // workspace everywhere else. Same markup, two landmark roles: `main` when it
  // is the page, otherwise a NAMED `complementary` — it used to be an unnamed
  // `<aside>` sharing the page with the inspector's unnamed `<aside>`, which is
  // both an axe `landmark-unique` failure and useless to landmark navigation.
  // A plain `div` carrying an explicit role, not an `<aside>`: overriding a
  // landmark element's implicit role is an `aria-allowed-role` violation, and
  // switching the ELEMENT between shells would remount the rail and lose the
  // list scroll position that DESIGN.md requires to survive a shell change.
  // One element, one role attribute that follows the route.
  const RailHeading = isPage ? 'h1' : 'h2';
  return (
    <div
      className="sidebar"
      id="rooms-rail"
      role={isPage ? 'main' : 'complementary'}
      aria-label={isPage ? undefined : 'Room rail'}
    >
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
        <RailHeading className="rooms-title">Your Rooms</RailHeading>
        <button type="button" className="icon-btn" onClick={onCreateRoom} aria-label="Create room" title="Create room">
          +
        </button>
      </div>

      {/* Search + lifecycle filter (issue #64). Both live OUTSIDE the Rooms
          navigation below, so the filter's "Active" chip never lands in a room
          row's accessible name and the retired "Active" state label stays
          retired. */}
      <div className="rooms-controls">
        <label className="visually-hidden" htmlFor="room-search">
          Search rooms by name or short id
        </label>
        <input
          id="room-search"
          type="search"
          className="room-search"
          placeholder="Search rooms…"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
        />
        <div className="lifecycle-filter" role="group" aria-label="Filter rooms by lifecycle">
          {FILTERS.map((f) => (
            <button
              key={f.key}
              type="button"
              className={`filter-chip${filter === f.key ? ' active' : ''}`}
              aria-pressed={filter === f.key}
              onClick={() => onFilterChange(f.key)}
            >
              {f.label}
            </button>
          ))}
        </div>
      </div>

      <nav className="rooms-list" aria-label="Rooms">
        {view.sections.map((section) => {
          // A section is a collapsed disclosure only when it is a genuine
          // put-away: never when the user explicitly filtered TO it (departed
          // under the Left & removed filter), and never while a search is
          // active — a query must reveal its matches, not bury them in a
          // collapsed bucket.
          const filterFocus = section.key === 'departed' && filter === 'departed';
          const collapsible = (COLLAPSIBLE[section.key] ?? false) && !filterFocus && !view.hasQuery;
          const expanded = collapsible ? (open[section.key] ?? false) : true;
          // A "Pinned" header groups the floated rooms; active rooms need no
          // header (they are simply the list). The collapsible sections own a
          // disclosure button with their count.
          const showHeader = section.key === 'pinned';
          return (
            <div key={section.key} className={`room-section room-section-${section.key}`}>
              {collapsible ? (
                <button
                  type="button"
                  className="room-section-toggle"
                  aria-expanded={expanded}
                  onClick={() => setOpen((o) => ({ ...o, [section.key]: !expanded }))}
                >
                  <span className="disclosure" aria-hidden="true">
                    {expanded ? '▾' : '▸'}
                  </span>
                  {SECTION_LABEL[section.key]} <span className="room-section-count">({section.rows.length})</span>
                </button>
              ) : showHeader ? (
                <div className="room-section-head">{SECTION_LABEL[section.key]}</div>
              ) : null}
              {expanded ? section.rows.map(renderRow) : null}
            </div>
          );
        })}

        {view.visibleCount === 0 ? (
          view.totalCount === 0 ? (
            <div className="rooms-empty muted">No rooms yet</div>
          ) : (
            <div className="rooms-empty muted">
              {view.hasQuery ? (
                <>
                  No rooms match “<span className="mono">{query.trim()}</span>”.
                </>
              ) : (
                <>No rooms in this filter.</>
              )}{' '}
              <button type="button" className="link-btn" onClick={clearSearch}>
                Clear
              </button>
            </div>
          )
        ) : null}
      </nav>

      <button type="button" className="create-room" onClick={onCreateRoom}>
        <span aria-hidden="true">⊕</span> Create Room
      </button>
      <button type="button" className="create-room join-room" onClick={onJoinRoom}>
        <span aria-hidden="true">⇥</span> Join with a ticket
      </button>

      {/* Not a `<footer>` element: outside sectioning content it would map to
          the page's `contentinfo` landmark, which this identity strip is not —
          it belongs to the rail, and on compact it would sit inside `main`. */}
      <div className="identity-footer">
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
      </div>
    </div>
  );
}
