// @vitest-environment node
//
// Conformance corpus replayed against the REAL daemon over WebSocket. Spawns
// jeliyad in loopback mode, reads its portfile token (Phase 0 supervision
// contract), and drives it through a minimal Node WS adapter implementing the
// Client interface. Skips automatically if the binary is not built, so the
// mock suite still runs in a JS-only environment.

import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

import type { Client, MethodMap, MethodName, PushMap, PushName } from '../protocol';
import { replayScenario, type Scenario } from './harness';
import corpus from './corpus.json';

const BINARY = resolve(process.cwd(), '..', 'target', 'debug', 'jeliyad');
const scenarios = (corpus.scenarios as Scenario[]).filter((s) => !s.tags?.includes('mockOnly'));

/** Minimal Node WebSocket client implementing the Client surface replay needs. */
class WsClientNode implements Client {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void }>();
  private handlers: { [P in PushName]: Set<(data: PushMap[P]) => void> } = {
    'room.event': new Set(),
    'peers.changed': new Set(),
  };

  constructor(private readonly url: string) {}

  connect(): Promise<void> {
    return new Promise((res, rej) => {
      const ws = new WebSocket(this.url);
      this.ws = ws;
      ws.onopen = () => res();
      ws.onerror = () => rej(new Error(`ws connect failed: ${this.url}`));
      ws.onmessage = (ev) => this.onFrame(String((ev as MessageEvent).data));
    });
  }

  private onFrame(raw: string): void {
    const frame = JSON.parse(raw) as Record<string, unknown>;
    if (typeof frame.push === 'string') {
      const set = this.handlers[frame.push as PushName];
      if (set) for (const h of set) (h as (d: unknown) => void)(frame.data);
      return;
    }
    if (typeof frame.id === 'number') {
      const waiter = this.pending.get(frame.id);
      if (!waiter) return;
      this.pending.delete(frame.id);
      if (frame.ok === true) waiter.resolve(frame.result);
      else waiter.reject(frame.error ?? { code: 'internal' });
    }
  }

  start(): void {}
  stop(): void {
    this.ws?.close();
  }
  getState() {
    return 'connected' as const;
  }
  onState() {
    return () => undefined;
  }
  describe() {
    return this.url;
  }
  on<P extends PushName>(push: P, handler: (data: PushMap[P]) => void): () => void {
    const set = this.handlers[push] as Set<(data: PushMap[P]) => void>;
    set.add(handler);
    return () => set.delete(handler);
  }
  call<M extends MethodName>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']> {
    return new Promise((res, rej) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve: res as (v: unknown) => void, reject: rej });
      this.ws?.send(JSON.stringify({ id, method, params }));
    });
  }
}

interface DaemonOracle {
  daemon: ChildProcess;
  dataDir: string;
  client: WsClientNode;
}

async function startDaemonOracle(withIdentity: boolean): Promise<DaemonOracle> {
  const dataDir = mkdtempSync(join(tmpdir(), 'jeliya-conf-'));
  const daemon = spawn(BINARY, ['--supervised', '--loopback', '--port', '0', '--data-dir', dataDir], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const ready = await new Promise<{ port: number }>((res, rej) => {
    let buf = '';
    const timer = setTimeout(() => rej(new Error('no ready line in 15s')), 15000);
    daemon.stdout!.on('data', (d: Buffer) => {
      buf += String(d);
      for (const line of buf.split('\n')) {
        const t = line.trim();
        if (!t.startsWith('{')) continue;
        try {
          const j = JSON.parse(t) as { event?: string; port?: number };
          if (j.event === 'ready' && typeof j.port === 'number') {
            clearTimeout(timer);
            res({ port: j.port });
            return;
          }
        } catch {
          /* keep reading */
        }
      }
    });
  });
  const portfile = JSON.parse(readFileSync(join(dataDir, 'daemon.json'), 'utf8')) as { auth_token: string };
  const client = new WsClientNode(`ws://127.0.0.1:${ready.port}/ws?token=${portfile.auth_token}`);
  await client.connect();
  if (withIdentity) await client.call('identity.create', {});
  return { daemon, dataDir, client };
}

function stopDaemonOracle(oracle: DaemonOracle | undefined): void {
  if (!oracle) return;
  oracle.client.stop();
  try {
    oracle.daemon.kill('SIGKILL');
  } catch {
    /* already gone */
  }
  rmSync(oracle.dataDir, { recursive: true, force: true });
}

const haveBinary = existsSync(BINARY);
// A silently-skipped conformance suite is worse than none — it reads as green
// while covering nothing. In CI (or when JELIYA_REQUIRE_DAEMON is set) a missing
// binary is a hard failure; locally it skips with a warning.
const requireDaemon = !!process.env.CI || !!process.env.JELIYA_REQUIRE_DAEMON;
if (!haveBinary && requireDaemon) {
  describe('conformance: real daemon', () => {
    it('daemon binary is built', () => {
      throw new Error(`${BINARY} not found — run \`cargo build\` before the conformance suite`);
    });
  });
}
const suite = haveBinary ? describe : describe.skip;

suite('conformance: real daemon', () => {
  let oracle: DaemonOracle;

  beforeAll(async () => {
    oracle = await startDaemonOracle(true);
  }, 30000);

  afterAll(() => {
    stopDaemonOracle(oracle);
  });

  for (const scenario of scenarios) {
    it(scenario.name, async () => {
      const preIdentity = scenario.tags?.includes('preIdentity') ?? false;
      const fresh = preIdentity ? await startDaemonOracle(false) : undefined;
      try {
        const results = await replayScenario(fresh?.client ?? oracle.client, scenario, 3000);
        const failures = results.filter((r) => !r.ok);
        expect(failures, JSON.stringify(failures, null, 2)).toEqual([]);
      } finally {
        stopDaemonOracle(fresh);
      }
    }, 30000);
  }
});

if (!haveBinary) {
  // Surface why the daemon oracle was skipped instead of silently passing.
  // eslint-disable-next-line no-console
  console.warn(`[conformance] daemon suite skipped: ${BINARY} not built (run \`cargo build\`)`);
}
