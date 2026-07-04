# Gate A result — NAT hole-punching CONFIRMED

**Status: PASS (direct P2P across different networks).** 2026-07-04.

Gate A is the one open risk the iroh-rooms runtime's own go/no-go memo left
`CONDITIONAL`: real-network NAT hole-punching, "the manual two-host run has not
[been done]." This is that run, for Bantaba's daemon on the pinned SDK.

## What was measured

Two `bantabad` daemons in real network mode (iroh N0 stack: public n0 relays +
DNS discovery), on **genuinely different networks**, driven by
`scripts/gate-a.mjs`:

| | machine A (host) | machine B (joiner) |
|---|---|---|
| host | this Mac | `demo1` (Hetzner cloud VM) |
| public IPv4 | `75.190.11.42` | `46.4.115.57` |
| binary | `target/debug/bantabad` | shipped static `x86_64-unknown-linux-musl` |

The orchestrator fingerprinted both public IPs and confirmed they differ
(`status: different`) **before** certifying — a same-network run is rejected as
"NOT A GATE A", so this result cannot be a same-LAN artifact.

## Result

Both sides settled on **`path=direct`** — a direct peer-to-peer UDP path, no
relay fallback. All 8 assertions passed on each side:

- invite (agent/member ticket) → join by ticket across the internet
- signed messages both directions
- 256 KiB file shared A→B, fetched by B, BLAKE3 content-verified, byte-exact

```
GATE A: PASS — direct P2P across different networks. NAT hole-punch CONFIRMED (A=direct B=direct).
```

Raw evidence (fingerprints, per-side paths, verdict) is emitted per run under
`.bantaba-gatea/gate-a-<ts>.json`.

## Reproduce

```sh
node scripts/gate-a.mjs --remote root@<a-linux-vm-on-another-network>
```

See [`realnet-runbook.md`](realnet-runbook.md) for the full runbook, the
phone-hotspot variant, and `--manual` mode. A `relay` path on either side would
be an honest **PARTIAL** (connectivity via relay fallback, hole-punch not
achieved on that network pair); this run achieved neither caveat.

## Caveat

One network pair (residential NAT ↔ Hetzner public IP) is proof that hole
punching *works*, not that it works on *every* NAT. Symmetric-NAT and
CGNAT-to-CGNAT pairs can still fall back to relay; re-run `gate-a.mjs` on any
pair that matters and record its verdict.
