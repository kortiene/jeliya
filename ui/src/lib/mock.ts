// In-memory fixture client for VITE_MOCK=1: the whole app runs with no
// daemon. Implements the same `Client` surface as WsClient and answers every
// PROTOCOL.md method from fixtures that echo the design mockups.
//
// `?mock=fresh` starts with no identity and no rooms so the onboarding flow
// can be exercised in mock mode too.

import type {
  Client,
  ConnectionState,
  DaemonStatus,
  FileEntry,
  FleetAgent,
  FleetAgentLatest,
  FleetAgentRoom,
  FleetResult,
  Identity,
  Liveness,
  Member,
  MethodMap,
  MethodName,
  PeerStatus,
  PipeEntry,
  PushMap,
  PushName,
  Role,
  RoomSummary,
  TimelineEvent,
  TimelineKind,
} from './protocol';
import { RequestError } from './protocol';
import { suggestedNames } from './names';

// -- deterministic ids -------------------------------------------------------

function hex(seed: string, len: number): string {
  // Absorb the whole seed, then squeeze — distinct seeds give distinct ids.
  let h1 = 2166136261 >>> 0;
  let h2 = 0x9e3779b9 >>> 0;
  for (let i = 0; i < seed.length; i++) {
    const c = seed.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 16777619) >>> 0;
    h2 = (Math.imul(h2 ^ c, 2246822519) + 0x9e3779b9) >>> 0;
  }
  let out = '';
  let i = 0;
  while (out.length < len) {
    h1 = Math.imul(h1 ^ (h2 >>> 13) ^ i, 16777619) >>> 0;
    h2 = (Math.imul(h2 ^ (h1 >>> 11), 2246822519) + i) >>> 0;
    out += ((h1 ^ h2) >>> 0).toString(16).padStart(8, '0');
    i += 1;
  }
  return out.slice(0, len);
}

const BASE32 = 'abcdefghijklmnopqrstuvwxyz234567';

function base32ish(seed: string, len: number): string {
  const h = hex(seed, len * 2);
  let out = '';
  for (let i = 0; i < len; i++) {
    out += BASE32[parseInt(h.slice(i * 2, i * 2 + 2), 16) % 32];
  }
  return out;
}

/** ms epoch for "yesterday" at h:m local time. Anchoring on yesterday (not
 *  today) guarantees every fixture timestamp is strictly before `Date.now()`
 *  regardless of what time the demo is opened — anchoring on today put
 *  morning fixtures (8:58 etc.) in the future for anyone opening the app
 *  before those hours, which buried live messages under "future" history and
 *  masked a hardcoded-stale agent behind relTime()'s just-now floor. Relative
 *  ordering between fixtures is unaffected since they all shift by the same
 *  24h. */
function at(h: number, m: number, s = 0): number {
  const d = new Date(Date.now() - 24 * 60 * 60 * 1000);
  d.setHours(h, m, s, 0);
  return d.getTime();
}

/** Parse an invite expiry (a number of seconds, or a duration string like
 *  "24h" / "90m" / "3600") into seconds, or null for none — mirrors the
 *  daemon's `expiry_spec` (rpc.rs). */
function parseExpirySeconds(expiry: number | string | undefined): number | null {
  if (typeof expiry === 'number') return expiry > 0 ? expiry : null;
  if (typeof expiry !== 'string') return null;
  const m = expiry.trim().match(/^(\d+)\s*([smhd])?$/i);
  if (!m) return null;
  const n = Number(m[1]);
  const unit = (m[2] ?? 's').toLowerCase();
  const mult = unit === 'd' ? 86400 : unit === 'h' ? 3600 : unit === 'm' ? 60 : 1;
  return n > 0 ? n * mult : null;
}

// -- people ------------------------------------------------------------------

interface Person {
  id: string;
  dev: string;
  ep: string;
  role: Role;
  name: string;
}

function person(seed: string, role: Role, name: string): Person {
  return {
    id: hex(`${seed}-identity`, 64),
    dev: hex(`${seed}-device`, 64),
    ep: hex(`${seed}-endpoint`, 64),
    role,
    name,
  };
}

const ALEX = person('alex', 'owner', 'Alex K.');
const MAYA = person('maya', 'member', 'Maya R.');
const SAM = person('sam', 'member', 'Sam D.');
const BACKEND = person('backend-agent', 'agent', 'Backend Agent');
const FRONTEND = person('frontend-agent', 'agent', 'Frontend Agent');
const QA = person('qa-agent', 'agent', 'QA Agent');
const RESEARCH = person('research-agent', 'agent', 'Research Agent');
// A failed-status agent for the fleet's Needs Attention section (#69). Kept out
// of EVERYONE deliberately: it lives only in the Agent Workspace room, so the
// MVP room's roster/agent counts are unchanged.
const DEPLOY = person('deploy-agent', 'agent', 'Deploy Agent');

const EVERYONE = [ALEX, MAYA, SAM, BACKEND, FRONTEND, QA, RESEARCH];

// -- room fixture ------------------------------------------------------------

interface MockRoom {
  room_id: string;
  name: string;
  myRole: Role;
  members: Member[];
  timeline: TimelineEvent[];
  files: FileEntry[];
  pipes: PipeEntry[];
  peers: PeerStatus[];
  open: boolean;
  timers: number[];
  simulated: boolean;
}

/** What a minted `invite.create` ticket actually redeems — looked up by
 *  `room.join` so it lands in the room it was really minted for instead of a
 *  fabricated one. `expiresAt: null` means no expiry was requested. */
interface TicketEntry {
  room_id: string;
  identity_id: string;
  role: Role;
  expiresAt: number | null;
}

let eventSeq = 0;

function ev(
  room_id: string,
  ts: number,
  sender: Person,
  kind: TimelineKind,
  extra: Partial<TimelineEvent> = {},
): TimelineEvent {
  eventSeq += 1;
  return {
    event_id: hex(`${room_id}:${kind}:${eventSeq}`, 64),
    room_id,
    ts,
    sender: { identity_id: sender.id, device_id: sender.dev, role: sender.role },
    kind,
    ...extra,
  };
}

const fid = (seed: string) => `file_${hex(seed, 32)}`;
const pid = (seed: string) => hex(seed, 32);

const MAIN_ID = `blake3:${hex('room-build-iroh-rooms-mvp', 64)}`;
// The room a `?mock_ticket=` preset ticket admits into. Deliberately NOT
// seeded into the room map: you cannot list a room you have not joined, so
// redeeming the ticket materializes it — the way a real join bootstraps a
// room this daemon has never seen — and the join is a real state transition.
const INVITED_ID = `blake3:${hex('room-invited-workspace', 64)}`;

const F_RELEASE = fid('release-notes.txt');
const F_PROTOCOL = fid('room-protocol.md');
const F_WIREFRAME = fid('wireframe.png');
const F_PRD = fid('PRD_v0.2.pdf');
const F_REPORT = fid('test-report.json');

const PIPE_PREVIEW = pid('pipe-frontend-preview');
const PIPE_LOGS = pid('pipe-logs-stream');

function member(p: Person, status = 'active'): Member {
  return { identity_id: p.id, role: p.role, status };
}

function buildMainRoom(): MockRoom {
  const r = MAIN_ID;
  const timeline: TimelineEvent[] = [
    ev(r, at(8, 45), ALEX, 'room_created'),
    ev(r, at(8, 50), MAYA, 'member_joined', { member: { identity_id: MAYA.id, role: 'member' } }),
    ev(r, at(8, 52), SAM, 'member_joined', { member: { identity_id: SAM.id, role: 'member' } }),
    ev(r, at(8, 54), BACKEND, 'member_joined', { member: { identity_id: BACKEND.id, role: 'agent' } }),
    ev(r, at(8, 55), FRONTEND, 'member_joined', { member: { identity_id: FRONTEND.id, role: 'agent' } }),
    ev(r, at(8, 56), QA, 'member_joined', { member: { identity_id: QA.id, role: 'agent' } }),
    ev(r, at(8, 58), BACKEND, 'file_shared', {
      file: { file_id: F_RELEASE, name: 'release-notes.txt', size: 8 * 1024, mime: 'text/plain' },
    }),
    ev(r, at(9, 12), ALEX, 'file_shared', {
      file: { file_id: F_PROTOCOL, name: 'room-protocol.md', size: 12 * 1024, mime: 'text/markdown' },
    }),
    ev(r, at(9, 48), SAM, 'file_shared', {
      file: { file_id: F_WIREFRAME, name: 'wireframe.png', size: 320 * 1024, mime: 'image/png' },
    }),
    ev(r, at(9, 5), BACKEND, 'agent_status', {
      label: 'working',
      status_message: 'Scaffolding room invite flow and peer discovery.',
      progress: 15,
    }),
    ev(r, at(9, 25), FRONTEND, 'agent_status', {
      label: 'working',
      status_message: 'Blocking out the room shell and timeline components.',
      progress: 20,
    }),
    ev(r, at(9, 40), BACKEND, 'agent_status', {
      label: 'working',
      status_message: 'File manifest sync wired; starting invite flow tests.',
      progress: 35,
    }),
    ev(r, at(9, 55), QA, 'agent_status', {
      label: 'working',
      status_message: 'Writing test suite v1 against the invite + sync paths.',
      progress: 40,
    }),
    ev(r, at(10, 2), ALEX, 'message', {
      body: 'Kicked off the rooms protocol spec and initial backend scaffolding. Next up: agent orchestration + pipe manager.',
    }),
    ev(r, at(10, 15), BACKEND, 'agent_status', {
      label: 'working',
      status_message:
        'Implemented room invite flow, peer discovery, and file manifest sync. Running integration tests…',
      progress: 60,
    }),
    ev(r, at(10, 20), BACKEND, 'pipe_opened', {
      pipe: { pipe_id: PIPE_LOGS, target: '127.0.0.1:4000', authorized_peer: ALEX.id },
    }),
    ev(r, at(10, 28), MAYA, 'message', {
      body: "Here's the updated PRD with rooms, pipes, and agent runtime.",
    }),
    ev(r, at(10, 28, 20), MAYA, 'file_shared', {
      file: { file_id: F_PRD, name: 'PRD_v0.2.pdf', size: 1887437, mime: 'application/pdf' },
    }),
    ev(r, at(10, 34), FRONTEND, 'agent_status', {
      label: 'preview_ready',
      status_message: 'UI scaffold is up with live data. Exposed preview on a pipe.',
    }),
    ev(r, at(10, 34, 30), FRONTEND, 'pipe_opened', {
      pipe: { pipe_id: PIPE_PREVIEW, target: '127.0.0.1:3000', authorized_peer: ALEX.id },
    }),
    ev(r, at(10, 45), QA, 'agent_status', {
      label: 'awaiting_review',
      status_message: 'Completed test suite v1. Summary attached.',
      artifacts: [F_REPORT],
    }),
    ev(r, at(10, 45, 30), QA, 'file_shared', {
      file: { file_id: F_REPORT, name: 'test-report.json', size: 85 * 1024, mime: 'application/json' },
    }),
    // A fresh working-class status so the fleet dashboard has one truthfully
    // "working" agent (connected peer + fresh working label, §1.2 row 4).
    ev(r, Math.max(at(10, 50), Date.now() - 90_000), BACKEND, 'agent_status', {
      label: 'working',
      status_message: 'Sync convergence suite running (14/24 green).',
      progress: 68,
    }),
  ];

  const files: FileEntry[] = [
    { file_id: F_RELEASE, name: 'release-notes.txt', size: 8 * 1024, mime: 'text/plain', sender_id: BACKEND.id, ts: at(8, 58), available: false, providers: 1 },
    { file_id: F_PROTOCOL, name: 'room-protocol.md', size: 12 * 1024, mime: 'text/markdown', sender_id: ALEX.id, ts: at(9, 12), available: true, providers: 3 },
    { file_id: F_WIREFRAME, name: 'wireframe.png', size: 320 * 1024, mime: 'image/png', sender_id: SAM.id, ts: at(9, 48), available: true, providers: 2 },
    { file_id: F_PRD, name: 'PRD_v0.2.pdf', size: 1887437, mime: 'application/pdf', sender_id: MAYA.id, ts: at(10, 28), available: true, providers: 4 },
    { file_id: F_REPORT, name: 'test-report.json', size: 85 * 1024, mime: 'application/json', sender_id: QA.id, ts: at(10, 45), available: true, providers: 2 },
  ];

  const pipes: PipeEntry[] = [
    { pipe_id: PIPE_PREVIEW, target: '127.0.0.1:3000', opened_by: FRONTEND.id, authorized_peer: ALEX.id, state: 'open', connected: true },
    { pipe_id: PIPE_LOGS, target: '127.0.0.1:4000', opened_by: BACKEND.id, authorized_peer: ALEX.id, state: 'open', connected: false },
  ];

  const peers: PeerStatus[] = [
    { endpoint_id: MAYA.ep, state: 'connected', path: 'direct', identity_id: MAYA.id },
    { endpoint_id: SAM.ep, state: 'connected', path: 'relay', identity_id: SAM.id },
    { endpoint_id: BACKEND.ep, state: 'connected', path: 'direct', identity_id: BACKEND.id },
    { endpoint_id: FRONTEND.ep, state: 'connected', path: 'direct', identity_id: FRONTEND.id },
    { endpoint_id: QA.ep, state: 'offline', path: null, identity_id: QA.id },
  ];

  return {
    room_id: r,
    name: 'Build Iroh Rooms MVP',
    myRole: 'owner',
    // Roster status is membership only (active|invited|left|removed) — QA's
    // offline-ness is peer/fleet liveness, never a member.status value.
    members: EVERYONE.map((p) => member(p, 'active')),
    timeline,
    files,
    pipes,
    peers,
    open: false,
    timers: [],
    simulated: false,
  };
}

function buildSideRoom(seed: string, name: string, people: Person[], myRole: Role, blurb: string): MockRoom {
  const r = `blake3:${hex(`room-${seed}`, 64)}`;
  const owner = people[0];
  // Roles are per-room: the creator owns the room, and my own role is the
  // declared myRole — the global Person.role only describes the default
  // (Alex owns the MVP room, not every room he's merely a member of).
  const roleHere = (p: Person): Role => (p.id === owner.id ? 'owner' : p.id === ALEX.id ? myRole : p.role);
  const timeline: TimelineEvent[] = [
    ev(r, at(8, 30), { ...owner, role: 'owner' }, 'room_created'),
    ...people.slice(1).map((p, i) =>
      ev(r, at(8, 32 + i), { ...p, role: roleHere(p) }, 'member_joined', {
        member: { identity_id: p.id, role: roleHere(p) },
      }),
    ),
    ev(r, at(9, 5), { ...owner, role: 'owner' }, 'message', { body: blurb }),
  ];
  return {
    room_id: r,
    name,
    myRole,
    members: people.map((p) => member({ ...p, role: roleHere(p) })),
    timeline,
    files: [],
    pipes: [],
    peers: people
      .filter((p) => p.id !== ALEX.id)
      .map((p) => ({ endpoint_id: p.ep, state: 'connected' as const, path: 'direct' as const, identity_id: p.id })),
    open: false,
    timers: [],
    simulated: false,
  };
}

// -- fleet liveness derivation (docs/agent-orchestration.md §1.2) -------------
//
// Mirrors the daemon's read-time rule exactly: primary signal = a currently
// connected peer in an OPEN room, secondary = the ts of the latest real
// agent_status event. A working-class latest label is never sufficient on its
// own — peer state overrides the last posted label.

const STALE_WORKING_MS = 20 * 60_000;

const LIVENESS_RANK: Record<Liveness, number> = {
  working: 0,
  'online-idle': 1,
  stale: 2,
  offline: 3,
};

/** Working-class iff the label is exactly `working`; unknown labels are
 *  idle-class (§1.1). */
const isWorkingClass = (label: string | undefined) => label === 'working';

function deriveLiveness(connected: boolean, latest: TimelineEvent | null, now: number): Liveness {
  if (!connected) {
    // Rows 1–3: no live peer — never online, never working.
    if (latest && isWorkingClass(latest.label)) return 'stale';
    return 'offline';
  }
  if (latest && isWorkingClass(latest.label)) {
    // Rows 4–5: connected + working-class label — fresh means working.
    return now - latest.ts <= STALE_WORKING_MS ? 'working' : 'stale';
  }
  // Row 6: connected, idle-class latest (or no status yet).
  return 'online-idle';
}

/** The room's newest signed event by ts — the recency source of
 *  docs/room-attention.md, decision 2 — or null for an empty timeline.
 *  A signed event timestamp, never "now". */
function newestEvent(timeline: TimelineEvent[]): TimelineEvent | null {
  let newest: TimelineEvent | null = null;
  for (const e of timeline) if (!newest || e.ts > newest.ts) newest = e;
  return newest;
}

// -- the client --------------------------------------------------------------

/** Deterministic failure injection for the browser regression suite:
 *  `?mock_fail=room.open:2` fails the first two `room.open` calls with a real
 *  `unavailable` error; `room.open:1:1` first allows one successful call.
 *  Comma-separated entries inject for several methods. Mock-only — the
 *  WebSocket client never reads this. */
interface FailureSpec {
  allowRemaining: number;
  failRemaining: number;
}

function parseFailSpecs(raw: string | null): Map<string, FailureSpec> {
  const specs = new Map<string, FailureSpec>();
  if (!raw) return specs;
  for (const entry of raw.split(',')) {
    const [method, count, after] = entry.split(':');
    const failRemaining = Number(count);
    if (!method || !Number.isInteger(failRemaining) || failRemaining <= 0) continue;
    const allowRemaining = Number(after ?? '0');
    specs.set(method, {
      allowRemaining: Number.isInteger(allowRemaining) && allowRemaining > 0 ? allowRemaining : 0,
      failRemaining,
    });
  }
  return specs;
}

/** Deterministic per-method extra latency for the browser regression suite:
 *  `?mock_delay=room.create:1200` holds every room.create response for an
 *  extra 1.2s so a test can exercise the in-flight (busy) window. Mock-only. */
function parseDelaySpecs(raw: string | null): Map<string, number> {
  const specs = new Map<string, number>();
  if (!raw) return specs;
  for (const entry of raw.split(',')) {
    const [method, ms] = entry.split(':');
    const delay = Number(ms);
    if (!method || !Number.isFinite(delay) || delay <= 0) continue;
    specs.set(method, delay);
  }
  return specs;
}

class MockClient implements Client {
  private state: ConnectionState = 'disconnected';
  private stateHandlers = new Set<(s: ConnectionState) => void>();
  private pushHandlers: { [P in PushName]: Set<(data: PushMap[P]) => void> } = {
    'room.event': new Set(),
    'peers.changed': new Set(),
  };
  private identity: Identity | null;
  private rooms = new Map<string, MockRoom>();
  private tickets = new Map<string, TicketEntry>();
  private portSeq = 41732;
  private startTimer: number | null = null;
  private failSpecs: Map<string, FailureSpec>;
  private delaySpecs: Map<string, number>;

  constructor(
    fresh: boolean,
    failSpecs: Map<string, FailureSpec> = new Map(),
    delaySpecs: Map<string, number> = new Map(),
    presetTicket: string | null = null,
  ) {
    this.failSpecs = failSpecs;
    this.delaySpecs = delaySpecs;
    for (const p of [...EVERYONE, DEPLOY]) suggestedNames[p.id] = p.name;
    if (fresh) {
      this.identity = null;
    } else {
      this.identity = { identity_id: ALEX.id, device_id: ALEX.dev };
      const main = buildMainRoom();
      this.rooms.set(main.room_id, main);
      const workspace = buildSideRoom(
        'agent-workspace',
        'Agent Workspace',
        [ALEX, BACKEND, FRONTEND, QA, DEPLOY],
        'owner',
        'Scratch room for agent runs. Post statuses here.',
      );
      // Failed-runner fixture (#69): a connected agent whose latest signed
      // status is a failure. It must surface in Needs Attention — the exact
      // red-tone case the old blue-only filter silently dropped.
      workspace.timeline.push(
        ev(workspace.room_id, at(10, 6), DEPLOY, 'agent_status', {
          label: 'deploy_failed',
          status_message: 'Deploy to staging failed: image build returned a non-zero exit.',
        }),
      );
      const review = buildSideRoom(
        'product-review',
        'Product Review',
        [MAYA, ALEX, SAM, BACKEND, QA],
        'member',
        'Weekly product review — drop artifacts before Friday.',
      );
      const design = buildSideRoom(
        'design-system',
        'Design System',
        [SAM, ALEX, MAYA, FRONTEND],
        'member',
        'Tokens v2 exploration lives here.',
      );
      // Connected-but-unknown-path fixture: PeerStatus.path is nullable while
      // state is already `connected` — the SDK knows a peer is reachable
      // before it knows how. The header must say "Connected" and never guess
      // `relay`, which is what a `connected.length > 0` fallthrough did.
      design.peers = design.peers.map((p) => ({ ...p, state: 'connected' as const, path: null }));
      const research = buildSideRoom(
        'research-lab',
        'Research Lab',
        [ALEX, RESEARCH],
        'owner',
        'P2P performance benchmarks and optimization research.',
      );
      // Crashed-runner fixture: the latest status is working-class but the
      // peer is gone — the fleet view must report `stale`, never `working`.
      research.peers = research.peers.map((p) =>
        p.endpoint_id === RESEARCH.ep ? { ...p, state: 'offline' as const, path: null } : p,
      );
      research.timeline.push(
        ev(research.room_id, at(8, 40), RESEARCH, 'agent_status', {
          label: 'working',
          status_message: 'Collecting NAT traversal benchmarks across relay paths.',
          progress: 20,
        }),
        ev(research.room_id, at(9, 10), RESEARCH, 'agent_status', {
          label: 'working',
          status_message: 'Benchmarks 12/30 done; profiling gossip fanout next.',
          progress: 45,
        }),
        ev(research.room_id, at(9, 35), RESEARCH, 'agent_status', {
          label: 'working',
          status_message: 'Profiling run 3 in flight — writing up interim notes.',
          progress: 60,
        }),
      );
      // Homonym fixture (docs/room-workbench.md, decision 6): two rooms that
      // render the SAME display name. `name` is a non-unique label, so the rail
      // rows, the fleet chips, and any destructive action must show the short
      // room id to keep them apart. Appended AFTER the rooms above so the first
      // active room (the boot-restored default) stays the MVP room. `triageA`
      // has this identity as a plain member (so its roster offers Leave);
      // `triageB` is owned here. QA is in both, so the fleet shows one agent
      // card with two identically-named chips that only the short id separates.
      const triageA = buildSideRoom(
        'bug-triage-a',
        'Bug Triage',
        [MAYA, ALEX, SAM, QA],
        'member',
        'Triage inbound bug reports here.',
      );
      const triageB = buildSideRoom(
        'bug-triage-b',
        'Bug Triage',
        [ALEX, SAM, QA],
        'owner',
        'Second triage room — same name, different room.',
      );
      for (const room of [workspace, review, design, research, triageA, triageB]) this.rooms.set(room.room_id, room);
      // Regression-suite hook (`?mock_ticket=<suffix>`): a pre-minted,
      // redeemable ticket into a room this identity has NOT joined yet (see
      // INVITED_ID), so a join success path exercises a real membership and
      // navigation transition instead of landing in an already-open room.
      if (presetTicket) {
        this.tickets.set(`roomtkt1${presetTicket}`, {
          room_id: INVITED_ID,
          identity_id: ALEX.id,
          role: 'member',
          expiresAt: null,
        });
      }
    }
  }

  describe(): string {
    return 'mock fixtures (VITE_MOCK=1) — no daemon';
  }

  start(): void {
    this.setState('connecting');
    this.startTimer = window.setTimeout(() => this.setState('connected'), 200);
  }

  stop(): void {
    if (this.startTimer !== null) window.clearTimeout(this.startTimer);
    for (const room of this.rooms.values()) {
      for (const t of room.timers) window.clearTimeout(t);
      room.timers = [];
    }
    this.setState('disconnected');
  }

  getState(): ConnectionState {
    return this.state;
  }

  onState(handler: (s: ConnectionState) => void): () => void {
    this.stateHandlers.add(handler);
    return () => this.stateHandlers.delete(handler);
  }

  on<P extends PushName>(push: P, handler: (data: PushMap[P]) => void): () => void {
    const set = this.pushHandlers[push] as Set<(data: PushMap[P]) => void>;
    set.add(handler);
    return () => set.delete(handler);
  }

  call<M extends MethodName>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']> {
    return new Promise((resolve, reject) => {
      window.setTimeout(() => {
        try {
          resolve(this.dispatch(method, params) as MethodMap[M]['result']);
        } catch (e) {
          reject(e);
        }
      }, 60 + Math.random() * 120 + (this.delaySpecs.get(method) ?? 0));
    });
  }

  // -- internals -------------------------------------------------------------

  private setState(state: ConnectionState): void {
    if (this.state === state) return;
    this.state = state;
    for (const handler of this.stateHandlers) handler(state);
  }

  private emit<P extends PushName>(push: P, data: PushMap[P]): void {
    for (const handler of this.pushHandlers[push]) handler(data);
  }

  private err(code: string, message: string, hint: string | null = null): never {
    throw new RequestError({ code, message, hint });
  }

  private needIdentity(): Identity {
    if (!this.identity) this.err('identity_missing', 'no identity on this daemon', 'run identity.create first');
    return this.identity;
  }

  private needRoom(room_id: unknown): MockRoom {
    if (typeof room_id !== 'string' || !room_id) {
      this.err('invalid_params', 'room_id is required', null);
    }
    const room = this.rooms.get(room_id);
    if (!room) this.err('room_unknown', `no room ${room_id.slice(0, 18)}… on this daemon`, 'room.list shows known rooms');
    return room;
  }

  private needOpenRoom(room_id: unknown): MockRoom {
    const room = this.needRoom(room_id);
    if (!room.open) this.err('room_not_open', `room "${room.name}" is not open`, 'call room.open first');
    return room;
  }

  private me(room: MockRoom): Person {
    const identity = this.needIdentity();
    const existing = EVERYONE.find((p) => p.id === identity.identity_id);
    const role = room.members.find((m) => m.identity_id === identity.identity_id)?.role ?? room.myRole;
    return existing
      ? { ...existing, role }
      : { id: identity.identity_id, dev: identity.device_id, ep: hex('self-endpoint', 64), role, name: 'You' };
  }

  /** Append + push (exactly once per event; pushes flow only for open rooms). */
  private ingest(room: MockRoom, event: TimelineEvent): void {
    room.timeline.push(event);
    if (room.open) {
      window.setTimeout(() => this.emit('room.event', { room_id: room.room_id, event }), 30);
    }
  }

  private pushPeers(room: MockRoom): void {
    if (room.open) this.emit('peers.changed', { room_id: room.room_id, peers: [...room.peers] });
  }

  private summary(room: MockRoom): RoomSummary {
    const identity = this.identity;
    const mine = identity ? room.members.find((m) => m.identity_id === identity.identity_id) : null;
    // Recency is the newest signed event's ts — a daemon projection
    // (docs/room-attention.md, decision 2). The real daemon does not emit this
    // yet (the identified, deferrable follow-up); the mock derives it so the
    // room-list recency slice of #64 can build against the real shape now.
    const newest = newestEvent(room.timeline);
    return {
      room_id: room.room_id,
      name: room.name,
      role: mine?.role ?? room.myRole,
      status: mine?.status ?? null,
      member_count: room.members.length,
      open: room.open,
      last_event_ts: newest?.ts ?? null,
      last_event_kind: newest?.kind ?? null,
    };
  }

  private endpointAddr(): string {
    return `${hex('self-endpoint', 64)}@127.0.0.1:52731`;
  }

  /** Live-update demo: only for the MVP room, only once per open. */
  private simulate(room: MockRoom): void {
    if (room.room_id !== MAIN_ID || room.simulated) return;
    room.simulated = true;
    const later = (ms: number, fn: () => void) => {
      room.timers.push(window.setTimeout(() => { if (room.open) fn(); }, ms));
    };
    later(6_000, () => {
      this.ingest(room, ev(room.room_id, Date.now(), BACKEND, 'agent_status', {
        label: 'working',
        status_message: 'Integration tests passing (17/24). Sync convergence suite up next.',
        progress: 72,
      }));
    });
    later(13_000, () => {
      room.peers = room.peers.map((p) => (p.endpoint_id === QA.ep ? { ...p, state: 'connecting' as const, path: null } : p));
      this.pushPeers(room);
    });
    later(19_000, () => {
      room.peers = room.peers.map((p) => (p.endpoint_id === QA.ep ? { ...p, state: 'connected' as const, path: 'relay' as const } : p));
      this.pushPeers(room);
      room.members = room.members.map((m) => (m.identity_id === QA.id ? { ...m, status: 'active' } : m));
    });
    later(26_000, () => {
      this.ingest(room, ev(room.room_id, Date.now(), BACKEND, 'agent_status', {
        label: 'tests_passed',
        status_message: 'All 24 integration tests passing. Ready for review.',
        progress: 100,
      }));
    });
  }

  /** `agents.fleet` fixture — computed live from the same evidence the daemon
   *  would use (folded events + peer state of open rooms), never hardcoded, so
   *  the dashboard's numbers stay real even as the mock simulation evolves. */
  private fleet(): FleetResult {
    const now = Date.now();
    interface Entry {
      rooms: FleetAgentRoom[];
      liveness: Liveness;
      latest: FleetAgentLatest | null;
      last_seen_ts: number | null;
    }
    const byAgent = new Map<string, Entry>();
    let rooms_covered = 0;

    for (const room of this.rooms.values()) {
      const agents = room.members.filter((m) => m.role === 'agent');
      if (agents.length > 0) rooms_covered += 1;
      for (const m of agents) {
        const entry =
          byAgent.get(m.identity_id) ??
          ({ rooms: [], liveness: 'offline', latest: null, last_seen_ts: null } as Entry);
        entry.rooms.push({ room_id: room.room_id, name: room.name });

        // Newest agent_status + newest event of any kind by this identity here.
        let latest: TimelineEvent | null = null;
        let lastSeen: number | null = null;
        for (const e of room.timeline) {
          if (e.sender.identity_id !== m.identity_id) continue;
          if (lastSeen === null || e.ts > lastSeen) lastSeen = e.ts;
          if (e.kind === 'agent_status' && (latest === null || e.ts > latest.ts)) latest = e;
        }
        if (latest && (entry.latest === null || latest.ts > entry.latest.ts)) {
          entry.latest = {
            label: latest.label ?? '',
            message: latest.status_message ?? null,
            progress: typeof latest.progress === 'number' ? latest.progress : null,
            ts: latest.ts,
            room_id: room.room_id,
          };
        }
        if (lastSeen !== null && (entry.last_seen_ts === null || lastSeen > entry.last_seen_ts)) {
          entry.last_seen_ts = lastSeen;
        }

        // Primary signal: is one of this identity's devices a connected peer
        // in a room this "daemon" has open? Non-open rooms are never online.
        const ep = EVERYONE.find((p) => p.id === m.identity_id)?.ep;
        const connected =
          room.open && ep !== undefined && room.peers.some((p) => p.endpoint_id === ep && p.state === 'connected');
        const lv = deriveLiveness(connected, latest, now);
        // Multi-room aggregate: strongest presence wins.
        if (LIVENESS_RANK[lv] < LIVENESS_RANK[entry.liveness]) entry.liveness = lv;
        byAgent.set(m.identity_id, entry);
      }
    }

    const agents: FleetAgent[] = [...byAgent.entries()].map(([identity_id, e]) => ({
      identity_id,
      rooms: e.rooms,
      liveness: e.liveness,
      latest: e.latest,
      last_seen_ts: e.last_seen_ts,
    }));
    agents.sort((a, b) => {
      const rank = LIVENESS_RANK[a.liveness] - LIVENESS_RANK[b.liveness];
      if (rank !== 0) return rank;
      const seen = (b.last_seen_ts ?? 0) - (a.last_seen_ts ?? 0);
      if (seen !== 0) return seen;
      return a.identity_id < b.identity_id ? -1 : 1;
    });

    return {
      active: agents.filter((a) => a.liveness === 'working' || a.liveness === 'online-idle').length,
      working: agents.filter((a) => a.liveness === 'working').length,
      total: agents.length,
      rooms_total: this.rooms.size,
      rooms_covered,
      agents,
    };
  }

  /** See parseFailSpecs: consumes one allowed call, then one injected
   *  failure, per configured method — after that the method recovers. */
  private maybeInjectFailure(method: MethodName): void {
    const spec = this.failSpecs.get(method);
    if (!spec || spec.failRemaining <= 0) return;
    if (spec.allowRemaining > 0) {
      spec.allowRemaining -= 1;
      return;
    }
    spec.failRemaining -= 1;
    this.err(
      'unavailable',
      `simulated ${method} failure (mock_fail)`,
      'retry — the mock recovers once the requested failures are spent',
    );
  }

  private dispatch(method: MethodName, params: unknown): unknown {
    this.maybeInjectFailure(method);
    switch (method) {
      case 'daemon.status': {
        const status: DaemonStatus = {
          version: '0.1.0-mock',
          // Must match the real daemon's contract (PROTOCOL.md "Protocol
          // version"): a client keys its version handshake on this.
          protocol: 1,
          pid: 4242,
          port: 7420,
          data_dir: '/mock/Jeliya',
          mode: 'loopback',
          identity: this.identity,
          endpoint: this.identity
            ? { endpoint_id: hex('self-endpoint', 64), addr: this.endpointAddr(), relay_url: null }
            : null,
          rooms_open: [...this.rooms.values()].filter((r) => r.open).map((r) => r.room_id),
        };
        return status;
      }

      case 'daemon.shutdown':
        return { shutting_down: true };

      case 'identity.create': {
        if (this.identity) {
          this.err('identity_exists', 'an identity already exists on this daemon', 'use daemon.status to read it');
        }
        this.identity = { identity_id: ALEX.id, device_id: ALEX.dev };
        return this.identity;
      }

      case 'room.create': {
        const p = params as MethodMap['room.create']['params'];
        this.needIdentity();
        if (!p.name || !p.name.trim()) this.err('invalid_params', 'room name must not be empty', null);
        const room_id = `blake3:${hex(`room-${p.name}-${Date.now()}`, 64)}`;
        const me = { ...ALEX, id: this.needIdentity().identity_id, dev: this.needIdentity().device_id };
        const room: MockRoom = {
          room_id,
          name: p.name.trim(),
          myRole: 'owner',
          members: [{ identity_id: me.id, role: 'owner', status: 'active' }],
          timeline: [ev(room_id, Date.now(), { ...me, role: 'owner' }, 'room_created')],
          files: [],
          pipes: [],
          peers: [],
          open: false,
          timers: [],
          simulated: false,
        };
        this.rooms.set(room_id, room);
        return { room_id };
      }

      case 'room.list': {
        if (!this.identity) return { rooms: [] };
        return { rooms: [...this.rooms.values()].map((r) => this.summary(r)) };
      }

      case 'room.open': {
        const room = this.needRoom((params as MethodMap['room.open']['params']).room_id);
        const identity = this.needIdentity();
        const mine = room.members.find((m) => m.identity_id === identity.identity_id);
        if (!mine || mine.status !== 'active') {
          this.err('not_a_member', `this identity is not an active member of "${room.name}"`, 'ask the room admin for an invite');
        }
        room.open = true;
        this.simulate(room);
        return {
          endpoint: { endpoint_id: hex('self-endpoint', 64), addr: this.endpointAddr() },
          members: [...room.members],
          timeline: [...room.timeline].sort((a, b) => a.ts - b.ts),
        };
      }

      case 'room.close': {
        const room = this.needRoom((params as MethodMap['room.close']['params']).room_id);
        room.open = false;
        room.simulated = false;
        for (const t of room.timers) window.clearTimeout(t);
        room.timers = [];
        return {};
      }

      case 'room.leave': {
        const room = this.needRoom((params as MethodMap['room.leave']['params']).room_id);
        const identity = this.needIdentity();
        const idx = room.members.findIndex((m) => m.identity_id === identity.identity_id);
        const mine = idx >= 0 ? room.members[idx] : null;
        if (!mine || mine.status !== 'active') {
          this.err('not_a_member', `this identity is not an active member of "${room.name}"`, 'ask the room admin for an invite');
        }
        if (mine.role === 'owner') {
          this.err('invalid_params', 'room owners cannot leave yet; close the local room session instead', null);
        }
        const event = ev(room.room_id, Date.now(), this.me(room), 'member_left', {
          member: { identity_id: identity.identity_id },
        });
        room.members[idx] = { ...mine, status: 'left' };
        this.ingest(room, event);
        room.open = false;
        room.simulated = false;
        for (const t of room.timers) window.clearTimeout(t);
        room.timers = [];
        return { event_id: event.event_id };
      }

      case 'room.timeline': {
        const p = params as MethodMap['room.timeline']['params'];
        const room = this.needRoom(p.room_id);
        const events = [...room.timeline].sort((a, b) => a.ts - b.ts);
        return { events: p.limit ? events.slice(-p.limit) : events };
      }

      case 'room.members': {
        const room = this.needRoom((params as MethodMap['room.members']['params']).room_id);
        return { members: [...room.members] };
      }

      case 'invite.create': {
        const p = params as MethodMap['invite.create']['params'];
        // Unlike room.open, the real daemon mints invites on CLOSED rooms too
        // (it persists member.invited directly), so use needRoom, not needOpenRoom.
        const room = this.needRoom(p.room_id);
        if (!p.identity_id || !/^[0-9a-f]{64}$/i.test(p.identity_id.trim())) {
          this.err('invalid_params', 'identity_id must be the invitee\'s hex identity id', 'ask the invitee to copy their identity id from their onboarding screen or sidebar footer');
        }
        const me = this.me(room);
        this.ingest(room, ev(room.room_id, Date.now(), me, 'member_invited', {
          member: { identity_id: p.identity_id.trim(), role: p.role },
        }));
        const ticket = `roomtkt1${base32ish(`${room.room_id}:${p.identity_id}:${Date.now()}`, 96)}`;
        // `expiry` accepts a duration string ("24h"/"3600") or a number of
        // seconds (see docs/PROTOCOL.md); no expiry means single-use, not
        // time-boxed.
        const expirySecs = parseExpirySeconds(p.expiry);
        const expiresAt = expirySecs !== null ? Date.now() + expirySecs * 1000 : null;
        this.tickets.set(ticket, { room_id: room.room_id, identity_id: p.identity_id.trim(), role: p.role, expiresAt });
        return { ticket };
      }

      case 'room.join': {
        const p = params as MethodMap['room.join']['params'];
        const identity = this.needIdentity();
        const ticket = (p.ticket ?? '').trim();
        if (!ticket.startsWith('roomtkt1') || ticket.length < 24) {
          this.err('bad_ticket', 'ticket is not a valid roomtkt1 token', 'ask the inviter for a fresh ticket');
        }
        const entry = this.tickets.get(ticket);
        if (!entry) {
          // A well-formed roomtkt1 string nobody minted on this daemon is
          // never something a real person hand-types — they only ever paste
          // one they received — so this is a genuine failure, not a demo
          // shortcut. (No fabricated room, unlike before.)
          this.err('bad_ticket', 'this ticket was never issued on this daemon', 'ask the inviter for a fresh ticket');
        }
        if (entry.expiresAt !== null && Date.now() > entry.expiresAt) {
          this.tickets.delete(ticket);
          this.err('ticket_expired', 'this ticket has expired', 'ask the inviter to generate a fresh one');
        }
        let room = this.rooms.get(entry.room_id);
        if (!room && entry.room_id === INVITED_ID) {
          // First redemption of the preset regression-suite ticket:
          // materialize the room locally, like a real join bootstrapping a
          // room this daemon has never seen.
          room = buildSideRoom(
            'invited-workspace',
            'Invited Workspace',
            [MAYA, SAM],
            'member',
            'Welcome — this room reached you as a ticket.',
          );
          this.rooms.set(room.room_id, room);
        }
        if (!room) this.err('room_unknown', 'the invited room no longer exists on this daemon', 'ask the inviter for a fresh ticket');
        // Single-use: redeeming the same ticket twice should fail like a real
        // spent one, not silently succeed again.
        this.tickets.delete(ticket);
        const existing = room.members.find((m) => m.identity_id === identity.identity_id);
        if (!existing) {
          room.members.push({ identity_id: identity.identity_id, role: entry.role, status: 'active' });
          this.ingest(room, ev(room.room_id, Date.now(), { ...ALEX, id: identity.identity_id, dev: identity.device_id, role: entry.role }, 'member_joined', {
            member: { identity_id: identity.identity_id, role: entry.role },
          }));
        } else if (existing.status !== 'active') {
          // A fresh ticket re-admits an identity that left or was removed;
          // keeping status 'left' after a successful join would strand the
          // UI in a departed room (the real daemon publishes a new
          // member_joined on re-admission).
          existing.status = 'active';
          existing.role = entry.role;
          this.ingest(room, ev(room.room_id, Date.now(), { ...ALEX, id: identity.identity_id, dev: identity.device_id, role: entry.role }, 'member_joined', {
            member: { identity_id: identity.identity_id, role: entry.role },
          }));
        }
        return { room_id: room.room_id };
      }

      case 'message.send': {
        const p = params as MethodMap['message.send']['params'];
        const room = this.needOpenRoom(p.room_id);
        if (!p.body || !p.body.trim()) this.err('invalid_params', 'message body must not be empty', null);
        const event = ev(room.room_id, Date.now(), this.me(room), 'message', { body: p.body });
        this.ingest(room, event);
        return { event_id: event.event_id };
      }

      case 'status.post': {
        const p = params as MethodMap['status.post']['params'];
        const room = this.needOpenRoom(p.room_id);
        if (!p.label) this.err('invalid_params', 'label is required', null);
        const event = ev(room.room_id, Date.now(), this.me(room), 'agent_status', {
          label: p.label,
          ...(p.message !== undefined ? { status_message: p.message } : {}),
          ...(p.progress !== undefined ? { progress: p.progress } : {}),
          ...(p.artifacts && p.artifacts.length > 0 ? { artifacts: p.artifacts } : {}),
        });
        this.ingest(room, event);
        return { event_id: event.event_id };
      }

      case 'file.share': {
        const p = params as MethodMap['file.share']['params'];
        const room = this.needOpenRoom(p.room_id);
        if (!p.path || !p.path.trim()) this.err('invalid_params', 'path is required', null);
        const path = p.path.trim();
        const name = p.name?.trim() || path.split('/').pop() || path;
        const mimeByExt: Record<string, string> = {
          pdf: 'application/pdf', md: 'text/markdown', txt: 'text/plain',
          json: 'application/json', png: 'image/png', jpg: 'image/jpeg',
        };
        const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : '';
        const mime = p.mime ?? mimeByExt[ext] ?? 'application/octet-stream';
        const size = 24_000 + (path.length * 137) % 900_000;
        const file_id = fid(`${path}-${Date.now()}`);
        const now = Date.now();
        room.files.push({
          file_id, name, size, mime,
          sender_id: this.needIdentity().identity_id,
          ts: now, available: false, providers: 1,
        });
        const event = ev(room.room_id, now, this.me(room), 'file_shared', {
          file: { file_id, name, size, mime },
        });
        this.ingest(room, event);
        return { file_id, event_id: event.event_id };
      }

      case 'file.list': {
        const room = this.needRoom((params as MethodMap['file.list']['params']).room_id);
        return { files: [...room.files] };
      }

      case 'file.fetch': {
        const p = params as MethodMap['file.fetch']['params'];
        const room = this.needOpenRoom(p.room_id);
        const file = room.files.find((f) => f.file_id === p.file_id);
        if (!file) this.err('file_unavailable', 'unknown file_id for this room', 'file.list shows shareable files');
        // Simulate the transfer: resolve/reject after a delay.
        return new Promise((resolve, reject) => {
          window.setTimeout(() => {
            if (!file.available) {
              reject(new RequestError({
                code: 'file_unavailable',
                message: 'no connected peer is currently providing these bytes',
                hint: `providers seen: ${file.providers} (offline) — retry when the sender is online`,
              }));
              return;
            }
            // Default matches the daemon: <data_dir>/downloads (PROTOCOL.md).
            const dir = p.save_dir ?? '/mock/Jeliya/downloads';
            const path = `${dir}/${file.name}`;
            file.fetched = true;
            file.local_path = path;
            file.local_bytes = file.size;
            file.fetched_at_ms = Date.now();
            resolve({ path, bytes: file.size, verified: true as const });
          }, 900 + Math.random() * 500);
        });
      }

      case 'pipe.expose': {
        const p = params as MethodMap['pipe.expose']['params'];
        const room = this.needOpenRoom(p.room_id);
        if (!p.target || !/^[\w.-]+:\d+$/.test(p.target.trim())) {
          this.err('invalid_params', 'target must look like 127.0.0.1:3000', null);
        }
        if (!p.peer_identity) {
          this.err('invalid_params', 'peer_identity is required — a pipe has exactly one authorized peer', null);
        }
        const pipe_id = pid(`pipe-${p.target}-${Date.now()}`);
        const me = this.me(room);
        room.pipes.push({
          pipe_id,
          target: p.target.trim(),
          opened_by: me.id,
          authorized_peer: p.peer_identity,
          state: 'open',
          connected: false,
        });
        const event = ev(room.room_id, Date.now(), me, 'pipe_opened', {
          pipe: { pipe_id, target: p.target.trim(), authorized_peer: p.peer_identity },
        });
        this.ingest(room, event);
        return { pipe_id, event_id: event.event_id };
      }

      case 'pipe.list': {
        const room = this.needRoom((params as MethodMap['pipe.list']['params']).room_id);
        return { pipes: [...room.pipes] };
      }

      case 'pipe.connect': {
        const p = params as MethodMap['pipe.connect']['params'];
        const room = this.needOpenRoom(p.room_id);
        const pipe = room.pipes.find((x) => x.pipe_id === p.pipe_id);
        if (!pipe) this.err('pipe_denied', 'unknown pipe for this room', 'pipe.list shows live pipes');
        if (pipe.state === 'closed') this.err('pipe_denied', 'pipe is closed', 'ask the owner to expose it again');
        const myId = this.needIdentity().identity_id;
        if (pipe.authorized_peer !== myId && pipe.opened_by !== myId) {
          this.err('pipe_denied', 'you are not the authorized peer for this pipe', null);
        }
        pipe.connected = true;
        this.portSeq += 1;
        return { local_addr: `127.0.0.1:${this.portSeq}` };
      }

      case 'pipe.close': {
        const p = params as MethodMap['pipe.close']['params'];
        const room = this.needOpenRoom(p.room_id);
        const pipe = room.pipes.find((x) => x.pipe_id === p.pipe_id);
        if (!pipe) this.err('pipe_denied', 'unknown pipe for this room', 'pipe.list shows live pipes');
        pipe.state = 'closed';
        pipe.connected = false;
        // A pipe_closed event nulls both target and authorized_peer, matching
        // the daemon's materializer (see PROTOCOL.md TimelineEvent field notes).
        const event = ev(room.room_id, Date.now(), this.me(room), 'pipe_closed', {
          pipe: { pipe_id: pipe.pipe_id, target: null, authorized_peer: null },
        });
        this.ingest(room, event);
        return { event_id: event.event_id };
      }

      case 'peers.status': {
        const room = this.needRoom((params as MethodMap['peers.status']['params']).room_id);
        return { peers: [...room.peers] };
      }

      case 'agents.fleet': {
        this.needIdentity();
        return this.fleet();
      }

      case 'agent.history': {
        const p = params as MethodMap['agent.history']['params'];
        const room = this.needRoom(p.room_id);
        if (typeof p.identity_id !== 'string' || !/^[0-9a-f]{64}$/i.test(p.identity_id.trim())) {
          this.err('invalid_params', 'identity_id must be a hex identity id', null);
        }
        const id = p.identity_id.trim();
        const limit = typeof p.limit === 'number' && p.limit > 0 ? Math.floor(p.limit) : 100;
        // One point per real agent_status event, chronological — no interpolation.
        const points = room.timeline
          .filter((e) => e.kind === 'agent_status' && e.sender.identity_id === id)
          .sort((a, b) => a.ts - b.ts)
          .slice(-limit)
          .map((e) => ({
            ts: e.ts,
            label: e.label ?? '',
            progress: typeof e.progress === 'number' ? e.progress : null,
          }));
        return { points };
      }

      default:
        this.err('invalid_params', `unknown method ${String(method)}`, null);
    }
  }
}

export function createMockClient(): Client {
  const params = new URLSearchParams(window.location.search);
  const fresh = params.get('mock') === 'fresh';
  return new MockClient(
    fresh,
    parseFailSpecs(params.get('mock_fail')),
    parseDelaySpecs(params.get('mock_delay')),
    params.get('mock_ticket'),
  );
}
