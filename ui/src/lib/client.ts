// Typed WebSocket client for the jeliyad protocol (docs/PROTOCOL.md).
//
// - request/response correlated by numeric `id`, surfaced as promises
// - push frames (`room.event`, `peers.changed`) fan out to subscribers
// - auto-reconnect with exponential backoff + jitter
// - connection state exposed and observable
//
// Daemon URL: ws://127.0.0.1:7420/ws, overridable with `?daemon=<port>`.

import type {
  Client,
  ConnectionState,
  DaemonErrorShape,
  MethodMap,
  MethodName,
  PushMap,
  PushName,
} from './protocol';
import { RequestError } from './protocol';
import { createMockClient } from './mock';

export const DEFAULT_DAEMON_URL = 'ws://127.0.0.1:7420/ws';

export function daemonUrl(search: string = window.location.search): string {
  const value = new URLSearchParams(search).get('daemon');
  if (value) {
    if (/^\d+$/.test(value)) return `ws://127.0.0.1:${value}/ws`;
    // Escape hatch: a full ws:// URL is accepted too.
    if (/^wss?:\/\//.test(value)) return value;
    return DEFAULT_DAEMON_URL;
  }
  // In a production build the daemon serves this SPA from its own loopback
  // origin, so the control socket is same-origin: derive it from the page host
  // (and matching ws/wss scheme). This tracks the daemon's actual port for free
  // — including a port-collision fallback — with no hardcoded value. The Vite
  // dev server serves the UI on a different origin than the daemon, so there we
  // keep the fixed default instead.
  if (import.meta.env.PROD && typeof window !== 'undefined' && window.location.host) {
    const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
    return `${scheme}://${window.location.host}/ws`;
  }
  return DEFAULT_DAEMON_URL;
}

export function daemonHttpBase(search: string = window.location.search): string {
  const ws = new URL(daemonUrl(search));
  ws.protocol = ws.protocol === 'wss:' ? 'https:' : 'http:';
  ws.pathname = '/';
  ws.search = '';
  ws.hash = '';
  return ws.toString().replace(/\/$/, '');
}

// ---------------------------------------------------------------------------
// Daemon auth token (docs/PROTOCOL.md, "Process supervision")
//
// The daemon requires a per-start token on /ws and /api/files/*. The browser
// UI is handed it by GET /api/session (served only to loopback-Origin browser
// requests); native clients read it from the portfile instead. The token is
// re-fetched on every connect attempt so a daemon restart (new token) heals
// through the normal reconnect loop.
// ---------------------------------------------------------------------------

let lastToken: string | null = null;

/** The most recently fetched daemon token (for sync URL building). */
export function daemonToken(): string | null {
  return lastToken;
}

export async function fetchDaemonToken(): Promise<string | null> {
  try {
    const response = await fetch(new URL('/api/session', daemonHttpBase()), { cache: 'no-store' });
    if (!response.ok) return null;
    const payload = (await response.json()) as { token?: unknown };
    if (typeof payload.token === 'string' && payload.token) {
      lastToken = payload.token;
      return payload.token;
    }
    return null;
  } catch {
    return null;
  }
}

export async function uploadFileToRoom(roomId: string, file: File): Promise<{ file_id: string; event_id: string }> {
  const url = new URL('/api/files/share', daemonHttpBase());
  url.searchParams.set('room_id', roomId);
  url.searchParams.set('name', file.name || 'upload.bin');
  if (file.type) url.searchParams.set('mime', file.type);
  // Always re-fetch: a cached token can be stale after a daemon restart (new
  // per-start token), which would 401 a healthy daemon. Fall back to the cached
  // one only if the refetch fails.
  const token = (await fetchDaemonToken()) ?? daemonToken();
  if (token) url.searchParams.set('token', token);
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': file.type || 'application/octet-stream' },
    body: file,
  });
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    throw new RequestError({
      code: 'internal',
      message: `upload failed with HTTP ${response.status}`,
      hint: 'is jeliyad serving the local UI endpoint?',
    });
  }
  const envelope = payload as {
    ok?: boolean;
    result?: { file_id?: string; event_id?: string };
    error?: Partial<DaemonErrorShape>;
  };
  if (!response.ok || envelope.ok !== true) {
    const err = envelope.error ?? {};
    throw new RequestError({
      code: err.code ?? 'internal',
      message: err.message ?? `upload failed with HTTP ${response.status}`,
      hint: err.hint ?? null,
    });
  }
  if (!envelope.result?.file_id || !envelope.result.event_id) {
    throw new RequestError({ code: 'internal', message: 'upload response was missing file_id', hint: null });
  }
  return { file_id: envelope.result.file_id, event_id: envelope.result.event_id };
}

interface Pending {
  resolve(value: never): void;
  reject(error: unknown): void;
}

interface QueuedFrame {
  id: number;
  frame: string;
}

const BACKOFF_BASE_MS = 500;
const BACKOFF_MAX_MS = 8_000;

export class WsClient implements Client {
  private readonly url: string;
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  /** ids of requests actually written to the current socket */
  private sent = new Set<number>();
  /** frames waiting for the socket to open */
  private queue: QueuedFrame[] = [];
  private pushHandlers: { [P in PushName]: Set<(data: PushMap[P]) => void> } = {
    'room.event': new Set(),
    'peers.changed': new Set(),
  };
  private stateHandlers = new Set<(state: ConnectionState) => void>();
  private state: ConnectionState = 'disconnected';
  private attempts = 0;
  private reconnectTimer: number | null = null;
  private stopped = true;
  /** Generation counter so a stale async open attempt (token fetch in flight
   *  across a stop()/start() cycle) can never install its socket. */
  private openSeq = 0;

  constructor(url: string = daemonUrl()) {
    this.url = url;
  }

  describe(): string {
    return this.url;
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.attempts = 0;
    this.open('connecting');
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer !== null) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    this.failInFlight('client stopped');
    // Reject requests that were queued but never written to a socket (issued
    // while the socket was down), so their callers don't hang forever.
    this.failQueued('client stopped');
    this.setState('disconnected');
  }

  getState(): ConnectionState {
    return this.state;
  }

  onState(handler: (state: ConnectionState) => void): () => void {
    this.stateHandlers.add(handler);
    return () => this.stateHandlers.delete(handler);
  }

  on<P extends PushName>(push: P, handler: (data: PushMap[P]) => void): () => void {
    const set = this.pushHandlers[push] as Set<(data: PushMap[P]) => void>;
    set.add(handler);
    return () => set.delete(handler);
  }

  call<M extends MethodName>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']> {
    return new Promise<MethodMap[M]['result']>((resolve, reject) => {
      if (this.stopped) {
        reject(new RequestError({ code: 'connection_lost', message: 'client is stopped', hint: null }));
        return;
      }
      const id = this.nextId++;
      this.pending.set(id, { resolve: resolve as (value: never) => void, reject });
      const frame = JSON.stringify({ id, method, params });
      if (this.ws && this.ws.readyState === WebSocket.OPEN) {
        this.ws.send(frame);
        this.sent.add(id);
      } else {
        // Not open yet (connecting / between reconnect attempts): queue and
        // flush on open so callers can fire requests right after start().
        this.queue.push({ id, frame });
      }
    });
  }

  private setState(state: ConnectionState): void {
    if (this.state === state) return;
    this.state = state;
    for (const handler of this.stateHandlers) handler(state);
  }

  private open(openingState: ConnectionState): void {
    this.reconnectTimer = null;
    this.setState(openingState);
    void this.openWithToken();
  }

  private async openWithToken(): Promise<void> {
    const seq = ++this.openSeq;
    // Fetch a fresh token every attempt: it costs one loopback GET and makes a
    // daemon restart (which mints a new token) heal automatically. A null
    // token still attempts the connect so the failure surfaces through the
    // normal onclose → reconnect path.
    const token = await fetchDaemonToken();
    if (this.stopped || seq !== this.openSeq) return;
    let url = this.url;
    if (token) {
      try {
        const withToken = new URL(this.url);
        withToken.searchParams.set('token', token);
        url = withToken.toString();
      } catch {
        // keep the raw URL; the daemon will refuse and we retry
      }
    }
    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;

    ws.onopen = () => {
      if (ws !== this.ws) return;
      this.attempts = 0;
      const queued = this.queue;
      this.queue = [];
      for (const { id, frame } of queued) {
        ws.send(frame);
        this.sent.add(id);
      }
      this.setState('connected');
    };

    ws.onmessage = (event) => {
      if (ws !== this.ws) return;
      this.handleFrame(String(event.data));
    };

    ws.onclose = () => {
      if (ws !== this.ws) return;
      this.ws = null;
      this.failInFlight('connection to daemon lost');
      if (!this.stopped) this.scheduleReconnect();
    };

    ws.onerror = () => {
      // onclose always follows; nothing to do here.
    };
  }

  private scheduleReconnect(): void {
    const backoff = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** this.attempts);
    const delay = backoff + Math.random() * 250;
    this.attempts += 1;
    this.setState('reconnecting');
    this.reconnectTimer = window.setTimeout(() => this.open('reconnecting'), delay);
  }

  /** Reject requests that were written to a socket that just died. Queued
   *  (never-sent) requests stay queued for the next connection. */
  private failInFlight(message: string): void {
    const error = new RequestError({
      code: 'connection_lost',
      message,
      hint: 'is jeliyad running? start it, or pass ?daemon=<port>',
    });
    for (const id of this.sent) {
      const pending = this.pending.get(id);
      if (pending) {
        this.pending.delete(id);
        pending.reject(error);
      }
    }
    this.sent.clear();
  }

  /** Reject requests that were queued (never written to a socket) and drop the
   *  queue — used by stop() so a caller awaiting a request issued while the
   *  socket was down does not hang forever. */
  private failQueued(message: string): void {
    const error = new RequestError({
      code: 'connection_lost',
      message,
      hint: 'is jeliyad running? start it, or pass ?daemon=<port>',
    });
    for (const { id } of this.queue) {
      const pending = this.pending.get(id);
      if (pending) {
        this.pending.delete(id);
        pending.reject(error);
      }
    }
    this.queue = [];
  }

  private handleFrame(raw: string): void {
    let msg: unknown;
    try {
      msg = JSON.parse(raw);
    } catch {
      return; // not JSON — ignore
    }
    if (typeof msg !== 'object' || msg === null) return;
    const frame = msg as Record<string, unknown>;

    if (typeof frame.push === 'string') {
      const push = frame.push as PushName;
      const handlers = this.pushHandlers[push];
      if (handlers) {
        for (const handler of handlers) {
          (handler as (data: unknown) => void)(frame.data);
        }
      }
      return;
    }

    if (typeof frame.id === 'number') {
      const pending = this.pending.get(frame.id);
      if (!pending) return; // late reply for a request we already gave up on
      this.pending.delete(frame.id);
      this.sent.delete(frame.id);
      if (frame.ok === true) {
        (pending.resolve as (value: unknown) => void)(frame.result);
      } else {
        const err = (frame.error ?? {}) as Partial<DaemonErrorShape>;
        pending.reject(
          new RequestError({
            code: err.code ?? 'internal',
            message: err.message ?? 'request failed',
            hint: err.hint ?? null,
          }),
        );
      }
    }
  }
}

/** VITE_MOCK=1 swaps in the in-memory fixture client. */
export function createClient(): Client {
  if (import.meta.env.VITE_MOCK === '1') return createMockClient();
  return new WsClient();
}
