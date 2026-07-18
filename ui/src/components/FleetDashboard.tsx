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
import {
  type AttentionReason,
  attentionRank,
  attentionReason,
  hasNumericProgress,
  needsAttention,
  statusUnverified,
} from '../lib/fleet';
import { homonymousRoomIds, roomDisplayName } from '../lib/rooms';
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

// -- status strip (inline SVG, discrete timestamped events) -------------------
//
// One mark per real agent_status event from agent.history, positioned by its
// REAL timestamp on the x-axis. There is no connecting line and no area fill:
// a curve between two events would fabricate intermediate state the log never
// recorded (docs/room-attention.md, decision 6). An event that carried real
// numeric `progress` rises to that height as a stem; a label-only (categorical)
// event is a dot on the baseline, tinted by its label tone — never lifted to an
// invented y-band. Single series per card, so no legend: the card title names it.

const TONE_VAR: Record<'red' | 'blue' | 'green' | 'neutral', string> = {
  red: 'var(--red)',
  blue: 'var(--blue)',
  green: 'var(--accent)',
  neutral: 'var(--text-mute)',
};

function StatusStrip({ points, color, muted }: { points: HistoryPoint[] | null; color: string; muted: boolean }) {
  const W = 132;
  const H = 40;
  const pad = 4;
  const base = H - pad;
  const stroke = muted ? 'var(--text-mute)' : color;

  // null = history not fetched yet — solid baseline placeholder, same size, so
  // the dashed "no history" treatment is never flashed before data arrives.
  if (points === null) {
    return (
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="Loading status history">
        <title>Loading status history</title>
        <line x1={pad} y1={base} x2={W - pad} y2={base} stroke="var(--border-strong)" strokeWidth="1.5" />
      </svg>
    );
  }

  if (points.length === 0) {
    return (
      <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label="No status history yet">
        <title>No status history yet</title>
        <line x1={pad} y1={base} x2={W - pad} y2={base} stroke="var(--border-strong)" strokeWidth="1.5" strokeDasharray="2 3" />
      </svg>
    );
  }

  const numericCount = points.filter((p) => hasNumericProgress(p.progress)).length;
  const label =
    `${points.length} status event${points.length === 1 ? '' : 's'}` +
    (numericCount > 0 ? `, ${numericCount} with numeric progress` : ', no numeric progress');

  const tsMin = points[0].ts;
  const tsMax = points[points.length - 1].ts;
  const span = tsMax - tsMin;
  const xAt = (i: number): number => {
    if (points.length === 1) return W - pad;
    const t = span > 0 ? (points[i].ts - tsMin) / span : i / (points.length - 1);
    return pad + t * (W - 2 * pad);
  };

  return (
    <svg className="spark" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label}>
      <title>{label}</title>
      {/* The time axis: a static baseline, not a data line. */}
      <line x1={pad} y1={base} x2={W - pad} y2={base} stroke="var(--border-strong)" strokeWidth="1" />
      {points.map((p, i) => {
        const x = xAt(i);
        // A real numeric-progress event rises to its measured height as a stem;
        // no interpolation joins it to its neighbours.
        if (hasNumericProgress(p.progress)) {
          const y = base - (Math.max(0, Math.min(100, p.progress)) / 100) * (H - 2 * pad);
          return (
            <g key={i}>
              <line x1={x} y1={base} x2={x} y2={y} stroke={stroke} strokeWidth="2" opacity={muted ? 0.7 : 1} />
              <circle cx={x} cy={y} r={2.6} fill={stroke} />
            </g>
          );
        }
        // A categorical event is a timestamped dot on the baseline, tinted by
        // its label tone — never lifted to a fabricated quantitative y-value.
        return <circle key={i} cx={x} cy={base} r={2.6} fill={muted ? 'var(--text-mute)' : TONE_VAR[labelTone(p.label)]} />;
      })}
    </svg>
  );
}

// -- per-agent card -----------------------------------------------------------

function AgentCard({
  agent,
  client,
  homonyms,
  onOpenRoom,
}: {
  agent: FleetAgent;
  client: Client;
  /** Room_ids that share a display name with another room in the fleet's room
   *  list — those chips carry a short id (docs/room-workbench.md, decision 6). */
  homonyms: Set<string>;
  onOpenRoom(roomId: string): void;
}) {
  const names = useNames();
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
  // A stale/offline agent's last posted label is a claim its liveness no longer
  // supports — it must be shown past-tense, never as a bare live status.
  const unverified = statusUnverified(agent.liveness);
  const openRoom = latest?.room_id ?? agent.rooms[0]?.room_id ?? null;

  return (
    // Liveness is stated ONCE as a legible fact: the dot+label pill below. The
    // card used to repeat it as a border hue (emerald for live, amber for
    // stale) — the same fact in a second, colour-only channel, which is exactly
    // the decorative-chrome duplication #75 removes. What survives is
    // `tone-off`, and only because it is not a border tone at all: it recedes
    // an inert agent through the GROUND (a muted card fill + dimmed avatar and
    // sparkline) while leaving every text contrast untouched. That is a
    // de-emphasis treatment, not a second copy of the liveness label.
    <article className={`fleet-card${tone === 'off' ? ' tone-off' : ''}`}>
      <div className="fleet-card-top">
        {/* `opacity: 1` cancels `.fleet-card.tone-off .fleet-hex { opacity: .5 }`
            in styles.css, which is a real WCAG failure and not mine to edit on
            this branch (see `notes`). That rule contradicts the comment sitting
            directly above it, which swears off "a blanket opacity, which would
            composite the info-bearing text below the WCAG AA floor" — and then
            applies exactly that to a box whose only child is the avatar's
            INITIALS. Halving them takes the glyphs under 4.5:1. axe had been
            reporting it as `incomplete` rather than a violation because the
            offline cards sat below the fold; deleting the KPI row lifted them
            into the viewport, where `elementFromPoint` resolves and the latent
            failure becomes a real one. The card still recedes — through the
            muted `--bg-raise` ground and the dimmed sparkline, which carries no
            text — so the de-emphasis survives with its contrast intact. */}
        <span
          className="fleet-hex"
          style={{ color: tint, background: `${tint}1f`, opacity: 1 }}
          aria-hidden="true"
        >
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
        {/* One card per agent, so a bare "Copy identity ID" would repeat N
            times with nothing to tell the copies apart — name whose id it is. */}
        <CopyButton
          text={agent.identity_id}
          label="⧉"
          ariaLabel={`Copy identity ID for ${names.display(agent.identity_id)}`}
        />
      </div>

      <div className="fleet-card-mid">
        <div className="fleet-status">
          {latest ? (
            <>
              {/* Past-tense and never green when unverified: a Stale pill beside
                  a live "Working" chip is the contradiction #69 removes. */}
              <span
                className={`chip chip-label tone-${
                  unverified && labelTone(latest.label) === 'green' ? 'neutral' : labelTone(latest.label)
                }`}
                title={unverified ? 'Last posted status — its liveness no longer supports it' : undefined}
              >
                {unverified ? `Last: ${prettyLabel(latest.label)}` : prettyLabel(latest.label)}
              </span>
              {latest.message ? <p className="fleet-msg">{latest.message}</p> : null}
            </>
          ) : (
            <p className="fleet-msg muted">No status posted yet.</p>
          )}
        </div>
        <StatusStrip points={points} color={tint} muted={tone === 'off' || tone === 'warn'} />
      </div>

      {latest && typeof latest.progress === 'number' ? (
        <div className="progress-row">
          <ProgressBar value={latest.progress} />
          <span className="progress-num">{Math.max(0, Math.min(100, latest.progress))}%</span>
        </div>
      ) : null}

      <div className="fleet-rooms">
        {agent.rooms.map((r) => {
          const name = roomDisplayName(r);
          const isHomonym = homonyms.has(r.room_id);
          return (
            <button
              key={r.room_id}
              type="button"
              className="room-chip"
              onClick={() => onOpenRoom(r.room_id)}
              title={r.name ?? r.room_id}
              // A homonym chip's name alone can't say which room it opens, so
              // the short id joins its accessible name, not just its visuals.
              aria-label={isHomonym ? `${name} (${shortId(r.room_id)})` : undefined}
            >
              <span aria-hidden="true">⬡</span>
              <span className="room-chip-name">{name}</span>
              {isHomonym ? <code className="room-disambig mono">{shortId(r.room_id)}</code> : null}
            </button>
          );
        })}
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

// -- room coverage ------------------------------------------------------------
//
// What used to stand here was a three-up grid of hero-metric tiles, each with a
// decorative glyph in a tinted square. PRODUCT.md and DESIGN.md both name
// "identical KPI card grids, hero-metric tiles" as generic-SaaS-dashboard slop
// and an explicit anti-reference, and two of the three numbers were already on
// screen: `fleet.active` is the "Live" filter chip's count, `fleet.working` is
// the "Working" chip's count, and the needs-attention total is that section's
// own badge. The compact shell had already reached this conclusion — styles.css
// hides `.fleet-stats` below 900px because "the filter chips already carry
// those counts, so drop this KPI row rather than repeat it". The desktop shell
// now agrees with it, and the chips become the single rendering of those facts.
//
// Room coverage is the one fact nothing else on this page renders, so it stays
// — as a sentence that says what it counts, not a percentage floating over a
// caption. The percentage is derived, so it is shown beside the counts it comes
// from rather than in place of them.

function FleetCoverage({ fleet }: { fleet: FleetResult }) {
  const coverage = fleet.rooms_total > 0 ? Math.round((fleet.rooms_covered / fleet.rooms_total) * 100) : 0;
  return (
    <p className="fleet-coverage muted">
      {fleet.rooms_total === 0
        ? 'Room coverage: no rooms yet.'
        : `Room coverage: ${fleet.rooms_covered} of ${fleet.rooms_total} room${
            fleet.rooms_total === 1 ? '' : 's'
          } have an agent (${coverage}%).`}
    </p>
  );
}

// -- needs attention (actionable agents, before the aggregate tiles) ----------

const REASON_LABEL: Record<AttentionReason, string> = {
  failed: 'Failed',
  review: 'Awaiting review',
  stale: 'Stale',
  offline: 'Offline after work',
};

/** The prioritized section the epic is named for: agents that need a human,
 *  ranked most-actionable first, rendered ABOVE the aggregate tiles. Membership
 *  and order come from the shared classifier (docs/room-attention.md,
 *  decision 4), so it never silently drops a failed or stale agent again. */
function NeedsAttention({ agents, onOpenRoom }: { agents: FleetAgent[]; onOpenRoom(roomId: string): void }) {
  const headingId = useId();
  const items = agents
    .map((a) => ({ a, reason: attentionReason(a.liveness, a.latest?.label ?? null) }))
    .filter((x): x is { a: FleetAgent; reason: AttentionReason } => x.reason !== null)
    .sort(
      (x, y) =>
        attentionRank(x.a.liveness, x.a.latest?.label ?? null) -
          attentionRank(y.a.liveness, y.a.latest?.label ?? null) ||
        (y.a.last_seen_ts ?? 0) - (x.a.last_seen_ts ?? 0),
    );

  return (
    <section className="fleet-attention" aria-labelledby={headingId}>
      <h2 className="fleet-section-head" id={headingId}>
        Needs attention <span className="count">{items.length}</span>
      </h2>
      {items.length === 0 ? (
        <p className="fleet-attention-empty muted">Nothing needs attention right now.</p>
      ) : (
        <ul className="fleet-attention-list">
          {items.map(({ a, reason }) => {
            const room = a.latest?.room_id ?? a.rooms[0]?.room_id ?? null;
            return (
              <li key={a.identity_id} className="attention-row">
                <Avatar id={a.identity_id} size={26} />
                <div className="attention-id">
                  <SenderName id={a.identity_id} className="attention-name" />
                  {/* dot + label, never colour alone (WCAG AA). */}
                  <span className={`chip attention-reason reason-${reason}`}>
                    <span className="dot" /> {REASON_LABEL[reason]}
                  </span>
                </div>
                {a.latest?.message ? <p className="attention-msg muted">{a.latest.message}</p> : null}
                <div className="attention-act">
                  <span className="muted attention-seen">
                    {a.last_seen_ts !== null ? `Last update ${relTime(a.last_seen_ts)}` : 'Never seen'}
                  </span>
                  {room ? (
                    <button type="button" className="btn btn-sm" onClick={() => onOpenRoom(room)}>
                      <span aria-hidden="true">⇱</span> Open Room
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

// -- filters ------------------------------------------------------------------

type FleetFilter = 'all' | 'active' | 'needs-attention' | 'working' | 'offline';

function matchesFilter(a: FleetAgent, f: FleetFilter): boolean {
  switch (f) {
    case 'active':
      return a.liveness === 'working' || a.liveness === 'online-idle';
    case 'needs-attention':
      // The full closed set (docs/room-attention.md, decision 4), not the old
      // blue-only match that silently dropped failed/blocked, stale, and
      // offline-after-work agents.
      return needsAttention(a.liveness, a.latest?.label ?? null);
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
  // Homonyms are keyed off this identity's room list (the same rule the rail
  // uses), so a chip is disambiguated the moment two of its rooms share a name.
  const homonyms = homonymousRoomIds(rooms);
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
    // A full-page destination owns the page's `main` landmark (issue #72),
    // named by its own `h1` rather than a duplicated `aria-label` — one string,
    // so the landmark and the heading can never drift apart.
    <main className="fleet-view" id="fleet-main" aria-labelledby="fleet-title">
      <header className="fleet-head">
        <div className="fleet-head-top">
          <div className="fleet-title">
            <TreeMark size={26} />
            {/* The destination is named once (docs/room-workbench.md,
                decision 1). A rail entry reading "Agent Fleet" that opens a
                page titled "Agents" leaves the user to guess whether this is
                the same place as a room's "Agents & Runs" — the exact
                collision the record exists to remove. */}
            <h1 id="fleet-title">Agent Fleet</h1>
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
        {/* Actionable agents appear before aggregate totals (#69). */}
        {loaded && fleet && fleet.total > 0 ? <NeedsAttention agents={fleet.agents} onOpenRoom={onOpenRoom} /> : null}
        {fleet ? <FleetCoverage fleet={fleet} /> : null}

        {!loaded ? (
          <>
            {/* Skeleton mirrors the real anatomy (one coverage line + cards) so
                the data swap is layout-shift-free. It tracked the three KPI
                tiles while they existed; now that the row is one sentence, so
                is its placeholder. */}
            <div role="status" className="visually-hidden">
              Loading agents
            </div>
            <div className="fleet-coverage" aria-hidden="true" style={{ marginBottom: 18 }}>
              <span className="skel-line skel" style={{ width: 260, height: 12 }} />
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
              <AgentCard key={a.identity_id} agent={a} client={client} homonyms={homonyms} onOpenRoom={onOpenRoom} />
            ))}
          </div>
        )}
      </div>

      {addOpen ? <AddAgentModal client={client} rooms={rooms} onClose={() => setAddOpen(false)} /> : null}
    </main>
  );
}
