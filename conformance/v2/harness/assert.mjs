// The assertion engine for the v2 conformance harness.
//
// Two families per the DSL: value assertions (a dotted `path` plus a closed
// `op`) and observation assertions (`observe`, facts about the daemon's
// behaviour). Matching is subset-by-default: a named key must be present and
// equal; an unnamed key is unconstrained; `absent` and `exact_keys` are the
// explicit pinners. Type tags (`<room_id>`, …) match by domain, not literal.

import { isTypeTag, matchesTypeTag, resolvePath, resolveValue } from './values.mjs';

/** A single assertion failure, with the path and reason for the report. */
export class AssertFailure extends Error {
  constructor(message, detail = {}) {
    super(message);
    this.name = 'AssertFailure';
    this.detail = detail;
  }
}

/**
 * A transport failure: the daemon never produced a protocol response to
 * match — a reply timeout, socket close, failed upgrade, or send failure. It
 * is deliberately NOT an `AssertFailure`: the case never reached its
 * assertion, so it is a runner/daemon ERROR, never a conformance verdict and
 * never a satisfied `blocked_on_*` expectation.
 */
export class TransportFailure extends Error {
  constructor(message, detail = {}) {
    super(message);
    this.name = 'TransportFailure';
    this.detail = detail;
  }
}

/**
 * The context a case's assertions evaluate against. `roots` maps the path
 * roots (`out`, `err`, `frame`, and `$variables` live under `$`) to values;
 * `observe` is a callback the harness provides for behaviour facts.
 */
export class AssertContext {
  constructor({ vars, observe, sessions }) {
    this.vars = vars;
    this.observe = observe; // async (assertObj) => boolean | throws
    this.sessions = sessions;
  }

  rootFor(name) {
    if (name.startsWith('$')) return this.vars;
    // `out`/`err`/`frame` are stored as reserved vars by the last step.
    return this.vars;
  }
}

/** Deep structural equality (JSON values, key order-insensitive). */
export function deepEqual(a, b) {
  if (a === b) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null) return a === b;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    return a.every((el, i) => deepEqual(el, b[i]));
  }
  if (typeof a === 'object') {
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => k in b && deepEqual(a[k], b[k]));
  }
  return false;
}

/**
 * Subset match: every named key in `expected` must be present and match in
 * `actual`. Type tags match by domain; `$var` resolves; arrays match
 * element-wise for as many elements as `expected` names.
 */
export function subsetMatch(expected, actual, vars) {
  // Type tag: a domain check, not a literal.
  if (typeof expected === 'string' && isTypeTag(expected)) {
    return matchesTypeTag(expected, actual);
  }
  // A `$var` reference resolves to its captured value.
  if (typeof expected === 'string' && expected.startsWith('$')) {
    const resolved = resolveValue(expected, vars);
    return deepEqual(resolved, actual);
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length < expected.length) return false;
    return expected.every((el, i) => subsetMatch(el, actual[i], vars));
  }
  if (expected !== null && typeof expected === 'object') {
    if (actual === null || typeof actual !== 'object' || Array.isArray(actual)) return false;
    return Object.entries(expected).every(([k, v]) => k in actual && subsetMatch(v, actual[k], vars));
  }
  return deepEqual(expected, actual);
}

/** Collect every value at a wildcard path (`a.b[*].c`). Returns array of values. */
export function collectWildcard(root, path) {
  const parts = String(path).split('.');
  let frontier = [root];
  for (const part of parts) {
    const next = [];
    for (const node of frontier) {
      if (part === '*') {
        if (Array.isArray(node)) next.push(...node);
      } else if (node !== null && typeof node === 'object') {
        if (Array.isArray(node)) {
          const idx = Number(part);
          if (Number.isInteger(idx) && idx < node.length) next.push(node[idx]);
        } else if (part in node) {
          next.push(node[part]);
        }
      }
    }
    frontier = next;
  }
  return frontier;
}

function byteLength(v) {
  return typeof v === 'string' ? Buffer.byteLength(v, 'utf8') : 0;
}

function compare(op, actual, expected) {
  switch (op) {
    case 'eq': return actual === expected;
    case 'ne': return actual !== expected;
    case 'lt': return actual < expected;
    case 'lte': return actual <= expected;
    case 'gt': return actual > expected;
    case 'gte': return actual >= expected;
    default: return false;
  }
}

/**
 * Evaluate one value assertion against the context. Throws AssertFailure on
 * the first violation; returns normally on hold.
 */
export function evalValueAssertion(a, ctx) {
  const rawPath = a.path;
  const op = a.op;
  const hasWildcard = String(rawPath).includes('[*]') || String(rawPath).includes('.*');
  const path = String(rawPath).replace(/\[\*\]/g, '.*');

  // Resolve the root: `out`, `err`, `frame`, or a `$var`.
  const [rootName, ...rest] = path.split('.');
  const root = rootName.startsWith('$') ? ctx.vars[rootName.slice(1)] : ctx.vars[rootName];
  const subPath = rest.join('.');

  const values = hasWildcard
    ? collectWildcard(root, subPath)
    : [resolvePath(root, subPath)];

  // `present`/`absent` operate on resolvability, not value.
  if (op === 'present') {
    const r = hasWildcard ? { found: values.length > 0 } : values[0];
    if (!r.found) throw new AssertFailure(`expected ${rawPath} to be present`);
    return;
  }
  if (op === 'absent') {
    const r = hasWildcard ? { found: values.length > 0 } : values[0];
    if (r.found) throw new AssertFailure(`expected ${rawPath} to be absent`, { got: r.value });
    return;
  }

  // Resolve the expected value, but NOT when it is a type tag or the assertion
  // compares against another path (eq_except) — resolveValue would turn a
  // `<tag>` string into itself harmlessly, but a literal `{state: ...}` object
  // must resolve its inner `$vars` while leaving type tags intact.
  const expected =
    a.value !== undefined && op !== 'eq_except' ? resolveValuePreservingTags(a.value, ctx.vars) : a.value;

  const each = (resolved, label) => {
    if (!resolved.found) throw new AssertFailure(`${label}: path ${rawPath} did not resolve`);
    const v = resolved.value;
    switch (op) {
      case 'eq':
        if (!matchOrEqual(expected, v, ctx.vars))
          throw new AssertFailure(`${label}: ${rawPath} != expected`, { expected, got: v });
        break;
      case 'ne':
        if (matchOrEqual(expected, v, ctx.vars))
          throw new AssertFailure(`${label}: ${rawPath} unexpectedly equalled`, { got: v });
        break;
      case 'lt': case 'lte': case 'gt': case 'gte':
        if (!compare(op, v, expected))
          throw new AssertFailure(`${label}: ${rawPath} ${v} !${op} ${expected}`);
        break;
      case 'member_of':
        if (!expected.some((e) => matchOrEqual(e, v, ctx.vars)))
          throw new AssertFailure(`${label}: ${rawPath} ${JSON.stringify(v)} not in ${JSON.stringify(expected)}`);
        break;
      case 'type':
        if (!matchesTypeTag(expected, v))
          throw new AssertFailure(`${label}: ${rawPath} not of domain ${expected}`, { got: v });
        break;
      case 'exact_keys': {
        const want = [...expected].sort();
        const got = v && typeof v === 'object' && !Array.isArray(v) ? Object.keys(v).sort() : null;
        if (!got || !deepEqual(want, got))
          throw new AssertFailure(`${label}: ${rawPath} keys ${JSON.stringify(got)} != exactly ${JSON.stringify(want)}`);
        break;
      }
      case 'len': {
        const len = Array.isArray(v) || typeof v === 'string' ? v.length : null;
        if (len === null || !compare(expected.op, len, expected.value))
          throw new AssertFailure(`${label}: ${rawPath} length ${len} !${expected.op} ${expected.value}`);
        break;
      }
      case 'byte_len': {
        const len = byteLength(v);
        if (!compare(expected.op, len, expected.value))
          throw new AssertFailure(`${label}: ${rawPath} byte length ${len} !${expected.op} ${expected.value}`);
        break;
      }
      case 'unique': {
        // Handled at the wildcard level below.
        break;
      }
      case 'increasing': case 'non_decreasing': case 'contiguous':
        break; // wildcard-level
      case 'no_nulls':
        if (containsNull(v)) throw new AssertFailure(`${label}: ${rawPath} contains a null`);
        break;
      case 'eq_except': {
        const otherPath = expected.path;
        const [orootName, ...orest] = String(otherPath).split('.');
        const oroot = orootName.startsWith('$') ? ctx.vars[orootName.slice(1)] : ctx.vars[orootName];
        const oresolved = resolvePath(oroot, orest.join('.'));
        const strip = (obj) => {
          const c = JSON.parse(JSON.stringify(obj));
          for (const k of expected.keys || []) delete c[k];
          return c;
        };
        if (!deepEqual(strip(v), strip(oresolved.value)))
          throw new AssertFailure(`${label}: ${rawPath} != ${otherPath} except ${JSON.stringify(expected.keys)}`);
        break;
      }
      default:
        throw new AssertFailure(`${label}: unsupported assert op ${op}`);
    }
  };

  if (op === 'unique') {
    const seen = values.map((r) => JSON.stringify(r.value));
    if (new Set(seen).size !== seen.length)
      throw new AssertFailure(`${rawPath} values are not unique`);
    return;
  }
  if (op === 'increasing' || op === 'non_decreasing' || op === 'contiguous') {
    const nums = values.map((r) => r.value);
    for (let i = 1; i < nums.length; i++) {
      const ok =
        op === 'increasing' ? nums[i] > nums[i - 1]
        : op === 'non_decreasing' ? nums[i] >= nums[i - 1]
        : nums[i] === nums[i - 1] + 1;
      if (!ok) throw new AssertFailure(`${rawPath} is not ${op} at index ${i}`);
    }
    return;
  }

  values.forEach((r, i) => each(r, hasWildcard ? `${rawPath}[${i}]` : rawPath));
}

/** Like resolveValue but leaves DSL type tags (e.g. `<room_id>`) untouched —
 * they are matchers, not literals. `$var` references still resolve. */
function resolveValuePreservingTags(node, vars) {
  if (typeof node === 'string') {
    if (isTypeTag(node)) return node;
    return resolveValue(node, vars);
  }
  if (Array.isArray(node)) return node.map((el) => resolveValuePreservingTags(el, vars));
  if (node !== null && typeof node === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(node)) out[k] = resolveValuePreservingTags(v, vars);
    return out;
  }
  return node;
}

/** A value matcher that honours type tags and `$var` inside `expected`. */
function matchOrEqual(expected, actual, vars) {
  if (typeof expected === 'string' && isTypeTag(expected)) return matchesTypeTag(expected, actual);
  if (typeof expected === 'string' && expected.startsWith('$')) {
    return deepEqual(resolveValue(expected, vars), actual);
  }
  if (expected !== null && typeof expected === 'object') return subsetMatch(expected, actual, vars);
  return deepEqual(expected, actual);
}

/** Whether a value contains a JSON null anywhere in its subtree. */
function containsNull(v) {
  if (v === null) return true;
  if (Array.isArray(v)) return v.some(containsNull);
  if (typeof v === 'object') return Object.values(v).some(containsNull);
  return false;
}

/** Evaluate a whole `assert` array. Throws on the first failure. */
export async function evalAssert(assertArray, ctx) {
  for (const a of assertArray) {
    if (a.observe) {
      await ctx.observe(a);
    } else {
      evalValueAssertion(a, ctx);
    }
  }
}
