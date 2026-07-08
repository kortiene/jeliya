# Changelog

## [Unreleased]

### Added

- Added a native desktop walking skeleton (Phase 2): a Flutter-agnostic Dart protocol client (`dart/jeliya_protocol/` — WebSocket transport, reconnect/backoff, and a client-side sidecar supervisor implementing the Phase 0 spawn/adopt/token contract) and a minimal Flutter macOS app (`app/`) that spawns the daemon, connects, and exchanges live messages. The Dart client is held to the **same** golden conformance corpus as the reference TypeScript client (`dart test`), so one spec now governs three implementations (daemon, mock, Dart). The built app bundle spawns the sidecar and self-terminates it cleanly on quit.
- Promoted docs/PROTOCOL.md to an authoritative, client-buildable spec: documented the previously TS-only invariants (insert-by-ts + `event_id` dedup on pushes, the echo-beats-response race and its `event_id` correlation, the connection lifecycle, verified-vs-fetched, the `labelTone` tone algorithm), a per-method error-code column, and the client-synthesized `connection_lost` convention.
- Added a Protocol version & forward-compatibility section: `protocol` is a single major int clients read from `daemon.status`; normative ignore-unknown-keys / unknown-`kind` rules keep v1 unbreakable; reserved (not yet emitted) `min_protocol`, a connect-time handshake slot, a `room.timeline` resync cursor, a `TimelineEvent` `delivery` marker for future queued/store-and-forward delivery, and optional voice-note `kind`/`duration_ms`/`waveform` — all named now so they stay non-breaking additions.
- Added an envelope-level conformance suite (`ui/src/lib/conformance/`, `npm test`): one golden corpus replayed identically against the real daemon (over WebSocket) and the in-memory mock, asserting on normalized frames so the same vectors will validate a future Dart client.
- Added the process-supervision contract (docs/PROTOCOL.md): a machine-readable `ready` JSON line on stdout, a `daemon.json` portfile (port, pid, protocol version, auth token; 0600), and `--port 0` support that reports the OS-assigned port truthfully.
- Added `--supervised` mode for sidecar parents: the daemon shuts down on stdin EOF (portable parent-death detection) and never auto-opens a browser.
- Added graceful shutdown on SIGTERM/SIGINT and a new authenticated `daemon.shutdown` method — all three paths close every open room (releasing blob locks) and remove the portfile.
- Added `GET /api/health` (unauthenticated liveness + identity for adoption checks) and `GET /api/session` (hands the auth token to loopback-Origin browser pages only).
- Added a daily-rolling daemon log at `<data_dir>/logs/`, filtered by `JELIYAD_LOG`/`RUST_LOG`.
- Added `scripts/sidecar-check.mjs`: an end-to-end gate for the supervision contract (ready line, token gate, adoption, SIGTERM, parent-death, kill -9 recovery).

### Changed

- **Breaking:** `/ws` and `/api/files/*` now require a per-start auth token (`?token=` or `Authorization: Bearer`). The served web UI fetches it automatically from `/api/session`; scripts read it from the portfile (`scripts/daemon-token.mjs`). Older clients against a new daemon are refused with 401.
- **Breaking:** one daemon per data dir. A second launch on the same data dir no longer silently binds a neighboring port (the double-daemon `state.json` corruption scenario); it now health-checks the incumbent, prints `already_running`, and exits 0 so supervisors adopt it.
- `daemon.status` now also reports `protocol`, `pid`, `port`, and `data_dir` so a client can verify which daemon it is attached to.
- `/ws` and `/api/*` refuse requests whose `Host` header is not loopback (DNS-rebinding guard), and `/api/files/local` is no longer reachable without auth.
- Files fetched from room peers are now served as downloads (`Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`, inert content-type) instead of rendered inline, so a peer-supplied `text/html`/`svg` cannot run script in the daemon's origin.
- Daemon diagnostics moved from bare stderr prints to `tracing` (stderr + rolling file).
- Fixed the file-fetch UI keying friendly copy on a phantom `provider_refused` code the daemon never emits; the authorization-wall case now correctly handles `file_unauthorized`, and `hash_mismatch` gets an explicit hard-stop message. Aligned the TypeScript wire types (`protocol.ts`) and the mock reference client with the daemon: `daemon.status` gains `protocol`/`pid`/`port`/`data_dir`, `daemon.shutdown` is typed, `room.open` documents its `peers` hints, `invite.create` expiry accepts a string or seconds, and pipe/room fields that can be null are typed nullable (surfacing several latent null-handling fixes in the UI).

## [0.4.3] - 2026-07-07

### Changed

- Made file cards show honest fetch states: checking availability, ready to fetch, fetching, fetched, failed, and no provider online.
- Replaced fetched-file status-only labels with direct `Open file` and `Copy path` actions.
- Added a `Recheck` action for files whose providers are currently offline.

### Fixed

- Stopped showing `Fetch` for files that have already been fetched or have no online provider.
- Improved file-row layout so provider status and file actions stay readable on desktop and mobile.

## [0.4.2] - 2026-07-07

### Added

- Added a support diagnostics panel in Settings so users can copy a privacy-safe snapshot for bug reports.
- Added a GitHub bug report form with a dedicated field for pasted Jeliya diagnostics.

### Changed

- Captured the latest UI action error across room, message, file, pipe, join, create, and leave flows so reports include the failing context without exposing room contents.
