#!/usr/bin/env bash
# Run the jeliya-client browser WebSocket integration tests (#171 §9.2).
#
# Starts a supervised jeliyad instance on a loopback port, reads its portfile,
# forwards the daemon coordinates to build.rs via env vars, then runs the
# wasm-bindgen-test suite in headless Chromium.
#
# Prerequisites:
#   - jeliyad built (cargo build -p jeliyad) or JELIYAD_BIN set to binary path
#   - wasm-bindgen-cli@0.2.126 installed (wasm-bindgen-test-runner on PATH)
#   - chromedriver installed and on PATH, OR CHROMEDRIVER env var set
#   - wasm32-unknown-unknown rustup target added
#
# Usage:
#   bash scripts/run-ws-web-browser-tests.sh [extra cargo test args]
#
# The tests skip gracefully when daemon coordinates are absent at compile time
# (option_env! returns None → early return). This script always provides them.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"

# ---- Locate jeliyad binary -----------------------------------------------
if [ -z "${JELIYAD_BIN:-}" ]; then
    JELIYAD_BIN="$ROOT/target/debug/jeliyad"
fi
if [ ! -x "$JELIYAD_BIN" ]; then
    echo "ERROR: jeliyad not found at $JELIYAD_BIN" >&2
    echo "       Build it first: cargo build -p jeliyad" >&2
    exit 1
fi

# ---- Locate chromedriver --------------------------------------------------
if [ -z "${CHROMEDRIVER:-}" ]; then
    for candidate in chromedriver chromium.chromedriver chromium-browser.chromedriver; do
        if cmd="$(command -v "$candidate" 2>/dev/null)"; then
            CHROMEDRIVER="$cmd"
            break
        fi
    done
fi
if [ -z "${CHROMEDRIVER:-}" ]; then
    echo "ERROR: chromedriver not found; install chromium-driver or set CHROMEDRIVER" >&2
    exit 1
fi
export CHROMEDRIVER

# ---- Start daemon on loopback with an OS-assigned port --------------------
TMPDIR="$(mktemp -d)"
PORTFILE="$TMPDIR/daemon.json"

cleanup() {
    if [ -n "${DAEMON_PID:-}" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        # Give the daemon a moment to close its portfile-locked resources.
        sleep 0.2
    fi
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

"$JELIYAD_BIN" --port 0 --data-dir "$TMPDIR" --no-open --loopback &
DAEMON_PID=$!

# ---- Wait for the portfile ------------------------------------------------
DEADLINE=$(( SECONDS + 20 ))
until [ -f "$PORTFILE" ] && python3 -c "
import json, sys
pf = json.load(open('$PORTFILE'))
sys.exit(0 if pf.get('port') and pf.get('auth_token') else 1)
" 2>/dev/null; do
    if [ $SECONDS -ge $DEADLINE ]; then
        echo "ERROR: daemon did not write a valid portfile within 20 s" >&2
        exit 1
    fi
    sleep 0.1
done

# ---- Extract coordinates from portfile ------------------------------------
DAEMON_PORT="$(python3 -c "import json; pf=json.load(open('$PORTFILE')); print(pf['port'])")"
# auth_token may be wrapped in a Redacted<T> JSON object {"inner":"<value>"}
# or stored as a plain string, depending on the daemon version. Handle both.
DAEMON_TOKEN="$(python3 -c "
import json
pf = json.load(open('$PORTFILE'))
tok = pf['auth_token']
print(tok['inner'] if isinstance(tok, dict) else tok)
")"

echo "ws_web browser tests: jeliyad on port $DAEMON_PORT (pid=$DAEMON_PID)"

# ---- Expose daemon coordinates to build.rs --------------------------------
# build.rs re-emits these as cargo:rustc-env= so option_env!() resolves in
# the wasm32 test binary at compile time.
export JELIYAD_WEB_TEST_PORT="$DAEMON_PORT"
export JELIYAD_WEB_TEST_TOKEN="$DAEMON_TOKEN"

# ---- Run the browser test suite -------------------------------------------
# CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER tells cargo to invoke
# wasm-bindgen-test-runner (from wasm-bindgen-cli@0.2.126) instead of the
# default native test runner. The runner picks up CHROMEDRIVER automatically.
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner

cargo test \
    --locked \
    -p jeliya-client \
    --features ws-web \
    --target wasm32-unknown-unknown \
    --test ws_web_browser \
    "$@"
