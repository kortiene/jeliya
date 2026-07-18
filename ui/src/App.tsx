import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type {
  Client,
  ConnectionState,
  DaemonErrorShape,
  DaemonStatus,
  FileEntry,
  Member,
  PeerStatus,
  PipeEntry,
  RoomSummary,
  TimelineEvent,
} from './lib/protocol';
import { daemonToken, uploadFileToRoom } from './lib/client';
import { errorShape } from './lib/protocol';
import { buildDiagnostics } from './lib/diagnostics';
import type { DiagnosticEvent } from './lib/diagnostics';
import { loadAliases, saveAliases, suggestedNames } from './lib/names';
import { shortId } from './lib/format';
import { roomDisplayName } from './lib/rooms';
import { loadRoomFlags, saveRoomFlags, togglePinned, toggleArchived } from './lib/roomFlags';
import type { RoomFlags } from './lib/roomFlags';
import { loadLastSeen, markRoomSeen, saveLastSeen, seedRoomSeen } from './lib/lastSeen';
import type { LastSeen } from './lib/lastSeen';
import type { LifecycleFilter } from './lib/roomList';
import type { ActivityCategory } from './lib/timelineRuns';
import { splitInvite } from './lib/invite';
import { joinRoomWithRetry } from './lib/join';
import type { JoinProgress } from './lib/join';
import { useRoute } from './lib/history';
import { inspectorDest, legacyTabDest, routeItem, routeRoomId } from './lib/routes';
import type { InspectorDest, RoomDest } from './lib/routes';
import { useShell } from './lib/shell';
import { documentTitle, pageRegion } from './lib/landmarks';
import type { PageRegion } from './lib/landmarks';
import type { Catalog } from './l10n/catalog';
import { roomDestLabel } from './l10n/destinations';
import { useFormats, useStrings } from './l10n/strings';
import type { Formats } from './l10n/formats';
import { Template } from './l10n/template';
import { BRAND, Command, Example, ISSUE_URL } from './l10n/tokens';
import uiPackage from '../package.json';
import { NamesContext } from './components/names';
import type { NameApi } from './components/names';
import { ErrorNote, Modal, TreeMark, Wordmark } from './components/ui';
import type { FetchState } from './components/ui';
import { Onboarding } from './components/Onboarding';
import { Sidebar } from './components/Sidebar';
import type { NavKey } from './components/Sidebar';
import { MobileTabBar } from './components/MobileTabBar';
import { RoomHeader } from './components/RoomHeader';
import { RoomNav } from './components/RoomNav';
import { Timeline } from './components/Timeline';
import type { PendingMessage, TimelineView } from './components/Timeline';
import { Composer } from './components/Composer';
import { RightPanel } from './components/RightPanel';
import type { PipeConnState } from './components/RightPanel';
import { InviteModal } from './components/InviteModal';
import { FleetDashboard } from './components/FleetDashboard';
import { SettingsPanel } from './components/SettingsPanel';

type Phase = 'boot' | 'no-identity' | 'no-rooms' | 'ready';

const LAST_ROOM_KEY = 'jeliya.lastRoom';

/** The DOM id of each pane that can be the page, so the skip link can target
 *  whichever one currently is (lib/landmarks.ts). */
const PAGE_REGION_IDS: Record<PageRegion, string> = {
  sidebar: 'rooms-rail',
  center: 'workspace',
  inspector: 'room-inspector',
  fleet: 'fleet-main',
  settings: 'settings-main',
};

const COMPOSER_ID = 'composer-input';
// i18n-exempt: literal combined-invite wire syntax rendered inside localized copy.
const COMBINED_INVITE_SYNTAX = 'ticket#address';

/** Skip links — the first two tab stops on every page.
 *
 *  Visually hidden until focused, then they become real, visible controls
 *  (`.skip-link:focus-visible` in styles.css). They target the CURRENT page
 *  region rather than a fixed id, because which pane is the page changes with
 *  the route and the shell.
 *
 *  An in-page anchor alone only scrolls — a pane is not a control, so it cannot
 *  take focus. `skipTo` gives the target a temporary programmatic tab stop so
 *  focus genuinely MOVES, which is what makes the next Tab continue from the
 *  destination instead of from the rail the user just skipped.
 */
function SkipLinks({ workspaceId, composerId }: { workspaceId: string; composerId: string | null }) {
  const s = useStrings();
  const skipTo = (id: string) => (e: React.MouseEvent<HTMLAnchorElement>) => {
    e.preventDefault();
    const el = document.getElementById(id);
    if (!el) return;
    // The pane is not a control, so it needs a programmatic tab stop to receive
    // focus. Removed again on blur so it never becomes a stray Tab stop of its
    // own for users who never invoke the skip link.
    const hadTabIndex = el.hasAttribute('tabindex');
    if (!hadTabIndex) el.setAttribute('tabindex', '-1');
    el.focus();
    if (hadTabIndex) return;
    // Focus can legitimately fail to land — a disabled composer while the
    // daemon is disconnected is the real case. Clean up NOW rather than arming
    // a blur listener that will never fire, which would strand `tabindex="-1"`
    // on the control and drop it out of the natural Tab order for the rest of
    // the session.
    if (document.activeElement !== el) {
      el.removeAttribute('tabindex');
      return;
    }
    el.addEventListener('blur', () => el.removeAttribute('tabindex'), { once: true });
  };
  return (
    <div className="skip-links">
      <a className="skip-link" href={`#${workspaceId}`} onClick={skipTo(workspaceId)}>
        {s.shellSkipToMain}
      </a>
      {composerId ? (
        <a className="skip-link" href={`#${composerId}`} onClick={skipTo(composerId)}>
          {s.shellSkipToComposer}
        </a>
      ) : null}
    </div>
  );
}

type DiagnosticErrorRecorder = (context: string, error: unknown) => DaemonErrorShape;

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

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
}

function localFileUrl(roomId: string, fileId: string): string {
  const params = new URLSearchParams({ room_id: roomId, file_id: fileId });
  // The daemon token-gates /api/files/*; by the time a file link renders, the
  // WS client has fetched the session token (links come from protocol data).
  const token = daemonToken();
  if (token) params.set('token', token);
  return `/api/files/local?${params.toString()}`;
}

function persistedFetchState(roomId: string, file: FileEntry): FetchState | null {
  if (!file.fetched || !file.local_path) return null;
  return {
    phase: 'fetched',
    path: file.local_path,
    bytes: file.local_bytes ?? file.size,
    url: localFileUrl(roomId, file.file_id),
  };
}

function mergeFetchedFiles(
  previous: Record<string, FetchState>,
  roomId: string,
  files: FileEntry[],
): Record<string, FetchState> {
  let next = previous;
  for (const file of files) {
    const persisted = persistedFetchState(roomId, file);
    if (!persisted) continue;
    const current = next[file.file_id];
    if (current?.phase === 'pending' || current?.phase === 'verified') continue;
    if (next === previous) next = { ...previous };
    next[file.file_id] = persisted;
  }
  return next;
}

export default function App({ client }: { client: Client }) {
  const s = useStrings();
  const [conn, setConn] = useState<ConnectionState>(client.getState());
  const [phase, setPhase] = useState<Phase>('boot');
  const [bootNonce, setBootNonce] = useState(0);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [rooms, setRooms] = useState<RoomSummary[]>([]);

  const [roomId, setRoomId] = useState<string | null>(null);
  const roomIdRef = useRef<string | null>(null);
  // The room we have actually opened, distinct from the one the route selects:
  // a route can name a room that has not synced into `rooms` yet. See the
  // route → session effect.
  const openedRef = useRef<string | null>(null);
  const [members, setMembers] = useState<Member[]>([]);
  const [timeline, setTimeline] = useState<TimelineEvent[]>([]);
  const timelineRef = useRef<TimelineEvent[]>([]);
  const pendingSeq = useRef(0);
  const [pendingMessages, setPendingMessages] = useState<Record<string, PendingMessage[]>>({});
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [pipes, setPipes] = useState<PipeEntry[]>([]);
  const [peers, setPeers] = useState<PeerStatus[]>([]);
  const [endpointAddr, setEndpointAddr] = useState<string | null>(null);
  const [roomError, setRoomError] = useState<DaemonErrorShape | null>(null);
  const [roomLoading, setRoomLoading] = useState(false);
  const [lastDiagnosticError, setLastDiagnosticError] = useState<DiagnosticEvent | null>(null);
  const [diagnosticsCopied, setDiagnosticsCopied] = useState(false);

  const [fetches, setFetches] = useState<Record<string, FetchState>>({});
  const [pipeConns, setPipeConns] = useState<Record<string, PipeConnState>>({});
  const [closingPipes, setClosingPipes] = useState<Set<string>>(new Set());

  // The route is the navigation state (docs/room-workbench.md, decision 2).
  // This replaced three fields — `tab`, `mobileView`, and a `roomId` the
  // bootstrap picked independently — that could, and did, contradict each
  // other. Everything below derives from `route`; nothing mirrors it.
  const [route, navigate] = useRoute();
  const shell = useShell();
  // The selected file/pipe is route state now (#67): `routeItem(route)`, wired
  // into the inspector below. It replaced an ephemeral `pipeFocus` useState that
  // a deep link or a reload could not restore.
  // Deliberate per-room reading positions (see TimelineView); a ref because
  // saving one on room switch must not re-render the outgoing timeline.
  const timelineViews = useRef(new Map<string, TimelineView>());
  const [inviteOpen, setInviteOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [joinOpen, setJoinOpen] = useState(false);
  const [leaveOpen, setLeaveOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [aliases, setAliases] = useState<Record<string, string>>(loadAliases);

  // Room-list view state (issue #64). Query and filter are session-local — they
  // survive entering a room and returning, but need not outlive a reload. Pin/
  // archive and the unread last-seen marks are device-local storage
  // (roomFlags.ts, lastSeen.ts) and persist.
  const [roomQuery, setRoomQuery] = useState('');
  const [roomFilter, setRoomFilter] = useState<LifecycleFilter>('all');
  const [roomFlags, setRoomFlags] = useState<RoomFlags>(loadRoomFlags);
  const [lastSeen, setLastSeen] = useState<LastSeen>(loadLastSeen);

  // Timeline activity-filter view state (issue #65). Session-local like
  // roomQuery/roomFilter: it survives entering a room and returning (and the
  // compact display:none hide) but is per-session — it does not persist across
  // reload, and is never written to the signed log.
  const [activityFilters, setActivityFilters] = useState<ReadonlySet<ActivityCategory>>(() => new Set());
  const toggleActivityFilter = useCallback((category: ActivityCategory) => {
    setActivityFilters((prev) => {
      const next = new Set(prev);
      if (next.has(category)) next.delete(category);
      else next.add(category);
      return next;
    });
  }, []);

  const toggleRoomPin = useCallback((roomId: string) => {
    setRoomFlags((f) => {
      const next = togglePinned(f, roomId);
      saveRoomFlags(next);
      return next;
    });
  }, []);
  const toggleRoomArchive = useCallback((roomId: string) => {
    setRoomFlags((f) => {
      const next = toggleArchived(f, roomId);
      saveRoomFlags(next);
      return next;
    });
  }, []);

  // Persist the unread marks whenever they change. Kept out of the state
  // updaters (which stay pure): a save side-effect inside a functional update
  // can be dropped when a rapid navigation races the commit, leaving the dot
  // stuck. One effect mirrors the committed state to storage.
  useEffect(() => {
    saveLastSeen(lastSeen);
  }, [lastSeen]);

  // Seed the device-local unread baseline the first time each listed room
  // appears on this device (docs/room-attention.md, decision 3): a backlog
  // synced before you ever saw the room must not read as unread. seedRoomSeen
  // writes only when no mark exists, so a returning user's advanced marks — the
  // whole basis of an honest unread dot — are untouched.
  useEffect(() => {
    setLastSeen((prev) => {
      let next = prev;
      for (const room of rooms) {
        if (room.last_event_ts != null) next = seedRoomSeen(next, room.room_id, room.last_event_ts);
      }
      return next;
    });
  }, [rooms]);

  const rememberError = useCallback<DiagnosticErrorRecorder>((context, e) => {
    const err = errorShape(e);
    setLastDiagnosticError({
      context,
      code: err.code,
      message: err.message,
      hint: err.hint,
      at: new Date().toISOString(),
    });
    return err;
  }, []);

  // -- data refresh helpers (stable: only touch client + setters) -------------

  const refreshFiles = useCallback(
    async (rid: string) => {
      try {
        const { files } = await client.call('file.list', { room_id: rid });
        if (roomIdRef.current === rid) {
          setFiles(files);
          setFetches((current) => mergeFetchedFiles(current, rid, files));
        }
      } catch {
        /* transient — next push retries */
      }
    },
    [client],
  );

  const refreshPipes = useCallback(
    async (rid: string) => {
      try {
        const { pipes } = await client.call('pipe.list', { room_id: rid });
        if (roomIdRef.current === rid) setPipes(pipes);
      } catch {
        /* transient */
      }
    },
    [client],
  );

  const refreshMembers = useCallback(
    async (rid: string) => {
      try {
        const { members } = await client.call('room.members', { room_id: rid });
        if (roomIdRef.current === rid) setMembers(members);
      } catch {
        /* transient */
      }
    },
    [client],
  );

  const refreshRooms = useCallback(async () => {
    try {
      const { rooms } = await client.call('room.list', {});
      setRooms(rooms);
    } catch {
      /* transient */
    }
  }, [client]);

  /** Everything that belongs to one room's open session. Cleared whenever the
   *  route moves off a room, and before opening the next one — a stale
   *  endpoint address or member list under a different room is a lie. */
  const resetRoomState = useCallback(() => {
    setMembers([]);
    setTimeline([]);
    setFiles([]);
    setPipes([]);
    setPeers([]);
    setEndpointAddr(null);
    setRoomError(null);
    setRoomLoading(false);
    setFetches({});
    setPipeConns({});
    setClosingPipes(new Set());
  }, []);

  const openRoom = useCallback(
    async (rid: string) => {
      roomIdRef.current = rid;
      setRoomId(rid);
      try {
        localStorage.setItem(LAST_ROOM_KEY, rid);
      } catch {
        /* ignore */
      }
      setRoomError(null);
      setRoomLoading(true);
      setMembers([]);
      setTimeline([]);
      setFiles([]);
      setPipes([]);
      setPeers([]);
      // Clear the previous room's dialable address: it belongs to that room's
      // node session (a distinct ephemeral port), so leaving it live would show
      // a stale, wrong address in the Invite modal if this open fails.
      setEndpointAddr(null);
      setFetches({});
      setPipeConns({});
      try {
        const opened = await client.call('room.open', { room_id: rid });
        // The daemon's open flag changed, and room.list is global state — not
        // this room's session. Refresh it before the guard below drops
        // everything that belongs to a room the user has already left, or the
        // rail goes on calling a live session "Closed" until something else
        // happens to refresh it.
        void refreshRooms();
        if (roomIdRef.current !== rid) return;
        setMembers(opened.members);
        setTimeline(opened.timeline);
        // Viewing a room clears its unread: advance the last-seen mark to the
        // newest event now loaded (docs/room-attention.md, decision 3).
        // markRoomSeen never moves the mark backward.
        const newestTs = opened.timeline.reduce((max, e) => (e.ts > max ? e.ts : max), 0);
        if (newestTs > 0) {
          setLastSeen((prev) => markRoomSeen(prev, rid, newestTs));
        }
        setPendingMessages((byRoom) => {
          const list = byRoom[rid];
          if (!list?.some((message) => message.eventId)) return byRoom;
          const eventIds = new Set(opened.timeline.map((event) => event.event_id));
          const nextList = list.filter((message) => !message.eventId || !eventIds.has(message.eventId));
          const next = { ...byRoom };
          if (nextList.length > 0) next[rid] = nextList;
          else delete next[rid];
          return next;
        });
        setRoomLoading(false);
        setEndpointAddr(opened.endpoint.addr ?? null);
        const [f, p, ps] = await Promise.all([
          client.call('file.list', { room_id: rid }),
          client.call('pipe.list', { room_id: rid }),
          client.call('peers.status', { room_id: rid }),
        ]);
        if (roomIdRef.current !== rid) return;
        setFiles(f.files);
        setFetches((current) => mergeFetchedFiles(current, rid, f.files));
        setPipes(p.pipes);
        setPeers(ps.peers);
      } catch (e) {
        if (roomIdRef.current === rid) {
          setRoomError(rememberError('room.open', e));
          setRoomLoading(false);
        }
      }
    },
    [client, refreshRooms, rememberError],
  );

  useEffect(() => {
    timelineRef.current = timeline;
  }, [timeline]);

  // -- lifecycle: connect once, subscribe pushes -------------------------------

  useEffect(() => {
    client.start();
    const offState = client.onState(setConn);
    const offEvent = client.on('room.event', ({ room_id, event }) => {
      if (room_id !== roomIdRef.current) return;
      // Insert by timestamp, not arrival order: a peer that reconnects after a
      // gap has its backlog validated late, so a `room.event` can carry an older
      // `ts` than events already shown. Appending blindly would render it out of
      // order and emit a stray day divider. `event_id` still dedupes.
      setTimeline((tl) => {
        if (tl.some((e) => e.event_id === event.event_id)) return tl;
        let i = tl.length;
        while (i > 0 && tl[i - 1].ts > event.ts) i -= 1;
        const next = tl.slice();
        next.splice(i, 0, event);
        return next;
      });
      // You are viewing this room (guarded above), so a new event here is seen,
      // not unread — advance the mark as it arrives (docs/room-attention.md,
      // decision 3).
      setLastSeen((prev) => markRoomSeen(prev, room_id, event.ts));
      if (event.kind === 'message') {
        setPendingMessages((byRoom) => {
          const list = byRoom[room_id];
          if (!list?.some((message) => message.eventId === event.event_id)) return byRoom;
          const nextList = list.filter((message) => message.eventId !== event.event_id);
          const next = { ...byRoom };
          if (nextList.length > 0) next[room_id] = nextList;
          else delete next[room_id];
          return next;
        });
      }
      if (event.kind === 'file_shared') void refreshFiles(room_id);
      if (event.kind === 'pipe_opened' || event.kind === 'pipe_closed') void refreshPipes(room_id);
      if (event.kind === 'member_joined' || event.kind === 'member_invited' || event.kind === 'member_left') {
        void refreshMembers(room_id);
        void refreshRooms();
      }
    });
    const offPeers = client.on('peers.changed', ({ room_id, peers }) => {
      if (room_id === roomIdRef.current) setPeers(peers);
    });
    return () => {
      offState();
      offEvent();
      offPeers();
      client.stop();
    };
  }, [client, refreshFiles, refreshPipes, refreshMembers, refreshRooms]);

  // -- bootstrap: runs on every (re)connect and after onboarding steps ---------
  //
  // It no longer picks a room. The route does (decision 2: an explicit route
  // always wins over a restored room) — a bootstrap that re-picked
  // `lastRoom` on every reconnect fought the URL, and dragged the user back
  // into the very room they had just escaped (issue #88).

  useEffect(() => {
    if (conn !== 'connected') return;
    let cancelled = false;
    void (async () => {
      try {
        const st = await client.call('daemon.status', {});
        if (cancelled) return;
        setStatus(st);
        if (!st.identity) {
          setPhase('no-identity');
          return;
        }
        const { rooms } = await client.call('room.list', {});
        if (cancelled) return;
        setRooms(rooms);
        setPhase(rooms.length === 0 ? 'no-rooms' : 'ready');
      } catch {
        // daemon.status failed (connection dropped mid-flight) — the
        // reconnect cycle will re-trigger this effect.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [conn, bootNonce, client]);

  // -- route -> session -------------------------------------------------------

  // Restore the last room, at most once, and only when the URL expressed no
  // opinion. `/` is no opinion; `/rooms` is the Rooms destination and is
  // honored as one — which is what keeps an explicit "Back to Rooms" from
  // being undone by the next reconnect.
  const initialPath = useRef(window.location.pathname);
  const restored = useRef(false);
  useEffect(() => {
    if (phase !== 'ready' || restored.current) return;
    restored.current = true;
    if (initialPath.current !== '/') return;
    const activeRooms = rooms.filter((r) => r.status !== 'left' && r.status !== 'removed');
    if (activeRooms.length === 0) return;
    let saved: string | null = null;
    try {
      saved = localStorage.getItem(LAST_ROOM_KEY);
    } catch {
      /* ignore */
    }
    const target = (saved && activeRooms.some((r) => r.room_id === saved) && saved) || activeRooms[0].room_id;
    // A legacy `?tab=` link names a destination but no room; honor it on the
    // room we restored, and let the redirect strip the key.
    const dest = legacyTabDest(window.location.search) ?? 'activity';
    // Two steps on purpose. `/` is replaced (the user never stood on it), but
    // the restored room is *pushed* on top of Rooms — so Back leaves the room
    // for the rooms list instead of leaving Jeliya. Restoring straight into
    // the room with a replace would make the first Back press exit the app.
    navigate({ kind: 'rooms' }, { replace: true });
    navigate({ kind: 'room', roomId: target, dest });
  }, [phase, rooms, navigate]);

  // The open room follows the route, and this is the only place that opens
  // one: a rail click, a fleet card, and a pasted URL all just navigate.
  //
  // `openedRef` tracks the room we have actually called room.open for, which
  // is NOT the same as the room the route selects. A deep link (or a
  // create/join whose refreshRooms was swallowed by a reconnect) can name a
  // room that room.list has not returned yet; we select it — the render shows
  // the recoverable "not on this device" state — but do not open it. When the
  // room later syncs into `rooms`, this effect reruns and must retry the open.
  // Guarding on `roomIdRef` alone would skip that retry (the id already
  // matches) and strand the user on an unopened room.
  useEffect(() => {
    if (phase !== 'ready') return;
    const rid = routeRoomId(route);
    if (rid !== roomIdRef.current) {
      roomIdRef.current = rid;
      setRoomId(rid);
      resetRoomState();
      openedRef.current = null;
    }
    if (rid === null) return;
    // `phase === 'ready'` means room.list has answered, so an id that is not
    // in it is genuinely not on this device (yet) — a recoverable state the
    // render resolves, not a reason to call room.open and surface a daemon
    // error. If it appears later, the rerun opens it.
    const room = rooms.find((r) => r.room_id === rid);
    if (!room || room.status === 'left' || room.status === 'removed') return;
    if (openedRef.current === rid) return;
    openedRef.current = rid;
    void openRoom(rid);
  }, [phase, route, rooms, openRoom, resetRoomState]);

  // -- names api ----------------------------------------------------------------

  const selfId = status?.identity?.identity_id ?? null;
  const names = useMemo<NameApi>(
    () => ({
      // Self resolves to its device-local label, falling back to the friendly
      // "You" — never the raw hex id (docs/self-label.md). Peers keep the
      // alias → mock-suggestion → short-id order.
      display: (id: string) =>
        selfId !== null && id === selfId
          ? aliases[id] ?? s.identitySelf
          : aliases[id] ?? suggestedNames[id] ?? shortId(id),
      isSelf: (id: string) => selfId !== null && id === selfId,
      requestRename: (id: string) => setRenameTarget(id),
    }),
    [aliases, selfId, s.identitySelf],
  );

  // The self label is just the self identity's alias; editing it from
  // onboarding/settings reuses the same local-alias write (no wire call).
  const selfLabel = selfId !== null ? aliases[selfId] ?? '' : '';
  const setSelfLabel = (label: string) => {
    if (selfId !== null) saveRename(selfId, label);
  };

  const saveRename = (id: string, name: string) => {
    setAliases((prev) => {
      const next = { ...prev };
      if (name.trim()) next[id] = name.trim();
      else delete next[id];
      saveAliases(next);
      return next;
    });
    setRenameTarget(null);
  };

  // -- actions --------------------------------------------------------------------

  const updatePendingForRoom = (rid: string, updater: (messages: PendingMessage[]) => PendingMessage[]) => {
    setPendingMessages((byRoom) => {
      const nextList = updater(byRoom[rid] ?? []);
      const next = { ...byRoom };
      if (nextList.length > 0) next[rid] = nextList;
      else delete next[rid];
      return next;
    });
  };

  const sendMessage = async (body: string, retryClientId?: string) => {
    if (!roomIdRef.current) return;
    const rid = roomIdRef.current;
    const clientId = retryClientId ?? `pending-${Date.now()}-${pendingSeq.current++}`;
    const ts = Date.now();

    if (retryClientId) {
      updatePendingForRoom(rid, (messages) =>
        messages.map((message) =>
          message.clientId === clientId ? { ...message, phase: 'sending', ts, error: undefined } : message,
        ),
      );
    } else {
      updatePendingForRoom(rid, (messages) => [
        ...messages,
        {
          clientId,
          body,
          ts,
          phase: 'sending',
        },
      ]);
    }

    try {
      const { event_id } = await client.call('message.send', { room_id: rid, body });
      const alreadyVisible = timelineRef.current.some((event) => event.event_id === event_id);
      updatePendingForRoom(rid, (messages) =>
        alreadyVisible
          ? messages.filter((message) => message.clientId !== clientId)
          : messages.map((message) =>
              message.clientId === clientId ? { ...message, phase: 'syncing', eventId: event_id } : message,
            ),
      );
    } catch (e) {
      const err = rememberError('message.send', e);
      updatePendingForRoom(rid, (messages) =>
        messages.map((message) =>
          message.clientId === clientId ? { ...message, phase: 'failed', error: err } : message,
        ),
      );
    }
  };

  const retryPendingMessage = (clientId: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    const pending = (pendingMessages[rid] ?? []).find((message) => message.clientId === clientId);
    if (!pending) return;
    void sendMessage(pending.body, clientId);
  };

  const recheckFiles = useCallback(() => {
    const rid = roomIdRef.current;
    if (!rid) return;
    void refreshFiles(rid);
  }, [refreshFiles]);

  const fetchFile = (fileId: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    const file = files.find((f) => f.file_id === fileId);
    if (selfId !== null && file?.sender_id === selfId) {
      setFetches((m) => {
        if (!m[fileId]) return m;
        const next = { ...m };
        delete next[fileId];
        return next;
      });
      return;
    }
    setFetches((m) => ({ ...m, [fileId]: { phase: 'pending' } }));
    void (async () => {
      try {
        const result = await client.call('file.fetch', { room_id: rid, file_id: fileId });
        if (roomIdRef.current !== rid) return;
        setFetches((m) => ({
          ...m,
          [fileId]: {
            phase: 'verified',
            path: result.path,
            bytes: result.bytes,
            url: localFileUrl(rid, fileId),
          },
        }));
      } catch (e) {
        if (roomIdRef.current !== rid) return;
        setFetches((m) => ({ ...m, [fileId]: { phase: 'error', error: rememberError('file.fetch', e) } }));
      }
      void refreshFiles(rid);
    })();
  };

  const shareFilePath = async (path: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    try {
      await client.call('file.share', { room_id: rid, path });
      void refreshFiles(rid);
    } catch (e) {
      rememberError('file.share', e);
      throw e;
    }
  };

  const shareBrowserFile = async (file: File) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    try {
      if (import.meta.env.VITE_MOCK === '1') {
        await client.call('file.share', {
          room_id: rid,
          path: file.name || 'upload.bin',
          name: file.name || 'upload.bin',
          mime: file.type || undefined,
        });
      } else {
        await uploadFileToRoom(rid, file);
      }
      void refreshFiles(rid);
    } catch (e) {
      rememberError('file.share', e);
      throw e;
    }
  };

  const pipeConnect = (pipeId: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'connecting' } }));
    void (async () => {
      try {
        const { local_addr } = await client.call('pipe.connect', { room_id: rid, pipe_id: pipeId });
        if (roomIdRef.current !== rid) return;
        setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'connected', local_addr } }));
        void refreshPipes(rid);
      } catch (e) {
        if (roomIdRef.current !== rid) return;
        setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'error', error: rememberError('pipe.connect', e) } }));
      }
    })();
  };

  const pipeClose = (pipeId: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    setClosingPipes((s) => new Set(s).add(pipeId));
    const doneClosing = () =>
      setClosingPipes((s) => {
        const next = new Set(s);
        next.delete(pipeId);
        return next;
      });
    void (async () => {
      try {
        await client.call('pipe.close', { room_id: rid, pipe_id: pipeId });
        doneClosing();
        setPipeConns((m) => {
          const next = { ...m };
          delete next[pipeId];
          return next;
        });
        void refreshPipes(rid);
      } catch (e) {
        doneClosing();
        setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'error', error: rememberError('pipe.close', e) } }));
      }
    })();
  };

  const pipeExpose = async (target: string, peerIdentity: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    try {
      await client.call('pipe.expose', { room_id: rid, target, peer_identity: peerIdentity });
      void refreshPipes(rid);
    } catch (e) {
      rememberError('pipe.expose', e);
      throw e;
    }
  };

  /** Forget the restored room. Both escapes below use it so that the next
   *  load of `/` does not drop the user back into the room they just left —
   *  the route effect handles clearing the session state itself. */
  const forgetLastRoom = () => {
    try {
      localStorage.removeItem(LAST_ROOM_KEY);
    } catch {
      /* ignore */
    }
  };

  // The escape hatch from a room whose open failed: navigating away is all it
  // takes now — nothing can render under a room the route no longer names.
  const backToRooms = () => {
    forgetLastRoom();
    navigate({ kind: 'rooms' });
  };

  const leaveCurrentRoom = () => {
    forgetLastRoom();
    navigate({ kind: 'rooms' });
    void refreshRooms();
    void client.call('daemon.status', {}).then(setStatus).catch(() => {
      /* transient */
    });
  };

  /** Room-row and fleet-card taps. Selecting a room is a navigation, not a
   *  state write. */
  const selectRoom = (rid: string) => {
    const room = rooms.find((r) => r.room_id === rid);
    if (room?.status === 'left' || room?.status === 'removed') {
      navigate({ kind: 'rooms' });
      return;
    }
    // Re-selecting the room whose open failed is the user's most instinctive
    // retry. The route is already this room, so navigating is a no-op —
    // honor the intent instead of ignoring the click.
    if (rid === roomIdRef.current && roomError) {
      void openRoom(rid);
      return;
    }
    navigate({ kind: 'room', roomId: rid, dest: 'activity' });
  };

  const currentRoom = rooms.find((r) => r.room_id === roomId) ?? null;

  const makeDiagnostics = useCallback(
    () =>
      buildDiagnostics({
        generatedAt: new Date().toISOString(),
        uiVersion: uiPackage.version,
        browser: navigator.userAgent,
        platform: navigator.platform || 'unknown',
        transport: client.describe(),
        connection: conn,
        status,
        rooms,
        currentRoomId: roomId,
        members,
        files,
        fetches,
        pipes,
        peers,
        lastError: lastDiagnosticError,
      }),
    [client, conn, files, fetches, lastDiagnosticError, members, peers, pipes, roomId, rooms, status],
  );

  const copyDiagnostics = useCallback(async () => {
    await copyText(makeDiagnostics());
    setDiagnosticsCopied(true);
    window.setTimeout(() => setDiagnosticsCopied(false), 1600);
  }, [makeDiagnostics]);

  const reportIssue = useCallback(() => {
    void copyDiagnostics();
    const params = new URLSearchParams({ title: s.settingsIssueReportTitle });
    window.open(`${ISSUE_URL}?${params.toString()}`, '_blank', 'noopener,noreferrer');
  }, [copyDiagnostics, s.settingsIssueReportTitle]);

  // -- derived navigation (all of it, from the route) ---------------------------

  // A room route highlights Rooms: the workbench is somewhere you stand
  // *inside* Rooms, not a fourth global destination.
  const activeNav: NavKey = route.kind === 'fleet' ? 'fleet' : route.kind === 'settings' ? 'settings' : 'rooms';

  /** Which single surface the compact shell shows. Derived — so the bar, the
   *  pane, and the inspector's own tab strip cannot disagree, because there is
   *  nothing left for them to disagree *with*. */
  // A room route to a room room.list does not have — a deep link that hasn't
  // synced, or a room this identity left. Its recovery surface (below) lives
  // in `.center`, so the pane must stay `room` even when the route names a
  // tool: an `inspector` pane would hide `.center` on compact and strand the
  // user in an empty tool with no visible way out — and let file/pipe actions
  // fire against a room that isn't open.
  const roomUnavailable =
    roomId !== null && (!currentRoom || currentRoom.status === 'left' || currentRoom.status === 'removed');

  const inspector: InspectorDest | null = roomUnavailable ? null : inspectorDest(route);

  const pane: 'rooms' | 'room' | 'inspector' | 'fleet' | 'settings' =
    route.kind === 'rooms'
      ? 'rooms'
      : route.kind === 'fleet'
        ? 'fleet'
        : route.kind === 'settings'
          ? 'settings'
          : inspector
            ? 'inspector'
            : 'room';
  const roomDest: RoomDest = route.kind === 'room' ? route.dest : 'activity';
  const inspectorLabel = inspector ? roomDestLabel(s, inspector) : null;

  // Which single pane is the page — the `main` landmark and the skip link's
  // target (lib/landmarks.ts). The shell keeps every pane mounted and hides the
  // inactive ones, so the landmark role has to move with the route instead of
  // living permanently on the workspace.
  const region = pageRegion(pane, shell);

  // Announce the destination. The SPA never changed `document.title`, so every
  // navigation left the same static "Jeliya" for a screen reader to read.
  useEffect(() => {
    document.title = documentTitle(pane, {
      roomName: currentRoom?.name ?? null,
      destLabel: inspectorLabel,
      labels: {
        rooms: s.destRooms,
        fleet: s.destFleet,
        settings: s.destSettings,
        app: BRAND,
      },
    });
  }, [pane, currentRoom?.name, inspectorLabel, s.destRooms, s.destFleet, s.destSettings]);

  /** Tab counts — facts the daemon has answered with, so they are only shown
   *  once it has. */
  const roomNavCounts: Partial<Record<RoomDest, number>> = {
    people: members.length,
    agents: members.filter((m) => m.role === 'agent').length,
    files: files.length,
    pipes: pipes.filter((p) => p.state === 'open').length,
  };

  /** Navigate to a room tool. On compact this pushes a pane over the room; on
   *  medium it opens the drawer; on wide it opens the column. One route, three
   *  mechanics — and Back undoes it on all three. */
  const openRoomDest = useCallback(
    (dest: InspectorDest, itemId?: string) => {
      const rid = roomIdRef.current;
      if (!rid) return;
      // The selected file/pipe is part of the destination now (#67): a timeline
      // "Open in Files/Pipes" deep-links to the item, and a bare tab click drops
      // it. One navigation carries both which tool and which item.
      navigate({ kind: 'room', roomId: rid, dest, item: itemId });
    },
    [navigate],
  );

  /** Select (or, with null, deselect) a file/pipe within the tool the route is
   *  already on. Selection is a deep link — `/rooms/:id/files/:fileId` — so it
   *  survives a reload and a paste, and the inspector reads it off the route. */
  const selectRoomItem = useCallback(
    (itemId: string | null) => {
      const rid = roomIdRef.current;
      if (!rid) return;
      const dest = inspectorDest(route);
      if (dest !== 'files' && dest !== 'pipes') return;
      navigate({ kind: 'room', roomId: rid, dest, item: itemId ?? undefined });
    },
    [navigate, route],
  );

  /** Closing the inspector *is* navigating to Activity (decision 3). */
  const closeInspector = useCallback(() => {
    const rid = roomIdRef.current;
    if (!rid) return;
    navigate({ kind: 'room', roomId: rid, dest: 'activity' });
  }, [navigate]);

  /** The room nav's single handler: Activity closes the inspector, a tool
   *  opens it, and both are the same navigation. */
  const goToDest = useCallback(
    (dest: RoomDest) => {
      if (dest === 'activity') closeInspector();
      else openRoomDest(dest);
    },
    [closeInspector, openRoomDest],
  );

  const navToGlobal = useCallback(
    (key: NavKey) => {
      navigate(key === 'fleet' ? { kind: 'fleet' } : key === 'settings' ? { kind: 'settings' } : { kind: 'rooms' });
    },
    [navigate],
  );

  // -- render -----------------------------------------------------------------------

  if (phase === 'boot') {
    return (
      // The first screen every user sees is a page like any other: a landmark,
      // an h1, and — unlike before — a live region, so the connection state it
      // reports is announced when it changes instead of swapping silently.
      <main className="boot-screen" id="boot-main">
        <TreeMark size={48} />
        <Wordmark as="h1" />
        <p className="muted" role="status" aria-live="polite">
          {conn === 'connected' ? s.bootSyncing : conn === 'disconnected' ? s.bootNotConnected : s.bootContacting}
        </p>
        <p className="boot-target mono">{client.describe()}</p>
        {conn === 'reconnecting' ? (
          <p className="muted">
            <Template
              template={s.bootRetryingHint}
              slots={{
                daemon: <code>{Command.daemon}</code>,
                port: <code>{Command.daemonPortParam}</code>,
              }}
            />
          </p>
        ) : null}
      </main>
    );
  }

  if (phase === 'no-identity' || phase === 'no-rooms') {
    return (
      <NamesContext.Provider value={names}>
        <Onboarding
          step={phase === 'no-identity' ? 'identity' : 'rooms'}
          client={client}
          identityId={status?.identity?.identity_id ?? null}
          selfLabel={selfLabel}
          onSetSelfLabel={setSelfLabel}
          onAdvance={() => setBootNonce((n) => n + 1)}
        />
      </NamesContext.Provider>
    );
  }

  return (
    <NamesContext.Provider value={names}>
      <div
        className={`app pane-${pane}${route.kind === 'fleet' ? ' app-fleet' : ''}${
          route.kind === 'settings' ? ' app-settings' : ''
        }${inspector ? ' inspector-open' : ''}`}
      >
        {/* Reaching the workspace used to mean tabbing the whole room rail, and
            reaching the composer meant tabbing the whole timeline. Both skips
            are real anchors, not scroll tricks: they move focus, so the next Tab
            continues from the destination. The composer link is only offered
            where a composer exists — a link to nothing is worse than no link. */}
        {/* The composer link follows the WORKSPACE, not the pane: opening a
            room tool makes `pane` 'inspector' on every shell, but only compact
            actually hides the workspace. On medium and wide the composer is
            still on screen and usable beside the tool, so the link belongs
            there too. `region === 'center'` is exactly "the workspace is the
            page", which is the condition that matters. */}
        <SkipLinks
          workspaceId={PAGE_REGION_IDS[region]}
          composerId={region === 'center' && currentRoom && !roomError && conn === 'connected' ? COMPOSER_ID : null}
        />

        {/* Reserved, not overlaid (decision 3): the banner is a grid row above
            every pane, so it can never cover Back, a header, or list content.
            One live region, announced once — `aria-live="polite"` on a node
            that stays mounted, rather than a node that appears and re-announces
            its whole content on every reconnect attempt. */}
        <div className="conn-region" role="status" aria-live="polite">
          {conn !== 'connected' ? (
            <div className={`conn-banner conn-${conn}`}>
              {conn === 'reconnecting' || conn === 'connecting'
                ? s.shellConnectionLost(client.describe())
                : s.shellDisconnected}
            </div>
          ) : null}
        </div>

        <Sidebar
          rooms={rooms}
          currentRoomId={roomId}
          status={status}
          conn={conn}
          activeNav={activeNav}
          onNav={navToGlobal}
          onSelectRoom={selectRoom}
          onCreateRoom={() => setCreateOpen(true)}
          onJoinRoom={() => setJoinOpen(true)}
          query={roomQuery}
          onQueryChange={setRoomQuery}
          filter={roomFilter}
          onFilterChange={setRoomFilter}
          flags={roomFlags}
          onTogglePin={toggleRoomPin}
          onToggleArchive={toggleRoomArchive}
          lastSeen={lastSeen}
          isPage={region === 'sidebar'}
        />

        <main className="center" id="workspace">
          {roomId && !currentRoom ? (
            // The route names a room room.list does not have. Not an error
            // page and not a blank panel — say which fact is true, and offer
            // the way out (decision 2).
            <div className="room-error-surface">
              {/* This surface IS the destination — no RoomHeader renders above
                  it — so its title is the page's h1 (issue #72). */}
              <h1 className="room-gone-title">{s.roomNotOnDevice}</h1>
              <p className="muted">
                <Template
                  template={s.roomNotOnDeviceDetail}
                  slots={{ id: <code className="mono">{shortId(roomId)}</code> }}
                />
              </p>
              <div className="room-error-actions">
                <button type="button" className="btn btn-primary" onClick={backToRooms}>
                  {s.roomBackToRooms}
                </button>
                <button type="button" className="btn btn-ghost" onClick={() => setJoinOpen(true)}>
                  {s.roomsJoinWithTicket}
                </button>
              </div>
            </div>
          ) : currentRoom && (currentRoom.status === 'left' || currentRoom.status === 'removed') ? (
            // A signed fact, so state it as one and do not open the room.
            <div className="room-error-surface">
              <h1 className="room-gone-title">
                {currentRoom.status === 'left' ? s.roomsYouLeft : s.roomsYouWereRemoved}
              </h1>
              <p className="muted">
                {currentRoom.status === 'left' ? s.roomLeftDetail : s.roomRemovedDetail}
              </p>
              <div className="room-error-actions">
                <button type="button" className="btn btn-primary" onClick={backToRooms}>
                  {s.roomBackToRooms}
                </button>
              </div>
            </div>
          ) : currentRoom ? (
            <>
              <RoomHeader
                room={currentRoom}
                name={currentRoom.name ?? s.roomsUntitled}
                members={members}
                membersLoaded={!roomLoading && !roomError}
                peers={peers}
                compact={shell === 'compact'}
                onBack={() => navigate({ kind: 'rooms' })}
                onInvite={() => setInviteOpen(true)}
                onShareFile={() => openRoomDest('files')}
                onOpenPipe={() => openRoomDest('pipes')}
              />
              {/* The room's nested navigation, under its header. Exactly one
                  strip is ever live: the workspace carries it on wide (where
                  the inspector is a column beside it) and whenever the
                  inspector is closed; when the inspector is open on compact or
                  medium it carries its own, and this one stands down. Two live
                  strips would duplicate the `room-tab-*` ids, and the roving-
                  tabindex keyboard handler's getElementById would move focus
                  into the hidden copy. */}
              {shell === 'wide' || !inspector ? (
                // Only claim to control the inspector's panel when one is
                // actually mounted — on Activity the workspace IS the content
                // and there is no tabpanel to point at.
                <RoomNav
                  dest={roomDest}
                  counts={roomNavCounts}
                  onDest={goToDest}
                  controlsId={inspector ? 'panel-body' : undefined}
                />
              ) : null}
              {roomError ? (
                // The open failed: the error owns the pane. Rendering the
                // empty timeline ("No events yet") and a live composer under
                // it would be a comforting lie about a room that isn't open.
                <div className="room-error-surface">
                  <ErrorNote error={roomError} />
                  <div className="room-error-actions">
                    <button
                      type="button"
                      className="btn btn-primary"
                      onClick={() => {
                        if (roomIdRef.current) void openRoom(roomIdRef.current);
                      }}
                    >
                      {s.commonRetry}
                    </button>
                    <button type="button" className="btn btn-ghost" onClick={backToRooms}>
                      {s.roomBackToRooms}
                    </button>
                  </div>
                </div>
              ) : (
                <>
                  {/* Keyed by room so the role="log" live region resets on room switch
                      instead of announcing the whole backlog as new content. */}
                  <Timeline
                    key={roomId}
                    events={timeline}
                    pendingMessages={roomId ? (pendingMessages[roomId] ?? []) : []}
                    files={files}
                    fetches={fetches}
                    loading={roomLoading}
                    selfId={selfId}
                    activityFilters={activityFilters}
                    onToggleActivityFilter={toggleActivityFilter}
                    savedView={roomId ? (timelineViews.current.get(roomId) ?? null) : null}
                    onSaveView={(view) => {
                      if (!roomId) return;
                      if (view) timelineViews.current.set(roomId, view);
                      else timelineViews.current.delete(roomId);
                    }}
                    onFetch={fetchFile}
                    onRecheckFiles={recheckFiles}
                    onRetryPendingMessage={retryPendingMessage}
                    onShowFiles={(fileId) => openRoomDest('files', fileId)}
                    onShowPipes={(pipeId) => openRoomDest('pipes', pipeId)}
                  />
                  <Composer
                    roomId={currentRoom.room_id}
                    roomName={currentRoom.name ?? s.roomsUntitled}
                    disabled={conn !== 'connected'}
                    compact={shell === 'compact'}
                    onSend={sendMessage}
                    onShareFile={shareBrowserFile}
                  />
                </>
              )}
            </>
          ) : (
            // No room selected: the room tools are unavailable and there is no
            // composer — the surface just names the one recoverable next step,
            // and the rooms list beside it is how you take it (#67). The
            // heading is the destination's `h1`: `/rooms` is the app's most
            // reached page and had no page title at all (issue #72).
            <div className="center-empty">
              <h1 className="center-empty-title">{s.destRooms}</h1>
              <p className="muted">{s.roomsChoose}</p>
            </div>
          )}
        </main>

        {/* The inspector is mounted only when the route opens it, and on
            `activity` it is closed. It is a view of the route, so its tab
            strip navigates rather than setting any state of its own. */}
        {inspector ? (
        <RightPanel
          tab={inspector}
          onDest={goToDest}
          counts={roomNavCounts}
          onClose={closeInspector}
          shell={shell}
          // Falls back like every other room-name call site: a joined room
          // whose genesis event has not synced has a null name, and on compact
          // this heading IS the page's h1 — omitting it would leave the
          // destination with no heading and an unnamed main.
          roomName={currentRoom ? (currentRoom.name ?? s.roomsUntitled) : null}
          members={members}
          timeline={timeline}
          files={files}
          pipes={pipes}
          selfId={selfId}
          fetches={fetches}
          selectedItem={routeItem(route)}
          onSelectItem={selectRoomItem}
          onFetch={fetchFile}
          onRecheckFiles={recheckFiles}
          onSharePath={shareFilePath}
          onShareBrowserFile={shareBrowserFile}
          pipeConns={pipeConns}
          closingPipes={closingPipes}
          onPipeConnect={pipeConnect}
          onPipeClose={pipeClose}
          onPipeExpose={pipeExpose}
          onLeaveRoom={() => setLeaveOpen(true)}
          isPage={region === 'inspector'}
        />
        ) : null}

        {route.kind === 'fleet' ? (
          <FleetDashboard client={client} rooms={rooms} onOpenRoom={selectRoom} />
        ) : null}

        <SettingsPanel
          status={status}
          conn={conn}
          selfLabel={selfLabel}
          onSetSelfLabel={setSelfLabel}
          diagnosticsCopied={diagnosticsCopied}
          lastDiagnosticError={lastDiagnosticError}
          onCopyDiagnostics={() => void copyDiagnostics()}
          onReportIssue={reportIssue}
          onCreateRoom={() => setCreateOpen(true)}
        />

        {/* Inside a room the bar gives way to the room's own app bar — the
            behavior mockups/mobile-triptych.png shows, and what buys the
            timeline its height back on a 320x568 phone. Back returns to it. */}
        {pane === 'room' || pane === 'inspector' ? null : <MobileTabBar active={activeNav} onNav={navToGlobal} />}

        {inviteOpen && roomId ? (
          <InviteModal
            client={client}
            roomId={roomId}
            members={members}
            endpointAddr={endpointAddr}
            connected={conn === 'connected'}
            onClose={() => setInviteOpen(false)}
          />
        ) : null}

        {createOpen ? (
          <CreateRoomModal
            client={client}
            connected={conn === 'connected'}
            rooms={rooms}
            onDiagnosticError={rememberError}
            onClose={() => setCreateOpen(false)}
            onCreated={(rid) => {
              setCreateOpen(false);
              // Navigate; do not open. Opening here would write the room behind
              // the route's back, and the route effect would then snap the
              // session back to whichever room the URL still named.
              //
              // Refresh first, and await it: the route effect resolves a room
              // against `rooms`, so naming one that room.list has not returned
              // yet would flash "that room isn't on this device" at the user
              // for the room they just made.
              void refreshRooms().then(() => navigate({ kind: 'room', roomId: rid, dest: 'activity' }));
            }}
          />
        ) : null}

        {joinOpen ? (
          <JoinRoomModal
            client={client}
            connected={conn === 'connected'}
            onDiagnosticError={rememberError}
            onClose={() => setJoinOpen(false)}
            onJoined={(rid) => {
              setJoinOpen(false);
              // See onCreated: refresh, then let the route open the room.
              void refreshRooms().then(() => navigate({ kind: 'room', roomId: rid, dest: 'activity' }));
            }}
          />
        ) : null}

        {leaveOpen && roomId && currentRoom ? (
          <LeaveRoomModal
            client={client}
            connected={conn === 'connected'}
            roomId={roomId}
            roomName={currentRoom.name ?? s.roomsUntitled}
            onDiagnosticError={rememberError}
            onClose={() => setLeaveOpen(false)}
            onLeft={() => {
              setLeaveOpen(false);
              leaveCurrentRoom();
            }}
          />
        ) : null}

        {renameTarget ? (
          <RenameModal
            id={renameTarget}
            current={aliases[renameTarget] ?? suggestedNames[renameTarget] ?? ''}
            onSave={saveRename}
            onClose={() => setRenameTarget(null)}
          />
        ) : null}
      </div>
    </NamesContext.Provider>
  );
}

// -- small modals -------------------------------------------------------------------

function JoinRoomModal({
  client,
  connected,
  onDiagnosticError,
  onClose,
  onJoined,
}: {
  client: Client;
  /** Submits are gated on a live daemon connection: a request queued while
   *  disconnected would keep the dialog busy — and undismissable — for as
   *  long as the reconnect takes. In-flight requests are safe either way
   *  (the client rejects them with connection_lost when the socket drops). */
  connected: boolean;
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onJoined(roomId: string): void;
}) {
  const s = useStrings();
  const formats = useFormats();
  const [ticket, setTicket] = useState('');
  const [peerAddr, setPeerAddr] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const [progress, setProgress] = useState<JoinProgress | null>(null);

  const join = async () => {
    if (!ticket.trim() || busy || !connected) return;
    setBusy(true);
    setError(null);
    setProgress(null);
    try {
      const { ticket: t, peerAddr: addr } = splitInvite(ticket, peerAddr);
      const { room_id } = await joinRoomWithRetry(client, {
        ticket: t,
        ...(addr ? { peers: [addr] } : {}),
      }, setProgress);
      onJoined(room_id);
    } catch (e) {
      setError(onDiagnosticError('room.join', e));
      setProgress(null);
      setBusy(false);
    }
  };

  return (
    <Modal title={s.roomsJoinWithTicket} onClose={onClose} busy={busy}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void join();
        }}
      >
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
            autoFocus
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
        <button type="submit" className="btn btn-primary" disabled={busy || !ticket.trim() || !connected}>
          {busy ? s.modalJoining : connected ? s.modalJoinSubmit : s.commonReconnecting}
        </button>
        {progress ? (
          <div className="join-progress" role="status">
            <span className="spinner" aria-hidden="true" />
            <span>{joinProgressMessage(s, formats, progress)}</span>
            <em>
              {s.modalJoinAttempt(
                progress.attempt,
                progress.maxAttempts,
                formats.count(progress.attempt),
                formats.count(progress.maxAttempts),
              )}
            </em>
          </div>
        ) : null}
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}

function CreateRoomModal({
  client,
  connected,
  rooms,
  onDiagnosticError,
  onClose,
  onCreated,
}: {
  client: Client;
  /** See JoinRoomModal.connected. */
  connected: boolean;
  /** The local room list, to warn — never block — on a name that already
   *  exists on this device (docs/room-workbench.md, decision 6). */
  rooms: RoomSummary[];
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onCreated(roomId: string): void;
}) {
  const s = useStrings();
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  // A name is a label, not an identity: the product does not own the user's
  // vocabulary, so a local collision warns and proceeds. Folded the same way
  // homonym detection folds (trim + case), so the warning and the resulting
  // short-id disambiguator agree on what "already exists" means.
  const typed = name.trim().toLowerCase();
  const collides =
    typed.length > 0 &&
    rooms.some((r) => roomDisplayName(r, s.roomsUntitled).trim().toLowerCase() === typed);

  const create = async () => {
    if (!name.trim() || busy || !connected) return;
    setBusy(true);
    setError(null);
    try {
      const { room_id } = await client.call('room.create', { name: name.trim() });
      onCreated(room_id);
    } catch (e) {
      setError(onDiagnosticError('room.create', e));
      setBusy(false);
    }
  };

  return (
    <Modal title={s.modalCreateTitle} onClose={onClose} busy={busy}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <label className="field">
          <span>{s.modalRoomNameLabel}</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={s.modalRoomNamePlaceholder}
            autoFocus
          />
        </label>
        {/* Non-blocking: role="status" (not "alert"), and the button stays
            enabled. A duplicate name is allowed — the new room gets its own id. */}
        {collides ? (
          <p className="inline-warning" role="status">
            {s.modalCreateHomonymWarning}
          </p>
        ) : null}
        <button type="submit" className="btn btn-primary" disabled={busy || !name.trim() || !connected}>
          {busy ? s.modalCreating : connected ? s.roomsCreate : s.commonReconnecting}
        </button>
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}

function LeaveRoomModal({
  client,
  connected,
  roomId,
  roomName,
  onDiagnosticError,
  onClose,
  onLeft,
}: {
  client: Client;
  /** See JoinRoomModal.connected. */
  connected: boolean;
  roomId: string;
  roomName: string;
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onLeft(): void;
}) {
  const s = useStrings();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const leave = async () => {
    if (busy || !connected) return;
    setBusy(true);
    setError(null);
    try {
      await client.call('room.leave', { room_id: roomId });
      onLeft();
    } catch (e) {
      setError(onDiagnosticError('room.leave', e));
      setBusy(false);
    }
  };

  return (
    <Modal title={s.modalLeaveTitle} onClose={onClose} busy={busy}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void leave();
        }}
      >
        <p className="muted">
          {/* The full sentence stays in one catalog message so translators can
              move the emphasized name and mono id independently. */}
          <Template
            template={s.modalLeaveCopy}
            slots={{
              room: <strong>{roomName}</strong>,
              id: <code className="room-disambig mono">{shortId(roomId)}</code>,
            }}
          />
        </p>
        <div className="field-row">
          <button type="submit" className="btn btn-danger" disabled={busy || !connected}>
            {busy ? s.modalLeaving : connected ? s.modalLeaveSubmit : s.commonReconnecting}
          </button>
          {/* Initial focus lands on Cancel, never the destructive action —
              Enter right after opening must not publish a departure. */}
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy} autoFocus>
            {s.commonCancel}
          </button>
        </div>
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}

function RenameModal({
  id,
  current,
  onSave,
  onClose,
}: {
  id: string;
  current: string;
  onSave(id: string, name: string): void;
  onClose(): void;
}) {
  const s = useStrings();
  const [name, setName] = useState(current);
  return (
    <Modal title={s.modalRenameTitle} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSave(id, name);
        }}
      >
        <p className="muted">{s.modalRenameCopy}</p>
        <p className="muted">
          <span>{s.modalRenameIdentityLabel}</span>
          <br />
          <code className="mono rename-id">{id}</code>
        </p>
        <label className="field">
          <span>{s.modalRenameAliasLabel}</span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={s.modalRenameAliasPlaceholder}
            autoFocus
          />
        </label>
        <div className="field-row">
          <button type="submit" className="btn btn-primary">
            {s.commonSave}
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => onSave(id, '')}>
            {s.modalRenameClearAlias}
          </button>
        </div>
      </form>
    </Modal>
  );
}
