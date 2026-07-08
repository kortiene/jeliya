# jeliya_protocol

Transport-agnostic Dart client for the Jeliya daemon protocol
([`docs/PROTOCOL.md`](../../docs/PROTOCOL.md)). Pure Dart, Flutter-agnostic — the
same package backs the **desktop sidecar** path (WebSocket to a local `jeliyad`)
today and a **mobile in-process FFI** transport later, behind one `Client`
interface. This is the concrete realization of the July 2026 native-evaluation
architecture decision.

## What's here

Everything below is exported from the main barrel,
`package:jeliya_protocol/jeliya_protocol.dart`.

| File | Role |
|---|---|
| `lib/src/protocol.dart` | Envelope, `Client` interface, `Push`, `ConnectionState`, `RequestError` — mirrors `ui/src/lib/protocol.ts`. |
| `lib/src/models.dart` | Typed view-models for every PROTOCOL.md shape (`DaemonStatus`, `RoomSummary`, `TimelineEvent`, `FileEntry`, `PipeEntry`, `PeerStatus`, `FleetResult`, …) plus the `ErrorCodes` constants. `fromJson` follows the normative forward-compat rules (unknown keys ignored, unknown event kinds still parse). |
| `lib/src/methods.dart` | `extension JeliyaMethods on Client` — one typed wrapper per PROTOCOL.md RPC (26) plus typed push streams `roomEvents` and `peersChanged`. |
| `lib/src/ws_client.dart` | WebSocket transport: id-correlated request/response, push fan-out, 500 ms→8 s backoff reconnect with jitter, offline send-queue, per-attempt token fetch. Ported 1:1 from `ui/src/lib/client.ts`. |
| `lib/src/supervisor.dart` | `SidecarSupervisor` — the client half of the Phase 0 process-supervision contract: spawn `jeliyad --supervised`, parse the `ready`/`already_running` line, adopt an incumbent. Also: full `Portfile` parse, `healthCheck()` (GET `/api/health`), attach-without-spawn (`SidecarSupervisor.attach` + `attachToRunning()`), `httpBase()`/`authToken`, rpc-based `stopDaemon()` for adopted daemons, and the post-(re)connect `verifyDaemonProtocol()` helper (`ProtocolMismatchError` on skew). |
| `lib/src/daemon_http.dart` | The daemon's HTTP side-channel: `stageAndShareFile` / `shareUserFile` (the documented native staging convention — copy into `<data_dir>/uploads/`, `file.share`, delete the staged copy; 100 MiB limit surfaced as a typed `invalid_params` error) and the `/api/files/local` URL builder (`buildLocalFileUrl` / `localFileUrl`). |
| `lib/src/conventions.dart` | 1:1 ports of the reference client conventions (`ui/src/lib`, `ui/src/App.tsx`): timeline fold + `event_id` dedup, optimistic pending-message lifecycle, `splitInvite`, `joinRoomWithRetry` (5 attempts, retries only `peer_unreachable`), `labelTone`, `FetchState` fold (never downgrade, `hash_mismatch` hard stop), and `buildDiagnostics` (redacted support report). |
| `lib/src/conformance.dart` | Envelope-level replay + normalizer, ported from `ui/src/lib/conformance/harness.ts`. |
| `lib/testing/mock_client.dart` | `MockClient` — deterministic in-memory `Client` with the `ui/src/lib/mock.ts` fixtures, for widget tests and demos. Exported via `package:jeliya_protocol/testing.dart` only, **never** from the main barrel. |

## Tests

```sh
cargo build                       # build the daemon the tests drive
cd dart/jeliya_protocol && dart test
```

- `test/conformance_test.dart` replays the **shared** corpus
  (`ui/src/lib/conformance/corpus.json`) against a real spawned `jeliyad`, so
  the Dart client is held to the exact same golden vectors as the daemon and the
  TypeScript reference client.
- `test/mock_client_test.dart` replays the same corpus against `MockClient`,
  then pins the mock-specific behaviors (echo-before-response ordering,
  injectable clock, fixture fleet liveness).
- `test/supervisor_test.dart` verifies the spawn/adopt handshake under the JIT
  VM (the adoption race the desktop app depends on).
- `test/daemon_http_test.dart` drives the portfile/health/attach/staged-upload
  surface against a real daemon.
- `test/models_test.dart` and `test/conventions_test.dart` pin the typed models
  and the convention ports to the reference behaviors.

The Flutter desktop shell that consumes this package lives in
[`../../app`](../../app).
