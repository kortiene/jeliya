#!/usr/bin/env node
/**
 * Validates conformance/v2 fixtures against the DSL frozen in
 * conformance/v2/README.md (#212). This checks DSL *conformance* — shape,
 * vocabulary, and closed sets — never the correctness of a case's intent.
 * The independence rule forbids deriving expected values from an
 * implementation; this validator never runs an implementation.
 *
 * Usage: node scripts/check-v2-corpus.mjs [--json]
 * Exit 0 when every case conforms; exit 1 with a per-file report otherwise.
 */

import { readFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath } from "node:url";

const DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "conformance", "v2");

const DSL_VERBS = new Set(["call", "http", "upgrade", "send", "await", "control", "assert"]);
const AUX_KEYS = new Set(["in", "op_id", "on", "expect", "save", "note", "stream", "defer"]);
const TOP_LEVEL_KEYS = new Set(["domain", "note", "cases", "authoring_notes"]);
const CASE_KEYS = new Set([
  "name", "kind", "operation", "intent", "requires", "steps", "targets",
  "blocked_on_upstream", "blocked_on_record",
]);
const TARGETS = new Set(["daemon", "in_process_core", "client_adapter"]);
const DOMAIN_BY_FILE = {
  "files.json": "files",
  "handshake.json": "handshake",
  "invites.json": "invites",
  "pipes.json": "pipes",
  "rooms.json": "rooms",
  "subject-daemon.json": "subject-daemon",
  "timeline-streams.json": "timeline-streams",
};
const COMPUTED_NODES = new Set([
  "$add", "$sub", "$bytes_of_len", "$concat", "$expires_in_ms", "$unknown",
  "$transfer_budget_ms",
]);
// `stream` is legal only on the two operations the record streams bytes for,
// and carries exactly one direction.
const STREAMING_OPS = new Map([
  ["file.share", "send_bytes"],
  ["file.read", "receive_bytes"],
]);
const KINDS = new Set(["success", "error", "malformed", "boundary", "authorization", "handshake", "push", "ordering"]);
const PRE_OPEN_STREAM_CODES = new Set([
  "invalid_argument", "subject_absent", "room_not_available", "membership_ended",
  "op_id_conflict", "file_unknown", "file_not_fetched", "resource_exhausted",
]);
const CONTROL_DO = new Set([
  "advance_clock", "idle", "disconnect", "reconnect", "inject_fault",
  "set_limit", "set_link_rate", "set_provider_response_bytes", "client_preflight",
  "client_render_limit", "client_render_file", "stop_daemon", "start_daemon",
  "start_transfers", "cancel_transfers", "pause_link",
]);
const ASSERT_OPS = new Set([
  "eq", "ne", "lt", "lte", "gt", "gte", "member_of", "type", "present",
  "absent", "exact_keys", "len", "unique", "increasing", "non_decreasing",
  "contiguous", "no_nulls", "byte_len", "eq_except",
]);
const OBSERVE = new Set([
  "no_network_activity", "no_durable_mutation", "no_event_authored",
  "bytes_streamed", "connection_open", "close_code", "push_count",
  "no_push", "timing_indistinguishable", "process_exited",
]);
const TYPE_TAGS = new Set([
  "room_id", "subject_id", "device_id", "event_id", "invite_id", "file_id",
  "pipe_id", "op_id", "request_id", "ts", "uint", "bool", "string",
  "pos", "capability", "daemon_sg", "port", "object", "any", "version",
  "standing", "link_connected", "link_reason",
]);
const ASSERTION_KEYS = new Set(["path", "op", "value", "note"]);
const ASSERTIONS_WITHOUT_VALUE = new Set([
  "present", "absent", "unique", "increasing", "non_decreasing", "contiguous",
  "no_nulls",
]);
const ASSERTIONS_WITH_VALUE = new Set([
  "eq", "ne", "lt", "lte", "gt", "gte", "member_of", "type", "exact_keys",
  "len", "byte_len", "eq_except",
]);
const COMPARISON_OPS = new Set(["eq", "ne", "lt", "lte", "gt", "gte"]);
const FILE_ERROR_KEYS = new Map([
  ["invalid_argument", ["code", "field", "reason"]],
  ["room_not_available", ["code", "room_id"]],
  ["membership_ended", ["code", "room_id", "standing"]],
  ["file_unknown", ["code", "file_id"]],
  ["file_not_fetched", ["code", "file_id"]],
  ["provider_unreachable", ["code", "file_id", "providers"]],
  ["transfer_unknown", ["code", "transfer_op_id"]],
  ["declared_size_mismatch", ["code", "declared_bytes", "observed_bytes"]],
  ["transfer_stalled", ["code", "transferred_bytes", "total"]],
  ["transfer_deadline_exceeded", ["code", "transferred_bytes", "total", "budget_ms"]],
  ["stream_aborted", ["code", "transferred_bytes", "total", "reason"]],
  ["file_too_large", ["code", "declared_bytes", "limit_bytes", "enforced_at"]],
  ["resource_exhausted", ["code", "resource", "limit"]],
  ["digest_mismatch", ["code", "expected", "observed"]],
  ["room_not_live", ["code", "room_id"]],
  ["subject_absent", ["code"]],
  ["op_id_conflict", ["code", "op_id"]],
  ["file_index_unreadable", ["code"]],
]);
const TRANSPORT_CODE_FIXTURES = new Map([
  ["frame_too_large", "close_code_4005_is_emitted_for_frame_too_large"],
  ["idle_timeout", "close_code_4004_is_emitted_on_idle_timeout"],
  ["malformed_frame", "a_frame_with_no_recoverable_id_closes_4007_malformed_frame"],
  ["not_ready", "close_code_4003_is_emitted_when_the_daemon_is_not_ready"],
]);
const PRINCIPAL_ISOLATION_CASES = new Set([
  "transfer_cancel_by_a_different_principal_is_indistinguishable_from_unknown",
  "transfer_progress_is_delivered_only_to_the_initiating_principal",
]);
const LEGACY_REQUEST_CONCURRENCY_CASES = new Set([
  "inflight_requests_at_the_served_limit_are_all_answered",
  "one_request_past_max_inflight_is_resource_exhausted_not_a_close",
]);
const REQUIRE_ARGS = {
  subject: new Set(["self", "none", "second", "outsider"]),
  daemon: new Set(["self", "second", "fresh", "restartable"]),
  room: new Set(["plain", "live", "quiescent", "left", "removed", "foreign", "with_history"]),
  member: new Set(["b", "c", "agent", "non_agent"]),
  link: new Set(["up", "down", "relay", "slow"]),
  resource: new Set(["tcp_service", "large_file", "shared_file", "fetched_file"]),
  observe: new Set(["network", "store", "frames", "timing", "process"]),
  control: new Set(["clock", "limits", "reconnect", "concurrency"]),
};
const REQUIRE_NAMESPACES = new Set([...Object.keys(REQUIRE_ARGS), "fault"]);
const BARE_REQUIRES = new Set(["subject", "daemon"]);
// The retired tags the README names explicitly.
const RETIRED_TAGS = new Set(["hex64", "u64", "number", "int", "variant", "array"]);

// The variable-binding contract for files.json (README "Runner-provided
// variables"). The harness resolves a whole-string "$name" anywhere a literal
// is legal, and conformance/v2/harness/values.mjs deliberately lets an
// unresolved reference degrade to its literal string form — which satisfies
// subset matching and turns the step into no evidence at all. Structural
// validation therefore proves every reference is either captured by an
// earlier save on the same case or documented below; otherwise a case can
// reference a never-established "$fid2" and still read as coverage.
const RUNNER_PROVIDED_VARIABLES = new Set([
  // Pre-seeded for every case before step 1 (conformance/v2/harness/runner.mjs).
  "op_id_new", "op_id_fixed", "limits", "daemon", "daemon_sg",
]);
// Which DECLARED precondition binds each documented variable (README
// "Runner-provided variables"). The historical $fid2 regression was a
// requires rewrite that lost a binding while the reference survived, so a
// documented name alone is never evidence — the case must declare the
// precondition that binds it, or capture the value itself with a save.
const FILE_RESOURCE_REQUIRES = new Set([
  "resource:shared_file", "resource:fetched_file", "resource:large_file",
]);
// A file exists only in a room, so a file resource precondition establishes
// the room (and its authority subject) along with the file.
const bindsOwnRoom = (requires) => requires.some((token) =>
  /^room:(plain|live|quiescent|left|removed|with_history)$/.test(token)
  || /^member:/.test(token) || FILE_RESOURCE_REQUIRES.has(token));
const PRECONDITION_BINDINGS = new Map([
  ["rid", bindsOwnRoom],
  ["self_sid", bindsOwnRoom],
  ["rid_left", (requires) => requires.includes("room:left")],
  ["foreign_rid", (requires) => requires.includes("room:foreign")],
  ["foreign_fid", (requires) => requires.includes("room:foreign")],
  ["member_b_sid", (requires) => requires.includes("member:b")],
  ["member_c_sid", (requires) => requires.includes("member:c")],
  ["sb", (requires) => requires.includes("subject:second")],
  ["sc", (requires) => requires.includes("subject:outsider")],
  ["svc_port", (requires) => requires.includes("resource:tcp_service")],
  ["svc_port_v6", (requires) => requires.includes("resource:tcp_service")],
  ["fid", (requires) => requires.some((token) => FILE_RESOURCE_REQUIRES.has(token))],
  ["fid_unsized", (requires) => requires.includes("resource:large_file")],
  ["fid_one_byte", (requires) => requires.includes("resource:large_file")],
]);
// Two pre-existing U2-blocked fixtures read the fid family without declaring
// a file resource precondition; their requires predate that vocabulary.
// Exempting them BY NAME lets the conditional check land without editing
// fixtures this change does not own — #233 tracks the requires repair, and
// deleting an entry here is how its exemption closes.
const PRECONDITION_GAP_CASES = new Map([
  ["a_transfer_with_no_forward_progress_fails_with_transfer_stalled", new Set(["fid", "fid_unsized"])],
  ["fetch_completed_result_replays_without_a_second_transfer", new Set(["fid"])],
]);
// Every documented binding except $limits and $daemon is a scalar id or
// port, so a dotted tail on one is statically unresolvable — an absent
// assertion on "$rid.secret" would silently pass.
const SCALAR_PROVIDED_VARIABLES = new Set([
  "op_id_new", "op_id_fixed", "daemon_sg",
  ...PRECONDITION_BINDINGS.keys(),
]);
// A whole-string value reference the harness can resolve: "$name" plus
// optional dot segments (values.mjs resolves the tail with resolvePath,
// which splits on "." only — "$fid-2" or "$fid[0]" resolve to nothing and
// degrade to literal strings, so they are malformed rather than clever).
const VALUE_REFERENCE = /^\$([A-Za-z_][A-Za-z0-9_]*)(?:\.[A-Za-z0-9_]+)*$/;
// A $variable-rooted assertion/save path the assert engine can resolve: the
// bare root, then dot segments, each optionally carrying the [*] wildcard.
// The engine splits on "." and rewrites only [*], so "$rid..secret",
// "$rid.", and "$rid.foo[0]" all resolve against nothing.
const VARIABLE_PATH = /^\$([A-Za-z_][A-Za-z0-9_]*)(?:\.[A-Za-z0-9_]+(?:\[\*\])?)*$/;
const VARIABLE_REFERENCE = /^\$([A-Za-z_][A-Za-z0-9_]*)/;

const OPERATIONS = new Set([
  "subject.ensure", "daemon.stop",
  "room.create", "room.list", "room.activate", "room.deactivate", "room.leave",
  "room.timeline", "room.members", "room.archive", "room.peers",
  "member.remove",
  "invite.mint", "invite.list", "invite.revoke", "invite.redeem",
  "message.send", "status.post", "status.history", "fleet.list",
  "file.share", "file.list", "file.fetch", "file.read", "transfer.cancel",
  "pipe.publish", "pipe.list", "pipe.connect", "pipe.release", "pipe.revoke",
  "stream.subscribe", "stream.unsubscribe", "stream.resync",
]);

const TAXONOMY_CODES = new Set([
  "authority_cannot_be_removed", "capability_expired", "capability_invalid", "capability_redeemed",
  "capability_revoked", "connection_unknown", "cursor_unknown", "declared_size_mismatch",
  "digest_mismatch", "file_index_unreadable", "file_not_fetched", "file_too_large", "file_unknown",
  "fleet_projection_unavailable", "forbidden_origin", "frame_too_large", "idle_timeout",
  "insufficient_standing", "invalid_argument", "invite_index_unreadable", "invite_unknown",
  "invitee_already_member", "malformed_frame", "member_unknown", "membership_ended",
  "membership_unresolved", "message_too_large", "not_ready", "op_id_conflict", "pairing_code_invalid",
  "pipe_index_unreadable", "pipe_not_publisher", "pipe_revoked", "pipe_target_refused", "pipe_unknown",
  "pipe_unreachable", "policy_refused", "protocol_unsupported", "provider_unreachable",
  "resource_exhausted", "resync_required", "role_not_grantable", "room_index_unreadable",
  "room_name_invalid", "room_not_available", "room_not_live", "room_still_active", "session_expired",
  "shutdown_in_progress", "sole_authority_cannot_leave", "status_label_unknown",
  "status_subject_unknown", "storage_generation_mismatch", "subject_absent",
  "subject_store_unwritable", "subscription_limit_reached", "subscription_unknown",
  "stream_aborted", "transfer_deadline_exceeded", "transfer_stalled", "transfer_unknown",
  "transport_unavailable", "unauthenticated", "unknown_operation",
]);

const problems = [];
let caseCount = 0;
let stepCount = 0;
let fixtureProblemCount = 0;
let computed = null;

function fail(file, caseName, where, msg) {
  problems.push({ file, case: caseName, where, msg });
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isComputedNode(value) {
  if (!isObject(value) || Object.keys(value).length !== 1) return false;
  const [operator] = Object.keys(value);
  if (!COMPUTED_NODES.has(operator)) return false;
  const operand = value[operator];
  if (["$add", "$sub"].includes(operator)) {
    return Array.isArray(operand) && operand.length === 2
      && operand.every((item) => Number.isInteger(item)
        || (typeof item === "string" && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(item)));
  }
  if (operator === "$concat") {
    return Array.isArray(operand) && operand.length > 0
      && operand.every((item) => typeof item === "string");
  }
  if (operator === "$expires_in_ms") return Number.isInteger(operand) && operand >= 0;
  if (operator === "$transfer_budget_ms") {
    return Array.isArray(operand) && operand.length === 3
      && operand.every((item) => Number.isInteger(item)
        || (typeof item === "string" && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(item)));
  }
  if (operator === "$bytes_of_len") {
    return Number.isInteger(operand) && operand >= 0
      || (typeof operand === "string" && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(operand));
  }
  return typeof operand === "string" && operand.length > 0;
}

function isValueNode(value, { positive = false } = {}) {
  if (Number.isInteger(value)) return positive ? value > 0 : value >= 0;
  if (typeof value === "string" && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(value)) return true;
  return isComputedNode(value)
    && ["$add", "$sub", "$transfer_budget_ms"].includes(Object.keys(value)[0]);
}

function isNumericAssertionValue(value) {
  if (typeof value === "number") return Number.isFinite(value);
  if (typeof value === "string" && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(value)) return true;
  return isComputedNode(value)
    && ["$add", "$sub", "$transfer_budget_ms"].includes(Object.keys(value)[0]);
}

function checkExactKeySet(value, required) {
  if (!isObject(value)) return false;
  const got = Object.keys(value).sort();
  const want = [...required].sort();
  return got.length === want.length && got.every((key, i) => key === want[i]);
}

function checkExactKeys(value, required, file, caseName, where) {
  if (!isObject(value)) {
    fail(file, caseName, where, `must be an object with keys {${required.join(", ")}}`);
    return false;
  }
  if (!checkExactKeySet(value, required)) {
    const got = Object.keys(value).sort();
    const want = [...required].sort();
    fail(file, caseName, where, `takes exactly {${want.join(", ")}}, got {${got.join(", ")}}`);
    return false;
  }
  return true;
}

function sameJson(actual, expected) {
  return isDeepStrictEqual(actual, expected);
}

function checkTypeTags(value, file, caseName, where) {
  if (typeof value === "string") {
    const m = value.match(/^<([a-z0-9_]+)>$/);
    if (m) {
      if (RETIRED_TAGS.has(m[1])) {
        fail(file, caseName, where, `retired type tag <${m[1]}> (README: names an encoding or asserts nothing)`);
      } else if (!TYPE_TAGS.has(m[1])) {
        fail(file, caseName, where, `unknown type tag <${m[1]}> — the README's tag table is the vocabulary, and a new tag must be added there`);
      }
    }
  } else if (Array.isArray(value)) {
    value.forEach((v) => checkTypeTags(v, file, caseName, where));
  } else if (value && typeof value === "object") {
    for (const [k, v] of Object.entries(value)) {
      if (k === "state" && typeof v === "string" && v === "<variant>") {
        fail(file, caseName, where, `{"state": "<variant>"} asserts a discriminant exists without naming legal arms — use member_of with an explicit token set`);
      }
      if (k === "code" && typeof v === "string" && !TAXONOMY_CODES.has(v)) {
        fail(file, caseName, where, `unknown taxonomy code "${v}"`);
      }
      checkTypeTags(v, file, caseName, where);
    }
  }
}

function checkFileError(err, file, caseName, where) {
  const required = FILE_ERROR_KEYS.get(err.code);
  if (!required) return false;
  let valid = checkExactKeys(err, required, file, caseName, where);
  if (err.code === "provider_unreachable" && Array.isArray(err.providers)) {
    if (err.providers.length === 0) {
      fail(file, caseName, where, `provider_unreachable.providers must contain at least one attempted provider`);
      valid = false;
    }
    err.providers.forEach((provider, index) => {
      if (!checkExactKeys(provider, ["subject_id", "device_id", "link"], file,
        caseName, `${where}.providers[${index}]`)) valid = false;
    });
  } else if (err.code === "provider_unreachable") {
    fail(file, caseName, where, `provider_unreachable.providers must be a non-empty array`);
    valid = false;
  }
  return valid;
}

function fileErrorIsCanonical(err) {
  const required = FILE_ERROR_KEYS.get(err?.code);
  if (!required || !checkExactKeySet(err, required)) return false;
  if (err.code !== "provider_unreachable") return true;
  return Array.isArray(err.providers) && err.providers.length > 0
    && err.providers.every((provider) =>
      checkExactKeySet(provider, ["subject_id", "device_id", "link"]));
}

/** The variable a "$name"-rooted path reads. Returns null for the
 * non-variable roots (out/err/frame, "$" the whole step value), {name,
 * hasTail} for a well-formed variable path, or {malformed: true} when the
 * shape cannot resolve — the assert engine splits paths on "." only, so
 * "$rid[0].secret" looks up the nonexistent variable "rid[0]" and an absent
 * assertion on it silently passes. "$request" is NOT special here: the DSL
 * reserves it solely as a deferred call's save source, which its scan site
 * skips; anywhere else it is an ordinary, never-bound name. */
function pathRootReference(path) {
  if (typeof path !== "string" || !path.startsWith("$")) return null;
  if (path === "$" || path.startsWith("$.")) return null;
  const m = path.match(VARIABLE_PATH);
  return m ? { name: m[1], hasTail: path.length > m[1].length + 1 } : { malformed: true };
}

/** Collect $name references embedded ANYWHERE in http/upgrade strings — the
 * harness's header/path resolution substitutes them mid-string ("Bearer
 * $token", "/files/$fid"), so whole-string scanning is not enough there. */
function collectInterpolatedReferences(node, at, refs) {
  if (typeof node === "string") {
    // Mirror #resolveHeaderValue's capture exactly: its name grammar
    // consumes dots ([A-Za-z0-9_.]*), so "$rid..secret" is one unresolvable
    // reference — not a read of $rid followed by text — and the literal
    // stays in the request when resolution fails.
    for (const m of node.matchAll(/\$([A-Za-z_][A-Za-z0-9_.]*)/g)) {
      const name = m[1];
      if (/^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$/.test(name)) {
        const root = name.split(".")[0];
        refs.push({ name: root, at, hasTail: root !== name });
      } else {
        refs.push({ name: null, raw: `$${name}`, at });
      }
    }
    return;
  }
  if (Array.isArray(node)) {
    node.forEach((el, i) => collectInterpolatedReferences(el, `${at}[${i}]`, refs));
    return;
  }
  if (isObject(node)) {
    for (const [k, v] of Object.entries(node)) {
      collectInterpolatedReferences(v, `${at}.${k}`, refs);
    }
  }
}

/** Collect every $variable read from a value node, recursing through arrays,
 * objects, and computed-node operands ($unknown takes a type tag, never a
 * reference). A whole-string "$…" value that is not a well-formed reference
 * is collected as malformed ({name: null, raw}). */
function collectValueReferences(node, at, refs) {
  if (typeof node === "string") {
    if (!node.startsWith("$")) return;
    const m = node.match(VALUE_REFERENCE);
    if (m) refs.push({ name: m[1], at, hasTail: node.length > m[1].length + 1 });
    else refs.push({ name: null, raw: node, at });
    return;
  }
  if (Array.isArray(node)) {
    node.forEach((el, i) => collectValueReferences(el, `${at}[${i}]`, refs));
    return;
  }
  if (isObject(node)) {
    const keys = Object.keys(node);
    if (keys.length === 1 && keys[0].startsWith("$")) {
      if (keys[0] !== "$unknown") collectValueReferences(node[keys[0]], `${at}.${keys[0]}`, refs);
      return;
    }
    for (const [k, v] of Object.entries(node)) collectValueReferences(v, `${at}.${k}`, refs);
  }
}

/** Every $variable a step reads: nested inputs, the envelope op_id, stream
 * counts, expectations, await matchers and reply handles, control value
 * fields, assertion paths / comparison values / observation arguments, raw
 * frames, http/upgrade requests, and save source paths. Label and vocabulary
 * positions (`on`, control `do`/`between`/`limit`/`fault`/`daemon`/
 * `op_id_prefix`, observation `scope`/`on`/`between`/`call`, `exact_keys`
 * key names, `type` tag names) never carry references. */
function collectStepVariableReferences(step) {
  const refs = [];
  if (!isObject(step)) return refs;
  for (const key of ["in", "op_id", "expect", "stream", "send"]) {
    if (key in step) collectValueReferences(step[key], key, refs);
  }
  // The runner interpolates $name mid-string in http/upgrade HEADERS and the
  // http PATH (#resolveHeaderValue), but resolves http BODY and upgrade QUERY
  // values whole (resolveValue) — so "$rid[0]" in a body is not a read of
  // $rid, it is an unresolvable literal.
  if (isObject(step.http)) {
    for (const key of ["path", "headers"]) {
      if (key in step.http) collectInterpolatedReferences(step.http[key], `http.${key}`, refs);
    }
    if ("body" in step.http) collectValueReferences(step.http.body, "http.body", refs);
  }
  if (isObject(step.upgrade)) {
    if ("headers" in step.upgrade) collectInterpolatedReferences(step.upgrade.headers, "upgrade.headers", refs);
    if ("query" in step.upgrade) collectValueReferences(step.upgrade.query, "upgrade.query", refs);
  }
  if (isObject(step.await)) {
    // `$id` is the reply-position builtin naming the connection's most
    // recent request id (runner.mjs) — not a variable.
    if (typeof step.await.reply === "string" && step.await.reply.startsWith("$")
        && step.await.reply !== "$id") {
      const m = step.await.reply.match(VALUE_REFERENCE);
      if (m) refs.push({ name: m[1], at: "await.reply", hasTail: step.await.reply.length > m[1].length + 1 });
      else refs.push({ name: null, raw: step.await.reply, at: "await.reply" });
    }
    for (const key of ["push", "frame"]) {
      if (key in step.await) collectValueReferences(step.await[key], `await.${key}`, refs);
    }
  }
  if (isObject(step.control)) {
    for (const [key, value] of Object.entries(step.control)) {
      if (["do", "on", "between", "limit", "fault", "daemon", "op_id_prefix"].includes(key)) continue;
      collectValueReferences(value, `control.${key}`, refs);
    }
  }
  if (Array.isArray(step.assert)) {
    step.assert.forEach((a, i) => {
      if (!isObject(a)) return;
      const at = `assert[${i}]`;
      if ("observe" in a) {
        if ("room_id" in a) collectValueReferences(a.room_id, `${at}.room_id`, refs);
        if (isObject(a.value) && "value" in a.value) {
          collectValueReferences(a.value.value, `${at}.value.value`, refs);
        } else if (typeof a.value === "string") {
          collectValueReferences(a.value, `${at}.value`, refs);
        }
        if (isObject(a.match)) collectValueReferences(a.match, `${at}.match`, refs);
        return;
      }
      const root = pathRootReference(a.path);
      if (root) refs.push({ name: root.name ?? null, raw: a.path, at: `${at}.path`, hasTail: root.hasTail });
      if (a.op === "eq_except" && isObject(a.value)) {
        const targetRoot = pathRootReference(a.value.path);
        if (targetRoot) refs.push({ name: targetRoot.name ?? null, raw: a.value.path, at: `${at}.value.path`, hasTail: targetRoot.hasTail });
      } else if (["len", "byte_len"].includes(a.op) && isObject(a.value)) {
        collectValueReferences(a.value.value, `${at}.value.value`, refs);
      } else if (!["exact_keys", "type"].includes(a.op) && "value" in a) {
        collectValueReferences(a.value, `${at}.value`, refs);
      }
    });
  }
  if (isObject(step.save)) {
    for (const [name, path] of Object.entries(step.save)) {
      // "$request" is legal only here, as a deferred call's save source;
      // checkStep validates that pairing separately.
      if (path === "$request") continue;
      const root = pathRootReference(path);
      if (root) refs.push({ name: root.name ?? null, raw: path, at: `save.${name}`, hasTail: root.hasTail });
    }
  }
  return refs;
}

function checkAssertion(a, file, caseName, where) {
  if (typeof a !== "object" || a === null || Array.isArray(a)) {
    fail(file, caseName, where, `assert element is not an object: ${JSON.stringify(a)?.slice(0, 80)}`);
    return;
  }
  const keys = Object.keys(a);
  if (keys.includes("observe")) {
    if (!OBSERVE.has(a.observe)) {
      fail(file, caseName, where, `unknown observation "${a.observe}" (closed set of ${OBSERVE.size})`);
      return;
    }
    const observationKeys = {
      no_network_activity: new Set(["observe", "scope"]),
      no_durable_mutation: new Set(["observe", "scope"]),
      no_event_authored: new Set(["observe", "scope", "note"]),
      bytes_streamed: new Set(["observe", "value", "call"]),
      connection_open: new Set(["observe", "on", "note"]),
      close_code: new Set(["observe", "value", "on"]),
      push_count: new Set(["observe", "value", "room_id"]),
      no_push: new Set(["observe", "room_id", "on", "match", "scope"]),
      timing_indistinguishable: new Set(["observe", "between"]),
      process_exited: new Set(["observe", "value"]),
    };
    const unexpected = keys.filter((key) => !observationKeys[a.observe].has(key));
    if (unexpected.length > 0) {
      fail(file, caseName, where, `${a.observe} has unexpected key(s): ${unexpected.join(", ")}`);
    }
    const requiredObservationKeys = {
      no_network_activity: ["scope"],
      no_durable_mutation: ["scope"],
      no_event_authored: ["scope"],
      bytes_streamed: ["value"],
      connection_open: ["on"],
      close_code: ["value", "on"],
      push_count: ["value", "room_id"],
      no_push: ["scope"],
      timing_indistinguishable: ["between"],
      process_exited: ["value"],
    };
    const missing = requiredObservationKeys[a.observe].filter((key) => !(key in a));
    if (missing.length > 0) {
      fail(file, caseName, where, `${a.observe} requires ${missing.join(" and ")}`);
    }
    if (a.observe === "bytes_streamed" && "value" in a) {
      if (checkExactKeys(a.value, ["op", "value"], file, caseName, `${where} value`)) {
        if (!["eq", "ne", "lt", "lte", "gt", "gte"].includes(a.value.op)) {
          fail(file, caseName, where, `bytes_streamed.value.op must be a numeric comparison`);
        }
        if (!isNumericAssertionValue(a.value.value)) {
          fail(file, caseName, where, `bytes_streamed.value.value must be numeric, a $variable, or an arithmetic computed node`);
        }
      }
    }
    if (a.observe === "bytes_streamed" && "call" in a
        && (typeof a.call !== "string" || !/^step:[1-9][0-9]*$/.test(a.call))) {
      fail(file, caseName, where, `bytes_streamed.call must be a one-indexed step selector such as "step:4"`);
    }
    if (a.observe === "no_push") {
      const legacy = checkExactKeySet(a, ["observe", "room_id", "scope"]);
      const matcher = checkExactKeySet(a, ["observe", "on", "match", "scope"]);
      if (!legacy && !matcher) {
        fail(file, caseName, where,
          `no_push must be exactly {observe, room_id, scope} or {observe, on, match, scope}`);
      } else if (legacy && (typeof a.room_id !== "string" || a.room_id.length === 0)) {
        fail(file, caseName, where, `no_push.room_id must be a non-empty room identifier or variable`);
      } else if (matcher) {
        if (typeof a.on !== "string" || a.on.length === 0) {
          fail(file, caseName, where, `no_push.on must be a non-empty session label`);
        }
        if (!isObject(a.match) || Object.keys(a.match).length === 0) {
          fail(file, caseName, where, `no_push.match must be a non-empty object`);
        }
      }
    }
    return;
  }
  if (!("path" in a) && !("op" in a)) {
    fail(file, caseName, where, `assertion has neither path nor observe: keys ${keys.join(",")}`);
    return;
  }
  if ("op" in a && !ASSERT_OPS.has(a.op)) {
    fail(file, caseName, where, `unknown assert op "${a.op}" (closed set of ${ASSERT_OPS.size})`);
  }
  if (file === "files.json") {
    const unknownKeys = keys.filter((key) => !ASSERTION_KEYS.has(key));
    if (unknownKeys.length > 0) {
      fail(file, caseName, where, `file assertion has unknown key(s): ${unknownKeys.join(", ")}`);
    }
    if (!("path" in a) || !("op" in a)) {
      fail(file, caseName, where, `file value assertion requires both path and op`);
    }
    if (ASSERTIONS_WITHOUT_VALUE.has(a.op) && "value" in a) {
      fail(file, caseName, where, `${a.op} takes no value`);
    }
    if (ASSERTIONS_WITH_VALUE.has(a.op) && !("value" in a)) {
      fail(file, caseName, where, `${a.op} requires value`);
    }
    if (a.op === "ne" && Array.isArray(a.value)) {
      fail(file, caseName, where, `array-valued ne is ambiguous — use one ne assertion per forbidden value`);
    }
    if (["lt", "lte", "gt", "gte"].includes(a.op) && !isNumericAssertionValue(a.value)) {
      fail(file, caseName, where, `${a.op}.value must be a finite number, $variable, or arithmetic computed node`);
    }
    if (a.op === "member_of" && (!Array.isArray(a.value) || a.value.length === 0)) {
      fail(file, caseName, where, `member_of.value must be a non-empty array`);
    }
    if (a.op === "type" && (typeof a.value !== "string" || !TYPE_TAGS.has(a.value))) {
      fail(file, caseName, where, `type.value must be one of the ${TYPE_TAGS.size} type-tag names without angle brackets`);
    }
    if (a.op === "exact_keys"
        && (!Array.isArray(a.value) || a.value.length === 0
          || a.value.some((key) => typeof key !== "string" || key.length === 0)
          || new Set(a.value).size !== a.value.length)) {
      fail(file, caseName, where, `exact_keys.value must be a non-empty unique array of non-empty strings`);
    }
    if (["len", "byte_len"].includes(a.op)) {
      if (checkExactKeys(a.value, ["op", "value"], file, caseName, `${where} value`)
          && (!COMPARISON_OPS.has(a.value.op) || !isNumericAssertionValue(a.value.value))) {
        fail(file, caseName, where, `${a.op}.value must contain a comparison op and numeric assertion value`);
      }
    }
    if (a.op === "eq_except") {
      if (checkExactKeys(a.value, ["path", "keys"], file, caseName, `${where} value`)) {
        if (typeof a.value.path !== "string" || a.value.path.length === 0) {
          fail(file, caseName, where, `eq_except.value.path must be a non-empty path string`);
        }
        if (!Array.isArray(a.value.keys) || a.value.keys.length === 0
            || a.value.keys.some((key) => typeof key !== "string" || key.length === 0)
            || new Set(a.value.keys).size !== a.value.keys.length) {
          fail(file, caseName, where, `eq_except.value.keys must be a non-empty unique array of non-empty strings`);
        }
      }
    }
  }
  if ("path" in a) {
    // `path` is "a dotted path rooted at out, err, frame, or a $variable"
    // (README). A non-string path resolves against nothing, so an assertion
    // carrying one asserts nothing while still reading as coverage.
    if (typeof a.path !== "string") {
      fail(file, caseName, where,
        `assert path is ${Array.isArray(a.path) ? "an array" : typeof a.path} — must be a dotted string`);
    } else {
      const root = a.path.split(/[.[]/)[0];
      if (!["out", "err", "frame"].includes(root) && !/^\$[A-Za-z_][A-Za-z0-9_]*$/.test(root)) {
        fail(file, caseName, where, `assert path rooted at "${root}" (must be out/err/frame/$variable)`);
      }
    }
  }
}

function checkStep(step, file, caseName, stepIdx) {
  const where = `step ${stepIdx + 1}`;
  if (!isObject(step)) {
    fail(file, caseName, where, "step is not an object");
    return;
  }
  const keys = Object.keys(step);
  const verbs = keys.filter((k) => !AUX_KEYS.has(k));
  if (verbs.length === 0) {
    fail(file, caseName, where, keys.includes("note")
      ? `note-only step — the DSL requires exactly one verb; note is auxiliary only`
      : `step has no verb (keys: ${keys.join(", ") || "none"})`);
    return;
  }
  if (verbs.length > 1) {
    fail(file, caseName, where, `step carries ${verbs.length} verbs (${verbs.join("+")}) — exactly one per step`);
  }
  for (const v of verbs) {
    if (!DSL_VERBS.has(v)) {
      fail(file, caseName, where, `off-DSL verb "${v}"`);
    }
  }
  // Verb-specific checks
  if (step.http !== undefined && (typeof step.http !== "object" || step.http === null || !step.http.method || !step.http.path)) {
    fail(file, caseName, where, `http is not the {method, path, headers, body} object form`);
  }
  if (step.upgrade !== undefined && !isObject(step.upgrade)) {
    fail(file, caseName, where, `upgrade must be an object`);
  }
  if (step.call !== undefined && !OPERATIONS.has(step.call)) {
    fail(file, caseName, where, `call names unknown operation "${step.call}"`);
  }
  if (step.on !== undefined && (typeof step.on !== "string" || step.on.length === 0)) {
    fail(file, caseName, where, `on must be a non-empty session label`);
  }
  if (step.note !== undefined && typeof step.note !== "string") {
    fail(file, caseName, where, `note must be a string`);
  }
  for (const key of ["stream", "defer"]) {
    if (key in step && step.call === undefined) {
      fail(file, caseName, where, `${key} is only legal on a call step`);
    }
  }
  if (step.defer !== undefined) {
    if (step.defer !== true) {
      fail(file, caseName, where, `defer must be boolean true`);
    }
    if (step.expect !== undefined) {
      fail(file, caseName, where, `a deferred call is matched by await {reply}; put expect on that await step`);
    }
    if (!isObject(step.save) || Object.values(step.save).length !== 1
        || Object.values(step.save)[0] !== "$request") {
      fail(file, caseName, where, `a deferred call must save its request handle as {handle: "$request"}`);
    }
  } else if (isObject(step.save) && Object.values(step.save).includes("$request")) {
    fail(file, caseName, where, `$request may be saved only by a deferred call`);
  }
  if (step.stream !== undefined) {
    if (step.defer === true) {
      fail(file, caseName, where, `stream and defer cannot share one call step`);
    }
    if (!STREAMING_OPS.has(step.call)) {
      fail(file, caseName, where,
        `stream on "${step.call}" — only ${[...STREAMING_OPS.keys()].join(" and ")} stream bytes`);
    } else if (typeof step.stream !== "object" || step.stream === null || Array.isArray(step.stream)) {
      fail(file, caseName, where, `stream is not an object`);
    } else {
      const direction = STREAMING_OPS.get(step.call);
      const got = Object.keys(step.stream);
      if (got.length !== 1 || got[0] !== direction) {
        fail(file, caseName, where,
          `stream on ${step.call} takes exactly {${direction}}, got {${got.join(", ")}}`);
      } else {
        const v = step.stream[direction];
        if (!isValueNode(v)) {
          fail(file, caseName, where,
            `stream.${direction} must be a <uint>, a $variable, or a computed node`);
        }
      }
    }
  }
  if (step.control !== undefined) {
    const c = step.control;
    if (!isObject(c) || !c.do || !CONTROL_DO.has(c.do)) {
      fail(file, caseName, where, `control.do "${c?.do}" not in the closed set of ${CONTROL_DO.size}`);
    } else {
      const exactControlKeys = {
        set_provider_response_bytes: ["do", "file_id", "bytes"],
        client_preflight: ["do", "source_bytes", "source_reports_size"],
        client_render_limit: ["do", "served_bytes"],
        client_render_file: ["do", "declared_content_type", "body_kind"],
        cancel_transfers: ["do", "op_id_prefix"],
      };
      if (c.do === "set_link_rate") {
        if (checkExactKeys(c, ["do", "between", "bits_per_second"], file, caseName, `${where} control`)) {
          if (!Array.isArray(c.between) || c.between.length !== 2
              || c.between.some((label) => typeof label !== "string" || label.length === 0)) {
            fail(file, caseName, where, `set_link_rate.between must contain exactly two non-empty session/provider labels`);
          } else if (c.between[0] === c.between[1]) {
            fail(file, caseName, where, `set_link_rate.between labels must be distinct`);
          }
          if (!isValueNode(c.bits_per_second, { positive: true })) {
            fail(file, caseName, where, `set_link_rate.bits_per_second must be a positive value node`);
          }
        }
      } else if (c.do === "start_transfers") {
        const fileShape = checkExactKeySet(c,
          ["do", "count", "aggregate_bytes", "op_id_prefix"]);
        const legacyShape = LEGACY_REQUEST_CONCURRENCY_CASES.has(caseName)
          && checkExactKeySet(c, ["do", "count"]);
        if (!fileShape && !legacyShape) {
          fail(file, caseName, `${where} control`, file === "files.json"
            ? `start_transfers takes exactly {aggregate_bytes, count, do, op_id_prefix} in files.json`
            : `start_transfers takes either its current legacy {count, do} form or the file-transfer form`);
        } else if (fileShape) {
          if (!isValueNode(c.count, { positive: true })) {
            fail(file, caseName, where, `start_transfers.count must be a positive value node`);
          }
          if (!isValueNode(c.aggregate_bytes)) {
            fail(file, caseName, where, `start_transfers.aggregate_bytes must be a nonnegative value node`);
          }
          if (typeof c.op_id_prefix !== "string" || c.op_id_prefix.length === 0) {
            fail(file, caseName, where, `start_transfers.op_id_prefix must be a non-empty string`);
          }
        } else if (!isValueNode(c.count, { positive: true })) {
          fail(file, caseName, where, `start_transfers.count must be a positive value node`);
        }
      } else if (exactControlKeys[c.do]
          && checkExactKeys(c, exactControlKeys[c.do], file, caseName, `${where} control`)) {
        if (c.do === "set_provider_response_bytes") {
          if (typeof c.file_id !== "string" || c.file_id.length === 0) {
            fail(file, caseName, where, `set_provider_response_bytes.file_id must be a non-empty file identifier or variable`);
          }
          if (!isValueNode(c.bytes)) {
            fail(file, caseName, where, `set_provider_response_bytes.bytes must be a nonnegative value node`);
          }
        } else if (c.do === "client_preflight") {
          if (!isValueNode(c.source_bytes)) {
            fail(file, caseName, where, `client_preflight.source_bytes must be a nonnegative value node`);
          }
          if (typeof c.source_reports_size !== "boolean") {
            fail(file, caseName, where, `client_preflight.source_reports_size must be boolean`);
          }
        } else if (c.do === "client_render_limit" && !isValueNode(c.served_bytes)) {
          fail(file, caseName, where, `client_render_limit.served_bytes must be a nonnegative value node`);
        } else if (c.do === "client_render_file"
            && [c.declared_content_type, c.body_kind].some((value) =>
              typeof value !== "string" || value.length === 0)) {
          fail(file, caseName, where, `client_render_file fields must be non-empty strings`);
        } else if (c.do === "cancel_transfers"
            && (typeof c.op_id_prefix !== "string" || c.op_id_prefix.length === 0)) {
          fail(file, caseName, where, `cancel_transfers.op_id_prefix must be a non-empty string`);
        }
      }
    }
    if (c?.do === "inject_fault" && c.fault !== undefined && !TAXONOMY_CODES.has(c.fault) && !["backpressure", "subscription_lapse", "retention"].includes(c.fault)) {
      fail(file, caseName, where, `inject_fault "${c.fault}" is not a taxonomy code, backpressure, or subscription_lapse`);
    }
  }
  if (step.assert !== undefined) {
    if (!Array.isArray(step.assert)) {
      fail(file, caseName, where, `assert is ${Array.isArray(step.assert) ? "array" : typeof step.assert}, must be an array`);
    } else {
      if (file === "files.json" && step.assert.length === 0) {
        fail(file, caseName, where, `files.json assert arrays must not be empty`);
      }
      step.assert.forEach((a, i) => checkAssertion(a, file, caseName, `${where} assert[${i}]`));
    }
  }
  if (step.expect !== undefined) {
    const e = step.expect;
    if (!isObject(e)) {
      fail(file, caseName, where, `expect must be an object`);
    } else {
      if ("ok" in e && typeof e.ok !== "boolean") {
        fail(file, caseName, where, `expect.ok must be boolean`);
      }
      if (e.ok === false && !("err" in e)) {
        fail(file, caseName, where, `expect {ok:false} without err`);
      }
      if (file === "files.json" && "err" in e) {
        if (typeof e.err?.code !== "string") {
          fail(file, caseName, `${where} expect.err`,
            `file-domain error matcher must carry a literal taxonomy code`);
        } else if (!FILE_ERROR_KEYS.has(e.err.code)) {
          fail(file, caseName, `${where} expect.err`,
            `literal file-domain error code "${e.err.code}" has no canonical matcher schema`);
        } else {
          checkFileError(e.err, file, caseName, `${where} expect.err`);
        }
      }
      checkTypeTags(e, file, caseName, `${where} expect`);
    }
  }
  if (step.in !== undefined) checkTypeTags(step.in, file, caseName, `${where} in`);
  if (step.op_id !== undefined) checkTypeTags(step.op_id, file, caseName, `${where} op_id`);
  if (step.await !== undefined) {
    const a = step.await;
    const awKeys = isObject(a) ? Object.keys(a) : [];
    if (!isObject(a) || awKeys.length !== 1 || !["push", "frame", "reply"].includes(awKeys[0])) {
      fail(file, caseName, where, `await must be exactly one of {push}, {frame}, or {reply}`);
    } else {
      if (!isObject(a[awKeys[0]]) && typeof a[awKeys[0]] !== "string") {
        fail(file, caseName, where, `await.${awKeys[0]} must be an object matcher or string handle`);
      }
      if (awKeys[0] === "reply" && (typeof a.reply !== "string" || a.reply.length === 0)) {
        fail(file, caseName, where, `await.reply must be a non-empty request handle or request id`);
      }
      checkTypeTags(a, file, caseName, `${where} await`);
    }
  }
  if (step.send !== undefined) checkTypeTags(step.send, file, caseName, `${where} send`);
  if (Array.isArray(step.assert)) checkTypeTags(step.assert, file, caseName, `${where} assert`);
  if (step.save !== undefined) {
    if (!isObject(step.save) || Object.keys(step.save).length === 0) {
      fail(file, caseName, where, `save is not a non-empty {name: path} object`);
    } else {
      for (const [varName, path] of Object.entries(step.save)) {
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(varName)) {
          fail(file, caseName, where, `save variable name "${varName}" is invalid`);
        }
        if (typeof path !== "string") {
          fail(file, caseName, where, `save "${varName}" path must be a string`);
          continue;
        }
        if (/^\$(?:result|error)(?:$|[.[])/.test(path)) {
          fail(file, caseName, where, `save "${varName}" uses retired path "${path}" — use out or err`);
          continue;
        }
        const root = path.split(/[.[]/)[0];
        if (!["out", "err", "frame", "$"].includes(root)
            && !/^\$[A-Za-z_][A-Za-z0-9_]*$/.test(root)) {
          fail(file, caseName, where, `save "${varName}" path rooted at "${root}" (must be out/err/frame/$/$variable)`);
        }
      }
    }
  }
  // Retired annotation keys
  for (const k of keys) {
    if (["why", "comment", "intent_note", "meaning", "as", "conn", "session", "client"].includes(k)) {
      fail(file, caseName, where, `retired key "${k}" (annotations collapse into note; actors use on)`);
    }
  }
}

function checkCase(c, file) {
  if (!isObject(c)) {
    fail(file, "(unnamed)", "case", "case is not an object");
    return;
  }
  const name = c.name ?? "(unnamed)";
  const unknownKeys = Object.keys(c).filter((key) => !CASE_KEYS.has(key));
  if (unknownKeys.length > 0) {
    fail(file, name, "case", `unknown case key(s): ${unknownKeys.join(", ")}`);
  }
  if (!c.name || !/^[a-z][a-z0-9_]*$/.test(c.name)) fail(file, name, "case", "name is not snake_case");
  if (!KINDS.has(c.kind)) fail(file, name, "case", `kind "${c.kind}" not in the closed set of ${KINDS.size}`);
  if (c.operation !== null && !OPERATIONS.has(c.operation)) {
    fail(file, name, "case", `operation "${c.operation}" is not one of the 33 and not null`);
  }
  if (typeof c.intent !== "string" || c.intent.length < 20) {
    fail(file, name, "case", "intent missing or too short to name a breaking change");
  }
  if (c.operation === undefined) {
    fail(file, name, "case", "operation must be present and may be null only explicitly");
  }
  if ("targets" in c) {
    if (!Array.isArray(c.targets) || c.targets.length === 0) {
      fail(file, name, "case", "targets must be a non-empty array when present");
    } else {
      const seen = new Set();
      for (const target of c.targets) {
        if (!TARGETS.has(target)) {
          fail(file, name, "case", `unknown target ${JSON.stringify(target)} (closed set: ${[...TARGETS].join(", ")})`);
        } else if (seen.has(target)) {
          fail(file, name, "case", `duplicate target "${target}"`);
        }
        seen.add(target);
      }
    }
  }
  if (!Array.isArray(c.requires)) {
    fail(file, name, "case", "requires is not an array");
  } else {
    const seenRequires = new Set();
    for (const r of c.requires) {
      if (seenRequires.has(r)) fail(file, name, "requires", `duplicate precondition ${JSON.stringify(r)}`);
      seenRequires.add(r);
      if (typeof r !== "string") {
        fail(file, name, "requires", `non-string precondition ${JSON.stringify(r)}`);
        continue;
      }
      if (BARE_REQUIRES.has(r)) continue;
      const [ns, arg] = r.split(":", 2);
      if (!REQUIRE_NAMESPACES.has(ns)) {
        fail(file, name, "requires", `unknown namespace "${ns}" in "${r}" (closed set of ${REQUIRE_NAMESPACES.size})`);
      } else if (arg === undefined && !BARE_REQUIRES.has(ns)) {
        fail(file, name, "requires", `"${r}" lacks namespace:argument form`);
      } else if (REQUIRE_ARGS[ns] && arg !== undefined && !REQUIRE_ARGS[ns].has(arg)) {
        fail(file, name, "requires", `"${r}": namespace "${ns}" permits only [${[...REQUIRE_ARGS[ns]].join(", ")}]`);
      } else if (ns === "fault" && arg !== undefined && !TAXONOMY_CODES.has(arg) && !["backpressure", "subscription_lapse", "retention"].includes(arg)) {
        fail(file, name, "requires", `"${r}": fault takes a taxonomy code, backpressure, or subscription_lapse`);
      }
    }
  }
  if ("blocked_on_upstream" in c && !["U1", "U2", "U3"].includes(c.blocked_on_upstream)) {
    fail(file, name, "case", `blocked_on_upstream is "${c.blocked_on_upstream}" — must be U1, U2, or U3, or the key omitted`);
  }
  if ("blocked_on_record" in c
      && (typeof c.blocked_on_record !== "string" || c.blocked_on_record.length === 0)) {
    fail(file, name, "case", `blocked_on_record must be a non-empty string`);
  }
  if (!Array.isArray(c.steps) || c.steps.length === 0) {
    fail(file, name, "case", "steps missing or empty");
    return;
  }
  if (c.blocked_on_upstream && c.blocked_on_record) {
    fail(file, name, "case", `a case cannot be blocked on upstream and the record simultaneously`);
  }
  checkTypeTags(c.intent, file, name, "intent");
  c.steps.forEach((s, i) => {
    stepCount++;
    checkStep(s, file, name, i);
  });
  const effectiveSession = (step) => step.on ?? "subject:self";
  const deferredHandles = new Map();
  c.steps.forEach((step, i) => {
    if (step?.defer === true && isObject(step.save)) {
      for (const [handle, path] of Object.entries(step.save)) {
        if (path === "$request") {
          const reference = `$${handle}`;
          if (deferredHandles.has(reference)) {
            fail(file, name, `step ${i + 1}`, `deferred request handle ${reference} is reused`);
          } else {
            deferredHandles.set(reference, {
              step: i,
              session: effectiveSession(step),
              terminals: 0,
            });
          }
        }
      }
    }
  });
  c.steps.forEach((step, i) => {
    if (typeof step?.await?.reply === "string" && deferredHandles.has(step.await.reply)) {
      const deferred = deferredHandles.get(step.await.reply);
      if (i <= deferred.step) {
        fail(file, name, `step ${i + 1}`, `await.reply must follow its deferred call`);
      } else if (effectiveSession(step) !== deferred.session) {
        fail(file, name, `step ${i + 1}`,
          `await.reply for ${step.await.reply} is on ${effectiveSession(step)}; deferred handle belongs to ${deferred.session}`);
      } else {
        deferred.terminals++;
      }
    }
    if (step?.control?.do === "disconnect") {
      for (const deferred of deferredHandles.values()) {
        if (i > deferred.step && step.control.on === deferred.session
            && deferred.terminals === 0) deferred.terminals++;
      }
    }
    if (step?.call === "file.fetch" && !("op_id" in step)
        && name !== "fetch_without_envelope_op_id_is_invalid_before_room_or_provider_work") {
      fail(file, name, `step ${i + 1}`, `file.fetch requires envelope op_id`);
    }
    if (step?.call === "file.fetch" && !("op_id" in step)
        && name === "fetch_without_envelope_op_id_is_invalid_before_room_or_provider_work") {
      if (!(step.expect?.ok === false && step.expect?.err?.code === "invalid_argument"
          && step.expect?.err?.field === "op_id"
          && step.expect?.err?.reason?.state === "missing")) {
        fail(file, name, `step ${i + 1}`, `the sole malformed file.fetch omission must expect missing op_id invalid_argument`);
      }
    }
    const daemonApplicable = !Array.isArray(c.targets) || c.targets.includes("daemon");
    if (file === "files.json" && daemonApplicable && STREAMING_OPS.has(step?.call)) {
      const replay = c.steps.slice(0, i).some((prior) => prior?.call === step.call
        && "op_id" in step && "op_id" in prior && sameJson(prior.op_id, step.op_id)
        && sameJson(prior.in, step.in) && prior.stream !== undefined
        && prior.expect !== undefined);
      const preOpen = step.expect?.ok === false
        && (PRE_OPEN_STREAM_CODES.has(step.expect?.err?.code)
          || (step.expect?.err?.code === "file_too_large"
            && ["stage_declared", "authoring"].includes(step.expect.err.enforced_at)));
      const admittedError = step.expect?.ok === false
        && (step.expect?.err?.code === "declared_size_mismatch"
          || (step.expect?.err?.code === "file_too_large"
            && step.expect.err.enforced_at === "stage_stream"));
      if (step.expect?.ok === true && !replay && step.stream === undefined) {
        fail(file, name, `step ${i + 1}`, `fresh admitted ${step.call} with an explicit success result requires stream`);
      }
      if (replay && step.stream !== undefined) {
        fail(file, name, `step ${i + 1}`, `completed faithful replay must not open a second stream`);
      }
      if (preOpen && step.stream !== undefined) {
        fail(file, name, `step ${i + 1}`, `terminal pre-OPEN refusal must not carry stream`);
      }
      if (preOpen) {
        const laterAssertions = c.steps.slice(i + 1).flatMap((later) =>
          Array.isArray(later?.assert) ? later.assert : []);
        const byteObservations = laterAssertions.filter((assertion) =>
          assertion?.observe === "bytes_streamed" && assertion.call === `step:${i + 1}`);
        if (byteObservations.some((assertion) =>
          assertion.value?.op !== "eq" || assertion.value?.value !== 0)) {
          fail(file, name, `step ${i + 1}`, `terminal pre-OPEN refusal can record only zero receiver-accepted bytes`);
        }
      }
      if (admittedError && step.stream === undefined) {
        fail(file, name, `step ${i + 1}`, `explicit admitted stream failure requires stream`);
      }
    }
    for (const assertion of Array.isArray(step?.assert) ? step.assert : []) {
      if (assertion?.observe !== "bytes_streamed" || !("call" in assertion)
          || typeof assertion.call !== "string" || !/^step:[1-9][0-9]*$/.test(assertion.call)) continue;
      const selectedIndex = Number(assertion.call.slice(5)) - 1;
      const selected = c.steps[selectedIndex];
      if (!selected || selected.call === undefined) {
        fail(file, name, `step ${i + 1}`, `bytes_streamed.call selects a step that is not a call`);
      } else if (selectedIndex >= i) {
        fail(file, name, `step ${i + 1}`, `bytes_streamed.call must select an earlier call step`);
      } else if (!["file.share", "file.fetch", "file.read"].includes(selected.call)) {
        fail(file, name, `step ${i + 1}`, `bytes_streamed.call must select a file transfer call`);
      }
    }
  });
  for (const [handle, deferred] of deferredHandles) {
    if (deferred.terminals === 0) {
      fail(file, name, `step ${deferred.step + 1}`,
        `deferred handle ${handle} remains live at case end; await it on ${deferred.session} or disconnect that session`);
    } else if (deferred.terminals !== 1) {
      fail(file, name, `step ${deferred.step + 1}`,
        `deferred handle ${handle} has ${deferred.terminals} terminal paths; exactly one later await or same-session disconnect is required`);
    }
  }
  // Variable-binding contract (files.json): every $name a step reads must be
  // captured by a save on an EARLIER step of this case, or be a documented
  // variable whose binding precondition the case DECLARES in requires.
  // Bindings are case-scoped, so nothing carries over from other cases.
  if (file === "files.json" && Array.isArray(c.steps)) {
    const requires = Array.isArray(c.requires)
      ? c.requires.filter((token) => typeof token === "string") : [];
    const exempt = PRECONDITION_GAP_CASES.get(c.name) ?? new Set();
    const savedAt = new Map();
    c.steps.forEach((step, i) => {
      if (!isObject(step?.save)) return;
      // The runner never applies a save on a send or control step (it
      // returns before the capture logic), so such a save binds nothing at
      // replay time and must not count as a binding here.
      if ("send" in step || "control" in step) {
        fail(file, name, `step ${i + 1}`,
          `save on a ${"send" in step ? "send" : "control"} step captures nothing — the runner applies captures only where a step produces a result (call, http, upgrade, await, assert, or a verbless capture step)`);
        return;
      }
      for (const variable of Object.keys(step.save)) {
        if (!savedAt.has(variable)) savedAt.set(variable, i);
      }
    });
    c.steps.forEach((step, i) => {
      for (const reference of collectStepVariableReferences(step)) {
        const { name: variable, at } = reference;
        if (variable === null) {
          fail(file, name, `step ${i + 1}`,
            `${JSON.stringify(reference.raw)} (${at}) is not a well-formed $variable reference — it resolves against nothing (a value degrades to a literal string; a path names a variable that cannot exist), so it asserts nothing`);
          continue;
        }
        const saved = savedAt.get(variable);
        const savedEarlier = saved !== undefined && saved < i;
        // A save shadows the documented binding with a value of unknown
        // shape; without one, a dotted tail into a documented scalar id
        // resolves against nothing.
        if (!savedEarlier && reference.hasTail && SCALAR_PROVIDED_VARIABLES.has(variable)) {
          fail(file, name, `step ${i + 1}`,
            `"$${variable}" (${at}) carries a dotted tail, but the documented ${variable} binding is a scalar — the tail resolves against nothing`);
          continue;
        }
        if (savedEarlier) continue;
        if (RUNNER_PROVIDED_VARIABLES.has(variable)) continue;
        const binder = PRECONDITION_BINDINGS.get(variable);
        if (binder) {
          if (binder(requires) || exempt.has(variable)) continue;
          fail(file, name, `step ${i + 1}`,
            `"$${variable}" (${at}) names a precondition variable, but this case declares no precondition that binds it and no earlier save captures it`);
        } else if (saved === undefined) {
          fail(file, name, `step ${i + 1}`,
            `"$${variable}" (${at}) is never bound: no save captures it and it is not a documented runner/precondition variable — the harness degrades an unbound reference to a literal string, so the step asserts nothing`);
        } else {
          fail(file, name, `step ${i + 1}`,
            `"$${variable}" (${at}) is used before its save on step ${saved + 1} — a capture binds later steps only`);
        }
      }
    });
  }
  if (PRINCIPAL_ISOLATION_CASES.has(name)) {
    if (c.requires.includes("subject:second")) {
      fail(file, name, "requires",
        `same-daemon principal isolation must not use subject:second, which denotes another daemon/subject`);
    }
    const labels = new Set(c.steps.flatMap((step) => [
      step?.on,
      ...((step?.assert ?? []).map((assertion) => assertion?.on)),
    ]).filter((on) => on === "principal:self" || on === "principal:second"));
    if (!labels.has("principal:self") || !labels.has("principal:second")) {
      fail(file, name, "steps", `principal-isolation case must use both principal:self and principal:second`);
    }
  }
}

const files = readdirSync(DIR).filter((f) => f.endsWith(".json") && f !== "manifest.json").sort();
const corpus = [];
for (const f of files) {
  let data;
  try {
    data = JSON.parse(readFileSync(join(DIR, f), "utf8"));
  } catch (e) {
    fail(f, "(file)", "parse", e.message);
    continue;
  }
  if (!isObject(data)) {
    fail(f, "(file)", "file", "domain file must be an object");
    continue;
  }
  const unknownKeys = Object.keys(data).filter((key) => !TOP_LEVEL_KEYS.has(key));
  if (unknownKeys.length > 0) {
    fail(f, "(file)", "file", `unknown top-level key(s): ${unknownKeys.join(", ")}`);
  }
  if (data.domain !== DOMAIN_BY_FILE[f]) {
    fail(f, "(file)", "file", `domain must be "${DOMAIN_BY_FILE[f]}"`);
  }
  if (typeof data.note !== "string" || data.note.length === 0) {
    fail(f, "(file)", "file", "note must be a non-empty string");
  }
  if ("authoring_notes" in data
      && (!Array.isArray(data.authoring_notes) || data.authoring_notes.length === 0
        || data.authoring_notes.some((note) => typeof note !== "string"))) {
    fail(f, "(file)", "file", "authoring_notes must be a non-empty array of strings when present");
  }
  if (!Array.isArray(data.cases)) {
    fail(f, "(file)", "file", "cases must be an array");
    continue;
  }
  for (const c of data.cases) {
    caseCount++;
    corpus.push({ file: f, case: isObject(c) ? c : {} });
    checkCase(c, f);
  }
}

const names = new Map();
for (const entry of corpus) {
  if (typeof entry.case?.name !== "string") continue;
  if (names.has(entry.case.name)) {
    fail(entry.file, entry.case.name, "case", `duplicate corpus-wide case name also used in ${names.get(entry.case.name)}`);
  } else {
    names.set(entry.case.name, entry.file);
  }
}
const fetchOmissions = corpus.flatMap(({ file, case: c }) =>
  Array.isArray(c.steps) ? c.steps.flatMap((step) =>
    step?.call === "file.fetch" && !("op_id" in step)
      ? [{ file, case: c.name }]
      : []) : []);
const expectedFetchOmissions = [{
  file: "files.json",
  case: "fetch_without_envelope_op_id_is_invalid_before_room_or_provider_work",
}];
fetchOmissions.sort((a, b) => a.file.localeCompare(b.file) || a.case.localeCompare(b.case));
if (!sameJson(fetchOmissions, expectedFetchOmissions)) {
  fail("files.json", "(corpus)", "file.fetch", `op_id omissions must be exactly ${JSON.stringify(expectedFetchOmissions)}`);
}
fixtureProblemCount = problems.length;

let manifest;
try {
  manifest = JSON.parse(readFileSync(join(DIR, "manifest.json"), "utf8"));
} catch (e) {
  fail("manifest.json", "(file)", "parse", e.message);
}

if (isObject(manifest)) {
  const attributed = corpus.filter(({ case: c }) => c.operation !== null).length;
  const operationNull = corpus.length - attributed;
  const usedVerbs = new Set();
  for (const { case: c } of corpus) {
    for (const step of Array.isArray(c.steps) ? c.steps : []) {
      for (const verb of DSL_VERBS) if (Object.hasOwn(step, verb)) usedVerbs.add(verb);
    }
  }
  let ciSelectedCases = 0;
  try {
    const ci = readFileSync(join(DIR, "..", "..", ".github", "workflows", "ci.yml"), "utf8");
    const liveStep = ci.match(/- name: Protocol-v2 conformance replay[\s\S]*?(?=\n      - name:|$)/)?.[0] ?? "";
    const selectors = [...liveStep.matchAll(/--case\s+([^\s\\]+)/g)].map((match) => match[1]);
    ciSelectedCases = selectors.length;
    const seenSelectors = new Set();
    for (const selector of selectors) {
      if (seenSelectors.has(selector)) {
        fail(".github/workflows/ci.yml", selector, "live slice", `duplicate --case selector`);
      }
      seenSelectors.add(selector);
      const matches = corpus.filter(({ case: c }) => c.name === selector).length;
      if (matches !== 1) {
        fail(".github/workflows/ci.yml", selector, "live slice",
          `--case selector must name exactly one corpus case; found ${matches}`);
      }
    }
  } catch (e) {
    fail(".github/workflows/ci.yml", "(file)", "parse", e.message);
  }
  if (ciSelectedCases === 0) {
    fail(".github/workflows/ci.yml", "(file)", "live slice", "no explicit protocol-v2 live cases found");
  }
  const targetCounts = Object.fromEntries([...TARGETS].map((target) => [target, 0]));
  let untargeted = 0;
  for (const { case: c } of corpus) {
    if (Array.isArray(c.targets)) {
      for (const target of c.targets) if (target in targetCounts) targetCounts[target]++;
    } else {
      untargeted++;
    }
  }
  const expectedTargetCounts = {
    untargeted,
    daemon: targetCounts.daemon,
    in_process_core: targetCounts.in_process_core,
    client_adapter: targetCounts.client_adapter,
  };
  if (!sameJson(manifest.target_counts, expectedTargetCounts)) {
    fail("manifest.json", "(manifest)", "target_counts", `does not equal recomputed counts ${JSON.stringify(expectedTargetCounts)}`);
  }
  const blocked = { U1: 0, U2: 0, U3: 0 };
  const blockedOnUpstreamCases = { U1: [], U2: [], U3: [] };
  const blockedOnRecordCases = [];
  for (const { file, case: c } of corpus) {
    if (["U1", "U2", "U3"].includes(c.blocked_on_upstream)) {
      blocked[c.blocked_on_upstream]++;
      blockedOnUpstreamCases[c.blocked_on_upstream].push({ file, case: c.name });
    }
    if (Object.hasOwn(c, "blocked_on_record")) {
      blockedOnRecordCases.push({ file, case: c.name, record: c.blocked_on_record });
    }
  }
  const compareBlockedEntry = (a, b) =>
    a.file.localeCompare(b.file) || a.case.localeCompare(b.case);
  for (const entries of Object.values(blockedOnUpstreamCases)) entries.sort(compareBlockedEntry);
  blockedOnRecordCases.sort(compareBlockedEntry);
  const blockedOnRecord = blockedOnRecordCases.length;
  const expectedTotals = {
    cases: corpus.length,
    attributed_to_an_operation: attributed,
    not_operation_scoped: operationNull,
    conforming_to_the_dsl: fixtureProblemCount === 0 ? corpus.length : null,
    distinct_step_verbs_in_use: usedVerbs.size,
    quarantined: 0,
    blocked_on_upstream: blocked.U1 + blocked.U2 + blocked.U3,
    blocked_on_record: blockedOnRecord,
  };
  if (attributed + operationNull !== corpus.length) {
    fail("manifest.json", "(manifest)", "totals", "operation coverage does not reconcile to total cases");
  }
  const totalKeys = Object.keys(expectedTotals).sort();
  if (!isObject(manifest.totals) || !sameJson(Object.keys(manifest.totals).sort(), totalKeys)) {
    fail("manifest.json", "(manifest)", "totals", `must contain exactly ${totalKeys.join(", ")}`);
  }
  for (const [key, value] of Object.entries(expectedTotals)) {
    if (key === "conforming_to_the_dsl" && value === null) {
      if (!Number.isInteger(manifest.totals?.[key])
          || manifest.totals[key] < 0 || manifest.totals[key] > corpus.length) {
        fail("manifest.json", "(manifest)", `totals.${key}`, `must be an integer in 0..=${corpus.length}`);
      }
    } else if (manifest.totals?.[key] !== value) {
      fail("manifest.json", "(manifest)", `totals.${key}`, `is ${JSON.stringify(manifest.totals?.[key])}; recomputed value is ${value}`);
    }
  }
  if (!isObject(manifest.blocked_on_upstream)
      || !sameJson(Object.keys(manifest.blocked_on_upstream).sort(), ["U1", "U2", "U3"])
      || Object.values(manifest.blocked_on_upstream).some((value) =>
        typeof value !== "string" || value.length === 0)) {
    fail("manifest.json", "(manifest)", "blocked_on_upstream", "must preserve non-empty U1/U2/U3 descriptions");
  }
  const replayability = {
    structural_validation: true,
    selected_json_envelope_subject_slice: {
      status: "partial",
      ci_selected_cases: ciSelectedCases,
    },
    binary_byte_stream_executor: {
      status: "partial",
      send_bytes: "implemented",
      receive_bytes: "implemented_no_executable_case",
      bytes_streamed_observation: "implemented",
      raw_record_and_fault_controls: "unimplemented",
    },
    adapter_targeted_declarative_cases: "declarative_only",
  };
  if (!sameJson(manifest.replayable, replayability)) {
    fail("manifest.json", "(manifest)", "replayable", "must equal the honest selected-slice and executor status from the README matrix");
  }
  if (!isObject(manifest.rules)
      || !sameJson(Object.keys(manifest.rules).sort(), [
        "counts_are_computed", "distinctive_code_exemption", "exemptions_fail",
        "required_per_operation",
      ])
      || typeof manifest.rules.required_per_operation !== "string"
      || typeof manifest.rules.exemptions_fail !== "string"
      || typeof manifest.rules.distinctive_code_exemption !== "string"
      || typeof manifest.rules.counts_are_computed !== "string") {
    fail("manifest.json", "(manifest)", "rules", "must preserve the hand-authored coverage and independence rules");
  }
  if (!isObject(manifest.exemptions)
      || !sameJson(Object.keys(manifest.exemptions), ["room.deactivate"])
      || !isObject(manifest.exemptions["room.deactivate"])
      || !sameJson(Object.keys(manifest.exemptions["room.deactivate"]).sort(), [
        "from", "reason", "reviewed_in",
      ])
      || typeof manifest.exemptions["room.deactivate"].reason !== "string") {
    fail("manifest.json", "(manifest)", "exemptions", "must preserve the hand-authored room.deactivate exemption");
  }
  if (manifest.authored !== "by hand, from the specification. NOT generated from any implementation.") {
    fail("manifest.json", "(manifest)", "authored", "must preserve the independence statement");
  }
  const directCanonicalCodeCases = new Map(
    [...TAXONOMY_CODES].map((code) => [code, new Set()]));
  for (const { file, case: c } of corpus) {
    for (const step of Array.isArray(c.steps) ? c.steps : []) {
      const code = step?.expect?.err?.code;
      if (typeof code !== "string" || !TAXONOMY_CODES.has(code)
          || step.call !== c.operation) continue;
      const canonical = file !== "files.json" || fileErrorIsCanonical(step.expect.err);
      if (canonical) directCanonicalCodeCases.get(code).add(`${file}:${c.name}`);
    }
  }
  const operations = [...OPERATIONS].sort();
  const coverageKeys = isObject(manifest.coverage) ? Object.keys(manifest.coverage).sort() : [];
  if (!sameJson(coverageKeys, operations)) {
    fail("manifest.json", "(manifest)", "coverage", "operation keys do not equal the closed operation set");
  }
  let coverageSum = 0;
  for (const operation of operations) {
    const operationCases = corpus.filter(({ case: c }) => c.operation === operation);
    const count = operationCases.length;
    coverageSum += count;
    const row = manifest.coverage?.[operation];
    const coverageRowKeys = [
      "cases", "distinctive_code", "distinctive_code_has_a_case",
      "satisfies_required_kinds", "success",
    ];
    if (!isObject(row) || !sameJson(Object.keys(row).sort(), coverageRowKeys)) {
      fail("manifest.json", "(manifest)", `coverage.${operation}`, `must contain exactly ${coverageRowKeys.join(", ")}`);
    }
    if (row?.cases !== count) {
      fail("manifest.json", "(manifest)", `coverage.${operation}.cases`, `is ${row?.cases}; recomputed value is ${count}`);
    }
    const hasSuccess = operationCases.some(({ case: c }) => c.kind === "success");
    if (row?.success !== hasSuccess) {
      fail("manifest.json", "(manifest)", `coverage.${operation}.success`, `is ${row?.success}; recomputed value is ${hasSuccess}`);
    }
    if (typeof row?.distinctive_code === "string") {
      const distinctiveCodeHasCase = operationCases.some(({ file, case: c }) =>
        directCanonicalCodeCases.get(row.distinctive_code)?.has(`${file}:${c.name}`) ?? false);
      if (row.distinctive_code_has_a_case !== distinctiveCodeHasCase) {
        fail("manifest.json", "(manifest)", `coverage.${operation}.distinctive_code_has_a_case`, `is ${row.distinctive_code_has_a_case}; recomputed value is ${distinctiveCodeHasCase}`);
      }
    } else if (row?.distinctive_code === null) {
      const exempt = isObject(manifest.exemptions?.[operation]);
      if (row.distinctive_code_has_a_case !== exempt) {
        fail("manifest.json", "(manifest)", `coverage.${operation}.distinctive_code_has_a_case`, `must reflect its named exemption`);
      }
    }
    const satisfiesRequiredKinds = hasSuccess && row?.distinctive_code_has_a_case === true;
    if (row?.satisfies_required_kinds !== satisfiesRequiredKinds) {
      fail("manifest.json", "(manifest)", `coverage.${operation}.satisfies_required_kinds`, `is ${row?.satisfies_required_kinds}; recomputed value is ${satisfiesRequiredKinds}`);
    }
    if (!isObject(row) || typeof row.success !== "boolean"
        || !(typeof row.distinctive_code === "string" || row.distinctive_code === null)
        || typeof row.distinctive_code_has_a_case !== "boolean"
        || typeof row.satisfies_required_kinds !== "boolean") {
      fail("manifest.json", "(manifest)", `coverage.${operation}`, "must preserve success and distinctive-code fields");
    }
  }
  if (coverageSum !== attributed) {
    fail("manifest.json", "(manifest)", "coverage", `coverage rows sum to ${coverageSum}; attributed total is ${attributed}`);
  }
  const unsatisfied = operations.filter((operation) =>
    manifest.coverage?.[operation]?.satisfies_required_kinds !== true);
  if (!sameJson(manifest.operations_not_yet_satisfying_required_kinds, unsatisfied)) {
    fail("manifest.json", "(manifest)", "operations_not_yet_satisfying_required_kinds", `does not equal recomputed list ${JSON.stringify(unsatisfied)}`);
  }
  const byFile = {};
  for (const file of files) {
    const caseNames = corpus
      .filter((entry) => entry.file === file && entry.case.operation === null)
      .map((entry) => entry.case.name)
      .sort();
    if (caseNames.length > 0) byFile[file] = caseNames;
  }
  if (!sameJson(manifest.operation_null_cases?.by_file, byFile)) {
    fail("manifest.json", "(manifest)", "operation_null_cases.by_file", "does not equal the recomputed operation:null case lists");
  }
  if (manifest.operation_null_cases?.count !== operationNull) {
    fail("manifest.json", "(manifest)", "operation_null_cases.count", `is ${manifest.operation_null_cases?.count}; recomputed value is ${operationNull}`);
  }
  const perFile = Object.fromEntries(files.map((file) => {
    const entries = corpus.filter((entry) => entry.file === file);
    const perFileBlocked = { U1: 0, U2: 0, U3: 0 };
    let perFileBlockedOnRecord = 0;
    for (const { case: c } of entries) {
      if (["U1", "U2", "U3"].includes(c.blocked_on_upstream)) {
        perFileBlocked[c.blocked_on_upstream]++;
      }
      if (Object.hasOwn(c, "blocked_on_record")) perFileBlockedOnRecord++;
    }
    return [file, {
      cases: entries.length,
      attributed_to_an_operation: entries.filter(({ case: c }) => c.operation !== null).length,
      not_operation_scoped: entries.filter(({ case: c }) => c.operation === null).length,
      blocked_on_upstream: perFileBlocked,
      blocked_on_record: perFileBlockedOnRecord,
    }];
  }));
  if (!sameJson(manifest.per_file_totals, perFile)) {
    fail("manifest.json", "(manifest)", "per_file_totals", "does not equal the recomputed fixture-file totals");
  }
  const transportRepresentedCodes = new Set();
  for (const [code, fixtureName] of TRANSPORT_CODE_FIXTURES) {
    if (corpus.some(({ case: c }) => c.name === fixtureName)) transportRepresentedCodes.add(code);
  }
  const codesWithoutCase = [...directCanonicalCodeCases.entries()]
    .filter(([code, cases]) => cases.size === 0 && !transportRepresentedCodes.has(code))
    .map(([code]) => code)
    .sort();
  const taxonomyCodes = [...TAXONOMY_CODES].sort();
  if (manifest.taxonomy_code_count !== taxonomyCodes.length) {
    fail("manifest.json", "(manifest)", "taxonomy_code_count", `is ${manifest.taxonomy_code_count}; recomputed value is ${taxonomyCodes.length}`);
  }
  if (!sameJson(manifest.codes_without_a_case?.codes, codesWithoutCase)) {
    fail("manifest.json", "(manifest)", "codes_without_a_case.codes", `does not equal recomputed list ${JSON.stringify(codesWithoutCase)}`);
  }
  const manifestBlocked = manifest.blocked_case_counts;
  const expectedBlocked = { U1: blocked.U1, U2: blocked.U2, U3: blocked.U3, blocked_on_record: blockedOnRecord };
  if (!sameJson(manifestBlocked, expectedBlocked)) {
    fail("manifest.json", "(manifest)", "blocked_case_counts", `does not equal recomputed counts ${JSON.stringify(expectedBlocked)}`);
  }
  if (!sameJson(manifest.blocked_on_upstream_cases, blockedOnUpstreamCases)) {
    fail("manifest.json", "(manifest)", "blocked_on_upstream_cases", "does not equal the recomputed U1/U2/U3 case lists");
  }
  if (!sameJson(manifest.blocked_on_record_cases, blockedOnRecordCases)) {
    fail("manifest.json", "(manifest)", "blocked_on_record_cases", "does not equal the recomputed blocked-on-record case list");
  }
  computed = {
    per_file: perFile,
    totals: { ...expectedTotals, conforming_to_the_dsl: corpus.length },
    target_counts: expectedTargetCounts,
    blocked_case_counts: expectedBlocked,
    ci_selected_cases: ciSelectedCases,
    coverage: Object.fromEntries(operations.map((operation) => [operation, {
      cases: corpus.filter(({ case: c }) => c.operation === operation).length,
      success: corpus.some(({ case: c }) => c.operation === operation && c.kind === "success"),
      distinctive_code_has_a_case: manifest.coverage?.[operation]?.distinctive_code_has_a_case,
    }])),
    coverage_sum: coverageSum,
    taxonomy_code_count: taxonomyCodes.length,
    codes_without_a_case: codesWithoutCase,
    blocked_on_upstream_cases: blockedOnUpstreamCases,
    blocked_on_record_cases: blockedOnRecordCases,
    operation_null_cases: byFile,
  };
} else if (manifest !== undefined) {
  fail("manifest.json", "(file)", "file", "manifest must be an object");
}

if (process.argv.includes("--json")) {
  console.log(JSON.stringify({ cases: caseCount, steps: stepCount, computed, problems }, null, 1));
} else {
  console.log(`v2 corpus: ${caseCount} cases, ${stepCount} steps`);
  if (problems.length === 0) {
    console.log("v2-corpus-check: OK — every case conforms to the DSL");
  } else {
    console.log(`v2-corpus-check: ${problems.length} problem(s)`);
    const byFile = {};
    for (const p of problems) {
      byFile[p.file] ??= [];
      byFile[p.file].push(p);
    }
    for (const [f, ps] of Object.entries(byFile)) {
      console.log(`\n${f} (${ps.length}):`);
      const shown = ps.slice(0, 40);
      for (const p of shown) console.log(`  ${p.case} [${p.where}]: ${p.msg}`);
      if (ps.length > 40) console.log(`  … and ${ps.length - 40} more`);
    }
  }
}
process.exit(problems.length === 0 ? 0 : 1);
