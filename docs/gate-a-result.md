---
type: "Status Report"
title: "Historical Gate A result — 2026-07-04"
description: "Historical evidence of one direct cross-network P2P run that does not certify the v0.5.0 candidate."
tags: ["nat", "networking", "p2p", "verification"]
timestamp: "2026-07-16T15:30:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "historical"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "operators"]
---

# Historical Gate A result — 2026-07-04

**Historical result: PASS for the tested 2026-07-04 build and network pair.**
This record is not current verification for `v0.5.0`.

> Historical note: this run predates the 2026-07-05 rename to Jeliya (see
> [`naming.md`](naming.md)). It was executed with the pre-rename `bantabad`
> binary; the equivalent binary today is `jeliyad`.

The run demonstrated that direct connectivity was possible on one residential
network to cloud-network pair. It did not test relay fallback, reconnect,
resynchronization, pipes, or unauthorized-room isolation. Those assertions are
part of the new candidate evidence ledger in
[`verification-evidence.md`](verification-evidence.md).

## Recorded revision and environment

Two `bantabad` daemons (pre-rename) in real network mode (iroh N0 stack:
public n0 relays and DNS discovery), on different networks, were driven by
`scripts/gate-a.mjs`.

| Field | Recorded value |
|---|---|
| Evidence-record commit | `f2aea0959ee5bf0f91fee030bdd2e2466163671c` |
| `iroh-rooms` revision at that commit | `1d2f014e783893ffeaea055c436370179a31110a` |
| Date | 2026-07-04 |
| Host role A | macOS residential-network host; debug `bantabad` |
| Host role B | Linux x86_64 cloud-network joiner; static musl `bantabad` |
| Observed public egress | Public-address fingerprints differed; this did not independently prove separate VPC or routing domains |
| Observed settled path | `direct` on both peers |

Public addresses, invite tickets, identity material, and raw frames are omitted
from this repository record. The raw JSON was written to the local pre-rename
`.bantaba-gatea/` directory and was not committed. Consequently, the record
pins the repository state that documented the result and its dependency, but it
does not provide independently reproducible artifact provenance for the two
executables. That limitation is why the page is marked `historical`, not
`verified` for the current candidate.

## Assertions recorded as passing

Both peers settled on `path=direct`, indicating a direct UDP path rather than
relay fallback. The historical harness recorded these assertions as passing:

- targeted invite and room join across the two networks;
- signed messages in both directions;
- a 256 KiB file shared from A to B, fetched by B, and verified byte-for-byte
  against its BLAKE3 hash.

```
GATE A: PASS — direct P2P across different networks. NAT hole-punch CONFIRMED (A=direct B=direct).
```

## Reproduce

```sh
node scripts/gate-a.mjs --remote root@<a-linux-vm-on-another-network>
```

See [`realnet-runbook.md`](realnet-runbook.md) for the full runbook, the
phone-hotspot variant, and `--manual` mode. A `relay` path on either side would
be an honest **PARTIAL** (connectivity via relay fallback, hole-punch not
achieved on that network pair); this run achieved neither caveat.

## Applicability to v0.5.0

This historical run matches neither the released `v0.5.0` nor the current
candidate. `v0.5.0` satisfied Gate A's intent through its own certifying
signed direct and forced-relay schema 2 runs at `c5f740e…` + `d0ceb0b…` (see
[`verification-evidence.md`](verification-evidence.md)). The post-release
candidate on `main` repins `iroh-rooms` to published `v0.1.0-rc.3`
(`71fbb5007bef4ce83631c94762ec68c2beef3d79`) and needs its own direct and
deliberately constrained relay runs at that exact commit and dependency
revision before the next release.
