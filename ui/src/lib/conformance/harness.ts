// Transport-agnostic conformance harness (docs/PROTOCOL.md, Phase 1).
//
// A single corpus of scenarios is replayed against ANY implementation of the
// `Client` interface — the real daemon (over WebSocket) and the in-memory
// mock — and asserted at the ENVELOPE level, never at the WebSocket level, so
// the same vectors will later validate a Dart client too. Nondeterministic
// scalars (ids, timestamps, ports, paths, addrs) are normalized to type tags
// before comparison; see `normalize`.

import type { Client, MethodName, PushName } from '../protocol';

// -- normalization -----------------------------------------------------------

const HEX64 = /^[0-9a-f]{64}$/i;
const HEX32 = /^[0-9a-f]{32}$/i;

/** Replace a dynamic scalar with a stable type tag so two conformant oracles
 *  compare equal despite different ids/timestamps. `key` is the parent object
 *  key, used for the few fields whose *value space* is volatile but whose type
 *  is fixed (pid, port, version, data_dir). */
export function normalize(value: unknown, key?: string): unknown {
  if (Array.isArray(value)) return value.map((v) => normalize(v));
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) out[k] = normalize(v, k);
    return out;
  }
  if (typeof value === 'number') {
    if (key === 'ts' || key === 'last_seen_ts' || key === 'fetched_at_ms' || key === 'started_at_ms') return '<ts>';
    if (key === 'pid' || key === 'port') return '<number>';
    if (key === 'size' || key === 'member_count' || key === 'providers' || key === 'bytes' || key === 'local_bytes') return '<number>';
    return value; // stable numbers (protocol:1, progress:60) compare literally
  }
  if (typeof value === 'string') {
    if (key === 'version') return '<version>';
    if (key === 'data_dir' || key === 'path' || key === 'local_path' || key === 'save_dir') return '<path>';
    // id-typed keys are tagged ONLY when the value actually has the expected
    // format — a regressed/malformed id (wrong length, non-hex) must surface as
    // a diff, not be masked to the tag. Same for the prefixed id tags below:
    // require the whole documented shape, not just the prefix.
    if (key === 'device_id' || key === 'identity_id' || key === 'sender_id' || key === 'endpoint_id') {
      return HEX64.test(value) ? '<hex64>' : value;
    }
    if (/^blake3:[0-9a-f]{64}$/i.test(value)) return '<room_id>';
    if (/^roomtkt1[a-z2-7]+$/i.test(value)) return '<ticket>';
    if (/^file_[0-9a-f]{32}$/i.test(value)) return '<file_id>';
    if (/^[0-9a-f]{64}@(\d|\[)/i.test(value)) return '<addr>';
    if (HEX64.test(value)) return '<hex64>';
    if (HEX32.test(value)) return '<hex32>';
    return value; // enum values, labels, bodies compare literally
  }
  return value;
}

// -- assertion ---------------------------------------------------------------

export interface Diff {
  path: string;
  expected: unknown;
  actual: unknown;
}

/** Deep-equal the normalized actual against a template; every template key must
 *  match. When `subset` is false the actual must have no extra keys either. */
export function diffAgainst(actual: unknown, template: unknown, subset: boolean, path = '$'): Diff[] {
  const a = normalize(actual);
  return walk(a, template, subset, path);
}

function walk(actual: unknown, template: unknown, subset: boolean, path: string): Diff[] {
  if (template && typeof template === 'object' && !Array.isArray(template)) {
    if (!actual || typeof actual !== 'object' || Array.isArray(actual)) {
      return [{ path, expected: template, actual }];
    }
    const diffs: Diff[] = [];
    const a = actual as Record<string, unknown>;
    const t = template as Record<string, unknown>;
    for (const k of Object.keys(t)) diffs.push(...walk(a[k], t[k], subset, `${path}.${k}`));
    if (!subset) {
      for (const k of Object.keys(a)) if (!(k in t)) diffs.push({ path: `${path}.${k}`, expected: undefined, actual: a[k] });
    }
    return diffs;
  }
  if (Array.isArray(template)) {
    if (!Array.isArray(actual)) return [{ path, expected: template, actual }];
    const diffs: Diff[] = [];
    for (let i = 0; i < template.length; i++) diffs.push(...walk(actual[i], template[i], subset, `${path}[${i}]`));
    return diffs;
  }
  return JSON.stringify(actual) === JSON.stringify(template) ? [] : [{ path, expected: template, actual }];
}

// -- corpus types ------------------------------------------------------------

export interface Step {
  /** The method to call. */
  call: MethodName;
  /** Params; string values of the form "$name" are substituted from saves. */
  params?: Record<string, unknown>;
  /** Save fields from the result into the scenario bag (bagKey -> resultPath). */
  save?: Record<string, string>;
  /** Assert the normalized result equals this template exactly. */
  expect?: unknown;
  /** Assert the normalized result matches this template (extra keys allowed). */
  expectSubset?: unknown;
  /** Assert the call rejects with this error code. */
  expectError?: string;
  /** After the call, wait for a push matching this (normalized subset). */
  expectPush?: { push: PushName; match: Record<string, unknown> };
}

export interface Scenario {
  name: string;
  /** Oracle routing tags; `preIdentity` selects a separate fresh instance. */
  tags?: string[];
  steps: Step[];
}

export interface StepResult {
  step: number;
  method: string;
  ok: boolean;
  detail?: string;
}

// -- replay ------------------------------------------------------------------

const PUSH_NAMES: PushName[] = ['room.event', 'peers.changed'];

function resolveParams(params: Record<string, unknown> | undefined, bag: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(params ?? {})) {
    out[k] = typeof v === 'string' && v.startsWith('$') ? bag[v.slice(1)] : v;
  }
  return out;
}

function pick(obj: unknown, dotted: string): unknown {
  return dotted.split('.').reduce<unknown>((acc, k) => (acc && typeof acc === 'object' ? (acc as Record<string, unknown>)[k] : undefined), obj);
}

/** Replay one scenario against a Client; returns per-step results. Captures all
 *  pushes for the scenario's lifetime so `expectPush` can match one that raced
 *  ahead of (or arrived after) its triggering response. */
export async function replayScenario(client: Client, scenario: Scenario, pushWaitMs = 2000): Promise<StepResult[]> {
  const bag: Record<string, unknown> = {};
  const pushes: Array<{ push: PushName; data: unknown }> = [];
  const waiters: Array<() => void> = [];
  const offs = PUSH_NAMES.map((name) =>
    client.on(name, (data) => {
      pushes.push({ push: name, data });
      for (const w of waiters.splice(0)) w();
    }),
  );
  const results: StepResult[] = [];
  try {
    for (let i = 0; i < scenario.steps.length; i++) {
      const step = scenario.steps[i];
      const params = resolveParams(step.params, bag);
      try {
        const result = await client.call(step.call, params as never);
        if (step.expectError) {
          results.push({ step: i, method: step.call, ok: false, detail: `expected error ${step.expectError}, got success` });
          continue;
        }
        for (const [bagKey, resultPath] of Object.entries(step.save ?? {})) bag[bagKey] = pick(result, resultPath);
        const detail = checkResult(result, step);
        if (detail) {
          results.push({ step: i, method: step.call, ok: false, detail });
          continue;
        }
        if (step.expectPush) {
          const pushOk = await waitForPush(step.expectPush, pushes, waiters, pushWaitMs);
          if (!pushOk) {
            results.push({ step: i, method: step.call, ok: false, detail: `no ${step.expectPush.push} push matched within ${pushWaitMs}ms` });
            continue;
          }
        }
        results.push({ step: i, method: step.call, ok: true });
      } catch (err) {
        const code = (err as { code?: string }).code;
        if (step.expectError) {
          results.push({ step: i, method: step.call, ok: code === step.expectError, detail: code === step.expectError ? undefined : `expected error ${step.expectError}, got ${code ?? err}` });
        } else {
          results.push({ step: i, method: step.call, ok: false, detail: `unexpected error: ${code ?? (err as Error).message}` });
        }
      }
    }
  } finally {
    for (const off of offs) off();
  }
  return results;
}

function checkResult(result: unknown, step: Step): string | undefined {
  const template = step.expect ?? step.expectSubset;
  if (template === undefined) return undefined;
  const diffs = diffAgainst(result, template, step.expect === undefined);
  if (diffs.length === 0) return undefined;
  const d = diffs[0];
  return `at ${d.path}: expected ${JSON.stringify(d.expected)}, got ${JSON.stringify(d.actual)}`;
}

function waitForPush(
  want: { push: PushName; match: Record<string, unknown> },
  pushes: Array<{ push: PushName; data: unknown }>,
  waiters: Array<() => void>,
  timeoutMs: number,
): Promise<boolean> {
  const matches = () =>
    pushes.some((p) => p.push === want.push && diffAgainst(p.data, want.match, true).length === 0);
  if (matches()) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(matches()), timeoutMs);
    const check = () => {
      if (matches()) {
        clearTimeout(timer);
        resolve(true);
      } else {
        waiters.push(check);
      }
    };
    waiters.push(check);
  });
}
