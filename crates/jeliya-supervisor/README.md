# jeliya-supervisor

The reusable **owned/adopted `jeliyad` supervisor** for native control planes
(#170). A headless, UI-agnostic crate that any native shell — the Dioxus desktop
app first, but also an agent host or a diagnostic tool — uses to obtain a live,
correctly-owned `jeliyad` and a freshly-validated connection target for every
dial, while enforcing the documented single-instance, token, portfile,
parent-death, and ownership invariants.

Spec: [`specs/rust-desktop-jeliyad-supervisor.md`](../../specs/rust-desktop-jeliyad-supervisor.md).
The authoritative contract is the daemon in [`crates/jeliyad`](../jeliyad) and
[`docs/protocol-v2.md`](../../docs/protocol-v2.md); where this crate and those
disagree, they are right and this crate has a bug.

## What it does

- **Resolves** the daemon binary (fail-closed; an installed `jeliyad` on `PATH`
  is never silently used) and the per-user data dir (canonicalized).
- **Spawns** `jeliyad --supervised --port 0 --data-dir <dir>`, or **adopts** the
  daemon already serving that dir, and makes the two lifecycles distinguishable
  in the public type (`Sidecar::is_owned`).
- **Validates** that the `ready`/`already_running` line, the `daemon.json`
  portfile, and the unauthenticated `/api/health` response describe the *same*
  daemon — PID on the advertised loopback port, and matching **protocol** and
  **storage generation**. `data_dir` is not consulted (v2 health removed it);
  identity binds through PID-on-port + the portfile's own location.
- **Adopts only an exact supported-generation incumbent.** A protocol or
  storage-generation mismatch **fails closed**; a mismatched incumbent is
  replaced only when it is *proven* to own the exact data dir (and only with
  `replace_incompatible` opted in), never on a bare PID.
- Hands transports a **fresh target and token on every reconnect** through a
  cheap, cloneable `TargetResolver` that re-reads and re-validates the portfile
  on each call. The token stays native (`Authorization: Bearer`) and is
  `<redacted>` in every `Debug`/log — never in a URL, never into WebView script.
- **Stops only owned daemons**, with bounded, process-tree-safe escalation;
  leaves adopted daemons running; and (via the `--supervised` stdin-EOF signal)
  guarantees both orderly and abrupt parent death leave no duplicate daemon.

## What it is not

No UI, no WebSocket request semantics (that is the client kernel/codec), no
token into WebView JavaScript, and no killing an incumbent that cannot be proven
to own the exact data directory. Clean-slate: it recognizes exactly the
built-against protocol/storage generation and never reads or migrates v1/Flutter
state.

## Boundary

Native-only. `tests/boundaries.rs` asserts the library graph never reaches Iroh,
`jeliya-core`, the `jeliyad` binary, `jeliya-client`, or any renderer, and that
the crate is absent from the `wasm32` UI graph. `unsafe_code` is forbidden; the
one place a raw signal is needed (`killpg`/`kill`) goes through `nix`'s safe
wrappers.

## Test discipline — deliberate regressions

Every fault case is confirmed to **fail against a deliberately broken supervisor
before it is trusted** (the #159 spike's rule, kept here). A test that passes
against a supervisor with the guard removed proves nothing. The real-daemon
fault matrix (owned/adopted lifecycles, recycled PID/port, incompatible
incumbent, hung shutdown, abrupt death, adoption, …) drives a real `jeliyad`
built from the workspace and lands with the focused-test slice; the pure-logic
guards (portfile parsing, loopback, token redaction, dial-URL shape) are unit
tests co-located with the code.
