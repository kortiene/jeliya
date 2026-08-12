# Spec — Reusable owned/adopted `jeliyad` supervisor for native control planes (#170)

- **Issue:** kortiene/jeliya#170 — `[Rust][Desktop]: Extract a reusable owned/adopted jeliyad supervisor`
- **Priority / labels:** p0 · rust · security · dioxus · migration · platform:desktop · clean-slate
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M4 (desktop lifecycle and packaging); the supervisor is a prerequisite of the `WsNative` adapter (#172) and of the M4 desktop packages.
- **Owning component:** the new `daemon supervisor` row in the architecture layering table (`docs/dioxus-architecture.md` §"Decision 3", the crate/module table) — owner role **desktop maintainer**. Proposed home: a new native-only crate `crates/jeliya-supervisor`.
- **Consumers:** `WsNative` (#172, the desktop transport adapter — "native async WebSocket through the reusable supervisor and target resolver"), the packaged desktop shell (#184/M4), and any other native control plane (an agent host, a diagnostic tool).
- **Depends on:** #157 (architecture record — landed) and the feasibility evidence in the #159 desktop spike (`spikes/dioxus-desktop/`). #159 remains "not measured" as a *packaged-WebView* result in `docs/dioxus-architecture.md` §"Measured unknowns"; this issue turns the spike's headless-proven ownership/teardown contract into one reviewed, reusable crate.
- **Daemon contract it speaks to:** `crates/jeliyad/src/lifecycle.rs`, `crates/jeliyad/src/main.rs`, `crates/jeliyad/src/serve.rs` (`/api/health`), and `docs/protocol-v2.md` §"Layer 0 — discovery" / §"Layer 1 — the generation gate". The stale `docs/PROTOCOL.md` (v1) supervision section is prior art only; where it and `docs/protocol-v2.md` disagree, **protocol-v2 wins** (see §4).
- **Owner role for this document:** desktop maintainer, with core-maintainer sign-off on the discovery-object dependency (`jeliya-api`).
- **Status of this document:** planning/spec only. **No production code is to be written for this issue by the planning phase.**

> Where this spec and `docs/dioxus-architecture.md`, `docs/protocol-v2.md`, or the live daemon in `crates/jeliyad` disagree, those records and the code are authoritative and this spec has a bug — say which in the PR. Every self-referential claim (fault-case count, invariant list) must be re-derived from the code, never trusted from prose.

---

## 1. Outcome

One reviewed Rust supervisor that any native control plane (Dioxus desktop first) can use to obtain a live, correctly-owned `jeliyad` and a freshly-validated connection target for every dial, while enforcing the documented single-instance, token, portfile, parent-death, and ownership invariants.

Concretely, the supervisor:

1. **Resolves** the daemon binary and the per-user data directory deterministically, fail-closed.
2. **Starts** a daemon with `--supervised --port 0 --data-dir <dir>`, or **adopts** the one already serving that data directory, and makes the two lifecycles **distinguishable** in the public type.
3. **Validates** that the `ready`/`already_running` line, the `daemon.json` portfile, and the unauthenticated `/api/health` response all describe **the same daemon** — matching PID on the advertised loopback port, and matching **protocol** and **storage generation**.
4. **Adopts only an exact supported-generation incumbent.** A protocol or storage-generation mismatch **fails closed**; a mismatched incumbent is replaced **only when it is proven to own the exact data directory**, never on a bare PID.
5. Hands transports a **fresh target and token on every reconnect** through a cheap, cloneable resolver that re-reads and re-validates the portfile on each call — never caching a port or token across a daemon restart.
6. **Stops only owned daemons**, with bounded, process-tree-safe escalation; leaves adopted daemons running; and guarantees that both orderly and abrupt parent death leave no duplicate daemon.

The supervisor **owns spawn and stop; transports do not** (architecture Decision 3, the `daemon supervisor` row). It carries **no UI state** and holds **no daemon token in any surface a WebView can read** (architecture §"Trust-boundary invariants": *daemon token — stays native, never crosses into untrusted WebView script, and is redacted in logs and diagnostics*).

## 2. What this issue is, and what it is not

**This issue owns:**

- the native **process-ownership handle** (`Supervisor`/`Sidecar`) that spawns-or-adopts and stops;
- the cheap cloneable **target resolver** (`TargetResolver`) that yields a validated `DialTarget` (loopback WS URL with the generation-gate query, plus a redacted bearer token) on every call;
- the **portfile deserializer**, health probe, and the **agreement/skew logic** that decides adopt vs. respawn vs. fail-closed;
- the **shutdown-escalation** state machine and its per-platform process-tree safety; and
- the deterministic **fault-test harness** that drives every case in §8 against a real `jeliyad`.

**This issue does not own (explicit non-goals from the issue and the architecture record):**

- **A desktop UI.** No Dioxus, no `jeliya-ui`, no window. The supervisor is headless and UI-agnostic.
- **WebSocket request semantics.** Framing, the codec, replay, resync, and the RPC surface are the client kernel/seam (#167/#168/#169) and codec (#164). The supervisor produces a validated target and token; it does not open the protocol socket or speak RPC. (The one RPC it *may* invoke — `daemon.shutdown` for an adopted daemon — is delegated through a caller-supplied closure, see §6.7; the supervisor does not link the client.)
- **Passing the daemon token into WebView JavaScript.** The token reaches the native transport and nothing else — never a Dioxus prop, DOM attribute, URL, or log line.
- **Killing an incumbent that cannot be proven to own the exact data directory.** A bare PID from a stale or foreign portfile is never signalled.
- **Reading or migrating Flutter-created (v1) state.** Clean-slate cutover (§12): the supervisor recognizes exactly the built-against protocol/storage generation and treats every other generation as incompatible, not as something to upgrade.

## 3. Where it lives, and its dependency boundary

### 3.1 A new native-only crate `crates/jeliya-supervisor`

The architecture table lists `daemon supervisor` as a **new** component distinct from both `client kernel and seam` (core maintainer) and the `WsNative` adapter. A dedicated crate — mirroring how `crates/jeliya-platform` is separate from `crates/jeliya-client` — gives it its own reviewed ownership boundary and keeps process authority out of both the UI crate and the sans-IO kernel.

Dependency rules (asserted by a `tests/boundaries.rs` graph check, mirroring `jeliya-client`/`jeliya-platform`):

- **May depend on:** `jeliya-api` (for the single-definition discovery object `{protocol, min_protocol, storage_generation, limits}` and `Limits`; protocol-v2 §"Layer 0" — "defined **once** in `jeliya-api`"), `tokio` (native `process`, `net`, `time`, `io-util`), `serde`/`serde_json`, and small native utilities (`dirs`).
- **Must not depend on:** Dioxus, `jeliya-ui`, `jeliya-core`/`iroh-rooms` (that would pull Iroh into a supervisor that only needs to talk loopback HTTP/WS discovery), the `jeliyad` **binary** crate, or `jeliya-client`. It must never be reachable from a `wasm32-unknown-unknown` build.
- The **expected generation** the supervisor validates against (`protocol`, `storage_generation`) is an **injected construction parameter**, not a compiled-in constant lifted from `jeliya-core::engine`. This keeps the supervisor decoupled from the Iroh-bearing core and makes "what this build was built against" explicit and testable. See Open Question OQ-1 on hoisting those integer constants into `jeliya-api`.

### 3.2 Position in the desktop data flow

```
packaged app -> system WebView renders jeliya-ui
  -> native Rust handlers + PlatformServices
  -> WsNative (#172) --uses--> jeliya-supervisor (#170)
                                   |  spawn/adopt, validate, resolve target+token, stop
                                   v
                             supervised jeliyad (v2-only, owned or adopted) -> jeliya-core -> iroh-rooms
```

`WsNative`'s dialer calls `TargetResolver::resolve()` on **every** connection attempt; only verified loopback endpoints are dialed; the token is attached as `Authorization: Bearer` and redacted everywhere else. The supervisor does not own the transport, and none of the retired Dart supervisor's public shape is retained (architecture Decision 4, `WsNative` row: "Dart behavior is not retained").

## 4. Prior art, and exactly what changes

Two implementations already speak this contract. Both are **prior art, not the deliverable**; the reusable crate corrects the drift below.

| Prior art | What it is | What the reusable supervisor keeps | What it must change |
|---|---|---|---|
| `spikes/dioxus-desktop/src/supervisor.rs` + `tests/supervision.rs` (#159) | Headless-proven ownership/teardown against real `jeliyad 0.6.1` | `Owned`/`Adopted` split, `is_owned()`, `Teardown::{Graceful,Forced,LeftRunning}`, `kill_on_drop(false)` + stdin-EOF parent-death mechanism, stderr drain, ready↔portfile PID/port agreement, private `auth_token` behind a `token()` accessor, binary-resolution order that refuses a silent `PATH` fallback | **Portfile shape is stale:** the spike gates on `schema == 1` and `protocol == 1`. The current daemon **removed `schema`** and serves `protocol/min_protocol/storage_generation` (protocol-v2 §Layer 0; `crates/jeliyad/src/lifecycle.rs` `Portfile`). Gating on `schema` **rejects every current daemon** and gating on `protocol == 1` is inverted. Replace with `protocol`+`storage_generation` agreement. Add storage-generation validation, process-group/Job-Object teardown, the fresh-per-dial resolver, and the evict-and-respawn skew path (the spike has none). |
| `dart/jeliya_protocol/lib/src/supervisor.dart` | Full client-side supervisor: spawn/adopt, `attachToRunning`, `verifyDaemonProtocol` (post-reconnect handshake), `evictIncumbent`, `stopDaemon` (`daemon.shutdown` for adopted), fresh portfile re-read for `wsUrl`/`httpBase`/`authToken` | The **semantics**: re-read the portfile on every target/token resolution; refuse a protocol major it was not built against; evict-then-respawn for a mismatched incumbent; `daemon.shutdown` (not signal) as the only lever that reaches an adopted daemon; never trust a portfile blind (health-check first) | **v1 assumptions:** `expectedProtocol == 1`, no `storage_generation` axis, and `healthCheck` requires `health['data_dir'] == pf.dataDir`. **v2 `/api/health` removed `data_dir`** (protocol-v2 §Layer 0 — "hands an absolute filesystem path to any unauthenticated local caller … removed"). The reusable supervisor must bind identity via **PID on the advertised port + the portfile's own location + protocol/sg**, not via a health `data_dir` field that no longer exists. The Dart API surface itself is **not ported** (architecture: "Dart behavior is not retained"). |

The net correctness deltas the reusable crate must encode (each is a fault case in §8):

- **D1 — generation-axis validity.** A daemon is a valid adoption target iff its portfile parses, names a live PID answering `/api/health` on the advertised port with a matching PID, and its `protocol` **and** `storage_generation` both equal the values this supervisor was built against. `schema` is not consulted (it no longer exists). *Absence of a required generation field is refusal, never a default* (mirrors protocol-v2 §Layer 1 step-3/step-4 rule).
- **D2 — data-dir binding without a health `data_dir`.** Identity binds through: portfile read from the exact resolved data dir + `/api/health` PID match on `portfile.port`. This defeats a recycled port (an unrelated listener answers with a different or absent PID → refused) and a recycled PID (the process at `portfile.pid` is not a jeliyad on `portfile.port` → the health probe on that port fails or reports a different PID → refused).
- **D3 — clean-slate incompatibility.** A v1 portfile (has `schema`, `protocol: 1`, no `storage_generation`) or any non-matching generation is an **incompatible incumbent**, never adopted and never migrated.

## 5. Public API (proposed types)

All types live in `crates/jeliya-supervisor/src`. Names are proposals; the review may rename. The design deliberately **splits process ownership from target resolution** so a transport can hold a cheap resolver without holding (or being able to kill) the process handle.

### 5.1 The process-ownership handle

```rust
/// A live daemon this process either started (`Owned`) or adopted (`Adopted`).
/// Not `Clone`: it owns the child process and the stdin parent-death pipe.
pub struct Sidecar { /* portfile snapshot + ownership */ }

impl Sidecar {
    /// True iff this process started the daemon and may therefore stop it.
    pub fn is_owned(&self) -> bool;

    /// A cheap, cloneable resolver bound to this daemon's data dir and the
    /// supervisor's expected generation. Hand this to the transport (#172).
    pub fn target_resolver(&self) -> TargetResolver;

    /// Stop an owned daemon (bounded, process-tree-safe) and report which
    /// teardown occurred; leave an adopted daemon running (`LeftRunning`).
    pub async fn shutdown(self) -> Result<Teardown, SupervisorError>;

    /// Stop an ADOPTED daemon through the protocol, using a caller-supplied
    /// `daemon.shutdown` invoker (the supervisor does not link the client).
    /// Owned daemons are stopped through `shutdown()` instead. Polls
    /// `/api/health` until the daemon goes dark; bounded; never signals a PID
    /// the health probe cannot prove is this data dir's daemon.
    pub async fn stop_adopted(
        self,
        shutdown_rpc: impl FnOnce() -> BoxFuture<'static, Result<(), CallerRpcError>>,
    ) -> Result<Teardown, SupervisorError>;
}

/// How this process came to have a daemon — and what it owes it at shutdown.
enum Ownership {
    Owned { child: tokio::process::Child, stdin: Option<tokio::process::ChildStdin> },
    Adopted,
}

/// What a teardown actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Teardown { LeftRunning, Graceful, Forced }
```

### 5.2 The supervisor entry point

```rust
pub struct Supervisor {
    binary: PathBuf,          // resolved, fail-closed
    data_dir: PathBuf,        // canonicalized per-user dir
    expected: Generation,     // { protocol, storage_generation } this build speaks
    // knobs: spawn timeout, health timeout, teardown budget, loopback mode
}

impl Supervisor {
    pub fn resolve(config: SupervisorConfig) -> Result<Self, SupervisorError>;

    /// Spawn a daemon for `data_dir`, or adopt the one already serving it.
    /// Blocks until the daemon has announced itself and passed validation, so
    /// an `Ok` sidecar is bound and answering — not still starting.
    pub async fn start_or_adopt(&self) -> Result<Sidecar, SupervisorError>;

    /// Attach-only: adopt a running daemon from its portfile alone (no spawn,
    /// no binary needed). For a second native client riding along a daemon
    /// someone else supervises. Health-checks and generation-gates first.
    pub async fn attach_to_running(&self) -> Result<Sidecar, SupervisorError>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Generation { pub protocol: u64, pub storage_generation: u64 }
```

### 5.3 The fresh-per-dial resolver and its output

```rust
/// Cheap and cloneable: holds only the data-dir path and the expected
/// generation. Re-reads and re-validates the portfile on EVERY call.
#[derive(Clone)]
pub struct TargetResolver { data_dir: PathBuf, expected: Generation, /* timeouts */ }

impl TargetResolver {
    /// Produce a freshly validated dial target + token, or a typed error if
    /// the portfile is missing, torn, stale, non-loopback, or the wrong
    /// generation. Called once per connection attempt by the transport.
    pub async fn resolve(&self) -> Result<DialTarget, SupervisorError>;
}

/// The one value the transport dials with.
pub struct DialTarget {
    /// `ws://127.0.0.1:<port>/ws?v=<protocol>&sg=<storage_generation>` — the
    /// generation-gate query (protocol-v2 §Layer 1). Carries NO token.
    /// `Display`-safe.
    ws_url: url::Url,
    /// The per-start bearer token. Private; exposed only through `bearer()`,
    /// which every read greps for. Attached by the transport as
    /// `Authorization: Bearer`, never placed in the URL or any log.
    bearer: Redacted<String>,
}

impl DialTarget {
    pub fn ws_url(&self) -> &url::Url;   // token-free, safe to display
    pub fn bearer(&self) -> &str;        // native transport only
}
```

`DialTarget` and any struct holding the token implement a hand-written `Debug` that renders `bearer` as `<redacted>` (the pattern already used in `jeliya-client::kernel::diag::Redacted` and the spike's private `auth_token` field).

### 5.4 The portfile deserializer

The supervisor defines its **own** tolerant portfile struct (as the spike and Dart do — proving an independent client can speak the contract), reusing `jeliya_api`'s discovery-object type for the generation axes so there is one definition. Load-bearing fields (`pid`, `port`, `protocol`, `storage_generation`, `auth_token`, `data_dir`) are required; a portfile missing any is **unreadable** and treated as absent. Optional/informational fields (`http`, `ws`, `version`, `min_protocol`, `limits`, `started_at_ms`) are parsed tolerantly. A `schema` field, if present, is **ignored** (a v1 portfile is caught by the generation check, not by a schema number).

### 5.5 The error taxonomy

A closed enum, so a caller can distinguish "wrong generation, show the reset path" from "the daemon died, retry":

```rust
pub enum SupervisorError {
    NoBinary { tried: Vec<String> },
    Spawn(std::io::Error),
    Handshake(String),                 // no/garbled ready line, or ready↔portfile disagreement
    PortfileMissing(PathBuf),
    PortfileUnreadable { path: PathBuf, why: String },
    Stale { port: u16 },               // portfile present but no healthy daemon answers
    GenerationMismatch { expected: Generation, actual: Generation },  // fail-closed
    NonLoopback { advertised: String },// a portfile advertising a non-loopback endpoint
    Wedged,                            // lock held, no healthy daemon, no progress in window
    ShutdownTimedOut { pid: u32 },     // adopted/incumbent survived its bounded stop
}
```

`GenerationMismatch` is the one a UI translates into the clean-slate reset path; the rest are operational.

## 6. Behavior contract, step by step

### 6.1 Binary resolution (fail-closed)

Order, matching the spike and `app/README.md`: (1) `JELIYAD_BIN` override if it is a file; (2) a `jeliyad` bundled beside `current_exe()`; (3) **debug builds only**, the repo `target/debug/jeliyad`. An installed daemon on `PATH` is **not** silently used — a release shell that lost its bundled sidecar fails closed rather than pairing a release UI with an unknown daemon. `attach_to_running` needs no binary. On exhaustion, `NoBinary { tried }` lists every path attempted.

> Note vs. the spike's rationale: #170 *does* add a generation check (§6.4), so a `PATH` daemon of the wrong generation would now be caught rather than silently mispaired. The default nonetheless stays fail-closed; a `PATH` daemon is opt-in only through `JELIYAD_BIN`.

### 6.2 Data-dir resolution

Default to the per-user platform data dir the daemon itself uses (`dirs::data_dir()/Jeliya`; matches `jeliyad`'s `default_data_dir`), with an explicit override in `SupervisorConfig`. **Canonicalize** it (so lock/portfile identity compares like-with-like regardless of `/var` vs `/private/var`, symlinks, or path spelling — the daemon canonicalizes too, `crates/jeliyad/src/main.rs`). The data dir is stable for a handle's life; "fresh resolution on every reconnect" (§6.6) means re-reading the **portfile within** this fixed dir, not re-choosing the dir.

### 6.3 Spawn and the ready line

Spawn `jeliyad --supervised --data-dir <dir> --port 0 [--loopback]` with `stdin/stdout/stderr` piped and `kill_on_drop(false)` (deliberate — the stdin pipe, not `Drop`, is the parent-death mechanism; §6.8). Take the child's `stdin` and hold it for the daemon's whole life. **Drain stderr forever** on a background task (the daemon keeps a synchronous stderr tracing layer for life; an unread full pipe deadlocks it — the same file log survives under `<data_dir>/logs`). Read the **first stdout line that starts with `{`** within a bounded timeout (default 30 s); parse `{"event": "ready" | "already_running", "pid", "port", …}`.

For process-tree safety, spawn the child into its **own process group** on Unix (`Command::process_group(0)` / `setsid`) and, on Windows, into a **Job Object** configured to kill on close, so escalation in §6.7 can tear down the whole subtree rather than a lone PID. (The spike used a bare `child.kill()`; this is the "process-tree safe per platform" delta.)

### 6.4 Validation — ready ⟷ portfile ⟷ health agreement

The portfile is written **before** the announcement, so it is readable the instant the line parses. Require, before returning a `Sidecar`:

1. **ready ⟷ portfile:** `ready.pid == portfile.pid && ready.port == portfile.port`. A disagreement means a stale portfile from a prior run is being read → `Handshake`.
2. **Loopback:** `portfile.http`/`portfile.ws` (and the port the resolver dials) must parse as a loopback endpoint (reuse the daemon's `host_header_is_loopback` semantics; refuse anything that merely looks loopback). A portfile advertising a non-loopback endpoint → `NonLoopback`.
3. **Health, PID-bound:** `GET /api/health` on `127.0.0.1:portfile.port` (bounded connect + read timeouts) must return `200` with `ok == true` and `pid == portfile.pid`. This is the stale-portfile guard protocol-v2/PROTOCOL mandate ("never trust it blind"). **`data_dir` is not consulted** — v2 health does not serve it (§4 D2).
4. **Generation, fail-closed:** health's `protocol` and `storage_generation` (and the portfile's) must equal `self.expected`. Any mismatch or missing field → `GenerationMismatch` (not adoption). Absence is refusal.

`ready` that passes 1–4 → `Ownership::Owned`. `already_running` that passes 1–4 → the spawned child has already exited 0; drop our stdin, await the child's exit (non-zero → `Wedged`), and return `Ownership::Adopted`. `attach_to_running` runs 2–4 only (no spawn, no ready line).

### 6.5 Single-instance race and the "wedged" verdict

The daemon owns the OS advisory lock (`daemon.lock`) and rides out a SIGTERM-then-respawn overlap itself (`crates/jeliyad/src/lifecycle.rs` `acquire_or_adopt`). The supervisor therefore does not re-implement locking; it relies on the daemon's own `ready` vs `already_running` verdict. An **already-running race** (a second `start_or_adopt` on a live dir) yields `already_running` → adopt. **Exit 1 with no JSON line** (lock held, no healthy daemon, no progress in the daemon's ~15 s window) → `Wedged`; the caller retries briefly. A **delayed ready** beyond the spawn timeout → `Handshake` and the spawned child is cleaned up (§6.9).

### 6.6 Fresh target and token on every reconnect

`TargetResolver::resolve()` is what a transport calls per dial. It **re-reads the portfile from disk every time** and re-runs §6.4 steps 2–4 (loopback, health/PID, generation), then constructs a `DialTarget`:

- `ws_url = ws://127.0.0.1:<portfile.port>/ws?v=<protocol>&sg=<storage_generation>` — the generation-gate query travels in the URL because a browser `WebSocket` controls only the URL (protocol-v2 §Layer 1); the native transport uses the identical shape for parity. **No token in the URL.**
- `bearer = portfile.auth_token`, redacted, attached by the transport as `Authorization: Bearer` (protocol-v2 §"The credential never travels in a URL, and never in script": "Native clients send `Authorization: Bearer <token>`, read only from the `0600` portfile").

Because it re-reads each call, a daemon **restart** (new port, new token, possibly new PID) **heals transparently** — exactly the property the Dart `wsUrl()`/`authToken()` getters provide. A restart that changed the **generation** makes `resolve()` return `GenerationMismatch`, so a reconnect loop cannot silently attach to an incompatible daemon (protocol-v2's post-reconnect concern; the kernel's generation fence #168 is the second line of defense).

### 6.7 Protocol/storage skew — adopt vs. respawn vs. fail-closed

On a `GenerationMismatch` at `start_or_adopt`:

- **Never adopt** and **never spawn a second daemon on the same data dir** (protocol-v2/PROTOCOL adopt-vs-respawn rule; two daemons on one dir corrupt last-writer-wins state).
- **Replace only a proven-owned incumbent.** "Proven-owned" = the incumbent is proven to be *the jeliyad serving this exact data directory*: its portfile was read from this canonical dir **and** `/api/health` on `portfile.port` returns the matching `portfile.pid`. Only then may the supervisor evict it — prefer the caller-supplied `daemon.shutdown` RPC (protocol-agnostic fallback: SIGTERM `portfile.pid`), wait bounded for `/api/health` to go dark, then respawn the bundled binary (§6.3). If eviction times out → `ShutdownTimedOut`.
- **Otherwise fail closed.** If health cannot prove the PID owns the port (recycled PID, foreign process, stale portfile), the supervisor **does not signal anything** and returns `GenerationMismatch`/`Stale`. The non-goal is absolute: *never kill an incumbent that cannot be proven to own the exact data directory.*

Eviction is gated behind an explicit caller policy flag (`replace_incompatible: bool`, default **off**). A pure adopter (an agent riding along) never evicts; a packaged shell that owns the machine's daemon may opt in.

### 6.8 Parent death — orderly and abrupt

`--supervised` makes the daemon exit on **stdin EOF**, the portable parent-death signal on all three OSes (`crates/jeliyad/src/main.rs`). The supervisor holds the child's stdin pipe for the daemon's life:

- **Orderly:** `Sidecar::shutdown()` drops the stdin handle deliberately → EOF → graceful daemon exit (rooms closed, portfile removed) within the bounded budget (§6.9).
- **Abrupt:** if the shell panics or is `kill -9`ed, Rust runs no destructors, but the **OS closes the process's fds**, EOF-ing the child's stdin, and the daemon self-terminates within seconds. `kill_on_drop(false)` is required for this to be the *measured* guarantee rather than a `Drop` artifact.

Either way, **no duplicate daemon** survives: the abrupt-death daemon exits and releases its lock, so the next launch starts cleanly; an orderly shutdown removes the portfile too.

### 6.9 Shutdown escalation (bounded, per-platform)

- **Owned** (`Sidecar::shutdown`): drop stdin → `child.wait()` with a bounded budget (default 15 s: the daemon's own room-close teardown is bounded at ~10 s, plus margin). Exit within budget → `Teardown::Graceful`. Timeout → escalate: kill the child's **process group** (Unix) / close the **Job Object** (Windows), `wait()`, → `Teardown::Forced` (no daemon cleanup ran; a stale `daemon.json` may remain, which the next start's health check discards).
- **Adopted** (`Sidecar::stop_adopted`): the app owns no process handle, so the only lever is `daemon.shutdown` over the caller's RPC closure; then poll `/api/health` until dark, bounded → `Graceful`, else `ShutdownTimedOut`. **No signal is sent** — an adopted daemon is never SIGKILLed by the client.
- **Adopted, default:** `Sidecar::shutdown()` on an adopted daemon is a deliberate **no-op** → `Teardown::LeftRunning`.

## 7. Security and correctness invariants

1. **Loopback only.** Every dialed endpoint must parse as loopback; a portfile advertising otherwise is refused (`NonLoopback`). Matches the daemon's own bind-`127.0.0.1`-only and Host/Origin guards.
2. **Token stays native and redacted.** The token lives only in `DialTarget::bearer` (private), reaches only the native transport as `Authorization: Bearer`, and is `<redacted>` in every `Debug`/log. It is never in a URL, never a Dioxus prop or DOM attribute, never crosses into WebView script (architecture trust-boundary invariant; issue non-goal). A `tests/` assertion greps rendered `Debug` and any log surface for the token and fails if present (mirrors `jeliya-client` `WireFrame` redaction test).
3. **Atomic, private portfile handling.** The daemon writes the portfile atomically (temp + rename) at `0600`; the supervisor never writes it and reads it as a whole (a readable portfile is never half of one). On Unix the supervisor **should** verify the mode is not group/other-readable and refuse (or loudly warn on) a world-readable portfile as token-leak defense-in-depth — noting the threat model (`docs/protocol-v2.md` §"cross-user") treats loopback as inherently local-user-readable, so this is best-effort, not a hard guarantee. (Decision point: warn vs. refuse — see OQ-3.)
4. **Identity agreement.** PID-on-advertised-port + portfile-location + protocol + storage-generation must all agree before adoption or before any dial (§6.4). This is the recycled-PID/recycled-port defense.
5. **Fail-closed generations.** A protocol or storage-generation mismatch never adopts and never migrates; it fails closed (`GenerationMismatch`) and only a proven-owned incumbent is ever replaced.
6. **Never over-signal.** The supervisor signals a PID only when it owns the process (owned teardown) or has proven that PID is this data dir's daemon (proven-owned eviction). Never on a bare portfile PID.
7. **Bounded everything.** Spawn wait, health probes, teardown, and eviction are all bounded; no unbounded wait can turn a hung daemon into a wedged app.

## 8. Test strategy — the fault matrix

A headless integration harness (no display; extends `spikes/dioxus-desktop/tests/supervision.rs`) drives a **real** `jeliyad` built from the workspace, plus **synthetic-portfile** unit tests for cases a real daemon will not produce on demand. Every case was, in the spike's discipline, confirmed to **fail against a deliberately broken supervisor before being trusted**; keep that rule (a "deliberate regressions" note in the crate README).

| # | Fault case (from the issue) | Setup | Expected outcome |
|---|---|---|---|
| 1 | **Owned lifecycle** | `start_or_adopt` on a fresh dir | `is_owned()`, portfile↔ready agree, token is 64-hex and absent from `ws_url`, `shutdown` → `Graceful`, PID gone, portfile removed |
| 2 | **Adopted lifecycle** | second `start_or_adopt` on a live dir | `!is_owned()`, adopts incumbent PID/port; `shutdown` → `LeftRunning`; incumbent survives; owner's `shutdown` ends it |
| 3 | **Adopted never stopped by client** | adopted `Sidecar`, drop it / `shutdown()` | incumbent still alive and healthy afterward |
| 4 | **Fresh target per reconnect** | resolve, restart daemon (new port/token), resolve again | second `DialTarget` carries the new port and token; no stale value cached |
| 5 | **Truncated portfile** | write half a JSON portfile | `PortfileUnreadable`; no adoption, no dial |
| 6 | **Wrong-schema / v1 portfile** | portfile with `schema:1, protocol:1`, no `storage_generation` | `GenerationMismatch` (fail-closed, clean-slate) — **not** adopted, **not** migrated |
| 7 | **Stale portfile (dead PID/port)** | portfile for a PID/port with nothing behind it | health fails → `Stale`; on spawn path, a fresh daemon starts and heals |
| 8 | **Delayed ready** | daemon slow to announce (inject via a stub binary) | `Handshake` after the bounded timeout; spawned child cleaned up |
| 9 | **Port collision** | occupy `--port` target so the daemon scans upward | supervisor reads the real bound port from ready/portfile; dials it |
| 10 | **Recycled PID** | portfile PID now belongs to an unrelated process | health on `portfile.port` fails/does not match → not adopted; incumbent **not** signalled |
| 11 | **Recycled port** | an unrelated listener answers on `portfile.port` | `/api/health` PID mismatch (or non-JSON) → refused |
| 12 | **Already-running race** | two `start_or_adopt` concurrently | one owns, one adopts; exactly one daemon |
| 13 | **Incompatible incumbent, proven-owned** | live daemon of a different generation, `replace_incompatible = true` | evicted via `daemon.shutdown`/SIGTERM, waited dark, respawned; new daemon matches expected generation |
| 14 | **Incompatible incumbent, unprovable** | mismatched generation but health cannot prove PID owns port | `GenerationMismatch`/`Stale`, **nothing signalled** |
| 15 | **Hung shutdown** | daemon that ignores stdin EOF (stub) | owned `shutdown` escalates to process-group/Job kill → `Forced`; no orphan subtree |
| 16 | **Orderly app death** | `Sidecar::shutdown()` | graceful; portfile removed; no duplicate on next start |
| 17 | **Abrupt app death** | drop `Sidecar` without shutdown (simulates panic/kill) | stdin EOF ends the owned daemon within seconds; PID gone; no duplicate |
| 18 | **Adoption (attach-only)** | `attach_to_running` against a daemon someone else spawned | adopts via portfile+health+generation; `shutdown` → `LeftRunning` |
| 19 | **Non-loopback portfile** | portfile advertising a non-loopback `ws`/`http` | `NonLoopback`; never dialed |
| 20 | **0600 enforcement (Unix)** | portfile relaxed to group/other-readable | refused-or-warned per OQ-3; asserted either way |
| 21 | **Token redaction** | render `Debug` of `DialTarget`/`Sidecar`; scan logs | token never appears; `ws_url` never contains the token |

CI: these are native-only tests; they run in the `rust-runtime` job (and MSRV `1.91`), gated behind building `jeliyad` first. `wasm32-unknown-unknown` must **not** attempt to build this crate — assert the crate is absent from the wasm graph (mirror the `jeliya-api` wasm-graph assertion).

## 9. Acceptance criteria → where satisfied

| Issue acceptance criterion | Satisfied by |
|---|---|
| Owned and adopted lifecycles are distinguishable and tested | §5.1 `Ownership`/`is_owned()`; tests #1, #2, #18 |
| Adopted daemons are never stopped by the client | §6.9 (`stop_adopted` uses RPC only; `shutdown` → `LeftRunning`); tests #2, #3, #18 |
| Every reconnect receives a freshly validated target/token | §6.6 `TargetResolver::resolve` re-reads + re-validates each call; test #4 |
| Protocol/storage mismatch fails closed; only a proven-owned incumbent may be replaced | §6.4 step 4, §6.7; tests #6, #13, #14 |
| Orderly and abrupt parent death leave no duplicate daemon | §6.8; tests #16, #17 |
| Shutdown escalation is bounded and process-tree safe per platform | §6.3 (process group / Job Object), §6.9; test #15 |

## 10. Risks

- **R1 — process-tree safety is platform-specific and easy to get subtly wrong.** Unix process groups vs. Windows Job Objects behave differently; a naive `child.kill()` (the spike) leaves grandchildren. Mitigation: the escalation path is a first-class, per-platform tested unit (test #15), and Windows support is gated behind the M4 Windows decision (`docs/dioxus-architecture.md` — Windows is "include or formally defer").
- **R2 — health `data_dir` removal.** Porting the Dart supervisor verbatim (which checks `health['data_dir']`) would break against every v2 daemon. Mitigation: §4 D2 makes PID-on-port the binding; test #10/#11 lock it.
- **R3 — token leakage through the WebView.** The whole point of a native supervisor is that the token never reaches script. Mitigation: private field + accessor + redaction test #21; the resolver hands the transport a token, and the transport (a separate reviewed crate #172) attaches it as a header.
- **R4 — over-eager eviction.** A bug that signals a bare portfile PID could kill an unrelated process or a foreign daemon. Mitigation: eviction requires proven ownership (health PID match on the advertised port) **and** an explicit `replace_incompatible` opt-in; test #14 proves the unprovable case signals nothing.
- **R5 — clean-slate regressions.** Silently adopting a v1/Flutter daemon would violate the cutover. Mitigation: `schema` is ignored, generation is required and gated (test #6).
- **R6 — decoupling vs. drift.** Injecting `expected` generation (rather than reading `jeliya-core` constants) risks a shell built against the wrong number. Mitigation: the transport/shell reads its expected generation from a single `jeliya-api` source (OQ-1); a startup self-check compares `expected` against the daemon it just built/bundled.

## 11. Open questions

- **OQ-1:** Should the integer `PROTOCOL_VERSION`/`STORAGE_GENERATION` (today in `jeliya-core::engine`) be hoisted into `jeliya-api` so the supervisor and transport share **one** source of the expected generation without depending on the Iroh-bearing core? protocol-v2 already places the discovery *object* in `jeliya-api`; the constants are a natural companion. **Recommendation:** yes, in a small companion change, but keep `expected` injectable so tests can construct mismatches.
- **OQ-2:** Does the `Portfile` struct get hoisted into a shared location (e.g., `jeliya-api`) so the daemon and supervisor share one definition, or does the supervisor keep its own tolerant deserializer to preserve the "independent client speaks the contract" property? **Recommendation:** keep a supervisor-local tolerant deserializer that reuses `jeliya-api`'s discovery object; revisit if a third native consumer appears.
- **OQ-3:** On Unix, a world-/group-readable portfile — **refuse** (`PortfileUnreadable`) or **warn and proceed**? The threat model says loopback is inherently local-user-readable, so refusing is defense-in-depth that could also break a legitimately odd umask. **Recommendation:** warn-and-proceed by default, with a strict-mode config that refuses.
- **OQ-4:** Where does the `daemon.shutdown` RPC used by `stop_adopted` and by eviction come from without linking the client? **Recommendation:** a caller-supplied closure (the desktop shell already holds a `ClientHandle`); the supervisor stays client-free. Confirm with #172.
- **OQ-5:** Windows in M4 — is the Job-Object escalation in-scope now or formally deferred with the rest of Windows packaging? Track against the architecture's Windows decision and #189 (desktop matrix).
- **OQ-6:** Should `start_or_adopt` expose a bounded **retry** for the `Wedged` verdict (mid-restart overlap) or leave retry to the caller? **Recommendation:** leave retry to the caller; expose `Wedged` distinctly so a UI can say "starting, retry in a moment".

## 12. Out of scope and the clean-slate cutover

- No desktop UI, no WebSocket request semantics, no token-into-WebView, no killing an unprovable incumbent (§2).
- **Clean-slate:** the supervisor recognizes **exactly** the built-against protocol and storage generation. It does **not** read or migrate Flutter/v1-created state. A non-matching generation is an incompatible incumbent (evict-if-proven-owned, else fail closed), never an upgrade source. Generation-scoped **data-dir/preferences** versioning is owned elsewhere (#185, the desktop preferences store and its version key); this supervisor only *gates* on the served `storage_generation`, it does not lay out or migrate on-disk state.
- The retired Dart supervisor and `docs/PROTOCOL.md` (v1) are **prior art**, not compatibility targets; their public shapes are not preserved.

## 13. Implementation steps (ordered, for the implementing phase)

1. **Scaffold `crates/jeliya-supervisor`** as a native-only workspace member: `Cargo.toml` (tokio `process`/`net`/`time`/`io-util`, `serde`, `serde_json`, `jeliya-api`, `dirs`, `url`), `[lints] workspace = true`, MSRV 1.91, `unsafe_code = "forbid"`. Add `tests/boundaries.rs` asserting no Dioxus/Iroh/`jeliya-core`/`jeliyad`-bin/`jeliya-client`/wasm dependency.
2. **Portfile deserializer + `Generation`** (§5.4), reusing `jeliya_api`'s discovery object; required vs. tolerant fields; ignore `schema`. Unit tests for truncated / missing-field / v1-shaped inputs (cases 5, 6).
3. **`TargetResolver` + `DialTarget`** (§5.3, §6.6): read-and-validate each call; loopback check; generation gate; token redaction. Unit + redaction tests (cases 4, 19, 21).
4. **Binary + data-dir resolution** (§6.1, §6.2), fail-closed order; `NoBinary { tried }`.
5. **`Supervisor::start_or_adopt`** (§6.3–6.5): spawn with process-group/Job-Object, stderr drain, `kill_on_drop(false)`, ready-line parse, agreement validation, owned/adopted split. Real-daemon tests (cases 1, 2, 8, 9, 12).
6. **`attach_to_running`** (§6.4 steps 2–4). Test 18.
7. **Skew handling** (§6.7): `GenerationMismatch`, proven-owned gate, `replace_incompatible` opt-in, evict-then-respawn via caller RPC/SIGTERM, bounded wait. Tests 13, 14.
8. **Shutdown escalation** (§6.9) owned/adopted/stop_adopted; per-platform process-tree kill. Tests 3, 15, 16, 17.
9. **Security guards** (§7): Unix 0600 check per OQ-3; loopback enforcement; redaction assertions. Tests 19, 20, 21.
10. **CI + docs:** add native-only tests to the `rust-runtime` matrix, assert absence from the wasm graph, and record the new component in `docs/dioxus-architecture.md` (the `daemon supervisor` row already exists — update it from "new" to "landed" with the crate path, exactly as prior slices do) and note the supervisor's contract in `docs/platform-matrix.md` if applicable. Update `docs/PROTOCOL.md`/`docs/protocol-v2.md` only if a discrepancy is found (do not restate the contract).

> Docs/profile note: any new page or amendment must satisfy `node scripts/check-docs.mjs` (OKF frontmatter, four status axes, index reachability). This spec file under `specs/` is not a `docs/` page and is exempt from that gate, consistent with the existing `specs/` entries.
