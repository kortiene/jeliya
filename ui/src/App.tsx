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
import { loadAliases, saveAliases, suggestedNames } from './lib/names';
import { shortId } from './lib/format';
import { splitInvite } from './lib/invite';
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
import { Composer } from './components/Composer';
import { RightPanel } from './components/RightPanel';
import type { PanelTab, PipeConnState } from './components/RightPanel';
import { InviteModal } from './components/InviteModal';
import { FleetDashboard } from './components/FleetDashboard';

type Phase = 'boot' | 'no-identity' | 'no-rooms' | 'ready';

/** Which single pane is shown on a narrow (mobile) viewport; ignored on desktop
 *  where all three columns are visible at once. */
type MobileView = 'rooms' | 'chat' | 'agents' | 'pipes' | 'files' | 'settings';

const LAST_ROOM_KEY = 'jeliya.lastRoom';

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
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [pipes, setPipes] = useState<PipeEntry[]>([]);
  const [peers, setPeers] = useState<PeerStatus[]>([]);
  const [endpointAddr, setEndpointAddr] = useState<string | null>(null);
  const [roomError, setRoomError] = useState<DaemonErrorShape | null>(null);
  const [roomLoading, setRoomLoading] = useState(false);

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
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [aliases, setAliases] = useState<Record<string, string>>(loadAliases);

  // -- data refresh helpers (stable: only touch client + setters) -------------

  const refreshFiles = useCallback(
    async (rid: string) => {
      try {
        const { files } = await client.call('file.list', { room_id: rid });
        if (roomIdRef.current === rid) setFiles(files);
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
        setRoomLoading(false);
        setEndpointAddr(opened.endpoint.addr ?? null);
        const [f, p, ps] = await Promise.all([
          client.call('file.list', { room_id: rid }),
          client.call('pipe.list', { room_id: rid }),
          client.call('peers.status', { room_id: rid }),
        ]);
        if (roomIdRef.current !== rid) return;
        setFiles(f.files);
        setPipes(p.pipes);
        setPeers(ps.peers);
        void refreshRooms(); // open flag changed
      } catch (e) {
        if (roomIdRef.current === rid) {
          setRoomError(errorShape(e));
          setRoomLoading(false);
        }
      }
    },
    [client, refreshRooms],
  );

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
      if (event.kind === 'file_shared') void refreshFiles(room_id);
      if (event.kind === 'pipe_opened' || event.kind === 'pipe_closed') void refreshPipes(room_id);
      if (event.kind === 'member_joined' || event.kind === 'member_invited') {
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
        let saved: string | null = null;
        try {
          saved = localStorage.getItem(LAST_ROOM_KEY);
        } catch {
          /* ignore */
        }
        const target =
          (roomIdRef.current && rooms.some((r) => r.room_id === roomIdRef.current) && roomIdRef.current) ||
          (saved && rooms.some((r) => r.room_id === saved) && saved) ||
          rooms[0].room_id;
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

  const sendMessage = async (body: string) => {
    if (!roomIdRef.current) return;
    await client.call('message.send', { room_id: roomIdRef.current, body });
    // The event itself arrives via the room.event push (exactly once).
  };

  const fetchFile = (fileId: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    setFetches((m) => ({ ...m, [fileId]: { phase: 'pending' } }));
    void (async () => {
      try {
        const result = await client.call('file.fetch', { room_id: rid, file_id: fileId });
        if (roomIdRef.current !== rid) return;
        setFetches((m) => ({ ...m, [fileId]: { phase: 'verified', path: result.path, bytes: result.bytes } }));
      } catch (e) {
        if (roomIdRef.current !== rid) return;
        setFetches((m) => ({ ...m, [fileId]: { phase: 'error', error: errorShape(e) } }));
      }
      void refreshFiles(rid);
    })();
  };

  const shareFilePath = async (path: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    await client.call('file.share', { room_id: rid, path });
    void refreshFiles(rid);
  };

  const shareBrowserFile = async (file: File) => {
    const rid = roomIdRef.current;
    if (!rid) return;
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
        setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'error', error: errorShape(e) } }));
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
        setPipeConns((m) => ({ ...m, [pipeId]: { phase: 'error', error: errorShape(e) } }));
      }
    })();
  };

  const pipeExpose = async (target: string, peerIdentity: string) => {
    const rid = roomIdRef.current;
    if (!rid) return;
    await client.call('pipe.expose', { room_id: rid, target, peer_identity: peerIdentity });
    void refreshPipes(rid);
  };

  const currentRoom = rooms.find((r) => r.room_id === roomId) ?? null;

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
                files={files}
                fetches={fetches}
                loading={roomLoading}
                onFetch={fetchFile}
                onShowPipes={() => setTab('pipes')}
              />
              <Composer roomName={currentRoom.name} disabled={conn !== 'connected'} onSend={sendMessage} />
            </>
          ) : (
            <div className="center-empty muted">Select a room</div>
          )}
        </main>

        <RightPanel
          tab={tab}
          onTab={setTab}
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
        />

        {activeNav === 'agents' ? (
          <FleetDashboard
            client={client}
            rooms={rooms}
            onOpenRoom={(rid) => {
              setMobileView('chat');
              if (rid !== roomId) void openRoom(rid);
            }}
          />
        ) : null}

        <section className="mobile-settings" aria-label="Settings">
          <h2 className="mobile-settings-title">Settings</h2>
          <div className="settings-card">
            <span className="settings-label">P2P Identity</span>
            <code className="mono settings-val">{status?.identity?.identity_id ?? '—'}</code>
          </div>
          <div className="settings-card">
            <span className="settings-label">Endpoint</span>
            <code className="mono settings-val">{status?.endpoint?.endpoint_id ?? '—'}</code>
          </div>
          <div className="settings-card">
            <span className="settings-label">Daemon</span>
            <span className="settings-val">
              {status?.mode ?? '—'} · {conn}
            </span>
          </div>
          <button type="button" className="btn btn-primary" onClick={() => setCreateOpen(true)}>
            Create a room
          </button>
        </section>

        <MobileTabBar active={activeNav} onNav={navigate} />

        {inviteOpen && roomId ? (
          <InviteModal client={client} roomId={roomId} endpointAddr={endpointAddr} onClose={() => setInviteOpen(false)} />
        ) : null}

        {createOpen ? (
          <CreateRoomModal
            client={client}
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
            onClose={() => setJoinOpen(false)}
            onJoined={(rid) => {
              setJoinOpen(false);
              void refreshRooms();
              void openRoom(rid);
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
  onClose,
  onJoined,
}: {
  client: Client;
  onClose(): void;
  onJoined(roomId: string): void;
}) {
  const [ticket, setTicket] = useState('');
  const [peerAddr, setPeerAddr] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const join = async () => {
    if (!ticket.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const { ticket: t, peerAddr: addr } = splitInvite(ticket, peerAddr);
      const { room_id } = await client.call('room.join', {
        ticket: t,
        ...(addr ? { peers: [addr] } : {}),
      });
      onJoined(room_id);
    } catch (e) {
      setError(errorShape(e));
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
        <ErrorNote error={error} />
      </form>
    </Modal>
  );
}

function CreateRoomModal({
  client,
  onClose,
  onCreated,
}: {
  client: Client;
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
      setError(errorShape(e));
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
