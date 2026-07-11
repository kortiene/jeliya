# Real-network NAT runbook

> Internal operator notes — the manual procedure behind the Gate A result
> (`docs/gate-a-result.md`). Not needed to use or contribute to Jeliya.

How to prove Jeliya works across **two different networks** — machine A (this
Mac) and machine B (any second Mac or Linux box on another network: a phone
hotspot, an office LAN, a cloud VM). Everything runs `jeliyad` in **real
network mode** (no `--loopback`): the iroh N0 stack with public n0 relays and
DNS discovery, exactly what the SDK's CLI uses for real networking.

The verdict that matters is each side's `peers.status` **path** field:

- `path=direct` — NAT hole punching succeeded (or the machines could dial each
  other directly). Traffic flows peer-to-peer over UDP.
- `path=relay` — hole punching **did not** complete; the connection works, but
  every byte is relayed through an n0 relay server. That is the designed
  fallback, not a failure of the product — but it *is* a failure of NAT
  traversal between those two specific networks, and worth recording.
- Both scripts print the path on their own side. The two sides can legitimately
  disagree for a short while (paths upgrade from relay to direct as hole
  punching completes); the settled value is the one to record.

Honesty note: a run where **everything passes but both paths say `relay`**
means "relay fallback works, hole punch failed" — the room, messages, files
and invites all still work, at relay latency. A `direct` path on either side
is the strong result. Same-network runs (both machines on one LAN) will
trivially report `direct` and prove nothing about NAT traversal.

## Fast path — one command (`gate-a.mjs`)

`scripts/gate-a.mjs` runs the whole test from machine A: it fingerprints both
sides' public IPs, **refuses to certify a pass when they share one** (a
same-NAT run cannot test hole punching), starts the host here, drives machine
B, and prints a single Gate A verdict. It ships a static Linux `jeliyad` +
the two scripts B needs, so a Linux B only needs **Node 22** — no Rust build.

**Recommended: a cloud VM as machine B.** A VM has a public IP that A can SSH
to from any network, and it is genuinely on a different network — so you do
not even have to move this Mac:

```sh
cargo build --workspace                      # A: debug binary for the host side
# (ship path is a static musl build; make it once:)
rustup target add x86_64-unknown-linux-musl
cargo zigbuild --release --target x86_64-unknown-linux-musl -p jeliyad

node scripts/gate-a.mjs --remote user@<vm-public-ip>
```

**Phone-hotspot variant.** Tether this Mac to a hotspot (now A is on a
different network from your home box B), and point `--remote` at a box you can
still reach — e.g. over its public IPv6, or use a VM. If A cannot SSH to B
from the hotspot, use manual mode:

```sh
node scripts/gate-a.mjs --manual --peer-identity <B id>
# prints the exact `realnet-check.mjs` command — run it on B yourself
```

**Dry-run the machinery** (same machine, no Gate A claim — verifies the
plumbing and that the validity gate correctly withholds certification):

```sh
node scripts/gate-a.mjs --local-dryrun
```

Verdicts: `PASS` (direct path both sides — hole punch confirmed) · `PARTIAL`
(connected across networks but via relay fallback) · `NOT A GATE A` /
`UNVERIFIED NETWORK` (same network, or B's public IP unseen — nothing
certified). Evidence JSON is written under `.jeliya-gatea/`.

The manual, step-by-step flow below is the fallback (and what `gate-a.mjs`
automates); use it when B is a Mac, or when you want to drive each side by hand.

## Prerequisites

| | machine A (host) | machine B (joiner) |
|---|---|---|
| OS | this Mac | macOS or Linux |
| Rust | >= 1.80 (workspace `rust-version`) | >= 1.80 |
| Node | >= 22 (global `WebSocket`, no npm deps) | >= 22 |
| Network | internet (relays, DNS discovery) | internet, on a **different network** than A |

Machine B also needs `git`, and internet access to `github.com` + `crates.io`
during `cargo build` (the workspace pins the `iroh-rooms` SDK as a git
dependency: `https://github.com/kortiene/iroh-room`).

Ports: the scripts default to WebSocket port **7431** on A and **7432** on B
(both bind `127.0.0.1` only — the WS control port is never exposed to the
network; the p2p traffic uses its own UDP sockets). Override with `--port`.

## Step 0 — get the code onto machine B

The repo is public — if B can reach GitHub, clone it directly:

```sh
git clone https://github.com/kortiene/jeliya jeliya
cd jeliya
cargo build --workspace           # first build fetches the pinned SDK; takes a while
```

When B has no GitHub access, distribute a git bundle instead. On **A**:

```sh
scripts/make-bundle.sh            # writes ./jeliya.bundle from branch main
```

Copy `jeliya.bundle` to B (scp / AirDrop / USB). On **B**:

```sh
git clone -b main jeliya.bundle jeliya
cd jeliya
cargo build --workspace           # first build fetches the pinned SDK; takes a while
```

(Build on A too if you haven't: `cargo build --workspace`.)

## Step 1 — machine B: create an identity

```sh
node scripts/realnet-check.mjs --identity-only
```

This starts a real-mode daemon, creates (or reuses) B's identity in
`.jeliya-realnet-b/`, and prints:

```
check: identity_id = <64 hex chars>
node scripts/realnet-host.mjs --peer-identity <64 hex chars>
```

Send that identity_id to whoever drives machine A (chat, email — it is public
key material, not a secret).

## Step 2 — machine A: host the room and mint the invite

```sh
node scripts/realnet-host.mjs --peer-identity <B_IDENTITY_FROM_STEP_1>
```

The script starts a real-mode daemon, creates A's identity (first run only), a
**fresh room**, opens it, mints an invite bound to B's identity, and prints a
block like:

```
host: ================= PASTE ON MACHINE B =================
node scripts/realnet-check.mjs --ticket '<ticket>' --peer '<endpoint_id>@<ip:port,...>'
host: =======================================================
```

It then waits (default 15 min, `--wait-mins` to change) and reports, in order:
B's join, A→B and B→A messages, B's final PASS, and A's `peers.status` path.

Send the printed one-liner to machine B verbatim. Notes:

- The `--peer` addr contains A's known socket addrs (LAN + publicly observed).
  Across a NAT the LAN ones are unreachable — that is fine; the invite ticket
  also carries A's endpoint id, and real mode resolves it via DNS discovery
  and the relay, then attempts the hole punch.
- The invite has **no expiry** and is bound to B's identity; it is single-use
  per room, and the host mints a fresh room + invite every run, so re-runs
  never trip over an already-redeemed ticket.

## Step 3 — machine B: join and run the check

Paste the exact line printed by A:

```sh
node scripts/realnet-check.mjs --ticket '<ticket>' --peer '<addr>'
```

The check script starts B's real-mode daemon (reusing the Step 1 identity),
joins with the ticket (retrying — the daemon's per-attempt join bootstrap
window is 15 s and the first dial across a NAT can miss it while
discovery/relay warm up), opens the room, then hard-asserts:

1. `member_joined` for B is visible in **B's** synced timeline (the host
   asserts the same event on **A's** timeline — both sides covered);
2. message each way — B receives A's hello, B sends its own (receipt asserted
   on A by the host);
3. file transfer — A shared a 256 KiB random payload; B waits for
   `file.list` to show it available, fetches it, and requires
   `verified:true` (blake3 content verification) plus a byte count matching
   the listed size;
4. prints **B's** `peers.status` path (direct vs relay).

On success it sends a final `realnet-check: PASS ...` room message (which the
host waits for), prints its verdict, and exits 0. The host then prints **A's**
path and exits 0. Record both paths.

## Reading the result

- **Both sides `path=direct`** — full NAT traversal: hole punch succeeded.
- **`relay` on either side** — connectivity via relay fallback; hole punching
  failed between these two networks (common with symmetric/CGNAT — e.g. some
  phone hotspots and cloud NATs). Everything still works; latency is higher.
- **Join times out on every retry** — check both machines actually have
  internet, that UDP outbound isn't blocked entirely, and A's host script is
  still running (the room only serves while A's daemon is up).

## Re-runs and cleanup

- Re-run = repeat Steps 2 and 3 (fresh room + invite each time). Identities in
  `.jeliya-realnet-host/` (A) and `.jeliya-realnet-b/` (B) are reused.
- Full reset: `rm -rf .jeliya-realnet-host` on A, `rm -rf .jeliya-realnet-b`
  on B (Step 1 must then be repeated, since B's identity changes).
- The scripts kill their own daemons on exit (success, failure, or Ctrl-C).

## Same-host smoke test (no second machine)

The whole flow can be dry-run on one machine with two terminals — useful for
validating the scripts, though the path result (`direct`) proves nothing about
NAT traversal:

```sh
# terminal 1
node scripts/realnet-check.mjs --identity-only --data-dir /tmp/rn-b
node scripts/realnet-host.mjs --peer-identity <printed id> --data-dir /tmp/rn-a   # keep running
# terminal 2 — paste the line host printed, adding B's data dir:
node scripts/realnet-check.mjs --ticket '<t>' --peer '<addr>' --data-dir /tmp/rn-b
```

There is also a full same-host real-mode e2e: `node scripts/e2e.mjs --mode real`
(67 assertions, identical to the loopback suite).
