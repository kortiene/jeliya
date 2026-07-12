// Wire contract between jeliyad and this shell.
// Mirrors docs/PROTOCOL.md (v1) exactly — that document is binding.

export type Role = 'owner' | 'member' | 'agent';

export interface Sender {
  identity_id: string;
  device_id: string;
  role: Role;
}

export type TimelineKind =
  | 'room_created'
  | 'member_invited'
  | 'member_joined'
  | 'member_left'
  | 'message'
  | 'agent_status'
  | 'file_shared'
  | 'pipe_opened'
  | 'pipe_closed';

export interface FileRef {
  file_id: string;
  name: string;
  size: number;
  mime: string;
}

export interface PipeRef {
  pipe_id: string;
  /** null on a pipe_closed event. */
  target: string | null;
  /** null when no peer is authorized; a comma-joined list for multi-identity
   *  authorization; null on a pipe_closed event. */
  authorized_peer: string | null;
}

export interface MemberRef {
  identity_id: string;
  role?: Role;
}

/** One validated room event, folded for display. Kind-specific fields are
 *  present only for that kind. */
export interface TimelineEvent {
  event_id: string;
  room_id: string;
  ts: number;
  sender: Sender;
  kind: TimelineKind;
  /** kind: message */
  body?: string;
  /** kind: agent_status */
  label?: string;
  status_message?: string;
  progress?: number;
  artifacts?: string[];
  /** kind: file_shared */
  file?: FileRef;
  /** kind: pipe_opened / pipe_closed */
  pipe?: PipeRef;
  /** kind: member_invited / member_joined / member_left */
  member?: MemberRef;
}

export type PeerState = 'connected' | 'connecting' | 'offline';
export type PeerPath = 'direct' | 'relay' | null;

export interface PeerStatus {
  endpoint_id: string;
  state: PeerState;
  path: PeerPath;
  /** The peer's membership identity, once the SDK has bound the device (on admit); null before/during admission. */
  identity_id: string | null;
}

export interface Identity {
  identity_id: string;
  device_id: string;
}

export interface EndpointInfo {
  endpoint_id: string;
  /** Dialable `<endpoint_id>@<ip:port>` string when known, else null. */
  addr: string | null;
  relay_url: string | null;
}

export interface DaemonStatus {
  version: string;
  /** Major protocol version spoken on /ws (docs/PROTOCOL.md). A client reads
   *  this before assuming a contract; see the "Protocol version" section. */
  protocol: number;
  pid: number;
  port: number;
  data_dir: string;
  mode: 'loopback' | 'real';
  identity: Identity | null;
  endpoint: EndpointInfo | null;
  rooms_open: string[];
}

export interface RoomSummary {
  room_id: string;
  /** null for a joined room whose genesis (name-bearing) event has not synced. */
  name: string | null;
  /** Compatibility-nullable for older protocol-v1 daemons; v0.5 requires an
   *  identity and emits a role for every authorized room.list row. */
  role: Role | null;
  /** Compatibility-nullable for older protocol-v1 daemons; v0.5 emits the
   *  local member's active/left/removed status for every listed room. */
  status: string | null;
  member_count: number;
  open: boolean;
}

export interface Member {
  identity_id: string;
  role: Role;
  status: string;
}

export interface FileEntry {
  file_id: string;
  name: string;
  size: number;
  mime: string;
  sender_id: string;
  ts: number;
  available: boolean;
  providers: number;
  fetched?: boolean;
  local_path?: string | null;
  local_bytes?: number | null;
  fetched_at_ms?: number | null;
}

export type PipeState = 'open' | 'closed';

export interface PipeEntry {
  pipe_id: string;
  target: string;
  opened_by: string;
  /** null when no peer is authorized; a comma-joined list for multi-identity. */
  authorized_peer: string | null;
  state: PipeState;
  connected: boolean;
}

// -- fleet reads (docs/agent-orchestration.md §3, PROTOCOL.md "Agents") -------

/** Derived at read time from real peer state + real events — never stored,
 *  never extrapolated. `working` requires a connected peer AND freshness. */
export type Liveness = 'online-idle' | 'working' | 'offline' | 'stale';

export interface FleetAgentRoom {
  room_id: string;
  name: string | null;
}

export interface FleetAgentLatest {
  label: string;
  message: string | null;
  progress: number | null;
  ts: number;
  room_id: string;
}

export interface FleetAgent {
  identity_id: string;
  rooms: FleetAgentRoom[];
  liveness: Liveness;
  /** Newest agent_status by this identity across its rooms, or null if it has
   *  never posted one. */
  latest: FleetAgentLatest | null;
  /** ts of the newest event of any kind by this identity — an event
   *  timestamp, never "now". */
  last_seen_ts: number | null;
}

export interface FleetResult {
  active: number;
  working: number;
  total: number;
  rooms_total: number;
  rooms_covered: number;
  agents: FleetAgent[];
}

/** One point per real agent_status event — no interpolation. */
export interface HistoryPoint {
  ts: number;
  label: string;
  progress: number | null;
}

/** `error` object of a failed response. `code` mirrors the SDK/CLI taxonomy. */
export interface DaemonErrorShape {
  code: string;
  message: string;
  hint: string | null;
}

export class RequestError extends Error {
  code: string;
  hint: string | null;
  constructor(err: DaemonErrorShape) {
    super(err.message);
    this.name = 'RequestError';
    this.code = err.code;
    this.hint = err.hint ?? null;
  }
}

export function errorShape(e: unknown): DaemonErrorShape {
  if (e instanceof RequestError) return { code: e.code, message: e.message, hint: e.hint };
  return { code: 'internal', message: e instanceof Error ? e.message : String(e), hint: null };
}

/** Every method in PROTOCOL.md with its params and result shapes. */
export interface MethodMap {
  'daemon.status': { params: Record<string, never>; result: DaemonStatus };
  'daemon.shutdown': { params: Record<string, never>; result: { shutting_down: true } };
  'identity.create': { params: Record<string, never>; result: Identity };
  'room.create': { params: { name: string }; result: { room_id: string } };
  'room.list': { params: Record<string, never>; result: { rooms: RoomSummary[] } };
  'room.open': {
    /** `peers` are optional dial hints (`<endpoint_id>@<ip:port>`) merged into
     *  the room's persisted hint set, same shape as room.join. */
    params: { room_id: string; peers?: string[] };
    result: {
      endpoint: { endpoint_id: string; addr: string | null };
      members: Member[];
      timeline: TimelineEvent[];
    };
  };
  'room.close': { params: { room_id: string }; result: Record<string, never> };
  'room.leave': { params: { room_id: string }; result: { event_id: string } };
  'room.timeline': { params: { room_id: string; limit?: number }; result: { events: TimelineEvent[] } };
  'room.members': { params: { room_id: string }; result: { members: Member[] } };
  'invite.create': {
    /** `expiry` accepts a duration string like "24h" / "3600" or a bare number
     *  of seconds; omitted means no expiry. */
    params: { room_id: string; identity_id: string; role: 'member' | 'agent'; expiry?: number | string };
    result: { ticket: string };
  };
  'room.join': { params: { ticket: string; name?: string; peers?: string[] }; result: { room_id: string } };
  'message.send': { params: { room_id: string; body: string }; result: { event_id: string } };
  'status.post': {
    params: { room_id: string; label: string; message?: string; progress?: number; artifacts?: string[] };
    result: { event_id: string };
  };
  'file.share': {
    params: { room_id: string; path: string; name?: string; mime?: string };
    result: { file_id: string; event_id: string };
  };
  'file.list': { params: { room_id: string }; result: { files: FileEntry[] } };
  'file.fetch': {
    params: { room_id: string; file_id: string; save_dir?: string };
    result: { path: string; bytes: number; verified: true };
  };
  'pipe.expose': {
    params: { room_id: string; target: string; peer_identity: string };
    result: { pipe_id: string; event_id: string };
  };
  'pipe.list': { params: { room_id: string }; result: { pipes: PipeEntry[] } };
  'pipe.connect': { params: { room_id: string; pipe_id: string }; result: { local_addr: string } };
  'pipe.close': { params: { room_id: string; pipe_id: string }; result: { event_id: string } };
  'peers.status': { params: { room_id: string }; result: { peers: PeerStatus[] } };
  'agents.fleet': { params: Record<string, never>; result: FleetResult };
  'agent.history': {
    params: { room_id: string; identity_id: string; limit?: number };
    result: { points: HistoryPoint[] };
  };
}

export type MethodName = keyof MethodMap;

export interface PushMap {
  'room.event': { room_id: string; event: TimelineEvent };
  'peers.changed': { room_id: string; peers: PeerStatus[] };
}

export type PushName = keyof PushMap;

export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'disconnected';

/** Shared surface of the real WebSocket client and the mock fixture client. */
export interface Client {
  start(): void;
  stop(): void;
  getState(): ConnectionState;
  onState(handler: (state: ConnectionState) => void): () => void;
  on<P extends PushName>(push: P, handler: (data: PushMap[P]) => void): () => void;
  call<M extends MethodName>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']>;
  /** Human-readable transport description for status surfaces. */
  describe(): string;
}
