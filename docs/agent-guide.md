---
type: "Guide"
title: "The Jeliya real agent"
description: "Operational and security guide for running the room-driven Jeliya agent."
tags: ["agents", "operations", "runner", "security"]
timestamp: "2026-07-12T19:25:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "partial"
release_status: "partial"
audience: ["contributors", "maintainers", "operators"]
---

# The Jeliya real agent

`scripts/jeliya-agent.mjs` turns a room into a place where an AI agent does
real work. It spawns its own `jeliyad`, joins a room by invite ticket, and
watches chat: any message from an allowed sender that starts with the trigger
phrase (default `@agent`) becomes a task. The task runs in a fresh workspace,
progress is posted as honest `agent_status` events, artifacts are shared as
room files, and the result lands back in chat.

Node 22+, zero npm deps. The daemon binary is `target/debug/jeliyad`
(override with the `JELIYAD` env var, same as the realnet scripts).

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

> **Installed `jeliyad` via Homebrew or the install script?** You don't need
> to build the workspace — but the agent runner is a repo script, so you
> still need a checkout: `git clone` the repo, skip step 0, and prefix every
> `node scripts/jeliya-agent.mjs …` command with
> `JELIYAD="$(command -v jeliyad)"` so the runner uses your installed daemon
> instead of `target/debug/jeliyad`. (Node 22+ required; no `npm install`
> needed — the scripts have no dependencies.)

```bash
# 0. Build once
cargo build --workspace

# 1. On the agent machine: create the agent identity, note the printed id
node scripts/jeliya-agent.mjs --identity-only

# 2. In the Jeliya UI (or over WS: invite.create with role "agent" and the
#    identity id from step 1), mint an agent-role invite and copy the ticket
#    plus the room's dial addr shown by room.open.

# 3. Run the agent for real (claude CLI must be on PATH)
node scripts/jeliya-agent.mjs --ticket <TICKET> --peer <ID@IP:PORT,...> --worker claude

# 4. In the room chat:
#    @agent write a haiku about NAT traversal into haiku.txt
```

`--worker claude` above is a deliberate opt-in to real execution (see trust
model). To try the flow first without it, drop the flag or pass `--worker
echo` — the safe, inert default.

The agent announces itself in chat when it comes online. On Ctrl-C/SIGTERM it
kills any in-flight claude worker (the whole process group), posts a
best-effort `failed` status for the aborted task plus an `offline` status,
gives its daemon a short grace to flush them to peers, kills it and exits 0.

If the agent identity is already a room member (e.g. the runner was
restarted), skip the ticket and rejoin:

```bash
node scripts/jeliya-agent.mjs --room <room_id> --worker claude
```

> **Resolved: stale dial address after `file.share`** (upstream issue #84,
> fixed as of SDK rev `3cb9bfd1e43eb755c967315c37b6d4fd1c2bf020`). `file.share`
> now imports into the blob store **on the live session** (`node.blob_import`),
> reusing the store handle the node already holds. There is no second store
> open, no node shutdown/respawn, and **no endpoint rebind** — the UDP port and
> dial address stay valid across a share, exactly like `message.send`. A dial
> address captured before a share remains dialable afterward, so you no longer
> need to re-fetch it before minting an invite or handing it to a joiner. A
> controlled real-mode probe confirms it: the endpoint id and every bound UDP
> port are identical before and after `file.share` (only extra reflexive
> address candidates get added by normal network discovery, reusing the same
> ports).

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `--ticket <T>` | — | Invite ticket to join with (join mode) |
| `--peer <ADDRS>` | — | Dial addr hint `<endpoint_id>@<ip:port,...>` from the inviter's `room.open`; repeatable |
| `--room <room_id>` | — | With `--ticket`: assert the joined room. Alone: rejoin mode (already a member — skip join, just open) |
| `--port <n>` | `7461` | WS port for the spawned `jeliyad` |
| `--data-dir <dir>` | OS data directory¹ | Daemon data dir; the identity persists here across runs. Keep explicit paths outside source checkouts |
| `--worker claude\|echo` | `echo` | `claude`: real work via the claude CLI. `echo`: deterministic test worker |
| `--workspace <dir>` | fresh OS temp dir | Parent dir; each task gets a fresh numbered subdir. The default is deliberately OUTSIDE `--data-dir` (which holds `identity.secret`) — keep any override outside it too |
| `--trigger <phrase>` | `@agent` | Task trigger; matched case-insensitively at the start of a message |
| `--allow-sender <64hex>` | room owner | Allowlisted sender identity; repeatable. See trust model |
| `--max-turns <n>` | `40` | Passed to `claude --max-turns` |
| `--loopback` | off | Spawn the daemon in loopback mode (must match the room's other daemons; used by the e2e) |
| `--identity-only` | off | Print the identity id for invite minting, then exit |

¹ Defaults: macOS `~/Library/Application Support/Jeliya/agents/default`,
Windows `%APPDATA%\\Jeliya\\agents\\default`, and Linux/other Unix
`$XDG_DATA_HOME/jeliya/agents/default` when set or
`~/.local/share/jeliya/agents/default` otherwise. The runner also writes a
deny-all `.gitignore` inside every data directory before the daemon starts,
so a custom path placed under a Git worktree cannot expose `identity.secret`
through an accidental `git add`. An existing marker without those deny-all
rules makes the runner fail closed.

## Workers

A worker is `async worker(task, ctx) -> { summary, artifacts }` with
`ctx = { workspace, postStatus(label, message), log }` — see the registry in
`scripts/jeliya-agent.mjs` to add one.

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
6. **Status labels are an English-token contract for display tone.** The UI
   colors a status chip/dot from known English tokens only (`done`, `working`,
   `failed`, `awaiting_review`, …; see `labelTone` in
   `dart/jeliya_protocol/lib/src/conventions/format.dart`, normative in
   `docs/PROTOCOL.md`).
   A label it can't read — including any non-English label — renders in a
   neutral tone, never green: the healthy color is earned, not a fallback.
   Idle-class labels (`idle`, `offline`, `claiming`) also render neutral by
   design — neutral is not an error state, it just isn't the earned green.
   Liveness itself comes from the daemon's derived states (§1.2 of
   `docs/agent-orchestration.md`), not from label color.

## Proof

```bash
node scripts/agent-e2e.mjs   # 28 hard assertions, loopback only, no LLM
```

Covers: identity/invite/join, online + announce, a real task round-trip
(working status → `file_shared` → `file.fetch` `verified:true` + exact
content → summary message → `done` with progress 100 and the artifact id),
the trust model (the runner provably receives the non-allowed member's
trigger — its SECURITY-ignored log line — and produces zero agent events and
zero files), loop survival after the ignored trigger, and clean SIGTERM
shutdown (`offline` status, exit 0).
