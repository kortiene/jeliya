#!/usr/bin/env bash
# Jeliya developer demo:
#   1. builds the workspace and the web UI
#   2. starts the human daemon on ws://127.0.0.1:7420/ws (data: .jeliya-demo/human),
#      serving the built UI — your browser opens into the live room by itself
#   3. starts a simulated agent (its own daemon on 7421) that joins the demo
#      room and posts periodic agent.status updates
#
# Ctrl-C stops everything. Data persists in .jeliya-demo/ across runs;
# `rm -rf .jeliya-demo` for a fresh demo.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HUMAN_PORT="${HUMAN_PORT:-7420}"
AGENT_PORT="${AGENT_PORT:-7421}"
DEMO_DIR="$REPO_ROOT/.jeliya-demo"

cd "$REPO_ROOT"

echo "demo: building the workspace…"
cargo build --workspace

echo "demo: building the web UI…"
if [ ! -d ui/node_modules ]; then
  (cd ui && npm ci --silent)
fi
(cd ui && npm run build --silent)

mkdir -p "$DEMO_DIR/human"

PIDS=()
cleanup() {
  echo
  echo "demo: shutting down…"
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "demo: starting the human daemon on ws://127.0.0.1:$HUMAN_PORT/ws"
# --ui-dir makes the debug daemon serve the built UI. --no-open on purpose:
# the browser opens below, AFTER demo-agent has created the identity and
# room, so the first page load lands in the live room instead of racing
# the bootstrap and parking on onboarding.
"$REPO_ROOT/target/debug/jeliyad" \
  --loopback --port "$HUMAN_PORT" --data-dir "$DEMO_DIR/human" \
  --ui-dir "$REPO_ROOT/ui/dist" --no-open &
PIDS+=($!)

# The agent orchestrator (creates identities/room as needed, spawns the agent
# daemon on $AGENT_PORT, joins it to the room, posts statuses forever).
node "$REPO_ROOT/scripts/demo-agent.mjs" \
  --human-port "$HUMAN_PORT" \
  --agent-port "$AGENT_PORT" \
  --agent-data-dir "$DEMO_DIR/agent" &
PIDS+=($!)

sleep 3
UI_URL="http://127.0.0.1:$HUMAN_PORT/"
open "$UI_URL" 2>/dev/null || xdg-open "$UI_URL" 2>/dev/null || true

cat <<EOF

============================================================
 Jeliya demo is running.

   Daemon (human):  ws://127.0.0.1:$HUMAN_PORT/ws
   Daemon (agent):  ws://127.0.0.1:$AGENT_PORT/ws
   Data:            $DEMO_DIR

 Your browser should have opened http://127.0.0.1:$HUMAN_PORT/
 by itself (open it manually if not). If the page shows
 onboarding instead of the demo room, the setup was still
 warming up — reload once.

 Iterating on UI code? Run the dev server instead:
   cd ui && npm run dev
   open http://localhost:5173/?daemon=$HUMAN_PORT

 The room "Build Iroh Rooms MVP" fills with agent statuses
 every few seconds. Ctrl-C here stops the demo.
============================================================

EOF

wait
