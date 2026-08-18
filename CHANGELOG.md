# Changelog

## [Unreleased]

### Added

- New workspace crate `crates/jeliya-platform` — the single injectable
  `PlatformServices` boundary for the clean-slate Dioxus stack (#156 program,
  #174). One cloneable, renderer-agnostic facade carries object-safe capability
  traits for files, persistence, lifecycle, URLs, clipboard/share, navigation,
  and window actions, with a closed outcome taxonomy (`CapabilityError` keeps
  `Unavailable`/`Denied`/`Cancelled`/typed failures apart, so a cancellation
  never becomes success), safe path/URL types that distinguish a browser blob
  from a desktop path from an Android `content://` URI, an allowlisted
  fail-closed external-URL launcher, honest storage durability, and a
  representable lifecycle-event model. `Route` is the canonical product route
  family (`/rooms`, `/rooms/:roomId/{activity,people,agents,files,pipes}` with
  typed file/pipe item selection, `/fleet`, `/settings`), parsed fail-closed
  (malformed percent-escapes and empty interior segments are errors, stricter
  by design than the web shell's total parse — the router maps `Err` to the
  Rooms recovery state) and rendered byte-identically to the web shell's
  `encodeURIComponent` spelling. Lifecycle control intents are lossless *and*
  bounded: a saturated mailbox run-length-encodes a Back burst and absorbs a
  restated close/restore into its still-undelivered twin, hard-capping the
  mailbox at capacity plus a fixed control allowance. File bytes cross the
  boundary as bytes, matching protocol v2's byte-stream framing: a
  `StagedBlobReader` (pull, CREDIT-shaped) feeds the `file.share` upload from
  a staged blob, and `FileSink`s from `export_sink`/`open_sink` accept
  `file.read` DATA chunks (write-resolution is credit-advance; dropping an
  uncommitted sink deletes the partial artifact) — the retired
  v1 local-file HTTP edge is unrepresentable. Sharing a room file uses the
  same byte discipline in the third direction: `share_sink` accepts the
  pumped `file.read` stream and `commit` mints a `FetchedArtifact` the OS
  share sheet accepts, so `ShareContent` carries only handles to bytes the
  producing service custodies — never a bare `(RoomId, FileId)` the platform
  has no way to read — and a successful share consumes the artifact. The
  handle custody is explicit in both directions. Every handle that names
  service-held bytes or a grant has exactly one release — `release_staged`,
  `release_artifact`, `discard_source`, `discard_export_target` — and
  consumption is the other half (`stage_for_share` drops its source on every
  outcome, `export_sink` consumes its target, a successful share consumes the
  artifact), so nothing is retained by a caller doing nothing. The signal
  cannot be inferred: reaching EOF in a reader means the bytes were read, not
  that the daemon accepted them, and dropping an opaque handle is invisible to
  the service, so without an explicit release "delete after share" is
  unimplementable.
  `FileName` is validated, not merely promised: `FileName::parse` fails
  closed on separators, `.`/`..`, empty, and control characters, so a
  peer-supplied name cannot carry portable path syntax into a native sink —
  with the platform-specific remainder (Windows `:` and trailing dots/spaces,
  reserved device names, normalization, length caps) explicitly left to the
  sink and documented as such, because those strings are ordinary file names
  on the committed targets. Text
  language and formatting locale are two independent preference keys, per the
  product contract's "text locale != formatting locale from day one". Clipboard writes are
  asynchronous (a browser denial is the `writeText` promise rejection), and a
  default-off `implementation` feature exposes path-free factories so the
  M3–M5 target crates (separate crates by design) can construct
  `PickedSource`/`ExportTarget`/`ShareableBlob`/`FetchedArtifact` without any
  path crossing the boundary. It ships a
  deterministic in-process fake for every service (`feature = "fake"`) in
  browser/desktop/Android shapes, scriptable for
  denied/unavailable/cancelled outcomes; scripted picker/dialog/share
  operations stay open until the test advances them via
  `FakeController::deliver_next` (outcome bound at call time, cancellation
  wins races, drop withdraws), so cancellation-vs-reply ordering is explicit,
  never a race. A shared Dioxus component compiled against the fakes links for
  both native and `wasm32-unknown-unknown` with no per-component `cfg`. No
  Iroh, WebSocket, native transport, `wry`/`tao`, `openssl-sys`, or Dioxus
  enters the library graph, and no `serde_json::Value` appears in any public
  signature. Target implementations (browser web-sys, desktop dialogs, Android
  SAF/JNI) follow in M3–M5 behind the unchanged facade. The decision is
  recorded at `docs/dioxus-architecture.md` §"Decision 4".

- New workspace crate `crates/jeliya-platform-implementation` — the single
  blessed door to those factories (#174 §K4). Cargo unifies features per
  package across a build graph, so a default-off feature is not a boundary in
  a target binary: the moment any crate enables `implementation`, the factory
  module is compiled into the one `jeliya-platform` instance the shared UI
  also links. A dependency edge does not unify, so the boundary is one — this
  crate is the only manifest permitted to enable the feature, the factories
  are path-addressed free functions (so a call site must spell
  `implementation`, which a workspace-wide code scan rejects outside the
  door), and a `cargo tree` test asserts the shared UI graph has no edge to
  it. Underneath all three, minted-token registries fail closed on any handle
  a service did not mint, so a forged or cross-service handle resolves
  nowhere.

- `crates/jeliya-ui` adopts that canonical contract (#174): its former
  provisional local seam (`src/services.rs` and `WebPlatformServices`) is
  deleted and replaced by a re-export of `jeliya_platform::PlatformServices`,
  with composition injecting a deterministic fake shape — the mechanical change
  #176 promised, and no shared component gains a `cfg` fork.

- New workspace crate `crates/jeliya-client` — the single UI-facing Rust client
  contract for the clean-slate Dioxus stack (#156 program, #167). Delivers
  compile-time request/output pairing (`ClientHandle::call<O: Operation>`
  returns `O::Output`; no untyped "call by string"), an honest observable
  lifecycle (`State`, `start`/`stop`, `ClientEvent::StateChanged`),
  multi-consumer fan-out events where no consumer can silently steal another's
  pushes, a total may-have-executed error model (`CallError::execution()` covers
  every variant), and a deterministic clock-free in-process mock
  (`feature = "mock"`) that scripts responses, errors, push-before-response,
  gaps, cancellation, and shutdown reproducibly on wasm and native. A shared
  Dioxus component compiled against the mock links for both native and
  `wasm32-unknown-unknown` with no per-component `cfg` logic. Backend erasure
  stays internal: `ClientHandle` wraps `Arc<dyn ClientBackend>`; no Iroh,
  WebSocket, `tao`/`wry`, or Dioxus dependency enters the library. The decision
  is recorded at `docs/dioxus-architecture.md` §"Decision 4".

- `crates/jeliya-client` gains the bounded, lifecycle-aware client kernel (#168):
  the transport-independent sans-IO state machine that sits behind
  `ClientBackend` and gives the seam its real machinery. The kernel's
  correctness properties — hard request bounds, deterministic settlement,
  deadlines, cancellation at every phase, generation fencing, capped jittered
  backoff, honest post-send uncertainty, and total stop — all live in a pure
  synchronous core (`src/kernel/core.rs`) that takes logical time as input and
  emits `Action` values for a driver to perform, with no wall clock, no spawns,
  and no RNG syscall. This makes every fault case a deterministic sequence of
  `step` calls, identical on wasm and native.

  Key guarantees:

  - **Bounded admission.** `KernelLimits.queue_depth` (count) and
    `outbound_bytes` (bytes) are admission-time refusals surfaced as
    `CallError::QueueFull`; `in_flight` is a throttle, not a rejection.
  - **Exactly-once settlement.** A take-once in-flight ledger makes duplicate,
    late, and malformed replies structurally unable to strand a call or
    double-settle it.
  - **Replay only where guaranteed.** Four gates, ALL required: the call is
    mutating, carries a caller `op_id`, names an operation in the protocol's
    13-operation `op_id`-deduplicated set, and the driver certifies dedup-scope
    continuity (`KernelConfig::stable_principal` — stable `client_id` AND same
    daemon incarnation; **default off**). Everything else is
    `ReplayPolicy::Never` and settles a disconnect honestly as
    `Disconnected { Unknown }` — a keyed `daemon.stop` never replays, and
    nothing replays under the default configuration.
  - **Honest post-send uncertainty.** Connection loss classifies outstanding
    work as `Execution::DefinitelyNot` (never-sent) or `Execution::Unknown`
    (may-have-executed) by consulting each call's `sent` state in the ledger.
  - **Cancellation without remote lies.** Dropping a future tombstones the
    call locally and sends no cancel frame; the daemon may still run the
    operation. Only `transfer.cancel` cancels remotely.
  - **Generation fencing.** Every in-flight call and every inbound frame is
    stamped with a monotonic connection generation; stale-generation replies and
    pushes are discarded before they can settle a call or overwrite newer state.
  - **Capped jittered backoff.** Reconnect attempts use full-jitter exponential
    backoff from a deterministic in-core xorshift PRNG seeded at construction
    (`KernelConfig.jitter_seed`), with `max_reconnect_attempts` exhaustion and
    an honest `State::Failed` settlement — no infinite spin.
  - **Total stop.** `Input::Stop` cancels any in-progress dial/backoff, drains
    the queue and in-flight ledger (settling each call exactly once), and leaves
    no unbounded task or map behind.

  New public types: `KernelLimits`, `KernelConfig`, `TickDelta` (re-exported
  from `jeliya_client`). The deterministic in-memory driver and
  `KernelController` ship behind the `test-transport` feature (default-off) as
  the reference the four adapters (#171/#172/#173) are diffed against (#175).
  `ClientHandle::with_kernel(config)` constructs a kernel-backed handle for
  tests and adapters. The kernel adds no new runtime dependency; no concrete
  socket is implemented (those are #171/#172/#173). MSRV 1.91.

- `crates/jeliya-client` gains the authoritative room/session reconciler
  (#169): the transport-independent coordinator that sits *above* the seam and
  ensures every detectable push gap, reconnect, local fan-out overflow, and
  Android process-resume produces the **same** authoritative re-baseline, and
  nothing else does. Like the kernel, it is a **sans-IO core**
  (`src/reconcile/core.rs`) wrapped by a thin async driver
  (`src/reconcile/driver.rs`); every fault case — push during bootstrap,
  reconnect during open, repeated gaps, overflow, cancellation, resume, stale
  generations — is a deterministic `step(Input) -> Vec<Action>` sequence,
  identical on wasm and native. It consumes the seam's `EventSubscription` and
  issues reads through `ClientHandle::call`; the seam's public call/lifecycle
  semantics and kernel behavior remain unchanged. Malformed-frame recovery uses
  the private adapter/reconciler path rather than adding a public event variant.

  Key guarantees:

  - **Every gap reason is observable.** `ResyncReason` is emitted at the *start*
    of every reconciliation, before the baseline read settles: `Bootstrap`,
    `Reconnect`, `Gap { reason, to }` (wire cause preserved so `backpressure`
    / `retention` / `subscription_lapse` stay distinguishable),
    `LocalOverflow { dropped }` (fan-out `Lagged` or per-room buffer overflow),
    `ResyncRequiredByDaemon { from_pos }`, and `Resume`. Malformed frames use
    the private adapter/reconciler decode-failure path and force bounded gap
    recovery without changing the public `ClientEvent` model.
  - **One serialized, coalesced reconciliation per room.** At most one baseline
    read is in flight per room at a time; a new trigger while one runs coalesces
    into a single pending re-run keeping the highest-priority reason. Repeated
    flap therefore yields one in-flight + one queued reconciliation, never an
    unbounded stack.
  - **Overflow cannot permanently deduplicate an undelivered event.** The dedup
    watermark and recent-`event_id` ring record only events the reconciler
    validated and applied; an overflowed push is never recorded as applied and
    is re-read by the forced fresh baseline. Any accrued rerun is a publication
    barrier, so a partial or repudiated intermediate view is not emitted. When
    a stronger cause performs that recovery, the deferred loss count appears as
    an attributed `Lagged` boundary before its final view without another read.
  - **Convergence by signed evidence.** Baseline reads (`room.timeline` or
    `stream.resync`) and buffered live pushes converge by `pos` (ordering),
    `event_id` (dedup), and signed `at`/`author` (insertion authority). A hole
    in the applied position range is a protocol violation → fresh resync.
  - **Strict origin and anchor semantics.** A replacement timeline must begin
    with the unique position-zero `RoomCreated`; later origins are rejected on
    authoritative, buffered, and live paths. Subscription `from_pos` is a
    concrete inclusive anchor (zero included) treated as a completeness floor,
    not a stop cursor: the bootstrap reads to `Complete` and retains validated
    post-anchor events, so a push that raced the room's activation (delivered
    while the room was untracked) is recovered by the baseline read instead of
    vanishing. A history that completes below the anchor is structurally
    invalid, and changing an anchor immediately fences/cancels stale I/O while
    preserving buffered evidence and replacement intent.
  - **Peer state replaced, never merged.** `room.members` and `room.peers` are
    replaced wholesale from authoritative reads on every reconciliation that
    implicates presence (bootstrap, reconnect, resume, daemon-forced). A
    stale-generation `peer` teardown cannot resurrect a member an authoritative
    read removed. Fences and tombstones are per device (not a room-global max),
    same-generation changes preserve arrival order, ambiguous removed keys force
    a refresh, and new transport epochs reset payload counters behind K7.
  - **Generation fencing at the coordinator.** A reconciler-local monotonic
    epoch advances on every liveness (re)establishment; a baseline completing
    under a stale epoch is discarded (`DropStale`), never applied.
  - **DirectClient resume without a fabricated reconnect.** `Reconciler::resume`
    triggers the same bounded re-baseline with `reason = Resume` and emits no
    synthetic `StateChanged` — the transport did not drop.
  - **Bounded by construction.** All payload-bearing reconciler collections
    carry count and byte bounds from `ReconcileLimits`: a combined event/peer
    push budget, opaque-identifier ceiling, backend-output byte/JSON-token/depth
    and decoded-page caps (transports still cap frames before `RawJson`),
    rendered timeline depth + bytes, member/peer capacities, tracked-room
    ceiling, and per-subscriber `RoomUpdate` bytes. Position-aware durable and
    per-scan exact identity maps enforce history-wide uniqueness up to explicit
    supported-history count/byte ceilings while permitting daemon-truncated
    suffixes to be re-read. Backend reads are globally capped and queued
    deterministically. Conservative defaults couple 16 active rooms, four
    concurrent 2-MiB replies, 1-MiB history/timeline ceilings, and 256-row
    snapshots so their aggregate stays operational rather than multi-GiB.
    Oversized authority is rejected rather than truncated, and ordinary controls
    use one finite serialized transaction while stop retains a dedicated slot.
  - **Audit-hardened convergence gates.** A trigger naming a committed position
    above the watermark (gap cursor, bounded gap end, or daemon
    `resync_required` cursor) blocks publication until authority serves through
    that frontier — one bounded chase, then a fail-closed park. Daemon
    `resync_required` redirect chains require strict cursor progress within a
    bounded length (a non-progressing redirect retries once, then parks). A
    rollback below the rendered window whose recovery suffix leaves the window
    empty rebuilds it with a full timeline replacement instead of publishing an
    empty view for a room that has history. Any lowering truncation forces an
    authoritative `room.members` replacement, so a repudiated membership event
    cannot survive in the derived roster. Contradictory evidence at a committed
    position discards back to before the disputed position so authority itself
    re-proves it — the first arbitrary claimant never survives into a published
    view. Peer pushes dropped by buffer limits (or while parked) keep their
    generation fence against stale same-generation resurrection, and saturated
    tombstone retention is deterministic. Quantitative `LocalOverflow` counts
    outranked in any coalesce are banked and surfaced as an attributed `Lagged`
    boundary before the covering view. The driver bounds push-before-reply
    priority with a fairness budget so continuous event traffic cannot starve a
    settled read, and the seam fan-out accounts mailbox bytes per slot so
    synthesized loss markers never uncharge retained payload.

  New public types: `Reconciler`, `ReconcileConfig`, `ReconcileLimits`,
  `ResyncReason`, `ResyncRequired`, `RoomView`, `RoomUpdate`,
  `RoomUpdateSubscription`, `ReconcileError` (all re-exported from
  `jeliya_client`). `RoomUpdate::Resyncing` carries the cause before the
  outcome; `RoomUpdate::Converged` carries the authoritative view. Mailboxes
  coalesce causally compatible views, share payloads across subscribers, and
  enforce both a 256-update depth and `update_mailbox_bytes`, surfacing any
  eviction before retained later work as an attributed `Lagged` marker. One
  shared oversized latest-authority allowance prevents permanent Lagged-only
  delivery and replaces repeated giants rather than stacking them. Driver/run
  cancellation and last-owner drop close subscribers and release reads through
  an RAII terminal guard. The reconciler adds no new runtime
  dependency; `tests/boundaries.rs` gains a source scan asserting no
  `std::time`, `Instant::now`, `SystemTime`, `getrandom`, `rand`, or `tokio`
  token appears in `src/reconcile/**`. The adapter-facing test suites
  (`tests/reconcile.rs` + `tests/reconcile_driver.rs`) and the four transport
  adapters (#171/#172/#173) with their parity suite (#175) follow as separate
  issues. MSRV 1.91.

- `crates/jeliya-client` gains the kernel stream lifecycle layer (#269):
  `call_stream::<FileShare>` and `call_stream::<FileRead>` now drive a full
  `OPEN → DATA/CREDIT → END → terminal Text reply` lifecycle (or `ABORT` on
  failure) through the kernel against the deterministic in-memory transport.
  The stream layer is a byte-free companion state machine
  (`src/kernel/streaming.rs`) over the same sans-IO core that drives
  request/reply — `StreamEntry` holds only offset scalars, never payload bytes,
  so the framing (`JBS2` header, per-kind field rules, offset arithmetic) stays
  owned by `jeliya-codec` and the daemon executor (#233/#242/#243). The
  transport seam (`kernel/transport.rs`) grows a binary-record concept
  (`StreamRecordIntent`, `StreamRecordMeta`, `Inbound::Record`) and a media seam
  (source/sink `Action`/`Input` variants) so adapters can frame via `jeliya-codec`
  at the driver boundary without the core holding any bytes.

  Key guarantees:

  - **Credit-bounded outbound bytes.** Outbound DATA never exceeds the daemon's
    cumulative `send_through`; the read-ahead window is byte-bounded
    (`stream_window_bytes`) independent of file size; ACK is ABORT-only — never
    on the success path.
  - **Per-stream absolute deadline.** Armed at OPEN as
    `transfer_connect_allowance + ceil(total·8 / transfer_floor_bits_per_second)`,
    replacing the request/reply base deadline. Produces `CallError::Timeout`
    (`Unknown`) on expiry and sends a courtesy client ABORT to release the
    daemon's transfer reservation.
  - **Stall timer.** A stream that stops making accepted progress for
    `transfer_stall` fails honestly. The timer re-arms on every accepted-progress
    event; `Finalizing` (END emitted or accepted, awaiting the terminal reply)
    is uncancellable — neither timer aborts it.
  - **Honest teardown.** ABORT/ACK, connection loss, cancellation, and total
    stop each settle the terminal exactly once and leave no unbounded task, map,
    or timer behind. Stream tombstones (bounded at `max_concurrent_streams`,
    evicted FIFO) absorb late daemon records so stranded state is structurally
    impossible.
  - **Streams never auto-replay.** `file.share` and `file.read` are now
    `ReplayPolicy::Never` regardless of `mutating`, `op_id`, or
    `stable_principal`. Previously, `"file.share"` appeared in the
    `op_id_deduplicated` set, so a `file.share` dispatched with `Dedup::Key`
    under `stable_principal = true` would have had its Text request re-sent
    across a reconnect into a mid-stream `op_id` — returning
    `stream_aborted{transport_lost}` as if it were the original result. The
    gate is `is_stream_op` in `replay.rs`; the fix is covered by a
    red-before-green test (`replay_hold_preserves_original_send_order` swapped
    `file.share` → `file.fetch`).
  - **Bounded by construction.** `StreamTable` (active, finalizing, and
    tombstoned streams), per-stream timers (≤ 2 while Active, 0 on terminal),
    and the outbound read-ahead / inbound quarantine window are all statically
    or configurably bounded; `streams()`, `stream_timers()`, and
    `stream_window_bytes_reserved()` observers let fault tests assert every
    bound holds under churn and after stop.

  New public type: `StreamLimits` (re-exported from `jeliya_client`) — the
  six served transfer bounds a stream is driven under
  (`transfer_connect_allowance`, `transfer_floor_bits_per_second`,
  `budget_ticks_per_second`, `transfer_stall`, `stream_window_bytes`,
  `max_concurrent_streams`); validated at `KernelConfig` construction so an
  invalid served configuration refuses readiness before any stream is admitted.
  The `test-transport` feature's `KernelController` gains `open`, `credit`,
  `deliver_data`, `end`, `abort`, `ack`, and the corresponding observers
  (`take_outbound_records`, `streams`, `stream_timers`,
  `stream_window_bytes_reserved`) so every stream fault is a deterministic
  sequence of `step` calls, identical on wasm and native.
  `SentRecord` (re-exported for adapter tests) captures outbound record
  observations (id, kind tag, offset, length) in a redaction-safe form — no
  payload bytes, file names, or free strings.
  The stream layer adds no new runtime dependency; `boundaries.rs`'s existing
  `src/kernel/**` token scan covers the new module unchanged.
  MSRV 1.91.

- `crates/jeliya-ui` gains the room **Activity** destination (#179 — M3,
  first room-content slice): the signed timeline, composer, and
  evidence-backed send lifecycle under `/rooms/:roomId/activity`, replacing
  the `RoomShell` Activity skeleton with the real pane. The split is a pure,
  renderer-/web-sys-free `room/` core (`projection`, `runs`, `send`, `scroll`,
  `reconcile`) whose exhaustiveness the compiler enforces, and thin Dioxus
  components (`ActivityPane`, `Composer`, `TimelineRowView`) that own DOM
  measurement through the renderer-agnostic mounted element API — no `web-sys`,
  no `cfg` (Decision-3/-6).

  Key properties:

  - **Exhaustive, total event projection.** A `match` over the closed
    10-kind `EventKindContent` that `rustc` forces to be exhaustive — no
    `return null` silent drop of any signed fact. A kind without a bespoke
    card renders as an inspectable generic row (author + signed time + localized
    kind label + safe metadata); a genuinely undecodable wire kind never reaches
    the view (the reconciler's `DecodeFailed` path forces a resync first).
  - **Folding / grouping as reversible view state.** Maximal same-author
    `AgentStatus` run folding (`RunSummary { count, first_at, last_at }`), five
    view-only activity filters (conversation / agent-runs / membership / files /
    pipes), day dividers, and 5-minute same-sender message compacting — all
    computed on top of the signed list, never mutating or dropping a signed
    fact. The counter and scroll accounting count the unfolded, unfiltered items.
  - **Evidence-backed send state machine.** `Pending` (call in flight) →
    `Syncing { event_id }` (daemon authored the event; awaiting the committed
    row) → **dropped** the instant the reconciler surfaces that `event_id`. A
    failed call classifies through `CallError::execution()`: `DefinitelyNot`
    ("not sent", clean retry), `Unknown` ("may not have sent", retry offered,
    never auto-taken), `Definitely` (treated as `Syncing`; a committed row will
    arrive). No fabricated "delivered" or checkmark (contract no-fake-state
    rules).
  - **Stable-`op_id` idempotent retry.** Each send mints one stable `OpId`
    (derived from the local `SendId`); Retry re-issues with the same `op_id`,
    so the daemon ledger returns the original `event_id` with no second effect
    on this connection (D4, contract rule 7). No auto-replay.
  - **Reconciler wired into `AppRoot`.** `AppRoot` constructs one
    `Reconciler` via `use_hook` and drives `reconciler.run()` via `use_future`;
    the Activity pane activates/deactivates its room on mount/unmount and
    folds the `RoomUpdateSubscription` into a `RoomActivityState` signal.
    Pending `Syncing` entries drop the instant the converged timeline contains
    their `event_id`; no duplicate can appear on reconnect.
  - **Pure scroll model.** `room/scroll.rs` holds stick-to-bottom / restore /
    new-item accounting as plain Rust math (no DOM); `ActivityPane` feeds it
    Dioxus-measured numbers and applies the result through the mounted element
    API. The "N new messages / N new activity" affordance words itself by what
    the trailing new items actually are.
  - **Per-room drafts across route changes.** `PreferenceKey::Draft{room_id}`
    in the `jeliya.dx.v1` namespace (session-scoped `WebPreferences`). Draft
    restoration on failure is guarded against clobbering fresh user input.
    Autosize re-derives from the restored draft on remount; no stored geometry.
  - **Capability-gated composer.** When the room lacks `MessageSend` (a
    departed room), the composer is suppressed as a typed capability outcome —
    not a disabled textarea — and the signed left/removed fact is stated
    plainly (D8, invariant 5 floor). Files/Pipes events render as inspectable
    inert references until #181.
  - **l10n.** All copy through the #177 typed catalog with
    compiler-enforced EN/FR parity; new timeline/composer catalog methods.

  The real-browser transport (`WsWeb`, #171) is not yet in place; the surface
  renders against the deterministic mock exactly as #176/#178 do. Host tests
  (pure `room/` modules) pass. The offline reconciler-read Playwright fixtures
  and the live re-qualification are deferred to the tests phase (#182).

- `crates/jeliya-client` gains the `DirectClient` Android adapter (#173),
  the production replacement for `crates/jeliya-ffi/src/host.rs`. Enabled by
  the default-off, native-only `direct` feature; `wasm32` and
  `--no-default-features` graphs are unchanged. Key properties:

  - **No JSON, Dart, C ABI, socket, token, or portfile in the data path.**
    The engine is called only through `Engine::execute_with(TypedCall, …)` and
    observed only through `subscribe_pushes() -> Push`. The one in-process
    struct↔struct transform (the shared `ClientHandle` edge forces it) reuses
    the daemon's own router (`route` → `TypedCall`), so adapter contract tests
    match WS view-level outcomes by construction, not by coincidence.
  - **Serialized dispatch** through a bounded mpsc request channel.
    `DirectEngineActor` runs one call at a time — the WAL-race guard inherited
    from the FFI host — so "calls execute serially" is structural, not a
    contract the adapter has to maintain separately.
  - **One owner per canonical data directory.** `OwnershipRegistry` mints an
    `OwnerToken` for the first caller and refuses all subsequent `start` calls
    with `OwnershipError` until the token is dropped.
  - **Push loop runs for the engine's whole life,** not only while subscribers
    are present, so the join-bootstrap `accept_joins` window stays open.
  - **Honest lifecycle.** DirectClient never fabricates a reconnect: it emits
    no `StateChanged(Connecting)` after the first open, never arms a backoff
    timer, and never calls `Reconciler::resume` on behalf of the caller. Resume
    is explicitly the caller's responsibility, via `Reconciler::resume()`, which
    re-baselines every active room with `ResyncReason::Resume`.
  - **Bounded teardown.** Stop drains accepted calls and awaits
    `close_all_rooms` (10 s) before dropping the engine. The teardown outcome
    (`TeardownOutcome::clean`) is propagated truthfully.
  - **Reuses the kernel and reconciler unchanged** (`KernelConfig::stable_principal = true`
    is the one DirectClient-specific setting: the in-process principal is
    session-stable, enabling op-id dedup within the session).

  New public API exported from `jeliya_client`: `DirectConfig`, `connect_direct`,
  `OwnershipError` (all behind the `direct` feature). `jeliya-codec` gains
  `pub use` re-exports of `route` and `Call` so the shared in-process bridge
  compiles in one place. CI gains a dedicated `jeliya-client DirectClient
  adapter` job (clippy + tests under `--features direct`). MSRV 1.91.

- `jeliya-client` gains the native protocol-v2 WebSocket adapter `WsNative`
  (#172), behind the default-off, native-only `ws-native` feature. It binds
  the transport-independent kernel (#168) to a real `tokio` +
  `tokio-tungstenite` transport that dials a loopback `jeliyad` via the
  reusable supervisor `TargetResolver` (#170). Landing order within the change:

  - **Codec client direction** (`jeliya-codec/src/client.rs`, new): the
    protocol-v2 encoder for outbound request envelopes (`encode_request`) and
    the decoder for inbound daemon Text frames (`decode_client_frame` →
    `ClientFrame::{Hello, Reply, Push, Malformed}`). JSON stays in the codec;
    `jeliya-client` never touches `serde_json::Value`.

  - **`DriverIo` seam** (`src/kernel/driver_io.rs`): a `pub(crate)` trait
    (`send`, `arm_timer`, `cancel_timer`, `dial`, `cancel_dial`) that factors
    the transport-touching arms out of `Shared::apply_one`, making the kernel's
    async shell — `Deferred`/`Runtime`/`drain_delivery`/ABBA-avoidance —
    reusable by all adapters. The existing in-memory driver becomes `InMemoryIo`
    implementing `DriverIo`; the entire `test-transport` and `kernel_fault`
    suite passes unchanged (the refactor's acceptance gate).

  - **Native adapter** (`src/adapter/`): `source.rs` (the injected
    `TargetSource` seam + fail-closed-vs-retry classification of every
    `SupervisorError`); `runtime.rs` (`connect_ws_native` constructor + async
    `RealDriver` loop); `ws_native.rs` (dial → hello agreement → serve →
    reconnect → stop); `clock.rs` (monotonic `Instant` → `Tick`, 1 ms =
    1 tick). Public construction surface re-exported from `jeliya_client`:
    `connect_ws_native`, `TargetSource`, `Dial`, `DialResolveError`,
    `NativeClientConfig`, `NativeError`.

  - **Security posture**: the bearer is a `Redacted<String>` exposed only when
    composing the `Authorization: Bearer` header — never a URL, log line, or
    value the `ClientHandle` surfaces, so it is unreachable from WebView JS.
    `Connected` is not reported until three independent checks agree: resolver
    health proof, daemon upgrade gate (`101`), and matching `hello` generation.
    Stale/malformed discovery fails closed. Only verified loopback endpoints
    (`127.0.0.1:<port>`) are dialed; a portfile advertising a non-loopback host
    is rejected before any connection attempt.

  - **Wasm-graph boundary guard** (`tests/boundaries.rs`): a new structural
    test resolves the `jeliya-ui` `web` feature tree for
    `wasm32-unknown-unknown` and asserts `tokio`, `tokio-tungstenite`, and
    `jeliya-supervisor` are absent — the jeliya-client-side twin of the
    supervisor's own wasm-graph assertion. Default library, wasm, and
    MSRV/clippy builds are unaffected.

  `stable_principal = false` by default (replay disabled) until #270 provides
  a daemon-incarnation fence. The real-daemon integration matrix (token
  rotation, stale portfile, abrupt daemon death, same-generation adoption,
  version mismatch, reconnect, stop) is `#[ignore]`-deferred and runs
  explicitly with a live `jeliyad`. Reconnect routes through #169's reconciler
  with no new resync logic in the adapter; it only guarantees the correct
  lifecycle+generation signal.

### Fixed

- `pipe.revoke` is now **idempotent at the room-fact layer**: a second (or Nth)
  revoke of an already-withdrawn pipe replays the **original** withdrawal —
  returning the first revoke's exact `event_id`, `pos`, and `revoked_at` — and
  authors **nothing** further. Previously the daemon published a fresh signed
  `pipe.closed` on every revoke (the op_id dedup ledger only covers a *same*-
  op_id replay, not a genuinely distinct second request), so an already-revoked
  pipe grew a second committed `pipe_revoked` event; under concurrent load the
  two authors could straddle a millisecond boundary and the second's differing
  instant broke the "returns the original withdrawal" guarantee — a correctness
  defect that surfaced as a flaky conformance case. `RoomSupervisor::pipe_close`
  now consults the canonically-earliest committed `pipe.closed` (after, not
  before, the unknown-pipe and publisher-only guards, so `pipe_unknown` /
  `pipe_not_publisher` are unchanged — including for a non-publisher re-revoking
  a closed pipe) and returns it without authoring; the withdrawal-event lookup
  selects the first canonical-order match rather than the last; and concurrent
  distinct-op_id revokes of the same pipe are serialized per `(room, pipe)` so a
  check-then-author cannot interleave into two withdrawals. No wire/schema,
  op_id-ledger, or `pipe.connect`/`pipe.list`/`pipe.release` change. Issue #271.

- The serve-crate test
  `websocket_file_share_progress_resets_stall_but_not_absolute_deadline` now
  drives its stall/deadline timers off a **paused** virtual clock
  (`tokio::time::pause()` + `advance`), matching its two siblings, instead of a
  real `tokio::time::sleep`. The real sleep drifted against the daemon's
  `Instant`-based timers under concurrent CI load and intermittently reordered
  the CREDIT/ABORT records; the conversion asserts the same observable sequence
  and post-conditions and passes deterministically under load. Issue #271.

- A room list backed by a daemon that supplies **no** `last_event_ts` can raise
  an unread dot again. Nothing seeded a baseline for such a room, and the
  unread predicate reads an unseeded room as not-unread, so no dot could ever
  appear. The first `room.event` observed for a room whose listed row carries
  no recency now establishes that room's baseline, and every later event flags
  normally. The first event is deliberately absorbed rather than claimed as
  unread: a push can carry late-validated backlog from a reconnecting peer, so
  arriving live is not by itself proof of new activity. Rooms from a current
  daemon are untouched — they always carry recency, so `room.list` keeps
  owning the baseline and the rule never fires. Issue #154.

  This rests on a daemon guarantee now pinned by a test: **every listed room
  carries recency.** A room with no stored events fails its own fold and is not
  listed at all, so a null `last_event_ts` on a listed row means one specific
  thing — the daemon predates the projection — and never "this room is empty".

- The Linux owned-process identity reader (`scripts/e2e-process-ownership.mjs`)
  now treats a process that exits between the `/proc/<pid>/stat` and
  `/proc/<pid>/cmdline` reads as **absent**, matching the `ENOENT`/`ESRCH`
  contract the reader already implements for a fully reaped process. The
  previous code threw `incomplete proc identity` when `cmdline` came back empty
  on a still-readable but vanishing `/proc` entry; that error carried no `.code`,
  so the absence guard did not catch it and the caller died — observed as an
  intermittent teardown failure 21 ms after a healthy check on the same pid. The
  fix re-reads `stat` after an empty `cmdline`: if the leader is now in a dead
  state (`Z`, or the kernel's `X`/`x` on the same exit path), return `null`; if
  the re-read itself throws `ENOENT`/`ESRCH`, fall through to the existing catch
  and return `null`; if the start-time changed (PID **recycled**), surface the
  new occupant's identity instead of absence, so
  `signalOwnedProcessGroup`'s recycled-leader guard refuses to signal — absence
  would have probed and signalled `-pid`, which may now name an unrelated
  process group. A live process that keeps the same identity yet exposes an
  empty `cmdline` still throws, as do `EACCES` and malformed `stat`. Issue #206.

### Added

- `room.list` rows now carry **`last_event_ts`** and **`last_event_kind`** —
  the `created_at` the newest locally-held signed event's author signed, and
  that event's kind. This is the daemon projection `docs/room-attention.md`
  (decision 2) specified and deferred: one bounded store read per row, no live
  session required, so a closed room answers exactly like an open one. Both are
  optional, read-only and nullable — a room with no readable event reports
  `null`, and `last_event_kind` is `null` for an event the kind enumeration
  does not name — so a client renders no recency rather than a fabricated one.

### Fixed

- Rooms created or joined from now on bind a **room-scoped device key**, so a
  daemon in several rooms no longer collapses them onto one `EndpointId`.
  iroh-rooms routes on `EndpointId == device_id`, so every room previously
  sharing the one global device meant only the last-opened room actually
  received traffic; the others sat open and silently deaf. The per-room key is
  derived with BLAKE3's KDF over the identity's device seed and the room id —
  **derived on demand, never persisted** — so it needs no migration, adds no
  second secret-bearing file, and is reproducible after a lost, rolled-back, or
  older-daemon-rewritten `state.json`. Which key a room uses is read back from
  that room's own signed log (the membership fold's device binding), never from
  local state, so the answer cannot drift from what peers will accept.
- Activity in a room you are **not** currently viewing is no longer thrown
  away. Both clients applied `room.event` pushes only when the push matched the
  open room, so with several rooms live the room list stayed silent about all
  but the current one until the next `room.list`. React and Flutter now record
  each push's signed timestamp and kind for **every** open room and fold it
  into the rows the room list renders, which lights the existing recency label
  and unread dot as activity happens. The values still come off the signed
  event — never a local clock — and live activity deliberately does not seed a
  room's own unread baseline, which would mark it seen the instant it became
  active. Completes the user-facing half of issue #151.

### Changed

- Rooms created **before** this change keep authoring with the global device,
  because their logs bind it and the owner's device binding is fixed by the
  genesis with no rebinding path. Two such rooms still cannot both be online;
  `room.open` now closes the colliding one explicitly and says so on stderr,
  rather than leaving it open and unable to receive. The closed room stays
  fully readable offline and re-opens on demand. Rooms created after this
  change are unaffected and stay live together.

## [0.6.1] - 2026-07-19

### Changed

- Repinned `iroh-rooms` from tag `v0.1.0-rc.3` (`71fbb500…`) to the
  deliberately untagged upstream merge `a5d98b70…`, the first `main` revision
  carrying the provisional-peer fanout fix, connection-generation teardown
  guards, and bounded store-insert recovery with durable critical degradation
  reporting. Exact-revision upstream, workspace, and loopback qualification
  passes. Signed direct and forced-relay evidence at the prior pin remains
  valid for released `v0.6.0` but does not transfer to the new candidate.
- Reserved a separate `docs/evidence/v0.6.1/` boundary for fresh qualification
  records. The `v0.6.0` manifests and signatures remain immutable, and the
  secret-storage gate now rejects private evidence-signing material even when
  it is gitignored inside the checkout.

## [0.6.0] - 2026-07-16

### Changed

- Repinned `iroh-rooms` from the `v0.5.0`-certified `d0ceb0b…` (rc.2-era) to
  the published `v0.1.0-rc.3` tag
  (`71fbb5007bef4ce83631c94762ec68c2beef3d79`). On top of the isolation
  remediation and relay seam the certified pin already carried, rc.3 brings
  the join-after-conversation deadlock fix (upstream PR #111 — at `d0ceb0b`,
  and therefore in released `v0.5.0`, an invite minted after any non-admin
  chat can never complete `room.join`), the join-bootstrap capability gate
  (PRs #117/#120), size-independent membership reconciliation (PR #118, no
  more ~30k-event ceiling), and deep pure-chat gap healing (PR #116).
  `room.join` now presents the invite's capability proof
  (`Node::spawn_join_bootstrap` with `BootstrapProof`); without it an rc.3
  admin never serves the join bootstrap. Mixed-fleet caution: an rc.3 joiner
  bootstrapping from a `v0.5.0`-era responder hard-stalls once it holds more
  than ~1k events, and a `v0.5.0`-era joiner cannot bootstrap from an rc.3
  admin (it sends no proof) — members of a room, especially its admin, must
  upgrade together. The certified `v0.5.0` network evidence binds `d0ceb0b`
  and does not transfer to this candidate; fresh certifying runs are required
  before the next release.

## [0.5.0] - 2026-07-12

### Added

- Evidence-backed preview hardening: centralized default-deny room-read authorization (including aggregate fleet/list filtering), a fail-closed gate for the required room-scoped upstream synchronization remediation, Android backup/device-transfer exclusions, protected mobile identity storage, agent state defaults outside the checkout, dependency-audit gates, MSRV coverage, complete Rust/TypeScript/Dart/Flutter/E2E CI, revision-bound real-network evidence tooling, checksum-verifying installers, and an OKF-compatible documentation status profile.
- Release promotion now runs the complete CI workflow twice on the exact current `main` revision, builds the five daemon+embedded-UI archives under read-only jobs, verifies all ten private archive/checksum files, and grants write permission only to the final tag/draft/byte-verification boundary. Native app, DMG, APK/AAB, and iOS artifacts are excluded from `v0.5.0`.
- Mobile release-readiness follow-ups: the bottom tab bar now GROWS with the OS accessibility font scale instead of clipping its labels (regression-pinned at textScale 2.0, en + fr); Android opts in to predictive back (`enableOnBackInvokedCallback`), with the shell keeping sole back authority — the nested Rooms navigator's `canHandlePop:false` notifications are absorbed so the OS never takes a back the shell policy owns (channel contract pinned by `predictive_back_test`; the OS gesture itself verified on classic back, predictive gesture pending a 14+ device); Android release builds sign from an optional gitignored `key.properties` (dev builds stay debug-signed; `flutter build appbundle` is the store path, `--split-per-abi` the sideload path — per-ABI release APKs measure 30-43 MB vs the 222 MB fat debug APK; documented in packaging/README.md); the invite ticket gains an OS share sheet below the breakpoint via `share_plus` (desktop stays copy-only; two new EN+FR catalog keys bring the catalog to 444; the pubspec's minimal-plugin policy note updated — this is the third deliberate plugin); and the timeline's touch behavior got a recorded verdict: Flutter's `SelectionArea` uses long-press-then-drag on touch and does NOT fight list scrolling (pinned by a gesture test; kept on all form factors).
- **Mobile bottom-tab shell (issue #17) — below 900dp the app lays out as a phone app.** ONE MediaQuery width breakpoint (`app/lib/src/layout.dart`) forked solely in `ShellScreen.build`: at ≥900dp the three-pane desktop shell renders exactly as before, below it the SAME state/handler/`_navView` machine drives a bottom-tab shell — a fixed 58dp five-tab bar (Rooms/Agents/Pipes/Files/Settings; glyph + small label, active = emerald text only, safe-area padded, reusing the `sidebarNav*` catalog keys), a phone Rooms home (rooms list with dot+label state, create/join affordances, identity footer + connection badge), chat as a PUSHED route under Rooms (RoomHeader + room-keyed timeline + composer, stick-to-bottom re-jump under the soft keyboard, new-messages pill, Android back pops it), a room-detail route hosting the RightPanel tabs (Members first; Share file / Open pipe / timeline pipe tiles deep-link to the matching tab), full-width pinned Pipes/Files surfaces with an honest select-a-room empty state, Agents mounted only while its tab is active (the 4s fleet poll never runs in the background; KPI strip hidden below the breakpoint, web parity), and width-aware Settings. Join-with-ticket, invite, and Add Agent present full screen below the breakpoint via a new `showJeliyaModalScreen` (identical awaited `Navigator.pop` contracts; the invite keeps observing the live RoomStore); create/leave/rename stay dialogs; a PopScope backs out pushed routes first, then to Rooms, then exits. Every control below the breakpoint meets the 44dp touch floor, timeline scroll survives tab switches, and the global connection banner overlays every mobile surface. Narrow hazards fixed along the way: the onboarding card's fixed 420 width is now a max, the onboarding room cards stack, and the fleet search flexes. ZERO new catalog keys — the tabs, members affordance, and empty states reuse existing keys (their `@description`s broadened). New coverage runs on a STRICT surface (360x800 AND 360x640, textScale 1.0, DPR 1.0, English AND French, recorded-overflow list asserted EMPTY), including breakpoint routing both ways on a live resize, the awaited pop contracts, fleet poll lifecycle, and mobile ports of the shared connection-banner and pending-send-lifecycle suites; the desktop suite passes untouched. Built and host-test-verified (analyze, full app suite, i18n gate, codegen drift-clean); the on-device pass (recorded on PR #19) PASSED on the moto g play 2023 — portrait, native density: rooms home, the pushed chat route with a message sent from the phone UI and echoed back through the engine push path, the composer riding the soft keyboard, room-scoped Files, truthful in-process engine facts in Settings, and live EN → FR → system switching with decision-7 typography and untruncated French tabs at 360dp.
- **i18n Phase D — the app speaks French.** `app_fr.arb` translates the full 442-key catalog (`untranslated_messages.json` empty; a French system locale now resolves to French, and the Settings picker or a live switch gets there without a restart). The translation is governed by `docs/glossary-fr.md`: Tier 1 vocabulary (salon, membre, réglages, rejoindre…), Tier 2 wire tokens verbatim (direct, relay, jeliyad, pipe…), the Tier 3 brand line as the onboarding tagline (« Jeliya — l’art du djéli, gardien de la mémoire vraie. »), and the newly settled decision 7 typography (U+202F before `; ! ?` and inside « », U+00A0 before `:`, U+2019 apostrophes, vouvoiement, octets o/Ko/Mo/Go, « 42 % »). Mechanically validated (placeholder/ICU parity, non-breaking-space and token conformance) and adversarially reviewed line-by-line (8 lenses; every accepted correction independently verified — terminology unified cross-surface: Pipes, « identifiant d’identité », the *diffuser* serving family, « Échec du contrôle de sécurité »; three count strings upgraded to fr-only ICU plurals so 0/1 agree grammatically). The macOS menu bar ships `fr.lproj/MainMenu.strings` (~65 entries; the APP_NAME token is substituted at runtime by FlutterAppDelegate) with `fr` in `knownRegions`/`CFBundleLocalizations` — the native menu follows the OS language while the in-app picker governs the Flutter surface. New `strings_fr_test` pins the French contract (CLDR-fr 0-is-singular plurals, tokens, tagline, octets, NBSP); `locale_test` now proves fr resolves and unsupported locales still fall back to en.
- i18n Phase C (language switching): the text locale and the formatting locale are SEPARATE persisted prefs (`PrefsStore.textLocale` / `.formattingLocale`; null follows the system — glossary decision 4), picked in a new Settings language card whose language names are endonyms from `tokens.dart` (never reaching translators, like the wordmark — a guard test fails if a supported locale lacks one). `MaterialApp.locale` binds to the text pref (unset keeps Flutter's system resolution) and a new `FormatsScope` above MaterialApp publishes the intl-verified formatting locale, so both switches apply LIVE — every consumer already resolves per build, pinned by a rendered-output test (timeline times flip 12-hour → 24-hour on the switch). OS-level locale changes re-resolve via a `WidgetsBindingObserver`; one startup `initializeDateFormatting()` loads every locale's bundled data. The catalog gained three settings keys (442 total; the generated parity test now pins 383+79). New `locale_switch_test` covers prefs persistence (junk drops to follow-system), live convention switching, system-follow, MaterialApp.locale binding, and the picker flow end-to-end; malformed persisted tags parse defensively and a stored tag this build's catalog lacks stays selectable, rendered raw.
- i18n enforcement moved into everyday CI: a new `ci.yml` runs the gate, gen-l10n/parity drift checks (untracked-aware — a generated file that was never committed, or was deleted, counts as drift), analyzers, and the app+package suites on every PR and every push to main, under a pinned Flutter (byte-exact codegen gates float with unpinned toolchains), and `release.yml`'s publish jobs now `needs:` that same verification — a failing gate blocks ALL release assets, not just the DMG. The gate gained rule 4: a string literal in `app/test/` that duplicates a catalog value fails the build (tests assert via the shared `en` instance; the generated parity test is the sanctioned exception) — the ~59 remaining copy-coupled test literals were migrated to `en.<key>` assertions. Unknown wire roles now pass through raw in roster pills too (new `WireDisplay.rolePill`, pinned by test), and the invite role dropdown resolves through the WireDisplay extension.
- i18n Phase B was adversarially reviewed (35 agents, 6 lenses); all 29 confirmed findings fixed: the invite modal's expiry-validation copy and the leave-room 'Untitled room' fallback now resolve at render time (nothing catalog-derived is cached in state); `JeliyaFormats.percent` formats digits under the formatting locale; six catalog descriptions were corrected (surfaces enumerated, a false mono-matching claim dropped) and `inviteReadyToSendCopy` now quotes the peer-unreachable error title verbatim; the path-placeholder example moved from tokens into the catalog (it is translatable); the i18n gate scans whole sources (formatter-wrapped literals can no longer hide), honors `i18n-exempt` for the locale-pinning rule, and its remediation text points at the ARB pipeline; `l10n.yaml` enforces translator metadata on every key and stamps generated files with a do-not-edit header; the parity test is now emitted by a committed generator (`scripts/gen-l10n-parity.mjs`) with EXACT rendered pins for all parameterized entries including both ICU plural branches (380+79 pins); new tests cover boot-failure narration per BootStage, fr-system-locale fallback, and the launch-command token assembly; the release workflow gained an i18n/app/package verification job (generated-code drift fails CI).
- i18n Phase B (gen-l10n/ARB migration): the entire UI catalog now lives in `app/lib/src/l10n/arb/app_en.arb` — 439 keys with translator-grade `@`descriptions (never-translate protocol tokens flagged per entry) — generating the `AppStrings` class every widget resolves AT RENDER TIME via `context.strings` (live language switching becomes a wiring exercise). Error copy and wire-enum display words are `AppStrings` extensions resolved per-build; decorative glyphs/URLs/shell commands stay in a non-exported `tokens.dart`; `{slot}` rich-text templates ride real ARB placeholders (translators can reorder them). The ten hand-rolled plurals are ICU plural messages. Date/number formatting moved to `JeliyaFormats` under an EXPLICIT formatting locale, separate from the text locale (glossary decision 4), backed by intl — one visible EN change: clock strings now use CLDR's narrow no-break space before AM/PM. The session stores structured `BootStage` facts instead of composed copy. A generated `l10n_parity_test` pins all 380 plain EN entries exactly and exercises all 59 parameterized entries (ICU validity); the i18n gate now also rejects locale-pinning `lookupAppStrings` calls in lib/. The old per-area `strings_*.dart` classes are gone (migrated via a temporary compile-safe bridge, then deleted).
- i18n Phase A was adversarially reviewed (39 agents, 5 lenses); all 30 confirmed findings fixed: templateText now applies the sentence style to the ROOT span so partially-styled slots (bold emphasis, the 'someone' invitee fallback) inherit it instead of falling back to the theme body style; share staging failures got their own client-synthesized codes (`file_too_large`, `file_unreadable` — documented in PROTOCOL.md) with friendly copy, instead of `invalid_params` with English composed in the package; a missing jeliyad binary now leads with the translatable guidance (not a raw SidecarError line); the pipes expose/connect flows get flow-specific error copy via ErrorNote overrides (the generic `pipe_denied`/`invalid_params` copy misled there); `'{label} (optional)'` field labels became a shared template; the last two formatter stragglers (right_panel clock, fleet relative time, ProgressBar semantics percent) moved into `format.dart`; the i18n gate is quote-agnostic and no longer truncates literals containing `//`. New test surface: 39 tests pinning friendlyError (all 17 codes + default never leak wire text), every production template × its call-site slot set, the formatters, the wire-enum display maps, join narration, and the error-surface fallbacks (57 app tests total; package 182).
- i18n Phase A (string hygiene, pre-ARB): every user-visible string now routes through the l10n layer. Six previously unmapped protocol error codes got friendly copy and all fallback paths (generic ErrorNote default, fetch-control default arm, boot failures) lead with translatable text — raw daemon/exception text is demoted to the Technical-details disclosure. Wire enums (roles, member statuses, peer paths, daemon modes, connection states) render via a new display map (Phase B's `l10n/wire_display.dart`) instead of verbatim; the invite Role dropdown no longer displays the wire value it submits. Join-progress narration moved out of `jeliya_protocol` (which now emits structured `phase`/`attempt`/`retryDelay` facts) into app strings. Fragment-built sentences (timeline syslines, Add-Agent guidance, join/leave copy, fetch detail lines) became single `{slot}` templates rendered by a new `template_text.dart` helper, so translations can reorder words. Display formatting (bytes ×2, percent ×3, prettyLabel ×3) unified into `app/lib/src/format.dart`. Duplicate copy deduped to canonical keys; EN casing/terminology drift normalized (sentence-case buttons, 'identity ID', typographic ellipsis). New `scripts/i18n-gate.mjs` enforces the no-literals rule; `docs/i18n.md` records the language decisions and rules; widget tests assert copy via the `*Strings` constants.

- **Mobile transport (Phase 4) — Android now runs the real protocol in-process** (the `FfiClient` code path is mobile-shared, but iOS has no platform scaffold or engine build yet — nothing runs there today). The mobile session swaps the interim in-memory `MockClient` for `FfiClient` over the in-process Rust engine, constructed with real networking enabled (`loopback: false`). Connection semantics are the engine lifecycle, reported truthfully: `connecting` → `connected` on start, `disconnected` after stop or once a call observes the engine itself gone — never `reconnecting`, because no transport exists that can drop independently of the app process. `daemon.status` stays honest (`port: 0` means *no listener*; `pid` is the app's own) and `daemon.shutdown` performs real engine teardown — all specified in a new *In-process transport (FFI)* section of PROTOCOL.md. Because the reconnect that triggers timeline re-sync can never fire in-process, the app re-runs the full re-sync on app-lifecycle resume (the honest equivalent of a transport gap under OS suspension), and file sharing keeps the staging convention via pure file I/O (no daemon HTTP origin exists). This transport is **host-conformance-verified and device-proven**: the golden corpus replays against the in-process engine in CI, and the on-device smoke passed 2026-07-10 on the moto g play 2023 (Android 13, armeabi-v7a) — identity create, room create, message send with the echo arriving through the push path, identity and room persistence across relaunch, force-kill (`am force-stop`, no clean teardown) recovery of the room and full timeline from `rooms.db`, and a hot restart where `jeliya_engine_start` adopted the live engine and a further message sent and echoed through it. Known gap: file local-open has no in-process equivalent of `GET /api/files/local` yet; an engine accessor is the tracked follow-up, as are the iOS staticlib wiring and Android foreground-service work.
- The production FFI bridge (Phase 4): `crates/jeliya-ffi` grew from the identity smoke to the production C-ABI over the core Engine — `jeliya_engine_start` (construct-or-rebind: a hot restart on the same data dir adopts the live engine), `jeliya_engine_request` (non-blocking; every reply envelope and push frame posts to one Dart `NativePort` as UTF-8 JSON — the same frames the WebSocket daemon speaks), `jeliya_engine_stop` (bounded teardown — each cleanly closed room releases its rooms.db handles and blob locks, and an unclean close is reported truthfully through the completion code instead of claimed clean), and buffer helpers — with `catch_unwind` at every export and zero new dependencies (`cc`/`tokio` were already locked; `dart_api_dl.c` ships with the pinned Flutter SDK). The Dart side is `FfiClient` behind a new `package:jeliya_protocol/ffi.dart` entry (SDK libraries only — the package stays pub-dependency-free), reusing `WsClient`'s exact frame-routing seam so the envelope rules cannot drift between transports. A **third conformance oracle** replays the same golden corpus against the in-process engine on every PR, next to the daemon and mock oracles. The bridge is hand-rolled over flutter_rust_bridge deliberately — the decision record lives in the crate's module doc (the surface is one string-typed seam; FRB's blessed build integrations were already rejected in this repo; the dependency-free corpus replay under plain `dart test` is the acceptance gate).
- Android scaffold (Phase 4): the Flutter app now builds, installs, and runs on a real Android phone. `app/android/` stands up the platform — applicationId `com.incubtek.jeliya` (matching the macOS bundle id), minSdk 26, ABIs armeabi-v7a + arm64-v8a + x86_64 (armeabi-v7a is REQUIRED: real target-market devices like the moto g play 2023 run 32-bit-only Android). New `scripts/build-android-libs.mjs` builds `libjeliya_ffi.so` for all three ABIs by driving the NDK r29 clang directly (cargo-ndk 4.1.2 panics against this repo's asdf-managed Rust; cargokit is archived), releasing stripped `.so`s into gitignored jniLibs. A `dart:ffi` binding (`app/lib/src/ffi/jeliya_ffi_smoke.dart`) proved the native library loaded and RAN inside the Flutter process, not just as a standalone binary (the smoke was retired once the `FfiClient` transport above superseded it). On mobile the session ran the full UI on an INTERIM in-memory `MockClient` (fixture data); mobile cannot spawn a sidecar subprocess (iOS forbids it; Android 13 SELinux blocks exec from writable dirs), so the production transport is an in-process `FfiClient` — landed above. Verified on device (moto g play 2023, Android 13, armeabi-v7a): the APK packages all three `.so`s, installs, launches, and logcat shows the FFI smoke's `created identity=…` line; desktop unaffected (flutter analyze clean, 82 app tests pass).
- New `crates/jeliya-ffi` (v0.1.0): a C-ABI shim over `jeliya-core` (cdylib + staticlib) exposing a panic-guarded identity smoke — `jeliya_ffi_identity_smoke` runs ed25519 keygen (OS CSPRNG + filesystem) in-process and never lets a panic unwind across the C ABI — plus the matching `jeliya_ffi_string_free`, and a `jeliya_smoke` bin for running the same proof on a device/emulator via `adb shell`. Android cross-compilation is proven for `x86_64-linux-android` and `aarch64-linux-android` with exported symbols verified — iroh-rooms and its tokio/quinn stack compile cleanly, and the crypto backend is ring (the known aws-lc-rs arm64 blocker does not apply). This was the spike, not the production FFI surface (which grew in place as a hand-rolled C-ABI — see above; flutter_rust_bridge was evaluated and not chosen); the crate opts out of the workspace's `unsafe_code = "forbid"` — the C ABI boundary genuinely needs `unsafe`, while `jeliya-core` itself stays unsafe-forbidden.
- The Jeliya desktop app (`app/`) reached web parity (Phase 3): the full three-column client — rooms, timeline with optimistic sends, files with fetch states, pipes, fleet dashboard, settings/diagnostics, invites — built on a now fully typed `dart/jeliya_protocol` (typed models and wrappers for all 26 methods, typed push streams, staged-upload HTTP surface, 1:1 ports of the client conventions, and an in-memory mock client for tests). 181 package tests + 18 widget tests; adversarially reviewed with all confirmed findings fixed.
- macOS packaging (Phase 5, development only): `scripts/package-macos.mjs` can produce a universal `Jeliya.app` with the jeliyad sidecar bundled at `Contents/Helpers/`, verify entitlements and the sandboxed spawn/teardown contract, and emit a DMG. The Homebrew cask remains an unpublished template; the current release workflow intentionally has no `macos-app` job and publishes no DMG until signing, notarization, and platform gates are satisfied.
- The shipped app implements PROTOCOL.md's version-skew rule end to end: a protocol-mismatched incumbent daemon is evicted (SIGTERM by portfile pid, protocol-agnostic) and the bundled binary respawned; a mid-session protocol change surfaces as a hard boot failure instead of a silent freeze.
- The release app runs SANDBOXED (`Release.entitlements`): App Sandbox + hardened runtime, with a home-relative exception for `~/Library/Application Support/Jeliya` so the app keeps sharing one identity and room store with a Homebrew-installed `jeliyad`. Data-dir resolution unwraps the sandbox container `$HOME`. Debug builds stay unsandboxed for repo-sidecar development (`JELIYA_DATA_DIR`/`JELIYAD_BIN` levers).
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

- The protocol seam moved out of the daemon binary into `jeliya-core`: the 24-method dispatch table, the request/response envelope, and the push fan-out loop moved verbatim into a transport-free `Engine` facade (`jeliya_core::engine`), and `jeliyad` was rewired onto it byte-identically — the TS conformance harness, the daemon-bound Dart corpus replay, and the full workspace test suites all passed unchanged, and the WebSocket daemon and the FFI bridge now exercise ONE dispatch/envelope/push implementation by construction. `PROTOCOL_VERSION` is a single core const the daemon re-exports, so the portfile, ready line, `/api/health`, and `daemon.status` can never drift apart.
- The desktop app's bundle identifier is now `com.incubtek.jeliya` (was the flutter-create placeholder `dev.jeliya.jeliyaApp`) — settled before the first signed builds, since signing and the sandbox container key on it.
- **Breaking:** `/ws` and `/api/files/*` now require a per-start auth token (`?token=` or `Authorization: Bearer`). The served web UI fetches it automatically from `/api/session`; scripts read it from the portfile (`scripts/daemon-token.mjs`). Older clients against a new daemon are refused with 401.
- **Breaking:** one daemon per data dir. A second launch on the same data dir no longer silently binds a neighboring port (the double-daemon `state.json` corruption scenario); it now health-checks the incumbent, prints `already_running`, and exits 0 so supervisors adopt it.
- `daemon.status` now also reports `protocol`, `pid`, `port`, and `data_dir` so a client can verify which daemon it is attached to.
- `/ws` and `/api/*` refuse requests whose `Host` header is not loopback (DNS-rebinding guard), and `/api/files/local` is no longer reachable without auth.
- Files fetched from room peers are now served as downloads (`Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`, inert content-type) instead of rendered inline, so a peer-supplied `text/html`/`svg` cannot run script in the daemon's origin.
- Daemon diagnostics moved from bare stderr prints to `tracing` (stderr + rolling file).
- Three `jeliya_protocol` test files located the repo root via the BUILT `target/debug/jeliyad`, so on an unbuilt checkout (CI, fresh clone) they failed to LOAD before their own `jeliyad not built` skip guard could run — ci.yml's first live run caught it. The repo-root marker is now the checked-in `docs/PROTOCOL.md`; daemon-bound suites skip cleanly without the binary and run in full with it.
- The 320px member panel now survives French-width copy: the self-owner roster row collapsed to one glyph per line (the inline « Le propriétaire reste » note — ~2× wider than 'Owner stays' — starved the name column; it now sits under the role/status pills, width-capped and scale-aware) and the Members tab ellipsized to « Me… » in its rigid quarter-width slot (tabs are now content-sized and justified, with tighter padding so all four French labels + badges fit, and a horizontal-scroll safety valve for pathological badge counts instead of clipping). Member IDs ellipsize on one line instead of wrapping. New `panel_fr_layout_test` pins the roster orientation, both wide tab labels at intrinsic width, and a zero-scroll-extent strip fit.
- Fixed the file-fetch UI keying friendly copy on a phantom `provider_refused` code the daemon never emits; the authorization-wall case now correctly handles `file_unauthorized`, and `hash_mismatch` gets an explicit hard-stop message. Aligned the TypeScript wire types (`protocol.ts`) and the mock reference client with the daemon: `daemon.status` gains `protocol`/`pid`/`port`/`data_dir`, `daemon.shutdown` is typed, `room.open` documents its `peers` hints, `invite.create` expiry accepts a string or seconds, and pipe/room fields that can be null are typed nullable (surfacing several latent null-handling fixes in the UI).

### Fixed

- Closed cross-room read exposure with a centralized accepted-room guard covering every public room-scoped read, accepted-room filtering for aggregates such as `room.list` and `agents.fleet`, and negative authorization coverage for timelines, members, agents, files, local files, and pipes.
- Made local room provenance recoverable and durable: create and join now persist accepted-room state before irreversible event publication, retries remain safe after persistence failures, concurrent state mutations are serialized, and `state.json` is atomically replaced with file/directory synchronization, Windows write-through replacement, and owner-only Unix permissions.
- Reused the authorized room snapshot cache for direct file, pipe, and agent-history reads, avoiding repeated full-history folds without weakening the authorization preflight.
- Aligned the `room.list` pre-identity contract across the daemon, TypeScript mock, Dart daemon/FFI/mock clients, the golden corpus, and protocol documentation while preserving protocol v1's privacy-safe `{ rooms: [] }` onboarding result.
- Isolated certifying source builds from ambient operator state: schema-2 evidence now requires a bare commit archive unaffected by checkout-local Git attributes, run-owned home/Cargo/npm/Git/temp state, a minimal proxy/CA allowlist, removal of unlisted variables, exact Node/npm/Cargo/cargo-zigbuild bindings, and the pinned official Zig 0.15.2 installation archive verified before extraction or execution; remote binary copies use a size-aware bounded deadline so slow qualifying links fail only at an explicit conservative floor.

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
