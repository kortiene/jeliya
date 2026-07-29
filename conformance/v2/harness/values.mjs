// Value resolution for the v2 conformance harness: `$variable` references,
// computed nodes (`$add`/`$sub`/`$bytes_of_len`/`$concat`/`$unknown`), and the
// DSL's type tags (`<room_id>`, `<ts>`, `<pos>`, …).
//
// Variables are scoped to the case. The harness pre-seeds a small set of
// well-known variables a fresh case can reference before any `save`:
// `$op_id_new` (a fresh dedup key), `$daemon.storage_generation`, and the
// per-requirement room/subject ids the fixture's `requires` establish.

/** The set of DSL type tags this harness recognises. */
const TYPE_TAGS = new Set([
  '<room_id>', '<subject_id>', '<device_id>', '<event_id>', '<invite_id>',
  '<file_id>', '<pipe_id>', '<op_id>', '<ts>', '<uint>', '<bool>', '<string>',
  '<pos>', '<capability>', '<daemon_sg>', '<port>', '<object>', '<any>',
  '<version>', '<standing>', '<link_connected>', '<link_reason>',
]);

const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/;

/** Whether a string is a DSL type tag. */
export function isTypeTag(s) {
  return typeof s === 'string' && TYPE_TAGS.has(s);
}

/** Whether a value inhabits the named type-tag domain. */
export function matchesTypeTag(tag, value) {
  switch (tag) {
    case '<any>':
      return true;
    case '<object>':
      return value !== null && typeof value === 'object' && !Array.isArray(value);
    case '<uint>':
    case '<pos>':
    case '<port>':
      return typeof value === 'number' && Number.isInteger(value) && value >= 0;
    case '<bool>':
      return typeof value === 'boolean';
    case '<ts>':
      return typeof value === 'string' && RFC3339.test(value);
    case '<version>':
    case '<string>':
      return typeof value === 'string';
    case '<standing>':
      return value === 'active' || value === 'left' || value === 'removed';
    case '<link_reason>':
      return (
        value === 'never_dialed' ||
        value === 'dial_failed' ||
        value === 'no_route' ||
        value === 'closed'
      );
    case '<link_connected>':
      return (
        value !== null &&
        typeof value === 'object' &&
        (value.state === 'direct' || value.state === 'relay')
      );
    case '<room_id>':
      return typeof value === 'string' && value.length > 0;
    case '<subject_id>':
    case '<device_id>':
    case '<event_id>':
    case '<invite_id>':
    case '<file_id>':
    case '<pipe_id>':
    case '<op_id>':
    case '<capability>':
    case '<daemon_sg>':
      return typeof value === 'string' || typeof value === 'number';
    default:
      return false;
  }
}

/** Resolve a dotted path against a root object. Returns {found, value}. */
export function resolvePath(root, path) {
  if (path === '' || path === undefined || path === null) return { found: true, value: root };
  const parts = String(path).split('.');
  let cur = root;
  for (const part of parts) {
    if (cur === null || cur === undefined) return { found: false, value: undefined };
    if (part === '*') {
      return { found: false, value: undefined }; // wildcard handled by caller
    }
    if (Array.isArray(cur)) {
      const idx = Number(part);
      if (!Number.isInteger(idx) || idx >= cur.length) return { found: false, value: undefined };
      cur = cur[idx];
    } else if (typeof cur === 'object') {
      if (!(part in cur)) return { found: false, value: undefined };
      cur = cur[part];
    } else {
      return { found: false, value: undefined };
    }
  }
  return { found: true, value: cur };
}

/**
 * Resolve `$var` references and computed nodes anywhere in a fixture value.
 * `vars` is the case's variable map. A `$name` string resolves to the captured
 * value; a single-key `{$add|...}` node computes. Everything else deep-maps.
 */
export function resolveValue(node, vars) {
  if (typeof node === 'string') {
    if (node.startsWith('$')) {
      const name = node.slice(1);
      const { found, value } = resolvePath(vars, name);
      if (!found) {
        // An unbound variable stays a literal string — some fixtures use
        // `$op_id_new`-style placeholders the harness pre-seeds, and an
        // unknown reference is more useful as a distinguishable string than a
        // hard error at resolve time.
        return node;
      }
      return value;
    }
    return node;
  }
  if (Array.isArray(node)) {
    return node.map((el) => resolveValue(el, vars));
  }
  if (node !== null && typeof node === 'object') {
    const keys = Object.keys(node);
    if (keys.length === 1 && keys[0].startsWith('$')) {
      return computeNode(keys[0], node[keys[0]], vars);
    }
    const out = {};
    for (const [k, v] of Object.entries(node)) out[k] = resolveValue(v, vars);
    return out;
  }
  return node;
}

/** Evaluate a computed node (`$add`, `$sub`, `$bytes_of_len`, `$concat`, `$unknown`). */
function computeNode(kind, operand, vars) {
  switch (kind) {
    case '$add': {
      const [a, b] = operand.map((x) => num(resolveValue(x, vars)));
      return a + b;
    }
    case '$sub': {
      const [a, b] = operand.map((x) => num(resolveValue(x, vars)));
      return a - b;
    }
    case '$concat': {
      return operand.map((x) => String(resolveValue(x, vars))).join('');
    }
    case '$bytes_of_len': {
      const n = num(resolveValue(operand, vars));
      return 'x'.repeat(Math.max(0, n));
    }
    case '$unknown': {
      // A well-formed value of the named domain that names nothing real. The
      // operand is a type tag; produce a syntactically valid, definitely
      // nonexistent identifier.
      const tag = typeof operand === 'string' ? operand : '<string>';
      return unknownValue(tag);
    }
    default:
      return node;
  }
}

function num(v) {
  if (typeof v === 'number') return v;
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
}

/** A well-formed-but-nonexistent value for a domain. */
function unknownValue(tag) {
  switch (tag) {
    case '<room_id>':
      return 'blake3:' + '00'.repeat(32);
    case '<subject_id>':
    case '<device_id>':
      return '00'.repeat(32);
    case '<event_id>':
      return '00'.repeat(32);
    case '<file_id>':
      return 'file_' + '00'.repeat(16);
    case '<pipe_id>':
      return '00'.repeat(16);
    case '<invite_id>':
      return '00'.repeat(16);
    default:
      return 'nonexistent-' + '00'.repeat(8);
  }
}
