import { useEffect, useId, useRef, useState } from 'react';
import type {
  Client,
  DaemonErrorShape,
  FleetAgent,
  FleetResult,
  HistoryPoint,
  Liveness,
  RoomSummary,
} from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { colorForId, labelTone, prettyLabel, relTime, shortId } from '../lib/format';
import { useNames } from './names';
import { Avatar, CopyButton, ErrorNote, Modal, ProgressBar, SenderName, TreeMark } from './ui';

// -- liveness presentation (the four §1.2 states, truthful) -------------------
//
// A `stale` agent whose last posted label was `working` is shown as STALE, never
// as active/working — peer state overrode the label upstream (docs §1.2 "THE
// RULE"), and this view must not walk that back.

const LIVENESS_LABEL: Record<Liveness, string> = {
  working: 'Working',
  'online-idle': 'Online',
  stale: 'Stale',
  offline: 'Offline',
};

/** CSS tone class + dot color per liveness. `working`/`online-idle` are live
 *  (accent), `stale` warns (amber), `offline` is dimmed and inert. */
const LIVENESS_TONE: Record<Liveness, 'live' | 'idle' | 'warn' | 'off'> = {
  working: 'live',
  'online-idle': 'idle',
  stale: 'warn',
  offline: 'off',
};

// -- sparkline (inline SVG, points-only, no interpolation) --------------------
//
// One mark per real agent_status event from agent.history. y = progress when the
// event carried one, else a band derived from the label class — never a fabricated
// intermediate point. Single series per card, so no legend: the card title names it.

function bandFor(p: HistoryPoint): number {
  if (typeof p.progress === 'number') return Math.max(0, Math.min(100, p.progress)) / 100;
  const tone = labelTone(p.label);
  // neutral (unknown label) sits low-mid: not a failure, but not an earned
  // healthy band either.
  return tone === 'red' ? 0.18 : tone === 'neutral' ? 0.45 : tone === 'blue' ? 0.62 : 0.8;
}

function Sparkline({ points, color, muted }: { points: HistoryPoint[] | null; color: string; muted: boolean }) {
  const gid = useId().replace(/[:]/g, '');
  const W = 132;
  const H = 40;
  const pad = 4;
  const stroke = muted ? 'var(--text-mute)' : color;

  // null = history not fetched yet — solid baseline placeholder, same size, so
  // the dashed "no history" treatment is never flashed before data arrives.
  if (points === null) {
    return (
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Loading status history">
        <title>Loading status history</title>
        <line x1={pad} y1={H - pad} x2={W - pad} y2={H - pad} stroke="var(--border-strong)" strokeWidth="1.5" />
      </svg>
    );
  }

  const label =
    points.length === 0
      ? 'No status history yet'
      : `${points.length} status event${points.length === 1 ? '' : 's'}`;

  if (points.length === 0) {
    return (
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label}>
        <title>{label}</title>
        <line x1={pad} y1={H - pad} x2={W - pad} y2={H - pad} stroke="var(--border-strong)" strokeWidth="1.5" strokeDasharray="2 3" />
      </svg>
    );
  }

  const tsMin = points[0].ts;
  const tsMax = points[points.length - 1].ts;
  const span = tsMax - tsMin;
  const xAt = (i: number): number => {
    if (points.length === 1) return W - pad;
    const t = span > 0 ? (points[i].ts - tsMin) / span : i / (points.length - 1);
    return pad + t * (W - 2 * pad);
  };
  const yAt = (i: number): number => H - pad - bandFor(points[i]) * (H - 2 * pad);

  const xs = points.map((_, i) => xAt(i));
  const ys = points.map((_, i) => yAt(i));
  const last = points.length - 1;

  if (points.length === 1) {
    return (
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label}>
        <title>{label}</title>
        <circle cx={xs[0]} cy={ys[0]} r={3.2} fill={stroke} />
      </svg>
    );
  }

  const line = points.map((_, i) => `${i ? 'L' : 'M'}${xs[i].toFixed(1)} ${ys[i].toFixed(1)}`).join(' ');
  const area = `${line} L${xs[last].toFixed(1)} ${H - pad} L${xs[0].toFixed(1)} ${H - pad} Z`;

  return (
    <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label}>
      <title>{label}</title>
      <defs>
        <linearGradient id={`sg-${gid}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={stroke} stopOpacity={muted ? 0.16 : 0.28} />
          <stop offset="1" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#sg-${gid})`} />
      <path d={line} fill="none" stroke={stroke} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" opacity={muted ? 0.7 : 1} />
      <circle cx={xs[last]} cy={ys[last]} r={3} fill={stroke} />
    </svg>
  );
}

// -- per-agent card -----------------------------------------------------------

function AgentCard({
  agent,
  client,
  onOpenRoom,
}: {
  agent: FleetAgent;
  client: Client;
  onOpenRoom(roomId: string): void;
}) {
  const [points, setPoints] = useState<HistoryPoint[] | null>(null);
  const historyRoom = agent.latest?.room_id ?? agent.rooms[0]?.room_id ?? null;

  useEffect(() => {
    if (!historyRoom) {
      setPoints([]);
      return;
    }
    let alive = true;
    client
      .call('agent.history', { room_id: historyRoom, identity_id: agent.identity_id, limit: 40 })
      .then((r) => {
        if (alive) setPoints(r.points);
      })
      .catch(() => {
        if (alive) setPoints([]);
      });
    return () => {
      alive = false;
    };
    // Refetch when the newest status advances (agent.latest.ts) so the sparkline
    // tracks live progress without polling per card.
  }, [client, agent.identity_id, historyRoom, agent.latest?.ts]);

  const tone = LIVENESS_TONE[agent.liveness];
  const tint = colorForId(agent.identity_id);
  const latest = agent.latest;
  const openRoom = latest?.room_id ?? agent.rooms[0]?.room_id ?? null;

  return (
    <article className={`fleet-card tone-${tone}`}>
      <div className="fleet-card-top">
        <span className="fleet-hex" style={{ color: tint, background: `${tint}1f` }} aria-hidden="true">
          <Avatar id={agent.identity_id} size={34} />
        </span>
        <div className="fleet-card-id">
          <div className="fleet-name-row">
            <SenderName id={agent.identity_id} className="fleet-name" />
            <span className={`live-pill live-${tone}`}>
              <span className="dot" /> {LIVENESS_LABEL[agent.liveness]}
            </span>
          </div>
          <code className="fleet-idhex mono" title={agent.identity_id}>
            {agent.identity_id.slice(0, 12)}…
          </code>
        </div>
        <CopyButton text={agent.identity_id} label="⧉" ariaLabel="Copy identity ID" />
      </div>

      <div className="fleet-card-mid">
        <div className="fleet-status">
          {latest ? (
            <>
              <span className={`chip chip-label tone-${labelTone(latest.label)}`}>{prettyLabel(latest.label)}</span>
              {latest.message ? <p className="fleet-msg">{latest.message}</p> : null}
            </>
          ) : (
            <p className="fleet-msg muted">No status posted yet.</p>
          )}
        </div>
        <Sparkline points={points} color={tint} muted={tone === 'off' || tone === 'warn'} />
      </div>

      {latest && typeof latest.progress === 'number' ? (
        <div className="progress-row">
          <ProgressBar value={latest.progress} />
          <span className="progress-num">{Math.max(0, Math.min(100, latest.progress))}%</span>
        </div>
      ) : null}

      <div className="fleet-rooms">
        {agent.rooms.map((r) => (
          <button
            key={r.room_id}
            type="button"
            className="room-chip"
            onClick={() => onOpenRoom(r.room_id)}
            title={r.name ?? r.room_id}
          >
            <span aria-hidden="true">⬡</span>
            <span className="room-chip-name">{r.name ?? shortId(r.room_id)}</span>
          </button>
        ))}
      </div>

      <div className="fleet-card-foot">
        <span className="muted fleet-seen">
          {agent.last_seen_ts !== null ? `Last update ${relTime(agent.last_seen_ts)}` : 'Never seen'}
        </span>
        {openRoom ? (
          <button type="button" className="btn btn-sm" onClick={() => onOpenRoom(openRoom)}>
            <span aria-hidden="true">⇱</span> Open Room
          </button>
        ) : null}
      </div>
    </article>
  );
}

// -- stat tiles ---------------------------------------------------------------

function StatTiles({ fleet }: { fleet: FleetResult }) {
  const coverage = fleet.rooms_total > 0 ? Math.round((fleet.rooms_covered / fleet.rooms_total) * 100) : 0;
  return (
    <div className="fleet-stats">
      <div className="fleet-stat">
        <span className="fleet-stat-icon icon-green" aria-hidden="true">✦</span>
        <div className="fleet-stat-body">
          <span className="fleet-stat-label">Active agents</span>
          <strong className="fleet-stat-value">{fleet.active}</strong>
          <span className="fleet-stat-sub muted">of {fleet.total} total</span>
        </div>
      </div>
      <div className="fleet-stat">
        <span className="fleet-stat-icon icon-amber" aria-hidden="true">⚡</span>
        <div className="fleet-stat-body">
          <span className="fleet-stat-label">Running tasks</span>
          <strong className="fleet-stat-value">{fleet.working}</strong>
          <span className="fleet-stat-sub muted">one task per agent</span>
        </div>
      </div>
      <div className="fleet-stat">
        <span className="fleet-stat-icon icon-blue" aria-hidden="true">⬡</span>
        <div className="fleet-stat-body">
          <span className="fleet-stat-label">Room coverage</span>
          <strong className="fleet-stat-value">{coverage}%</strong>
          <span className="fleet-stat-sub muted">
            {fleet.rooms_covered} of {fleet.rooms_total} rooms
          </span>
        </div>
      </div>
    </div>
  );
}

// -- filters ------------------------------------------------------------------

type FleetFilter = 'all' | 'active' | 'needs-attention' | 'working' | 'offline';

function matchesFilter(a: FleetAgent, f: FleetFilter): boolean {
  switch (f) {
    case 'active':
      return a.liveness === 'working' || a.liveness === 'online-idle';
    case 'needs-attention':
      return a.latest != null && labelTone(a.latest.label) === 'blue';
    case 'working':
      return a.liveness === 'working';
    case 'offline':
      return a.liveness === 'offline' || a.liveness === 'stale';
    default:
      return true;
  }
}

// -- Add Agent modal (security boundary — mints an invite, spawns nothing) -----

function AddAgentModal({
  client,
  rooms,
  onClose,
}: {
  client: Client;
  rooms: RoomSummary[];
  onClose(): void;
}) {
  const ownedRooms = rooms.filter((r) => r.role === 'owner');
  const [roomId, setRoomId] = useState(ownedRooms[0]?.room_id ?? '');
  const [identityId, setIdentityId] = useState('');
  const [worker, setWorker] = useState<'echo' | 'claude'>('echo');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const [result, setResult] = useState<{ ticket: string; addr: string | null } | null>(null);

  const generate = async () => {
    const invitee = identityId.trim();
    if (!invitee || !roomId || busy) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      // Open the room to obtain its dialable session address (docs §5 step 2),
      // then mint an agent-role ticket. The browser never spawns a runner.
      const opened = await client.call('room.open', { room_id: roomId });
      const { ticket } = await client.call('invite.create', {
        room_id: roomId,
        identity_id: invitee,
        role: 'agent',
      });
      setResult({ ticket, addr: opened.endpoint.addr ?? null });
    } catch (e) {
      setError(errorShape(e));
    } finally {
      setBusy(false);
    }
  };

  const command = result
    ? `node scripts/jeliya-agent.mjs --ticket ${result.ticket}${result.addr ? ` --peer ${result.addr}` : ''} --worker ${worker}`
    : '';

  return (
    <Modal title="Add an agent" onClose={onClose} wide>
      {ownedRooms.length === 0 ? (
        <p className="muted">
          You don’t own any rooms yet. Create a room first — agent invites can only be minted for a room you own.
        </p>
      ) : !result ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void generate();
          }}
        >
          <p className="muted">
            Mint an agent-role ticket for a room you own. This <strong>does not start anything</strong> — running the
            command below on the agent’s machine is a deliberate, human step (the security boundary).
          </p>
          <label className="field">
            <span>Room</span>
            <select value={roomId} onChange={(e) => setRoomId(e.target.value)}>
              {ownedRooms.map((r) => (
                <option key={r.room_id} value={r.room_id}>
                  {r.name}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>Agent identity id</span>
            <input
              value={identityId}
              onChange={(e) => setIdentityId(e.target.value)}
              placeholder="64-hex identity id (from jeliya-agent.mjs --identity-only)"
              className="mono"
              spellCheck={false}
              autoFocus
            />
          </label>
          <label className="field">
            <span>Worker</span>
            <select value={worker} onChange={(e) => setWorker(e.target.value as 'echo' | 'claude')}>
              <option value="echo">echo (safe — no real execution, for trying the flow)</option>
              <option value="claude">claude (runs real commands — arbitrary code/file execution for this room’s allowlisted senders)</option>
            </select>
          </label>
          {worker === 'claude' ? (
            <p className="error-note" role="alert">
              WARNING — --worker claude runs the <code>claude</code> CLI with --permission-mode acceptEdits on every
              triggered message from an allowlisted sender. That is arbitrary code / file execution on this host.
              Only enable it for a room and senders you trust.
            </p>
          ) : null}
          <button type="submit" className="btn btn-primary" disabled={busy || !identityId.trim() || !roomId}>
            {busy ? 'Minting…' : 'Mint agent invite'}
          </button>
          <ErrorNote error={error} />
        </form>
      ) : (
        <div>
          <p className="muted">
            Run this on the agent’s machine to bring it into the room. The daemon has no “spawn agent” call — this is
            copied and run by a human on purpose.
          </p>
          <div className="ticket-box">
            <textarea
              className="mono"
              readOnly
              value={command}
              rows={4}
              aria-label="Agent launch command"
              onFocus={(e) => e.target.select()}
            />
            <CopyButton text={command} label="Copy command" />
          </div>
          <p className="muted">
            The runner lives in the repo — clone it and run this from the checkout (no <code>npm install</code>{' '}
            needed; Node 22+ required). Installed <code>jeliyad</code> via brew/script instead of building? Prefix
            the command with <code>JELIYAD=&quot;$(command -v jeliyad)&quot;</code> so the runner finds it. Full guide:{' '}
            <code>docs/agent-guide.md</code>.
          </p>
          <div className="addr-box">
            <p className="muted">Ticket only (if you assemble the command yourself):</p>
            <div className="ticket-box">
              <code className="mono addr-code">{result.ticket}</code>
              <CopyButton text={result.ticket} label="Copy ticket" />
            </div>
          </div>
          {!result.addr ? (
            <p className="muted">
              This daemon reported no dialable address — the agent may connect via relay or discovery.
            </p>
          ) : null}
          <button type="button" className="btn btn-ghost" onClick={() => setResult(null)}>
            <span aria-hidden="true">←</span> New invite
          </button>
        </div>
      )}
    </Modal>
  );
}

// -- dashboard ----------------------------------------------------------------

export function FleetDashboard({
  client,
  rooms,
  onOpenRoom,
}: {
  client: Client;
  rooms: RoomSummary[];
  onOpenRoom(roomId: string): void;
}) {
  const names = useNames();
  const [fleet, setFleet] = useState<FleetResult | null>(null);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [filter, setFilter] = useState<FleetFilter>('all');
  const [query, setQuery] = useState('');
  const [addOpen, setAddOpen] = useState(false);
  const debounceRef = useRef<number | null>(null);

  // Poll agents.fleet on an interval AND nudge a refresh on any room.event push
  // so the numbers stay live without hammering the daemon.
  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const f = await client.call('agents.fleet', {});
        if (!alive) return;
        setFleet(f);
        setError(null);
        setLoaded(true);
      } catch (e) {
        if (!alive) return;
        setError(errorShape(e));
        setLoaded(true);
      }
    };
    void load();
    const timer = window.setInterval(load, 4000);
    const off = client.on('room.event', () => {
      if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
      debounceRef.current = window.setTimeout(load, 400);
    });
    return () => {
      alive = false;
      window.clearInterval(timer);
      if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
      off();
    };
  }, [client]);

  const counts = {
    all: fleet?.agents.length ?? 0,
    active: fleet?.agents.filter((a) => matchesFilter(a, 'active')).length ?? 0,
    'needs-attention': fleet?.agents.filter((a) => matchesFilter(a, 'needs-attention')).length ?? 0,
    working: fleet?.agents.filter((a) => matchesFilter(a, 'working')).length ?? 0,
    offline: fleet?.agents.filter((a) => matchesFilter(a, 'offline')).length ?? 0,
  };

  const q = query.trim().toLowerCase();
  const visible = (fleet?.agents ?? []).filter((a) => {
    if (!matchesFilter(a, filter)) return false;
    if (!q) return true;
    return names.display(a.identity_id).toLowerCase().includes(q) || a.identity_id.toLowerCase().includes(q);
  });

  const filters: { key: FleetFilter; label: string }[] = [
    { key: 'all', label: 'All' },
    // This filter is `working || online-idle` — an agent whose peer is
    // reachable. "Live" says that; "Active" said it in a word the room rail
    // used for a local session and the wire uses for signed membership
    // (docs/room-workbench.md, decision 4).
    { key: 'active', label: 'Live' },
    { key: 'needs-attention', label: 'Needs attention' },
    { key: 'working', label: 'Working' },
    { key: 'offline', label: 'Offline' },
  ];

  return (
    <section className="fleet-view" aria-label="Agent Fleet">
      <header className="fleet-head">
        <div className="fleet-head-top">
          <div className="fleet-title">
            <TreeMark size={26} />
            {/* The destination is named once (docs/room-workbench.md,
                decision 1). A rail entry reading "Agent Fleet" that opens a
                page titled "Agents" leaves the user to guess whether this is
                the same place as a room's "Agents & Runs" — the exact
                collision the record exists to remove. */}
            <h1>Agent Fleet</h1>
          </div>
          <div className="fleet-head-actions">
            <input
              className="fleet-search"
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search agents…"
              aria-label="Search agents"
              spellCheck={false}
            />
            <button type="button" className="btn btn-primary" onClick={() => setAddOpen(true)}>
              <span aria-hidden="true">＋</span> Add Agent
            </button>
          </div>
        </div>
        {/* Mutually-exclusive filter toggles — a group of pressed-state buttons,
            not an ARIA tabs widget (there are no tabpanels and no roving
            tabindex/arrow-key model to back that contract). */}
        <div className="fleet-filters" role="group" aria-label="Filter agents">
          {filters.map((f) => (
            <button
              key={f.key}
              type="button"
              aria-pressed={filter === f.key}
              className={`fleet-filter${filter === f.key ? ' active' : ''}`}
              onClick={() => setFilter(f.key)}
            >
              {f.label} <span className="count">{counts[f.key]}</span>
            </button>
          ))}
        </div>
      </header>

      <div className="fleet-body" aria-busy={!loaded || undefined}>
        {error ? <ErrorNote error={error} /> : null}
        {fleet ? <StatTiles fleet={fleet} /> : null}

        {!loaded ? (
          <>
            {/* Skeleton mirrors the real anatomy (stat tiles + cards) so the
                data swap is layout-shift-free. No sr-only utility class exists,
                hence the inline visually-hidden style on the status text. */}
            <div
              role="status"
              style={{ position: 'absolute', width: 1, height: 1, overflow: 'hidden', clipPath: 'inset(50%)' }}
            >
              Loading agents
            </div>
            <div className="fleet-stats" aria-hidden="true">
              {[0, 1, 2].map((i) => (
                <div key={i} className="fleet-stat">
                  <span className="skel-icon skel" />
                  <div className="skel-lines">
                    <span className="skel-line skel" style={{ width: 92, height: 10 }} />
                    <span className="skel-line skel" style={{ width: 52, height: 22 }} />
                  </div>
                </div>
              ))}
            </div>
            <div className="fleet-grid" aria-hidden="true">
              {[0, 1, 2].map((i) => (
                <div key={i} className="fleet-card">
                  <div className="skel-row">
                    <span className="skel-avatar skel" />
                    <div className="skel-lines">
                      <span className="skel-line skel" style={{ width: 120, height: 12 }} />
                      <span className="skel-line skel" style={{ width: 96, height: 10 }} />
                    </div>
                  </div>
                  <span className="skel-line skel" style={{ width: '100%', height: 40 }} />
                  <span className="skel-line skel" style={{ width: '60%', height: 10 }} />
                </div>
              ))}
            </div>
          </>
        ) : visible.length === 0 ? (
          <div className="fleet-empty muted">
            {fleet && fleet.total === 0
              ? 'No agents in any room yet. Use “Add Agent” to mint an invite.'
              : 'No agents match this filter.'}
          </div>
        ) : (
          <div className="fleet-grid">
            {visible.map((a) => (
              <AgentCard key={a.identity_id} agent={a} client={client} onOpenRoom={onOpenRoom} />
            ))}
          </div>
        )}
      </div>

      {addOpen ? <AddAgentModal client={client} rooms={rooms} onClose={() => setAddOpen(false)} /> : null}
    </section>
  );
}
