# Security

Jeliya is a network daemon people run on their own machines — and, on
phones, the same engine linked directly into the app — holding their own
keys and data. Security reports are taken seriously and handled privately.

## Reporting a vulnerability

Use **GitHub's private vulnerability reporting**: the
[Security tab](https://github.com/kortiene/jeliya/security) → "Report a
vulnerability". That opens a private advisory only the maintainer can see.

Please include what you can: affected component (`jeliyad`, `jeliya-core`,
the `jeliya-ffi` in-process bridge / the Flutter app, the web UI, the agent
runner), a reproduction, and the impact as you understand it. Please don't
open a public issue for something exploitable before it's fixed.

If the "Report a vulnerability" button is ever missing, open a plain issue
saying only "security — requesting a private channel" with **no details**,
and the maintainer will reach out.

## What to expect — honestly

This is a small open-source project, not a company:

- **No bug bounty.** Credit in the release notes if you want it.
- **Best-effort response.** The aim is an acknowledgment within a week and a
  fix prioritized by real impact; there is no SLA.
- **No embargo theater.** Once a fix ships, the advisory is published and
  the release notes say plainly what was wrong.

## Scope notes

- An agent runner executes tasks on the machine it runs on, gated by a
  sender allowlist — that is a documented trust decision, not a
  vulnerability (see the trust model in `docs/agent-guide.md`). Bypassing
  the allowlist, however, absolutely is one.
- The daemon binds to `127.0.0.1` only; anything that gets it listening on
  another interface without explicit intent is a vulnerability.
- On phones there is no daemon and no control socket at all: the app links
  the engine in-process (`crates/jeliya-ffi`), and `daemon.status` reports
  `port: 0` to mean exactly that. The engine there still does real
  peer-to-peer networking (today the phone is the build doing it — the
  macOS app runs its sidecar loopback-only), so the P2P surface applies;
  anything that exposes the control protocol on a socket from the
  in-process build is a vulnerability.
- Release binaries are currently unsigned (`docs/signing-notarization.md`
  tracks the plan). Verify downloads with the `.sha256` sidecars.
