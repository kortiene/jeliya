# Bantaba

A private peer-to-peer workspace for humans and AI agents, built on the
[Iroh Rooms](https://github.com/kortiene/iroh-room) runtime. *Bantaba* is the
Mandinka word for the gathering place under the village meeting tree.

Bantaba is the product layer; Iroh Rooms is the engine. Everything Bantaba
renders — chat timeline, shared files, live pipes, agent statuses — is a fold
over Iroh Rooms' signed event log. No central server holds your rooms.

## Layout

| Path | What |
|---|---|
| `crates/bantaba-core` | The only consumer of the `iroh-rooms` SDK: room supervisor (one node per open room), event materializer (log → view-models), local state (room names, read markers) |
| `crates/bantabad` | The resident daemon: local-only WebSocket API over `bantaba-core` (see `docs/PROTOCOL.md`) |
| `ui/` | The Bantaba shell: Vite + React, implements `mockups/` |
| `docs/PROTOCOL.md` | The daemon ⇄ shell contract (the spine) |
| `mockups/` | The original product mockups the UI is built to |
| `scripts/` | Harnesses: the two-daemon loopback demo + e2e, the real-agent runner (real network stack by default — see `docs/agent-guide.md`) with its three-daemon agent e2e, and the two-machine realnet NAT scripts |

## Quickstart

```bash
# 1. Build and start the daemon (loopback mode for local demos)
cargo run -p bantabad -- --loopback --port 7420 --data-dir .bantaba-data

# 2. Start the shell
cd ui && npm install && npm run dev
# open http://localhost:5173

# Full two-peer demo (two daemons, invite/join, messages, file, agent status):
scripts/demo.sh
```

### Real agent

`scripts/bantaba-agent.mjs` joins a room as a working agent: chat messages
starting with a trigger (default `@agent`) from allowlisted senders become
tasks run by a worker — the `claude` CLI for real work, or a deterministic
`echo` worker used by the proof (`node scripts/agent-e2e.mjs`). Statuses,
artifacts and results are posted back to the room honestly. Trust model
(this is room-driven code execution — read it) and quickstart:
`docs/agent-guide.md`.

## Architecture

See the sketch this implementation follows: `docs/PROTOCOL.md` for the
contract; one `bantaba-core` room session per open room wraps
`Node`/`SyncEngine`/`EventStore` from the SDK's experimental tier, and the
daemon bridges poll-based reads into WebSocket pushes. The SDK's experimental
tier may change on any release — nothing outside `bantaba-core` imports it.

Honesty rules (from the runtime, kept visible in the UI): best-effort P2P
delivery (no fake "delivered"), truthful direct/relay path badges, and file
fetch failures surface as typed states (`unavailable` / `unauthorized` /
`hash_mismatch`), never silent partials.
