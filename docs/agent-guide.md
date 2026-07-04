# The Bantaba real agent

`scripts/bantaba-agent.mjs` turns a room into a place where an AI agent does
real work. It spawns its own `bantabad`, joins a room by invite ticket, and
watches chat: any message from an allowed sender that starts with the trigger
phrase (default `@agent`) becomes a task. The task runs in a fresh workspace,
progress is posted as honest `agent_status` events, artifacts are shared as
room files, and the result lands back in chat.

Node 22+, zero npm deps. The daemon binary is `target/debug/bantabad`
(override with the `BANTABAD` env var, same as the realnet scripts).

## Trust model — read this first

**This is room-driven code execution.** With `--worker claude`, every allowed
sender's triggered message becomes a prompt to the `claude` CLI running with
`--permission-mode acceptEdits` — that sender can make an LLM read, write and
create files inside the per-task workspace on the machine running the agent,
and reach anything the `claude` process can reach. Treat "allowed sender" as
"person I would hand a shell to".

The gate is a sender allowlist, checked on the signed room event's sender
identity:

- Allowed senders are **exactly** the identities passed via `--allow-sender`
  (repeatable, 64-hex identity ids).
- If no `--allow-sender` is given, the allowlist defaults to **exactly one
  identity: the sender of the `room_created` event** in the timeline — the
  room owner. Nobody else, regardless of role.
- A triggered message from any non-allowed sender is **ignored and logged
  locally. It is never executed and gets no reply of any kind** — a probing
  sender cannot learn whether the trigger, the agent, or the allowlist exist
  (no oracle).
- The agent never reacts to its own events or to non-message event kinds.
- Trigger messages timestamped before the runner started (minus 60s of clock
  slack) are ignored, and a message with a missing/non-numeric timestamp is
  treated as stale (fail closed): no backlog execution on (re)join.
- One task at a time. A trigger while busy gets a "busy, not queued" reply
  (allowed senders only) and is dropped, so a task cannot queue up work behind
  itself.

`scripts/agent-e2e.mjs` proves the allowlist deterministically: a room member
who is *not* on the allowlist sends a triggered message and the agent
provably does nothing — no execution, no status, no reply, no file.

## Quickstart

```bash
# 0. Build once
cargo build --workspace

# 1. On the agent machine: create the agent identity, note the printed id
node scripts/bantaba-agent.mjs --identity-only

# 2. In the Bantaba UI (or over WS: invite.create with role "agent" and the
#    identity id from step 1), mint an agent-role invite and copy the ticket
#    plus the room's dial addr shown by room.open.

# 3. Run the agent for real (claude CLI must be on PATH)
node scripts/bantaba-agent.mjs --ticket <TICKET> --peer <ID@IP:PORT,...> --worker claude

# 4. In the room chat:
#    @agent write a haiku about NAT traversal into haiku.txt
```

The agent announces itself in chat when it comes online. On Ctrl-C/SIGTERM it
kills any in-flight claude worker (the whole process group), posts a
best-effort `failed` status for the aborted task plus an `offline` status,
gives its daemon a short grace to flush them to peers, kills it and exits 0.

If the agent identity is already a room member (e.g. the runner was
restarted), skip the ticket and rejoin:

```bash
node scripts/bantaba-agent.mjs --room <room_id> --worker claude
```

> **Known daemon limitation (join ordering):** `room.join` currently only
> bootstraps into a room whose event log is membership-only. Once any chat
> message / status / file event exists, later joins fail with
> `peer_unreachable` ("could not reach the room admin…"). Reproduced with
> plain three-daemon probes in both loopback and real mode — it is a
> `bantaba-core`/SDK join-bootstrap bug, not an agent-harness one. Practical
> guidance until it is fixed: **invite and join the agent before the room's
> first message.** Restarts of an already-joined agent (`--room` rejoin mode)
> are unaffected.

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `--ticket <T>` | — | Invite ticket to join with (join mode) |
| `--peer <ADDRS>` | — | Dial addr hint `<endpoint_id>@<ip:port,...>` from the inviter's `room.open`; repeatable |
| `--room <room_id>` | — | With `--ticket`: assert the joined room. Alone: rejoin mode (already a member — skip join, just open) |
| `--port <n>` | `7461` | WS port for the spawned `bantabad` |
| `--data-dir <dir>` | `.bantaba-agent` | Daemon data dir; the identity persists here across runs |
| `--worker claude\|echo` | `claude` | `claude`: real work via the claude CLI. `echo`: deterministic test worker |
| `--workspace <dir>` | fresh OS temp dir | Parent dir; each task gets a fresh numbered subdir. The default is deliberately OUTSIDE `--data-dir` (which holds `identity.secret`) — keep any override outside it too |
| `--trigger <phrase>` | `@agent` | Task trigger; matched case-insensitively at the start of a message |
| `--allow-sender <64hex>` | room owner | Allowlisted sender identity; repeatable. See trust model |
| `--max-turns <n>` | `40` | Passed to `claude --max-turns` |
| `--loopback` | off | Spawn the daemon in loopback mode (must match the room's other daemons; used by the e2e) |
| `--identity-only` | off | Print the identity id for invite minting, then exit |

## Workers

A worker is `async worker(task, ctx) -> { summary, artifacts }` with
`ctx = { workspace, postStatus(label, message), log }` — see the registry in
`scripts/bantaba-agent.mjs` to add one.

- **echo** — writes `result.txt` containing `echo: <task>`, posts one
  `working` status, returns `echoed <n> bytes`. Exists so the e2e proves the
  whole harness with no LLM and no network beyond loopback.
- **claude** — spawns `claude -p "<task>" --output-format stream-json
  --verbose --max-turns <N> --permission-mode acceptEdits` with the task
  workspace as cwd, streams the NDJSON, and posts a throttled `working`
  status naming the tool actually being run (`running: Bash (tool call 7)` —
  an honest count of observed tool invocations, not the CLI's turn number).
  Hard cap 15 minutes per task. Files left in the workspace become artifacts.

After the worker returns, the harness shares the artifacts as room files
(capped at 8; anything over the 100 MiB share limit is skipped and noted),
posts the summary in chat, then posts the terminal status. Because the daemon
confines `file.share` to its data dir, artifacts living elsewhere are staged
via a copy under `<data-dir>/share/<task>/` first.

## Honesty rules

These bind the agent the same way the daemon's honesty rules bind the UI:

1. **No fabricated progress.** Statuses carry no progress number at all,
   except the literal `100` on the final `done` status. A percentage the
   agent cannot measure is a lie.
2. **Statuses reflect real activity** — the `working` message names the tool
   invocation actually observed in the claude stream, throttled to at most
   one status per 15s per task.
3. **Failures are failures.** Nonzero exit, stream parse failure or the
   15-minute cap post `failed` with the real reason; artifacts that exist are
   still shared.
4. **Chat is not a dumping ground.** Results longer than the 16 KiB message
   limit go to a `result.md` artifact; the chat message carries the head of
   the text and says so.
5. All outbound strings are truncated to the wire limits (status label 64 B,
   status message 4 KiB, message body 16 KiB) instead of erroring.

## Proof

```bash
node scripts/agent-e2e.mjs   # 25 hard assertions, loopback only, no LLM
```

Covers: identity/invite/join, online + announce, a real task round-trip
(working status → `file_shared` → `file.fetch` `verified:true` + exact
content → summary message → `done` with progress 100 and the artifact id),
the trust model (the runner provably receives the non-allowed member's
trigger — its SECURITY-ignored log line — and produces zero agent events and
zero files), loop survival after the ignored trigger, and clean SIGTERM
shutdown (`offline` status, exit 0).
