# jeliya_protocol

Transport-agnostic Dart client for the Jeliya daemon protocol
([`docs/PROTOCOL.md`](../../docs/PROTOCOL.md)). Pure Dart, Flutter-agnostic — the
same package backs the **desktop sidecar** path (WebSocket to a local `jeliyad`)
today and a **mobile in-process FFI** transport later, behind one `Client`
interface. This is the concrete realization of the July 2026 native-evaluation
architecture decision.

## What's here

| File | Role |
|---|---|
| `lib/src/protocol.dart` | Envelope, `Client` interface, `Push`, `ConnectionState`, `RequestError` — mirrors `ui/src/lib/protocol.ts`. |
| `lib/src/ws_client.dart` | WebSocket transport: id-correlated request/response, push fan-out, 500 ms→8 s backoff reconnect with jitter, offline send-queue, per-attempt token fetch. Ported 1:1 from `ui/src/lib/client.ts`. |
| `lib/src/supervisor.dart` | `SidecarSupervisor` — the client half of the Phase 0 process-supervision contract: spawn `jeliyad --supervised`, parse the `ready`/`already_running` line, adopt an incumbent, read the portfile token. |
| `lib/src/conformance.dart` | Envelope-level replay + normalizer, ported from `ui/src/lib/conformance/harness.ts`. |

## Tests

```sh
cargo build                       # build the daemon the tests drive
cd dart/jeliya_protocol && dart test
```

- `test/conformance_test.dart` replays the **shared** corpus
  (`ui/src/lib/conformance/corpus.json`) against a real spawned `jeliyad`, so
  the Dart client is held to the exact same golden vectors as the daemon and the
  TypeScript reference client.
- `test/supervisor_test.dart` verifies the spawn/adopt handshake under the JIT
  VM (the adoption race the desktop app depends on).

The Flutter desktop shell that consumes this package lives in
[`../../app`](../../app).
