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
import { fileURLToPath } from "node:url";

const DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "conformance", "v2");

const DSL_VERBS = new Set(["call", "http", "upgrade", "send", "await", "control", "assert"]);
const AUX_KEYS = new Set(["in", "op_id", "on", "expect", "save", "note"]);
const KINDS = new Set(["success", "error", "malformed", "boundary", "authorization", "handshake", "push", "ordering"]);
const CONTROL_DO = new Set([
  "advance_clock", "idle", "disconnect", "reconnect", "inject_fault",
  "set_limit", "stop_daemon", "start_daemon", "start_transfers", "pause_link",
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
  "pipe_id", "op_id", "ts", "uint", "bool", "string",
  "pos", "capability", "daemon_sg", "port", "object", "any", "version",
  "standing", "link_connected", "link_reason",
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
  "transfer_stalled", "transfer_unknown", "transport_unavailable", "unauthenticated", "unknown_operation",
]);

const problems = [];
let caseCount = 0;
let stepCount = 0;

function fail(file, caseName, where, msg) {
  problems.push({ file, case: caseName, where, msg });
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
      checkTypeTags(v, file, caseName, where);
    }
  }
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
    }
    if (a.observe === "close_code" && (!("value" in a) || !("on" in a))) {
      fail(file, caseName, where, `close_code requires both value and on`);
    }
    if (a.observe === "push_count" && !("value" in a)) {
      fail(file, caseName, where, `push_count requires value`);
    }
    if (a.observe === "timing_indistinguishable" && !("between" in a)) {
      fail(file, caseName, where, `timing_indistinguishable requires between`);
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
  if ("path" in a && typeof a.path === "string") {
    const root = a.path.split(/[.[]/)[0];
    if (!["out", "err", "frame"].includes(root) && !root.startsWith("$")) {
      fail(file, caseName, where, `assert path rooted at "${root}" (must be out/err/frame/$variable)`);
    }
  }
}

function checkStep(step, file, caseName, stepIdx) {
  const where = `step ${stepIdx + 1}`;
  if (typeof step !== "object" || step === null) {
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
  if (step.call !== undefined && !OPERATIONS.has(step.call)) {
    fail(file, caseName, where, `call names unknown operation "${step.call}"`);
  }
  if (step.control !== undefined) {
    const c = step.control;
    if (!c.do || !CONTROL_DO.has(c.do)) {
      fail(file, caseName, where, `control.do "${c.do}" not in the closed set of ${CONTROL_DO.size}`);
    }
    if (c.do === "inject_fault" && c.fault !== undefined && !TAXONOMY_CODES.has(c.fault) && !["backpressure", "subscription_lapse", "retention"].includes(c.fault)) {
      fail(file, caseName, where, `inject_fault "${c.fault}" is not a taxonomy code, backpressure, or subscription_lapse`);
    }
  }
  if (step.assert !== undefined) {
    if (!Array.isArray(step.assert)) {
      fail(file, caseName, where, `assert is ${Array.isArray(step.assert) ? "array" : typeof step.assert}, must be an array`);
    } else {
      step.assert.forEach((a, i) => checkAssertion(a, file, caseName, `${where} assert[${i}]`));
    }
  }
  if (step.expect !== undefined) {
    const e = step.expect;
    if (typeof e === "object" && e !== null) {
      if ("ok" in e) {
        if (e.ok === true && !("out" in e)) {
          // ok:true without out is legal (success, unconstrained) — no-op
        }
        if (e.ok === false && !("err" in e)) {
          fail(file, caseName, where, `expect {ok:false} without err`);
        }
      }
      checkTypeTags(e, file, caseName, `${where} expect`);
    }
  }
  if (step.in !== undefined) checkTypeTags(step.in, file, caseName, `${where} in`);
  if (step.await !== undefined) {
    const a = step.await;
    const awKeys = typeof a === "object" && a !== null ? Object.keys(a) : [];
    if (typeof a !== "object" || a === null || awKeys.length !== 1 || !["push", "frame", "reply"].includes(awKeys[0])) {
      fail(file, caseName, where, `await must be exactly one of {push}, {frame}, or {reply}`);
    } else {
      checkTypeTags(a, file, caseName, `${where} await`);
    }
  }
  if (step.send !== undefined) checkTypeTags(step.send, file, caseName, `${where} send`);
  if (Array.isArray(step.assert)) checkTypeTags(step.assert, file, caseName, `${where} assert`);
  if (step.save !== undefined) {
    if (typeof step.save !== "object" || step.save === null || Array.isArray(step.save)) {
      fail(file, caseName, where, `save is not a {name: path} object`);
    } else {
      for (const [varName, path] of Object.entries(step.save)) {
        if (typeof path === "string") {
          const root = path.split(/[.[]/)[0];
          if (!["out", "err", "frame"].includes(root) && !root.startsWith("$")) {
            fail(file, caseName, where, `save "${varName}" path rooted at "${root}" (must be out/err/frame)`);
          }
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
  const name = c.name ?? "(unnamed)";
  if (!c.name || !/^[a-z][a-z0-9_]*$/.test(c.name)) fail(file, name, "case", "name is not snake_case");
  if (!KINDS.has(c.kind)) fail(file, name, "case", `kind "${c.kind}" not in the closed set of ${KINDS.size}`);
  if (c.operation !== null && !OPERATIONS.has(c.operation)) {
    fail(file, name, "case", `operation "${c.operation}" is not one of the 33 and not null`);
  }
  if (typeof c.intent !== "string" || c.intent.length < 20) {
    fail(file, name, "case", "intent missing or too short to name a breaking change");
  }
  if (!Array.isArray(c.requires)) {
    fail(file, name, "case", "requires is not an array");
  } else {
    for (const r of c.requires) {
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
  if ("blocked_on_upstream" in c && c.blocked_on_upstream !== null && !["U1", "U2", "U3"].includes(c.blocked_on_upstream)) {
    fail(file, name, "case", `blocked_on_upstream is "${c.blocked_on_upstream}" — must be U1, U2, or U3, or the key omitted`);
  }
  if (!Array.isArray(c.steps) || c.steps.length === 0) {
    fail(file, name, "case", "steps missing or empty");
    return;
  }
  c.steps.forEach((s, i) => {
    stepCount++;
    checkStep(s, file, name, i);
  });
}

const files = readdirSync(DIR).filter((f) => f.endsWith(".json") && f !== "manifest.json").sort();
for (const f of files) {
  let data;
  try {
    data = JSON.parse(readFileSync(join(DIR, f), "utf8"));
  } catch (e) {
    fail(f, "(file)", "parse", e.message);
    continue;
  }
  const cases = Array.isArray(data) ? data : data.cases ?? [];
  for (const c of cases) {
    caseCount++;
    checkCase(c, f);
  }
}

if (process.argv.includes("--json")) {
  console.log(JSON.stringify({ cases: caseCount, steps: stepCount, problems }, null, 1));
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
