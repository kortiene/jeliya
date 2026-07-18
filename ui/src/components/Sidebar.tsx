import { useState } from 'react';
import type { ConnectionState, DaemonStatus, RoomSummary } from '../lib/protocol';
import { colorForId, shortId } from '../lib/format';
import { projectRoomList, type LifecycleFilter, type RoomListRow, type RoomSectionKey } from '../lib/roomList';
import type { RoomFlags } from '../lib/roomFlags';
import { isRoomUnread, type LastSeen } from '../lib/lastSeen';
import type { Catalog } from '../l10n/catalog';
import { useFormats, useStrings } from '../l10n/strings';
import { Glyph, Punct } from '../l10n/tokens';
import { CopyButton, TreeMark, Wordmark } from './ui';
import { useNames } from './names';

const CONN_LABEL = {
  connected: 'shellConnConnected',
  connecting: 'shellConnConnecting',
  reconnecting: 'shellConnReconnecting',
  disconnected: 'shellConnDisconnected',
} as const satisfies Record<ConnectionState, keyof Catalog>;

/** The global destinations — the only three (docs/room-workbench.md,
 *  decision 1). Files and Pipes left this rail because neither can answer a
 *  question without a room_id: they were always secretly about one room,
 *  chosen elsewhere, and now live in the room's own workbench. Home went
 *  because it duplicated Rooms, and Calls because a destination that only
 *  says "Soon" is a promise the product has not earned. */
export type NavKey = 'rooms' | 'fleet' | 'settings';

const NAV = [
  { key: 'rooms', labelKey: 'destRooms', glyph: Glyph.rooms },
  { key: 'fleet', labelKey: 'destFleet', glyph: Glyph.fleet },
  { key: 'settings', labelKey: 'destSettings', glyph: Glyph.settings },
] as const;

/** The lifecycle filter — separates Active from Left/Removed without dropping a
 *  room from existence (issue #64, docs/room-attention.md, decision 4). */
const FILTERS = [
  { key: 'all', labelKey: 'roomsFilterAll' },
  { key: 'active', labelKey: 'roomsFilterActive' },
  { key: 'departed', labelKey: 'roomsFilterDeparted' },
] as const satisfies readonly { key: LifecycleFilter; labelKey: keyof Catalog }[];

/** The two collapsible put-away sections. Pinned and (unheadered) active rooms
 *  are always expanded; these two are disclosures so a long tail of departed or
 *  archived rooms never buries the rooms you actually work in. */
const COLLAPSIBLE: Record<string, boolean> = { departed: true, archived: true };

const SECTION_LABEL = {
  pinned: 'roomsSectionPinned',
  active: 'roomsFilterActive',
  departed: 'roomsFilterDeparted',
  archived: 'roomsSectionArchived',
} as const satisfies Record<RoomSectionKey, keyof Catalog>;

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
  const s = useStrings();
  const formats = useFormats();
  const names = useNames();
  // Collapsed/expanded state for the two disclosure sections. Local and
  // cosmetic — a room's search/filter/pin state lives in App and survives nav;
  // whether the archive drawer is open does not need to.
  const [open, setOpen] = useState<Record<string, boolean>>({ departed: false, archived: false });

  const identityId = status?.identity?.identity_id ?? null;
  const endpointId = status?.endpoint?.endpoint_id ?? null;
  const selfName = identityId ? names.display(identityId) : s.identitySelf;
  const handle = s.roomsProfileHandle(identityId ? shortId(identityId).replace(/…/g, '') : Punct.missingValue);

  // The searched, filtered, ordered, sectioned view — and the disambiguator set
  // recomputed over exactly the rooms this render shows (roomList.ts).
  const view = projectRoomList({
    rooms,
    untitledLabel: s.roomsUntitled,
    query,
    filter,
    pinned: flags.pinned,
    archived: flags.archived,
  });

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
        ? s.roomsStateLeft
        : s.roomsStateRemoved
      : room.open
        ? s.roomsStateOpen
        : s.roomsStateClosed;
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
          title={departed ? (room.status === 'left' ? s.roomsYouLeft : s.roomsYouWereRemoved) : undefined}
        >
          <span className="room-hex" style={{ color: tint, background: `${tint}1f` }} aria-hidden="true">
            {Glyph.hex}
          </span>
          <span className="room-info">
            <span className="room-name-line">
              <span className="room-name">{row.displayName}</span>
              {unread ? (
                // Device-local unread (docs/room-attention.md, decision 3): a
                // dot, never a count, and never an implication that anyone
                // received or read anything. Carries a real label + a non-colour
                // weight cue on the row, so it is not colour alone.
                <span className="unread-dot" title={s.roomsUnread}>
                  <span className="visually-hidden">{s.roomsUnread}</span>
                </span>
              ) : null}
            </span>
            <span className="room-meta">
              {s.roomsMemberCount(room.member_count, formats.count(room.member_count))}
              {Punct.metaSep}
              {stateLabel}
              {row.isHomonym ? (
                // Real text, not aria-hidden, so it lands in the row's accessible
                // name and a screen-reader user can tell the homonyms apart too.
                <>
                  {Punct.metaSep}
                  <code className="room-disambig mono">{shortId(room.room_id)}</code>
                </>
              ) : null}
              {last != null ? (
                // Last activity is the newest signed event's ts — a daemon
                // projection (decision 2), rendered relative, never the wall
                // clock. Absent (older daemon / not synced) renders nothing, not
                // a fabricated recency.
                <>
                  {Punct.metaSep}
                  <span className="room-last">{formats.relTime(last)}</span>
                </>
              ) : null}
            </span>
          </span>
          {room.open ? <span className="dot dot-green" title={s.roomsSessionOpen} /> : null}
        </button>
        <div className="room-row-actions">
          <button
            type="button"
            className={`room-row-action${pinned ? ' on' : ''}`}
            aria-pressed={pinned}
            aria-label={pinned ? s.roomsUnpin(row.displayName) : s.roomsPin(row.displayName)}
            title={pinned ? s.roomsUnpinShort : s.roomsPinShort}
            onClick={() => onTogglePin(room.room_id)}
          >
            {pinned ? Glyph.pinOn : Glyph.pinOff}
          </button>
          <button
            type="button"
            className={`room-row-action${archived ? ' on' : ''}`}
            aria-pressed={archived}
            aria-label={archived ? s.roomsRestore(row.displayName) : s.roomsArchive(row.displayName)}
            title={archived ? s.roomsRestoreShort : s.roomsArchiveShort}
            onClick={() => onToggleArchive(room.room_id)}
          >
            {archived ? Glyph.restore : Glyph.archive}
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
      aria-label={isPage ? undefined : s.roomsRailLabel}
    >
      <div className="brand">
        <TreeMark size={30} />
        <Wordmark className="brand-name" />
      </div>

      <button type="button" className="profile-card" onClick={() => onNav('settings')} title={s.roomsProfile}>
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
            {Punct.missingValue}
          </span>
        )}
        <span className="profile-info">
          <span className="profile-name">{selfName}</span>
          <span className="profile-handle mono">{handle}</span>
        </span>
        <span className="profile-chevron" aria-hidden="true">
          {Glyph.chevronDown}
        </span>
      </button>

      <nav className="nav-list" aria-label={s.shellNavPrimary}>
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
            <span className="nav-label">{s[item.labelKey]}</span>
          </button>
        ))}
      </nav>

      <div className="rooms-head">
        <RailHeading className="rooms-title">{s.roomsYourRooms}</RailHeading>
        <button
          type="button"
          className="icon-btn"
          onClick={onCreateRoom}
          aria-label={s.roomsCreate}
          title={s.roomsCreate}
        >
          {Glyph.add}
        </button>
      </div>

      {/* Search + lifecycle filter (issue #64). Both live OUTSIDE the Rooms
          navigation below, so the filter's "Active" chip never lands in a room
          row's accessible name and the retired "Active" state label stays
          retired. */}
      <div className="rooms-controls">
        <label className="visually-hidden" htmlFor="room-search">
          {s.roomsSearchLabel}
        </label>
        <input
          id="room-search"
          type="search"
          className="room-search"
          placeholder={s.roomsSearchPlaceholder}
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
        />
        <div className="lifecycle-filter" role="group" aria-label={s.roomsFilterLegend}>
          {FILTERS.map((f) => (
            <button
              key={f.key}
              type="button"
              className={`filter-chip${filter === f.key ? ' active' : ''}`}
              aria-pressed={filter === f.key}
              onClick={() => onFilterChange(f.key)}
            >
              {s[f.labelKey]}
            </button>
          ))}
        </div>
      </div>

      <nav className="rooms-list" aria-label={s.roomsListLabel}>
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
                    {expanded ? Glyph.disclosureOpen : Glyph.disclosureClosed}
                  </span>
                  {s[SECTION_LABEL[section.key]]}{' '}
                  <span className="room-section-count">
                    {s.roomsSectionCount(section.rows.length, formats.count(section.rows.length))}
                  </span>
                </button>
              ) : showHeader ? (
                <div className="room-section-head">{s[SECTION_LABEL[section.key]]}</div>
              ) : null}
              {expanded ? section.rows.map(renderRow) : null}
            </div>
          );
        })}

        {view.visibleCount === 0 ? (
          view.totalCount === 0 ? (
            <div className="rooms-empty muted">{s.roomsEmpty}</div>
          ) : (
            <div className="rooms-empty muted">
              {view.hasQuery ? (
                s.roomsNoMatch(query.trim())
              ) : (
                s.roomsNoneInFilter
              )}{' '}
              <button type="button" className="link-btn" onClick={clearSearch}>
                {s.commonClear}
              </button>
            </div>
          )
        ) : null}
      </nav>

      <button type="button" className="create-room" onClick={onCreateRoom}>
        <span aria-hidden="true">{Glyph.create}</span> {s.roomsCreate}
      </button>
      <button type="button" className="create-room join-room" onClick={onJoinRoom}>
        <span aria-hidden="true">{Glyph.join}</span> {s.roomsJoinWithTicket}
      </button>

      {/* Not a `<footer>` element: outside sectioning content it would map to
          the page's `contentinfo` landmark, which this identity strip is not —
          it belongs to the rail, and on compact it would sit inside `main`. */}
      <div className="identity-footer">
        <span className="identity-hex" aria-hidden="true">
          <TreeMark size={22} />
        </span>
        <div className="identity-info">
          <span className="identity-label">{s.identityP2P}</span>
          <span className="identity-id mono" title={identityId ?? undefined}>
            {identityId ? shortId(identityId) : Punct.missingValue}
            {endpointId ? (
              <span className="identity-ep" title={s.identityEndpointTitle(endpointId)}>
                {Punct.metaSep}
                {s.identityEndpointShort(shortId(endpointId))}
              </span>
            ) : null}
          </span>
        </div>
        {identityId ? <CopyButton text={identityId} label={Glyph.copy} ariaLabel={s.identityCopy} /> : null}
        <span className={`conn-badge conn-${conn}`} title={s[CONN_LABEL[conn]]}>
          <span className="dot" /> {s[CONN_LABEL[conn]]}
        </span>
      </div>
    </div>
  );
}
