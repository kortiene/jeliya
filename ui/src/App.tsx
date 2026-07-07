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
import { uploadFileToRoom } from './lib/client';
import { errorShape } from './lib/protocol';
import { buildDiagnostics } from './lib/diagnostics';
import type { DiagnosticEvent } from './lib/diagnostics';
import { loadAliases, saveAliases, suggestedNames } from './lib/names';
import { shortId } from './lib/format';
import { splitInvite } from './lib/invite';
import { joinRoomWithRetry } from './lib/join';
import type { JoinProgress } from './lib/join';
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
import { Timeline } from './components/Timeline';
import type { PendingMessage } from './components/Timeline';
import { Composer } from './components/Composer';
import { RightPanel } from './components/RightPanel';
import type { PanelTab, PipeConnState } from './components/RightPanel';
import { InviteModal } from './components/InviteModal';
import { FleetDashboard } from './components/FleetDashboard';
import { SettingsPanel } from './components/SettingsPanel';

type Phase = 'boot' | 'no-identity' | 'no-rooms' | 'ready';

/** Which single pane is shown on a narrow (mobile) viewport; ignored on desktop
 *  where all three columns are visible at once. */
type MobileView = 'rooms' | 'chat' | 'agents' | 'pipes' | 'files' | 'settings';

const LAST_ROOM_KEY = 'jeliya.lastRoom';
const ISSUE_URL = 'https://github.com/kortiene/jeliya/issues/new';

type DiagnosticErrorRecorder = (context: string, error: unknown) => DaemonErrorShape;

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
  const [conn, setConn] = useState<ConnectionState>(client.getState());
  const [phase, setPhase] = useState<Phase>('boot');
  const [bootNonce, setBootNonce] = useState(0);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [rooms, setRooms] = useState<RoomSummary[]>([]);

  const [roomId, setRoomId] = useState<string | null>(null);
  const roomIdRef = useRef<string | null>(null);
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

  const [tab, setTab] = useState<PanelTab>(() => {
    const t = new URLSearchParams(window.location.search).get('tab');
    return t === 'members' || t === 'files' || t === 'pipes' || t === 'agents' ? t : 'members';
  });
  const [mobileView, setMobileView] = useState<MobileView>('rooms');
  const [inviteOpen, setInviteOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [joinOpen, setJoinOpen] = useState(false);
  const [leaveOpen, setLeaveOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [aliases, setAliases] = useState<Record<string, string>>(loadAliases);

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
        if (roomIdRef.current !== rid) return;
        setMembers(opened.members);
        setTimeline(opened.timeline);
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
        void refreshRooms(); // open flag changed
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
        if (rooms.length === 0) {
          setPhase('no-rooms');
          return;
        }
        setPhase('ready');
        const activeRooms = rooms.filter((r) => r.status !== 'left' && r.status !== 'removed');
        if (activeRooms.length === 0) {
          roomIdRef.current = null;
          setRoomId(null);
          return;
        }
        let saved: string | null = null;
        try {
          saved = localStorage.getItem(LAST_ROOM_KEY);
        } catch {
          /* ignore */
        }
        const target =
          (roomIdRef.current && activeRooms.some((r) => r.room_id === roomIdRef.current) && roomIdRef.current) ||
          (saved && activeRooms.some((r) => r.room_id === saved) && saved) ||
          activeRooms[0].room_id;
        void openRoom(target);
      } catch {
        // daemon.status failed (connection dropped mid-flight) — the
        // reconnect cycle will re-trigger this effect.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [conn, bootNonce, client, openRoom]);

  // -- names api ----------------------------------------------------------------

  const selfId = status?.identity?.identity_id ?? null;
  const names = useMemo<NameApi>(
    () => ({
      display: (id: string) => aliases[id] ?? suggestedNames[id] ?? shortId(id),
      isSelf: (id: string) => selfId !== null && id === selfId,
      requestRename: (id: string) => setRenameTarget(id),
    }),
    [aliases, selfId],
  );

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

  const leaveCurrentRoom = () => {
    roomIdRef.current = null;
    setRoomId(null);
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
    setMobileView('rooms');
    try {
      localStorage.removeItem(LAST_ROOM_KEY);
    } catch {
      /* ignore */
    }
    void refreshRooms();
    void client.call('daemon.status', {}).then(setStatus).catch(() => {
      /* transient */
    });
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
    const params = new URLSearchParams({ title: 'Jeliya issue report' });
    window.open(`${ISSUE_URL}?${params.toString()}`, '_blank', 'noopener,noreferrer');
  }, [copyDiagnostics]);

  // -- primary navigation (desktop left rail + mobile bottom bar) ----------------

  const activeNav: NavKey =
    mobileView === 'settings'
      ? 'settings'
      : mobileView === 'agents'
        ? 'agents'
        : mobileView === 'pipes'
          ? 'pipes'
          : mobileView === 'files'
            ? 'files'
            : mobileView === 'chat'
              ? 'home'
              : 'rooms';

  const navigate = useCallback((key: NavKey) => {
    if (key === 'agents') {
      // Top-level fleet dashboard — distinct from the in-room Agents tab, which
      // stays reachable via the right-panel tab strip.
      setMobileView('agents');
    } else if (key === 'pipes' || key === 'files') {
      setTab(key);
      setMobileView(key);
    } else if (key === 'settings') {
      setMobileView('settings');
    } else if (key === 'home') {
      setMobileView(roomIdRef.current ? 'chat' : 'rooms');
    } else if (key === 'rooms') {
      setMobileView('rooms');
    }
  }, []);

  // -- render -----------------------------------------------------------------------

  if (phase === 'boot') {
    return (
      <div className="boot-screen">
        <TreeMark size={48} />
        <Wordmark as="h1" />
        <p className="muted">
          {conn === 'connected' ? 'Syncing…' : conn === 'disconnected' ? 'Not connected.' : 'Contacting daemon…'}
        </p>
        <p className="boot-target mono">{client.describe()}</p>
        {conn === 'reconnecting' ? (
          <p className="muted">Retrying with backoff — start <code>jeliyad</code> or pass <code>?daemon=&lt;port&gt;</code>.</p>
        ) : null}
      </div>
    );
  }

  if (phase === 'no-identity' || phase === 'no-rooms') {
    return (
      <NamesContext.Provider value={names}>
        <Onboarding
          step={phase === 'no-identity' ? 'identity' : 'rooms'}
          client={client}
          identityId={status?.identity?.identity_id ?? null}
          onAdvance={() => setBootNonce((n) => n + 1)}
        />
      </NamesContext.Provider>
    );
  }

  return (
    <NamesContext.Provider value={names}>
      <div
        className={`app mv-${mobileView}${activeNav === 'agents' ? ' app-fleet' : ''}${
          activeNav === 'settings' ? ' app-settings' : ''
        }`}
      >
        {conn !== 'connected' ? (
          <div className={`conn-banner conn-${conn}`} role="status">
            {conn === 'reconnecting' || conn === 'connecting'
              ? `Connection to daemon lost — reconnecting… (${client.describe()})`
              : 'Disconnected from daemon.'}
          </div>
        ) : null}

        <Sidebar
          rooms={rooms}
          currentRoomId={roomId}
          status={status}
          conn={conn}
          activeNav={activeNav}
          onNav={navigate}
          onSelectRoom={(rid) => {
            const room = rooms.find((r) => r.room_id === rid);
            if (room?.status === 'left' || room?.status === 'removed') {
              setMobileView('rooms');
              return;
            }
            setMobileView('chat');
            if (rid !== roomId) void openRoom(rid);
          }}
          onCreateRoom={() => setCreateOpen(true)}
          onJoinRoom={() => setJoinOpen(true)}
        />

        <main className="center">
          {currentRoom ? (
            <>
              <RoomHeader
                name={currentRoom.name}
                memberCount={members.length || currentRoom.member_count}
                members={members}
                peers={peers}
                onInvite={() => setInviteOpen(true)}
                onShareFile={() => setTab('files')}
                onOpenPipe={() => setTab('pipes')}
              />
              {roomError ? (
                <div className="room-error">
                  <ErrorNote error={roomError} />
                </div>
              ) : null}
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
                onFetch={fetchFile}
                onRetryPendingMessage={retryPendingMessage}
                onShowPipes={() => setTab('pipes')}
              />
              <Composer
                roomId={currentRoom.room_id}
                roomName={currentRoom.name}
                disabled={conn !== 'connected'}
                onSend={sendMessage}
                onShareFile={shareBrowserFile}
              />
            </>
          ) : (
            <div className="center-empty muted">Select a room</div>
          )}
        </main>

        <RightPanel
          tab={tab}
          onTab={setTab}
          roomName={currentRoom?.name ?? null}
          members={members}
          timeline={timeline}
          files={files}
          pipes={pipes}
          selfId={selfId}
          fetches={fetches}
          onFetch={fetchFile}
          onSharePath={shareFilePath}
          onShareBrowserFile={shareBrowserFile}
          pipeConns={pipeConns}
          closingPipes={closingPipes}
          onPipeConnect={pipeConnect}
          onPipeClose={pipeClose}
          onPipeExpose={pipeExpose}
          onLeaveRoom={() => setLeaveOpen(true)}
        />

        {activeNav === 'agents' ? (
          <FleetDashboard
            client={client}
            rooms={rooms}
            onOpenRoom={(rid) => {
              const room = rooms.find((r) => r.room_id === rid);
              if (room?.status === 'left' || room?.status === 'removed') return;
              setMobileView('chat');
              if (rid !== roomId) void openRoom(rid);
            }}
          />
        ) : null}

        <SettingsPanel
          status={status}
          conn={conn}
          diagnosticsCopied={diagnosticsCopied}
          lastDiagnosticError={lastDiagnosticError}
          onCopyDiagnostics={() => void copyDiagnostics()}
          onReportIssue={reportIssue}
          onCreateRoom={() => setCreateOpen(true)}
        />

        <MobileTabBar active={activeNav} onNav={navigate} />

        {inviteOpen && roomId ? (
          <InviteModal client={client} roomId={roomId} endpointAddr={endpointAddr} onClose={() => setInviteOpen(false)} />
        ) : null}

        {createOpen ? (
          <CreateRoomModal
            client={client}
            onDiagnosticError={rememberError}
            onClose={() => setCreateOpen(false)}
            onCreated={(rid) => {
              setCreateOpen(false);
              void refreshRooms();
              void openRoom(rid);
            }}
          />
        ) : null}

        {joinOpen ? (
          <JoinRoomModal
            client={client}
            onDiagnosticError={rememberError}
            onClose={() => setJoinOpen(false)}
            onJoined={(rid) => {
              setJoinOpen(false);
              void refreshRooms();
              void openRoom(rid);
            }}
          />
        ) : null}

        {leaveOpen && roomId && currentRoom ? (
          <LeaveRoomModal
            client={client}
            roomId={roomId}
            roomName={currentRoom.name}
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
  onDiagnosticError,
  onClose,
  onJoined,
}: {
  client: Client;
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onJoined(roomId: string): void;
}) {
  const [ticket, setTicket] = useState('');
  const [peerAddr, setPeerAddr] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const [progress, setProgress] = useState<JoinProgress | null>(null);

  const join = async () => {
    if (!ticket.trim() || busy) return;
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
    <Modal title="Join with a ticket" onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void join();
        }}
      >
        <p className="muted">
          Paste the invite you received. A combined invite (<code>ticket#address</code>) fills in the peer address
          automatically.
        </p>
        <label className="field">
          <span>Ticket</span>
          <textarea
            value={ticket}
            onChange={(e) => setTicket(e.target.value)}
            placeholder="roomtkt1… or roomtkt1…#<endpoint_id>@host:port"
            rows={3}
            spellCheck={false}
            autoFocus
          />
        </label>
        <label className="field">
          <span>
            Peer address <em className="muted">(optional)</em>
          </span>
          <input
            value={peerAddr}
            onChange={(e) => setPeerAddr(e.target.value)}
            placeholder="<endpoint_id>@203.0.113.7:4242"
            spellCheck={false}
          />
        </label>
        <button type="submit" className="btn btn-primary" disabled={busy || !ticket.trim()}>
          {busy ? 'Joining…' : 'Join room'}
        </button>
        {progress ? (
          <div className="join-progress" role="status">
            <span className="spinner" aria-hidden="true" />
            <span>{progress.message}</span>
            <em>
              Attempt {progress.attempt}/{progress.maxAttempts}
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
  onDiagnosticError,
  onClose,
  onCreated,
}: {
  client: Client;
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onCreated(roomId: string): void;
}) {
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const create = async () => {
    if (!name.trim() || busy) return;
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
    <Modal title="Create a room" onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <label className="field">
          <span>Room name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Build Iroh Rooms MVP" autoFocus />
        </label>
        <button type="submit" className="btn btn-primary" disabled={busy || !name.trim()}>
          {busy ? 'Creating…' : 'Create room'}
        </button>
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}

function LeaveRoomModal({
  client,
  roomId,
  roomName,
  onDiagnosticError,
  onClose,
  onLeft,
}: {
  client: Client;
  roomId: string;
  roomName: string;
  onDiagnosticError: DiagnosticErrorRecorder;
  onClose(): void;
  onLeft(): void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const leave = async () => {
    if (busy) return;
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
    <Modal title="Leave room" onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void leave();
        }}
      >
        <p className="muted">
          Leaving <strong>{roomName}</strong> publishes a signed membership departure. This is different from closing
          the local session; you’ll need a new invite to join again.
        </p>
        <div className="field-row">
          <button type="submit" className="btn btn-danger" disabled={busy} autoFocus>
            {busy ? 'Leaving…' : 'Leave room'}
          </button>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={busy}>
            Cancel
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
  const [name, setName] = useState(current);
  return (
    <Modal title="Name this peer" onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSave(id, name);
        }}
      >
        <p className="muted">
          Local alias only — names never leave this machine. Identity:
          <br />
          <code className="mono rename-id">{id}</code>
        </p>
        <label className="field">
          <span>Alias</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Maya R." autoFocus />
        </label>
        <div className="field-row">
          <button type="submit" className="btn btn-primary">
            Save
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => onSave(id, '')}>
            Clear alias
          </button>
        </div>
      </form>
    </Modal>
  );
}
