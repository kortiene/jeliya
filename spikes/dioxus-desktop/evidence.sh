#!/usr/bin/env bash
# End-to-end evidence for issue #159, against the REAL built shell binary.
#
# The headless tests in `tests/supervision.rs` compile the supervisor into a
# test binary and never launch a WebView, so they cannot see anything that only
# breaks in the real process. This script drives the actual executable.
#
# It deliberately starts jeliyad ITSELF and lets the shell adopt it. Two reasons:
#   1. the harness then knows the auth token (the portfile stays readable for
#      the whole run), which is what makes the token-absence assertion possible;
#   2. it exercises the adopted path in the real binary, where getting it wrong
#      means killing a daemon that belongs to someone else.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
shell_bin="$here/target/debug/jeliya-spike-dioxus-desktop"
jeliyad="$repo/target/debug/jeliyad"
work="$(mktemp -d)"
data="$work/data"
dom="$work/dom.html"
probe="$work/probe.json"
fail=0

cleanup() {
  # Both children, on EVERY exit path. An early failure (no portfile, an
  # interrupt) used to leave `sleep 600` holding the unlinked FIFO open for ten
  # minutes, so repeated failed runs accumulated orphans.
  [[ -n "${DAEMON_PID:-}"  ]] && kill "$DAEMON_PID"  2>/dev/null
  [[ -n "${KEEPALIVE:-}"   ]] && kill "$KEEPALIVE"   2>/dev/null
  [[ -n "${DAEMON_PID:-}"  ]] && wait "$DAEMON_PID"  2>/dev/null
  [[ -n "${KEEPALIVE:-}"   ]] && wait "$KEEPALIVE"   2>/dev/null
  rm -rf "$work"
  return 0
}
trap cleanup EXIT INT TERM

check() { # check <name> <condition-result>
  if [[ "$2" == "0" ]]; then echo "  PASS  $1"; else echo "  FAIL  $1"; fail=1; fi
}

[[ -x "$shell_bin" ]] || { echo "build first: cargo build"; exit 2; }
[[ -x "$jeliyad" ]]   || { echo "build first: cargo build -p jeliyad (from the repo root)"; exit 2; }

mkdir -p "$data"
echo "==> starting a daemon the harness owns (the shell must ADOPT it)"
# Keep stdin open via a long-lived FIFO writer, or --supervised exits at once.
mkfifo "$work/keepalive"
sleep 600 > "$work/keepalive" &
KEEPALIVE=$!
"$jeliyad" --supervised --data-dir "$data" --port 0 < "$work/keepalive" > "$work/daemon.out" 2>"$work/daemon.err" &
DAEMON_PID=$!

for _ in $(seq 1 60); do [[ -s "$data/daemon.json" ]] && break; sleep 0.25; done
[[ -s "$data/daemon.json" ]] || { echo "daemon never wrote a portfile"; cat "$work/daemon.err"; exit 2; }

token=$(python3 -c "import json;print(json.load(open('$data/daemon.json'))['auth_token'])")
dpid=$(python3 -c "import json;print(json.load(open('$data/daemon.json'))['pid'])")
echo "    daemon pid $dpid, token captured (${#token} chars)"

echo "==> launching the shell against that data dir"
JELIYA_DATA_DIR="$data" SPIKE_RENDER_PROBE=1 SPIKE_PROBE_DOM="$dom" \
  DISPLAY="${DISPLAY:-:1}" timeout 40 "$shell_bin" > "$probe.raw" 2>"$work/shell.err"
shell_exit=$?

grep -o 'SPIKE_RENDER_PROBE .*' "$probe.raw" | sed 's/^SPIKE_RENDER_PROBE //' > "$probe" || true

echo
echo "==> assertions"
[[ -s "$probe" ]]; check "the shell produced render evidence" $?

python3 - "$probe" "$dom" "$token" "$dpid" <<'PY'
import json, sys, re
probe, dom_path, token, dpid = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
d = json.load(open(probe))
dom = open(dom_path).read()
def check(name, ok):
    print(("  PASS  " if ok else "  FAIL  ") + name)
    if not ok: sys.exit(1)

# Rendering, measured rather than assumed.
check("the heading has non-zero laid-out geometry",
      d.get("heading_width", 0) > 100 and d.get("heading_height", 0) > 8)
check("the stylesheet applied (weight 700, not a browser default)",
      d.get("font_weight") == "700")
check("the dark theme applied (body background is not transparent/white)",
      d.get("body_bg") not in (None, "rgba(0, 0, 0, 0)", "rgb(255, 255, 255)"))

# The shell adopted rather than started its own.
check("the shell reports the ADOPTED daemon", "adopted" in d.get("heading_text", ""))
check("it rendered the harness daemon's pid", d.get("pid_text") == dpid)

# The security property no screenshot can show.
check("the auth token is absent from the entire rendered DOM", token not in dom)
check("no 64-hex string at all appears in the DOM",
      re.search(r"[0-9a-f]{64}", dom) is None)
PY
[[ $? == 0 ]]; check "render + token assertions" $?

echo "==> the adopted daemon must have survived the shell"
sleep 1
kill -0 "$DAEMON_PID" 2>/dev/null; check "the adopted daemon is still alive" $?
[[ -f "$data/daemon.json" ]]; check "its portfile is intact" $?

echo
if [[ $fail == 0 ]]; then echo "ALL EVIDENCE PASSED"; else echo "EVIDENCE FAILED"; fi
exit $fail
