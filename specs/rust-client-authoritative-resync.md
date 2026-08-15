# Spec — Authoritative room/session resynchronization (#169)

- **Issue:** kortiene/jeliya#169 — `[Rust][Client]: Make reconnect, lag, overflow, and mobile-resume gaps authoritatively resynchronizable`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters).
- **Records/derives its decision from:** `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary" (the **"One resync path (#169): `ResyncRequired { generation, reason }` is the only gap and resync path for v2 clients. There is no legacy bootstrap fallback."** paragraph) and `docs/protocol-v2.md` §"Pushes, ordering, gap detection, and resync", §"`stream.resync`", §"Presence", and §"Idempotency and retry".
- **Depends on (all landed):** #167 (`crates/jeliya-client` seam: `ClientHandle`, `ClientEvent`, `EventSubscription`, `State`, `CallError`), #168 (`crates/jeliya-client/src/kernel/` bounded kernel: connection generation fencing, bounded fan-out with `Lagged`, honest disconnect classification), and the **typed pushes of #166** (`jeliya_api::Push`, `Event`, `GapReason`, `GapTo`, `ApiError::ResyncRequired`).
- **Blocks / is consumed by:** #171 `WsWeb`, #172 `WsNative`, #173 `DirectClient` (each drives this reconciler on its own lifecycle), #175 (the one fault-injected four-adapter parity suite), and the room UI (#178+) which renders the reconciler's converged per-room view.
- **Owner role:** core maintainer (the reconciler is transport-independent and shared by all four adapters, exactly as the kernel is).
- **Status of this document:** proposed. No production code is written by the planning phase. This is the authoritative design record; where it and `docs/protocol-v2.md` or `docs/dioxus-architecture.md` disagree, **those records are authoritative and this spec has a bug** — the PR must say which, exactly as the architecture record requires of every slice that tests against it.

---

## 1. Outcome

Centralize **all** room/session reconciliation into one bounded, generation-fenced coordinator so that every detectable push gap, reconnect, local fan-out overflow, and Android process-resume produces the **same** authoritative re-baseline, and nothing else does. Concretely:

1. **Every detectable gap is named and observable.** A `ResyncReason` accompanies every re-baseline, so "why did we resync" is a first-class, tested value — never inferred from timing.
2. **One serialized, coalesced reconciliation per room.** At most one reconciliation is in flight for a room at a time; triggers that arrive while one runs coalesce into a single pending re-run rather than stacking.
3. **Bounded bootstrap/reconcile buffering.** Live pushes that arrive while a baseline read is outstanding are buffered in a **byte- and count-bounded** ring; overflow forces a fresh baseline instead of marking the dropped events consumed.
4. **Convergence by signed evidence.** A baseline read and the buffered pushes converge into a gap-free, deduplicated, ordered timeline keyed on the dense position `pos`, with `event_id` as the dedup identity and the signed `at` as the evidence that authorizes insertion.
5. **Peer state replaced, never merged.** Presence/membership is replaced wholesale from authoritative reads (`room.members` / `room.peers`); a stale `peer` push can never resurrect a peer an authoritative read removed.
6. **Generation fencing at the coordinator.** A completing baseline whose generation is older than the room's current generation is discarded — a stale baseline can never overwrite newer state.
7. **DirectClient resume is the same outcome.** Android resume triggers the identical reconciliation through an explicit `Resume` input and **never fabricates a socket reconnect** (no synthetic `Interrupted → Ready`).

This is the **only** gap/resync path for v2 clients; there is no legacy `room.activate`-again bootstrap fallback (architecture Decision 4).

## 2. What this issue is, and what it is not

The client stack has three adjacent slices plus the adapters:

- **#167 — the seam.** Public contract: `ClientHandle::{call, subscribe, state, start, stop}`, the `ClientEvent` model (`StateChanged`, `Push`, `Gap`, `ResyncRequired { room_id, from_pos }`, `Lagged`), `EventSubscription`, `State`, `CallError`/`Execution`.
- **#168 — the kernel.** Bounded request/reply machinery *below* the seam: connection generation stamping and stale-generation fencing of replies/pushes, the bounded multi-consumer fan-out that surfaces `Lagged` on local overflow, honest disconnect classification, and total stop. The kernel **fences generations and surfaces `Gap`/`ResyncRequired`/`Lagged`; it does not reconcile a room** (its §11 non-goal names #169 as the owner of the single resync path).
- **#169 (this issue) — the reconciler *above* the seam.** Consumes the seam's `EventSubscription` and issues reads/resyncs through `ClientHandle::call`. It owns the per-room baseline, the bounded reconcile buffer, convergence, peer replacement, event-ID dedup, and the serialized/coalesced resync engine. It is transport-independent: the four adapters differ only in *which* lifecycle inputs occur, not in how reconciliation runs.

**Consequence for scope.** #169 adds a new module tree (`src/reconcile/`) and a small public surface (`Reconciler` handle, `ResyncReason`, `RoomView`, `ReconcileLimits`/`ReconcileConfig`). It consumes the existing seam **without changing it** — the same "sufficient without a breaking change" discipline #168 proved for `ClientBackend`. The one place this spec touches shared vocabulary is the *reconciler-facing* generation: the architecture's canonical type is `ResyncRequired { generation, reason }`, and §R4 explains why a **reconciler-local monotonic epoch** satisfies that `generation` role with **no seam change** (the alternative — surfacing the kernel's exact connection generation on a lifecycle event — is Open Question 1).

## 3. Owning crate and layout

The reconciler lives **inside** `crates/jeliya-client`, next to the kernel, for the same reasons the kernel does: it reuses the seam's private and public infrastructure (`ClientHandle`, `ClientEvent`, `EventSubscription`, `RoomId`, `Event`) directly, and a separate crate would force those into a shared-public boundary. It sits *above* the seam (it is a consumer of `ClientHandle`), whereas the kernel sits *below* it — so it does not touch `kernel/` at all.

```
crates/jeliya-client/
  src/
    lib.rs              # add `mod reconcile;` + re-export the new public surface
    reconcile/
      mod.rs            # Reconciler (the async driver) + ReconcileConfig/ReconcileLimits + public re-exports
      core.rs           # the SANS-IO reconciliation state machine: step(Input) -> Vec<Action>; no I/O, no clock
      room.rs           # per-room state: baseline watermark, convergence, bounded dedup window, peer set
      buffer.rs         # the bounded, byte-aware reconcile buffer (live pushes held during a baseline read)
      reason.rs         # ResyncReason (every observable gap cause) + the ResyncRequired { generation, reason } view
      view.rs           # RoomView: the converged, gap-free timeline + replaced peer/membership snapshot
      driver.rs         # binds core Actions to ClientHandle::call and the EventSubscription; owns single-flight I/O
      diag.rs           # redaction: room ids ok; tokens/op_id/payload bytes never enter diagnostics
  tests/
    reconcile.rs        # property/fault suite over the sans-IO core (deterministic; one test per Verification bullet)
    reconcile_driver.rs # integration over the mock backend: bootstrap, reconnect, overflow, resume, cancellation
```

**Boundary invariants (extend `tests/boundaries.rs`):**

- No new runtime dependency. The reconciler uses only what the crate already carries (`jeliya-api`, `serde_json` at the erased boundary, `futures` primitives). No `tokio`, no clock, no RNG.
- **No wall clock and no RNG in the reconciler core.** The core is a pure `step(Input) -> Vec<Action>` function; ordering and dedup are driven by `pos`/`event_id`/`at` carried on inputs, never by a read of `std::time`. Extend `boundaries.rs` with a source scan asserting `std::time`, `Instant::now`, `SystemTime`, `getrandom`, `rand`, and `tokio` appear in **no** `src/reconcile/**` file. (The signed `at` is *data on an event*, not a clock read.)
- **No spawn in the reconciler.** The driver is polled by the adapter's event loop (or the mock in tests); it never spawns.
- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` continue to hold; every new public item is documented to `jeliya-api` density.

## 4. Architecture — a sans-IO reconciler with a driver seam

The reconciler mirrors the kernel's proven shape: a **pure, synchronous core** (all convergence, fencing, dedup, and single-flight logic) plus a thin async **driver** that performs the two I/O effects the core requests — issuing a `ClientHandle::call` and consuming an `EventSubscription`. Because the core is a function of its inputs, every fault the Verification section names (push during bootstrap, reconnect during open, repeated gaps, overflow, cancellation, resume, stale generations) is an ordinary deterministic sequence of `step` calls.

```
UI ──subscribe()──▶ RoomView stream (converged, per room)
                          ▲
                 ┌────────┴─────────┐
                 │   Reconciler      │  the async driver (driver.rs)
                 │  (single-flight)  │  owns: one EventSubscription in, N ClientHandle::call out
                 └────────┬─────────┘
                          │ step(Input) -> Vec<Action>
                 ┌────────┴─────────┐
                 │  reconcile::core  │  SANS-IO: trigger classification, single-flight + coalesce,
                 │   (pure, sync)    │  convergence, peer replacement, dedup, generation fencing
                 └────────┬─────────┘
                          │  Action
                          ▼
        IssueBaselineRead / IssueResync / BufferPush / ApplyBaseline /
        EmitView / EmitResyncRequired / DropStale / Cancel
                          │
                 ClientHandle::call (stream.resync, room.timeline, room.members, room.peers)
```

- **`step`** consumes one `Input` and returns a `Vec<Action>`; it never blocks and never awaits.
- **`Input`** — `Lifecycle(State, epoch_hint)`, `Event(ClientEvent)` (a `Push`/`Gap`/`ResyncRequired`/`Lagged` lifted off the subscription), `ReadReply { room, read_id, epoch, result: Result<BaselinePage, CallError> }`, `ActivateRoom(RoomId, from_pos)` (the caller opened a room; `from_pos` from `stream.subscribe`), `DeactivateRoom(RoomId)`, `Resume`, `Cancel(RoomId)`, `Stop`.
- **`Action`** — `IssueBaselineRead { room, read_id, epoch, request }` (a `room.timeline`/`room.members`/`room.peers` at bootstrap), `IssueResync { room, read_id, epoch, from_pos }`, `EmitResyncRequired { room, generation, reason }`, `EmitView(RoomView)`, `DropStale { room, read_id }`, `CancelRead { room, read_id }`.
- The driver applies `IssueBaselineRead`/`IssueResync` by calling `ClientHandle::call` and feeding the settled result back as `Input::ReadReply` tagged with the `read_id` and the `epoch` it was issued under. It applies `Emit*` by broadcasting on a reconciler-owned fan-out (a second, independent `EventBus` reused from `event.rs`, or a thin per-room channel) that the UI subscribes to for `RoomView` updates.

**All reads go through `ClientHandle::call`, never around it**, so they inherit the kernel's bounds, deadlines, cancellation, and generation fencing for free — a resync that is issued while the connection is dropping is settled `Disconnected { Unknown }` by the kernel and reaches the core as `Input::ReadReply(Err(Disconnected))`, which the core treats as "reconciliation failed, relaunch under the next epoch." The reconciler adds a **second** fence (the epoch, §R4) for the narrow window the kernel's per-connection fence cannot see (a queued re-run spanning a flap).

## 5. Design decisions

### R1 — Sans-IO core is the whole point (Verification: determinism)

Because the core is pure and time/ordering are inputs, "push during bootstrap", "reconnect during open", "repeated gaps", "overflow", "cancellation", "resume", and "stale generation" are all **sequences of `step` calls**, identical on wasm and native, with no timing dependence. The async surface is a thin shell; all correctness lives in `core.rs`/`room.rs`/`buffer.rs`.

### R2 — One reconciliation model unifies bootstrap and resync (AC: baseline + buffered pushes converge)

A room is always in exactly one phase:

| Phase | Meaning |
|---|---|
| `Converged { watermark }` | The room holds an authoritative, gap-free timeline up to position `watermark`; live pushes with `pos == watermark + 1` extend it in place. |
| `Reconciling { epoch, mode, read_id, buffer }` | A baseline read is outstanding under connection epoch `epoch`; live pushes for this room are held in the bounded `buffer`. `rerun: Option<ResyncReason>` records a coalesced re-trigger. |

`mode` is:

- **`Bootstrap`** — first activation. The room separately records the concrete inclusive subscription anchor (`stream.subscribe.from_pos`, including zero). The baseline pages `room.timeline` to `Complete`, treating the anchor as an inclusive **completeness floor, not a stop cursor**: a history that reaches `Complete` below the anchor is structurally invalid, while validated events beyond the anchor are retained and committed — an event committed after the anchor but before the activation was admitted may already have been pushed (and dropped, the room being untracked), and the baseline read is its only recovery. Termination is bounded by the total-scan ceiling and the strict cursor checks, never by daemon trust. The baseline also reads `room.members` and `room.peers`. A later anchor change forces a full replacement immediately when Ready; observable cause coalescing does not erase replacement mode.
- **`Incremental { from_pos }`** — recovery. The baseline is `stream.resync { from_pos = watermark }` (paged by `next_pos`), plus `room.peers`/`room.members` when the trigger implicated presence (reconnect, resume, overflow, daemon cursor) or when a truncation lowered the watermark. A `resync_required` redirect is honoured only while the chain makes strict progress within a bounded length: a redirect whose clamped effective cursor equals the failed read's own start gets exactly one retry, and even a strictly-descending redirect chain parks after a fixed bound rather than issuing reads forever.

Both modes share the same convergence, buffering, fencing, and coalescing; only the baseline source differs. This is why there is exactly one code path and one set of tests for "gap", "reconnect", "overflow", and "resume".

### R3 — Every gap reason is observable (AC-1)

`ResyncReason` is a closed enum; the core emits `Action::EmitResyncRequired { generation, reason }` at the *start* of every reconciliation, so the cause is observable before the outcome:

```rust
/// Why an authoritative re-baseline is being taken. Every detectable gap maps
/// to exactly one arm; the reconciler never re-baselines without naming a cause.
#[non_exhaustive]
pub enum ResyncReason {
    /// First activation of a room — the baseline is built from nothing.
    Bootstrap,
    /// A `stream.subscribe` / reconnect established (or re-established) liveness.
    /// Every entry into `State::Ready`, and every coalesced-through-`Interrupted`
    /// transition, re-baselines the affected active rooms.
    Reconnect,
    /// A position discontinuity was observed directly, or a `gap` frame arrived.
    /// Carries the wire cause so `backpressure` / `retention` / `subscription_lapse`
    /// stay distinguishable.
    Gap { reason: jeliya_api::GapReason, to: jeliya_api::GapTo },
    /// The reconciler's own subscription lagged the bounded fan-out (`Lagged`),
    /// or its per-room reconcile buffer overflowed: local loss, not a wire gap.
    LocalOverflow { dropped: u64 },
    /// A `resync_required` reply named a position that can no longer be served;
    /// discard back to `from_pos` and re-read from there.
    ResyncRequiredByDaemon { from_pos: u64 },
    /// Android process resume (or any adapter resume) — `DirectClient`'s path,
    /// with NO fabricated socket reconnect (§R11).
    Resume,
}
```

A test asserts each arm is reachable and surfaces on its own trigger, so "every gap reason is observable" is a covered contract, not a claim.

### R4 — Generation fencing at the coordinator; the reconciler-local epoch (AC: peer state replaced; Security: stale baselines cannot overwrite newer state)

The reconciler stamps every reconciliation with a **monotonic `epoch: u64`** and fences completions by it:

- The epoch **increments on every liveness (re)establishment**: each `Input::Lifecycle` into `State::Ready`, and each `StateChanged { coalesced_through_problem: true }` (a flap the fan-out merged but did not hide — §R12), and each `Input::Resume`.
- A reconciliation issued under `epoch = E` whose `Input::ReadReply` arrives when the room's current epoch is `> E` is **discarded** (`Action::DropStale`) — it can never apply a stale baseline over newer state. It is *not* an error; the newer epoch already relaunched (or will), so the stale reply is simply dropped.

**Why a reconciler-local epoch, not the kernel's connection generation.** The architecture's canonical type is `ResyncRequired { generation, reason }`. The `generation` there is a **fence value**: monotonic, advancing on every liveness change, used only to reject a stale baseline. A reconciler-local epoch satisfies that role exactly, is **honest** (it counts "the Nth live connection this reconciler has observed", never fabricating one), is **bounded**, and requires **no seam change** — preserving #168's proof that the seam is sufficient. It does not need to equal the kernel's `generation` numerically because the kernel already fences replies/pushes to its own current generation *before* they reach the reconciler; the epoch is the belt-and-suspenders for the one case the kernel's per-connection fence cannot see (a coalesced re-run that spans a flap). Whether to instead surface the kernel's exact generation on a lifecycle event — making the two numbers identical and letting the reconciler skip its own counter — is **Open Question 1**; this spec recommends the local epoch and treats the exact-generation surfacing as an additive refinement.

### R5 — Bounded reconcile/bootstrap buffering; overflow forces a re-baseline (AC-3, Security: bound every buffer)

While a room is `Reconciling`, live `event` pushes for it are held in a **byte- and count-bounded** ring (`buffer.rs`), sized by `ReconcileLimits::{buffer_depth, buffer_bytes}`. The buffer holds pushes so they can converge with the baseline (§R6) instead of being applied against a timeline that does not yet exist.

**On overflow the reconciler does the one correct thing: it forces another baseline.** It records `LocalOverflow`, sets the coalesced `rerun` reason, and — crucially — **does not advance the dedup watermark past the dropped pushes** and **does not mark them consumed**. The dropped events will be re-read authoritatively from the daemon by the relaunched baseline. This is the literal implementation direction ("trigger another baseline after any buffer overflow rather than marking dropped events consumed") and the mechanism behind AC-3 (§R10).

### R6 — Convergence by signed evidence: `pos` orders, `event_id` dedups, `at` authorizes (AC: converge by event ID and signed timestamp)

When a baseline read settles, the core converges it with the buffered pushes into the room's timeline:

1. **Apply the baseline first.** The baseline read's `events` (from `stream.resync` or `room.timeline`) are authoritative; they extend the timeline by ascending `pos` and advance the `watermark` to the read's `next_pos` (resync) or the last applied `pos` (timeline). Positions are dense and gap-free, so a hole in the applied range is itself a protocol violation → force a fresh resync.
2. **Drain the buffer, converging by evidence.** Each buffered push is:
   - **discarded** if its `pos <= watermark` (already in the baseline) — the primary, O(1) dedup;
   - **discarded/rejected** if its `event_id` already appears in the bounded durable identity map (§R7) — an exact replay at the same applied position is harmless, while reuse at a new position is corruption;
   - **applied** if its `pos == watermark + 1`, advancing the watermark;
   - **re-triggers a resync** if its `pos > watermark + 1` (a gap opened *during* reconciliation) — coalesced into the pending re-run rather than applied out of order.
3. **The signed `at` is the insertion evidence.** The reconciler inserts only events that carry a signed `at`, a signed `event_id`, and a resolved-or-explicitly-unresolved `author` — it never fabricates a position or a timestamp, and when a buffered push and a baseline event collide it trusts the baseline (the daemon's authoritative answer). "Insert events using signed evidence" is this rule: `event_id` is the identity, `pos` is the order, `at`/`author` are the non-repudiable evidence the record carries.

The result is a **gap-free, deduplicated, position-ordered** timeline — made structural by the dense-rank invariant plus exact bounded history identity evidence.

### R7 — Event-ID deduplication, bounded (AC-3, Security: bound every buffer)

Dedup is two-tier and both tiers are bounded:

- **The `watermark`** is the primary dedup: any event with `pos <= watermark` is already held, dropped in O(1) with no per-event storage.
- **Bounded exact identity evidence** is durable for the supported history of each room: a position-aware `event_id → pos` map catches reuse even after render-tail eviction, while a recent ring remains a small hot window. Daemon truncation retains entries at or below `from_pos` and drops the repudiated suffix, so the same suffix ids may be authoritatively re-read. A separate exact scan map validates every multi-page response (including pages older than the rendered tail) and commits every validated position of the scan — the activation anchor is a completeness floor, not a commit ceiling. Both maps are capped by `max_baseline_events` and `baseline_dedup_bytes`, and ids by `max_identifier_bytes`; reaching the explicit supported-history ceiling fails closed through the bounded structural-retry/park path, with no probabilistic collision.

**Overflow can never permanently dedup an undelivered event (AC-3):** the watermark and recent-id ring record only events the reconciler actually validated and applied. A dropped/overflowed push is neither applied nor recorded as seen, so the forced re-baseline (§R5) re-reads it and it converges normally. If any rerun is pending, the just-applied prefix is useful durable progress but is a publication barrier: no partial or repudiated intermediate view is emitted.

**Publication gates beyond the rerun barrier.** Convergence withholds a view in three further cases, each fail-closed:

1. **Required frontier.** A trigger that names a committed position above the watermark — a gap's `from_pos`, a bounded gap end, or a daemon `resync_required` cursor — is evidence the clamped read start cannot encode. No view publishes until authority serves through that frontier: one bounded follow-up read chases it, and a persistently short daemon parks the room rather than publishing a converged view below a position the daemon itself proved.
2. **Window rebuild.** A discard below the oldest retained render event whose recovery suffix leaves the rendered window empty forces a full timeline replacement instead of publishing an empty "authoritative" view for a room that still has history. A replacement's window is policy-conformant by construction, so the rebuild cannot loop.
3. **Disputed positions.** Contradictory evidence at or below the watermark (an unknown identity, a position conflict, or two claimants for one position) discards back to before the disputed position so authority itself re-proves it; the first arbitrary claimant never survives into a published view, and recovery never starts merely after the dispute.

Separately, any **lowering truncation forces an authoritative `room.members` replacement** before the next published view: the discarded suffix may have carried membership events already folded into the derived roster, and rolling back the timeline cannot reverse that fold.

### R8 — Peer state is replaced from authoritative reads, never merged (AC: peer state replaced)

Presence and membership are **replaced wholesale** from authoritative reads:

- `room.members` yields the signed roster (`subject_id`, `role`, `standing`, `joined_at`); it **replaces** the room's membership set. A `member.remove`/`room.leave` event the client missed is reflected by the removed member's absence from the fresh roster — the reconciler never keeps a member the authoritative roster omits.
- `room.peers` yields the per-device `Link` snapshot; it **replaces** the presence/link set.
- Live `peer` pushes between reads update the *replaced* set, and are fenced twice: the kernel drops old-transport frames (§K7), while the presence fold keeps bounded per-`(subject,device)` generations plus bounded tombstones for snapshot removals. There is no invalid room-global generation floor. `generation` fences a peer connection, not updates: later changes at the same generation win for a present key; equality against a removed-key tombstone is ambiguous and forces an authoritative refresh. A peer push dropped by the bounded transient buffer, or while the room is parked, leaves the same generation fence — a stale same-generation replay after an omitting snapshot must never masquerade as an unknown fresh peer. Tombstone overflow makes all unknown keys refresh fail-closed, and when the bounded tombstone map saturates, which removed keys stay fenced is a deterministic (sorted) function of the inputs, never hash-map iteration order. On a new kernel transport epoch the maps reset before cause coalescing (K7 already fenced the old transport); Resume retains them. The driver re-polls events after a read becomes ready so a push broadcast before its reply crosses the core first — up to a bounded fairness budget: continuously nonempty event traffic cannot starve a settled read forever, and the core is interleaving-robust when the budget forces bounded reordering. A stale-generation teardown therefore cannot resurrect a peer omitted by authority.

Replacement (not merge) is what makes a missed removal converge: a merge would keep phantom members forever.

### R9 — Serialized and coalesced resync (AC-2)

Reconciliation is **single-flight per room**:

- While a room is `Reconciling`, any new trigger (another `gap`, a reconnect, an overflow, a `resync_required`) does **not** launch a second reconciliation. It sets `rerun: Some(reason)`, coalescing repeated triggers into **one** pending re-run and keeping the *most authoritative* reason (a `ResyncRequiredByDaemon` or `Reconnect` outranks a `Gap`, which outranks a `LocalOverflow`, because the stronger cause implies the weaker's recovery). Cause priority selects the recovery but never erases quantitative loss: whenever a `LocalOverflow` count is outranked in a coalesce — in the pending re-run, at a failed settle, at a structural retry, or at a superseding launch — the count is banked and surfaced as a room-attributed `Lagged` boundary immediately before the covering converged view.
- When the outstanding baseline settles and `rerun` is set, the core suppresses that intermediate view, relaunches exactly once under the current epoch, then clears `rerun`. If a stronger coalesced cause covers quantitative local loss, its one authoritative read remains sufficient: the deferred count is emitted as a room-attributed `RoomUpdate::Lagged` boundary immediately before the successful `Converged`, not as a redundant second read. Repeated flapping therefore yields one in flight + one queued, never an unbounded stack or stale publication.
- Across rooms, reconciliations are independent (positions are per-room; the protocol defines no cross-room order), so N active rooms may each have one in-flight + one queued reconciliation — bounded by `max_active_rooms` (§R15).

A test drives repeated back-to-back gaps during an outstanding resync and asserts exactly one reconciliation is in flight at a time and exactly one coalesced re-run follows.

### R10 — Overflow cannot permanently deduplicate an undelivered event (AC-3)

This is the composition of §R5 and §R7, stated as its own invariant because it is its own acceptance criterion:

> The watermark and exact identity state advance **only** for validated, applied events. A buffer overflow (or fan-out `Lagged`) records **loss**, forces authoritative recovery, and leaves the dropped range unseen. No incomplete intermediate view is published; if a stronger cause performs the same recovery, the deferred count is an attributed `Lagged` boundary before its final view rather than another read.

A fault test fills the reconcile buffer to overflow during a bootstrap, then asserts every event authored in the room is present in the converged `RoomView` exactly once — none was silently consumed.

### R11 — DirectClient resume is the same outcome, with no fabricated reconnect (AC: DirectClient resume)

`DirectClient` (#173) runs `jeliya-core` in-process; its transport is a function call, so there is **no socket to reconnect**. On Android resume (surfaced by `PlatformServices` lifecycle, #174), the adapter feeds the reconciler `Input::Resume`, which:

- bumps the epoch (§R4) and launches an `Incremental` reconciliation of every active room with `reason = Resume`, and
- **emits no synthetic `StateChanged`** — the reconciler never fabricates an `Interrupted → Ready` for a transport that did not drop.

The outcome (a bounded authoritative re-baseline) is byte-identical to the reconnect path; only the *reason* and the *absence of a lifecycle transition* differ. A test asserts `Input::Resume` produces a reconciliation with `reason = Resume` and that no `ClientEvent::StateChanged` is synthesized by the reconciler. The #175 parity suite asserts the resulting `RoomView` matches the socket adapters' post-reconnect view.

### R12 — Reconnect and coalesced-flap detection (Verification: reconnect during open, repeated gaps)

The reconciler consumes lifecycle from its one `EventSubscription`:

- **Entry into `State::Ready`** (from `Connecting`/`Interrupted`) is a reconnect: bump epoch, relaunch every active room `Incremental` with `reason = Reconnect`.
- **A coalesced flap** — `StateChanged { to: Ready, coalesced_through_problem: true }`, the fan-out's honest signal that a `Ready → Interrupted → Ready` window was merged (§event.rs) — is treated **identically** to an explicit reconnect. This is why the seam preserves `coalesced_through_problem` instead of rewriting endpoints: the reconciler must reconcile even when the fan-out merged the flap.
- **A `Lagged` marker on the reconciler's own subscription** means it missed live pushes: relaunch the affected rooms (or all active rooms when `room_id` is `None`) with `reason = LocalOverflow`.

### R13 — Cancellation and stop are bounded and honest (Verification: cancellation)

- **`Input::DeactivateRoom` / `Input::Cancel(room)`** drops the room's outstanding read (the driver drops the `call` future, which the kernel handles as a local cancel — no fabricated remote cancel, §K9), clears its buffer, and forgets its state. Bounded: one room's collections are released.
- **`Input::Stop`** cancels every outstanding/driver-queued read, clears every buffer and per-room state, and closes the `RoomView` fan-out. `Reconciler::stop()` only admits the dedicated stop signal; an adapter that must stop its handle afterward awaits `run()` completion first. An RAII run guard makes cancellation/panic total as well: local futures drop before status becomes `Stopped` and subscribers close. Last-owner bus drop closes subscriptions even if `run()` was never polled. After stop, every collection is empty and later controls are refused.

Dropping a `RoomView` consumer, like dropping a `ClientEvent` subscriber, never cancels a reconciliation other consumers still observe.

### R14 — Unsupported events are surfaced safely, never silently dropped (Security: safely surface unsupported events)

`jeliya_api::EventKind` is closed at ten; an `event` push whose `kind` a client cannot decode fails deserialization at the api boundary (the record's "not rendered and not counted" rule). The reconciler must not let that become a silent hole:

- Once an adapter has decoded an envelope and usable `room_id` but cannot decode its typed `Event` content, it routes the room through the crate-private reconciler decode-failure command (`Input::DecodeFailed`) rather than a new public seam arm; the core treats it as a forced gap (`reason = Gap`). A generic malformed frame with no recoverable room remains the kernel's uncorrelated K4 drop. The authoritative read either returns placeable authority or fails closed without publication; the client never renders an event it cannot decode or silently publishes past a detected hole.
- Convergence never inserts an event the reconciler cannot decode; it re-reads instead. "Safely surface unsupported events" is this: an unknown event is a *detected* gap, not a dropped push.

### R15 — Bounded by construction; no unbounded growth (Security: bound every buffer)

Every reconciler collection has a static or configured bound:

| Structure | Bound |
|---|---|
| combined per-room event/peer reconcile buffer | `buffer_depth` items and `buffer_bytes` estimated bytes; loss is counted once and forces recovery |
| opaque identifiers retained anywhere | `max_identifier_bytes` UTF-8 bytes each |
| recent exact `event_id` ring | `max(dedup_window, timeline_depth)` ids; ids are length-capped |
| durable + all-page identity evidence | position-aware exact maps, each capped by `max_baseline_events` and `baseline_dedup_bytes`; exceeding the supported-history ceiling fails closed |
| decoded authoritative reply | `max_read_page_events` events (timeline requests use `min(read_page_size, max_read_page_events)`); backend output ≤ `max_read_reply_bytes`, ≤ `max_read_reply_tokens`, and ≤64 JSON nesting before typed decode, bounding tiny-element allocation amplification; transports separately cap frames before `RawJson` |
| durable/transient timeline tail | `timeline_depth` events and `timeline_bytes` estimated bytes; scalar watermark/scan cursor survives eviction |
| membership / peer snapshot | `member_capacity` / `peer_capacity` rows, with length-capped ids and duplicate-key rejection |
| tracked rooms | `max_active_rooms`, each with a length-capped id; excess activation is a typed refusal |
| in-flight + queued reconciliations per room | ≤ 2 (one in flight, one coalesced re-run — §R9) |
| backend reads across all rooms | `max_concurrent_reads` active; remaining room/read keys wait in a deterministic queue bounded by `max_active_rooms` |
| ordinary control ingress | `min(2·max_active_rooms + 1, 1024)` commands; admission/status/active-set mutation is one serialized transaction; stop has a separate one-slot signal |
| `RoomUpdate` mailbox | 256 ordinary slots plus loss-marker allowance and `update_mailbox_bytes` ordinary estimated bytes; one shared-`Arc` oversized latest-authority allowance prevents an otherwise permanent Lagged-only state, and repeated giants replace rather than stack |

There is **no** collection keyed by an unlimited external string: every opaque id has a byte ceiling, every cardinality is finite, and every payload-bearing retained collection also has a byte ceiling. Defaults deliberately couple the multiplicative limits (16 active rooms, four concurrent 2-MiB replies, 1-MiB timeline/history/scan ceilings, 256-row rosters) so their worst-case retained payload plus fixed map/row overhead stays in the low-hundreds-of-MiB range rather than multi-GiB. Hosts may raise these trusted configuration limits only as an explicit product memory decision. A fault test drives saturation + repeated flap + overflow + churn and asserts bounds and total stop.

### R16 — Secrets never enter diagnostics (Security)

`diag.rs` centralizes the reconciler's log/`Debug`/error strings, reusing the kernel's posture: bearer tokens, browser tickets, `client_id`, `op_id`, opaque event ids, and payload bytes (message bodies, file digests) are **never** rendered. Public `RoomView`/`RoomUpdate` use hand-written redacted `Debug` implementations that expose only bounded aggregate timeline length/range and collection counts rather than delegating to payload-bearing wire types. A test asserts neither payload text nor opaque event ids are printed.

## 6. Configuration and public surface

New **public** items (documented, `#![deny(missing_docs)]`), re-exported from `jeliya_client`:

```rust
/// The reconciler's hard bounds. Every field is explicit; none defaults to "unbounded".
pub struct ReconcileLimits {
    pub buffer_depth: u32,
    pub buffer_bytes: u64,
    pub dedup_window: u32,
    pub max_identifier_bytes: u32,
    pub max_baseline_events: u32,
    pub baseline_dedup_bytes: u64,
    pub max_active_rooms: u32,
    pub max_concurrent_reads: u32,
    pub read_page_size: u64,
    pub max_read_page_events: u32,
    pub max_read_reply_bytes: u64,
    pub max_read_reply_tokens: u32,
    pub timeline_depth: u32,
    pub timeline_bytes: u64,
    pub member_capacity: u32,
    pub peer_capacity: u32,
    pub update_mailbox_bytes: u64,
}

/// Reconciler construction inputs that are not limits.
pub struct ReconcileConfig {
    pub limits: ReconcileLimits,
}

impl Default for ReconcileLimits { /* documented, conservative defaults */ }

/// Why a re-baseline is being taken (§R3).
#[non_exhaustive]
pub enum ResyncReason { /* Bootstrap, Reconnect, Gap{..}, LocalOverflow{..}, ResyncRequiredByDaemon{..}, Resume */ }

/// The converged, per-room view the UI renders: a gap-free ordered timeline
/// window plus the replaced membership/peer snapshot, stamped with the epoch
/// it was reconciled under.
pub struct RoomView { /* room_id, generation (epoch), timeline window, members, peers, reachability */ }

/// The reconciler handle: constructed over a ClientHandle, exposes a per-room
/// RoomView stream and the activate/deactivate/resume/stop controls.
pub struct Reconciler { /* ... */ }
```

- **Not exported:** the sans-IO `Core`, `Input`/`Action`, `buffer.rs`, `room.rs` internals — the machinery stays internal exactly as the kernel's does.
- The seam's public surface (`ClientHandle`, `ClientEvent`, `EventSubscription`, `State`, `CallError`) remains unchanged; malformed-frame recovery uses a private adapter/reconciler path and does not alter reply semantics.

## 7. Implementation steps

1. **`reconcile/reason.rs`** — `ResyncReason` and the `ResyncRequired { generation, reason }` reconciler-facing record. Unit-test reason ordering (which trigger outranks which when coalescing, §R9) and that every arm is constructible.
2. **`reconcile/buffer.rs`** — the byte- and count-bounded per-room push ring; `push()` → `Ok` or `Overflow`; `drain()`. Unit-test both bounds and that overflow reports loss without discarding silently.
3. **`reconcile/room.rs`** — per-room state: watermark, the recent-`event_id` FIFO, the membership/peer sets, and convergence (`apply_baseline`, `drain_and_converge`). Unit-test convergence by `pos`/`event_id`/`at`: dedup-by-watermark, dedup-by-id, in-order apply, out-of-order → re-trigger, and peer replacement.
4. **`reconcile/reason.rs` + `core.rs`** — the sans-IO `step(Input) -> Vec<Action>`: trigger classification, epoch bump/fence, single-flight + coalesce, baseline dispatch, convergence, `EmitResyncRequired`/`EmitView`, stale-drop, cancel/stop. This is the correctness heart; it owns §R2–§R14.
5. **`reconcile/view.rs`** — `RoomView` and the reconciler-owned fan-out (reuse `event.rs`'s bounded `EventBus`).
6. **`reconcile/driver.rs`** — the async `Reconciler`: subscribe once, translate `ClientEvent` → `Input::Event`, apply `Action::Issue*` via `ClientHandle::call`, feed settled results back as `Input::ReadReply { epoch, read_id }`, and broadcast `RoomView`. Wire `activate`/`deactivate`/`resume`/`stop`.
7. **`reconcile/diag.rs`** — redaction wrappers (§R16).
8. **`src/lib.rs`** — `mod reconcile;` and re-export `Reconciler`, `ReconcileConfig`, `ReconcileLimits`, `ResyncReason`, `RoomView`.
9. **`tests/reconcile.rs`** — the property/fault suite over the core: one test per Verification bullet plus the AC map (§8), fully deterministic.
10. **`tests/reconcile_driver.rs`** — integration over the seam's **mock** backend (`--features mock`): scripted bootstrap, reconnect, overflow, resume, cancellation, and stale-generation sequences, asserting the emitted `RoomView` and `ResyncReason` stream.
11. **`tests/boundaries.rs`** — extend the source scan: no `std::time`/`Instant::now`/`SystemTime`/`getrandom`/`rand`/`tokio` token in `src/reconcile/**`; confirm zero new runtime deps.
12. **Docs** — normative surface is crate rustdoc (match `jeliya-api`/seam density). No new `docs/` page is required; the decision is already in `docs/dioxus-architecture.md` §Decision 4. If a reference page is later wanted it must satisfy `docs/PROFILE.md` (exactly 10 frontmatter fields, index reachability) as a separate follow-up.

## 8. Test strategy — every acceptance criterion and Verification bullet mapped

**Acceptance criteria:**

| Issue AC | Reconciler mechanism | Test |
|---|---|---|
| Every gap reason is observable | `ResyncReason` emitted at reconciliation start (§R3) | drive each trigger (bootstrap, reconnect, gap×3 wire reasons, overflow, `resync_required`, resume) ⇒ assert the matching reason surfaces |
| Reconciliation is serialized and coalesced | single-flight + `rerun` coalescing (§R9) | back-to-back gaps during an outstanding resync ⇒ exactly one in-flight read, exactly one coalesced re-run, highest-priority reason kept |
| Overflow cannot permanently deduplicate an undelivered event | loss-not-suppression dedup + forced re-baseline (§R5/§R7/§R10) | overflow the bootstrap buffer ⇒ every authored event present in the converged view exactly once |
| Baseline and buffered pushes converge by event ID and signed timestamp | convergence by `pos`/`event_id`/`at` (§R6) | interleave a baseline with duplicate, in-order, and out-of-order pushes ⇒ gap-free, deduplicated, position-ordered timeline |
| Peer state is replaced from authoritative reads | wholesale replacement + double presence fence (§R8) | authoritative read removes a member; a later stale-generation `peer` push ⇒ member stays removed, phantom not resurrected |
| DirectClient resume uses the same outcome without pretending a socket reconnected | `Input::Resume` (§R11) | resume ⇒ reconciliation with `reason = Resume`, **no** synthesized `StateChanged`; view matches the reconnect view |

**Verification bullets (property/fault):**

- **push during bootstrap** — pushes arrive while the bootstrap read is outstanding; buffered, then converged; a push at `watermark + 1` applies, a duplicate drops, an out-of-order push re-triggers.
- **reconnect during open** — a room `Converged`, then `Interrupted → Ready` (and the coalesced-flap variant) ⇒ `Incremental` resync of the room, epoch bumped, stale prior read (if any) dropped.
- **repeated gaps** — a burst of `gap` frames during an outstanding resync ⇒ one coalesced re-run; positions still converge gap-free.
- **overflow** — reconcile-buffer overflow *and* a fan-out `Lagged` ⇒ `LocalOverflow`, forced re-baseline, no event marked consumed.
- **cancellation** — deactivate a room mid-resync (read dropped, buffer cleared, no remote-cancel claim); stop mid-reconciliation (all reads cancelled, all state empty, idempotent second stop).
- **resume** — `Input::Resume` ⇒ `Resume` reconciliation, no fabricated lifecycle transition.
- **stale generation** — a read issued under epoch E settles after a reconnect to epoch E+1 ⇒ discarded (`DropStale`), newer state untouched; a stale `peer` teardown ⇒ dropped.

**Determinism guard:** every core test uses only scripted `Input`s (no clock, no RNG, no thread); a test asserts no reconciler test constructs a real clock or timer. Behavior is identical on native and `wasm32-unknown-unknown` (the crate already links on wasm; the reconciler adds no wasm-hostile dependency).

## 9. CI changes (`.github/workflows/ci.yml`, Rust job)

1. `cargo test --locked --workspace` already compiles and runs `tests/reconcile.rs` (default-on, no feature gate — the reconciler is production code, not scaffolding).
2. `tests/reconcile_driver.rs` drives the mock, so CI runs it with the existing `cargo test -p jeliya-client --features mock` step (add `reconcile_driver` to the `required-features = ["mock"]` test targets in `Cargo.toml`, mirroring the `seam` target).
3. **MSRV:** confirm the reconciler compiles under **1.91.0** (no edition-2024-only syntax, no std API newer than 1.91). The existing MSRV job gates this.
4. `boundaries.rs` (run by the workspace test) gains the reconciler source scan (§7.11); no new CI step needed.
5. No new toolchain, target, or runner capability is required — the reconciler adds no dependency and no wasm test run (compilation is already gated by the example/build steps).

## 10. Risks and mitigations

- **Scope creep into the adapters (#171/#172/#173).** Building a real transport/resume signal here would violate the transport-independence. *Mitigation:* the reconciler consumes only `ClientHandle` + `EventSubscription` + explicit `Resume`; the adapters translate their lifecycle into those inputs.
- **A stale baseline overwriting newer state.** *Mitigation:* the epoch fence (§R4) plus the kernel's per-connection generation fence; a stale completion is dropped, never applied.
- **Silent loss on overflow.** *Mitigation:* dedup structures record validated application, never a dropped push (§R10); any pending recovery is a publication barrier and re-reads the dropped range.
- **Unbounded growth under flap/overflow churn.** *Mitigation:* every collection is bounded (§R15); single-flight caps in-flight+queued reconciliations at two per room; a stress fault test asserts the bounds and that stop empties everything.
- **Merging peer state and keeping phantoms.** *Mitigation:* wholesale replacement from authoritative reads (§R8); a merge is never performed.
- **An unknown event kind leaving an undetected hole.** *Mitigation:* a decode-failed push is a *detected* gap → resync (§R14), never a silent drop.
- **Fabricating a reconnect for DirectClient.** *Mitigation:* `Resume` produces the same outcome and synthesizes no lifecycle transition (§R11); a test asserts the absence of a synthetic `StateChanged`.
- **A hidden clock/RNG.** *Mitigation:* sans-IO core with ordering driven by `pos`/`event_id`/`at`; the `boundaries.rs` source scan.
- **Seam surface drift.** If the reconciler needed a seam change, #167/#168's "sufficient without a breaking change" claim would fail. *Mitigation:* the reconciler is designed to consume the existing seam; the one place a seam addition *would* help (surfacing the kernel's exact generation) is deferred to Open Question 1 as an additive, non-breaking refinement, not a requirement.

## 11. Non-goals (from the issue, restated)

- **Choosing a cursor, snapshot revision, or generation outside #161** — the reconciler consumes `pos`/`from_pos`/`next_pos` and the connection generation as the protocol and #161 define them; it does not invent a new recovery coordinate.
- **Exactly-once sends** — the reconciler reconciles *received* state; send-side idempotency is the kernel's `op_id` dedup (#168), not this.
- **Fabricated reconnect states for DirectClient** — resume is an explicit, honest input (§R11).
- **UI-specific room rendering** — the reconciler emits a `RoomView`; how a component renders it is #178+.
- **The public seam surface** — unchanged; #169 adds reconciler types above it. Malformed pushes use a private adapter/reconciler recovery path rather than changing `ClientEvent`.
- **Full byte-stream framing / file transfer reconciliation** (#233/#242/#243 and the kernel's stream hooks #269) — the reconciler handles room *events*, not file byte streams.

## 12. Open questions

1. **Reconciler epoch vs. the kernel's connection generation.** §R4 uses a reconciler-local monotonic epoch (no seam change). Should the seam instead surface the kernel's exact `generation` on a lifecycle signal (e.g. a `generation` field on `StateChanged`'s `Ready`, or a dedicated `Connected { generation }` event) so the reconciler's fence value equals the kernel's numerically? Recommend the **local epoch** for this slice (honest, bounded, seam-preserving) and treat the exact-generation surfacing as an additive follow-up if the parity suite (#175) shows a case the local epoch cannot fence.
2. **`RoomView` delivery channel.** Should the reconciler broadcast `RoomView` on a second reuse of `event.rs`'s `EventBus`, on a per-room channel, or by extending `ClientEvent` with a `RoomReconciled` arm? Recommend a **reconciler-owned fan-out** (reuse `EventBus`) so the seam's `ClientEvent` stays the raw wire model and the reconciled view is a distinct, opt-in subscription.
3. **Baseline read composition for `Reconnect`/`Resume`.** Reconnect/resume re-baseline events (`stream.resync`) *and* presence (`room.peers`/`room.members`). Should presence always be re-read on every reconcile, or only when a `peer` push or a membership event was implicated? Recommend **always** on reconnect/resume (presence can change silently while offline) and **events-only** on a pure position `gap`, revisited under #175.
4. **`dedup_window` sizing.** The recent-`event_id` FIFO exists only for the narrow cross-boundary case (§R7). What is the right constant default, and does any real flow need it larger than the reconcile buffer? Recommend a small default (e.g. equal to `read_page_size`) and confirm under the fault suite.
5. **Interaction with `stream.subscribe` at bootstrap.** Bootstrap anchors on `stream.subscribe.from_pos`. Does the reconciler own the `stream.subscribe`/`stream.unsubscribe` lifecycle, or does the adapter subscribe and hand the reconciler the `from_pos`? Recommend the **reconciler owns subscribe/unsubscribe** (they are connection-scoped, `op_id`-ignored, and must be re-issued per generation — exactly the reconciler's job), confirmed against #171/#172/#173.
6. **`max_active_rooms` refusal shape.** Activation beyond the bound is refused with a typed error (§R15). Is a `CallError`-shaped refusal right, or a distinct `ReconcileError`? Recommend a small dedicated `ReconcileError` so a room-capacity refusal is not confused with a wire error.

## 13. Assumptions

- `crates/jeliya-client` (#167/#168) is landed at its current shape: `ClientHandle::{call, subscribe, state, start, stop}`, `ClientEvent` (`StateChanged { from, to, coalesced_through_problem }`, `Push(RoomPush)`, `Gap`, `ResyncRequired { room_id, from_pos }`, `Lagged { room_id, dropped }`), `EventSubscription: Stream`, `State`, `CallError`/`Execution`, and the mock — all consumed **without** modifying their public surface.
- `jeliya_api` is stable at its current shapes: `Event { pos, event_id, at, author, kind }`, `Push`, `GapReason`, `GapTo`, `Truncated`/`Cursor`, `ApiError::ResyncRequired { room_id, from_pos }`, and the reads `RoomTimeline`/`RoomTimelineOut`, `RoomMembers`/`RoomMembersOut`, `RoomPeers`/`RoomPeersOut`, `StreamSubscribe`/`StreamSubscribeOut { from_pos }`, `StreamResync`/`StreamResyncOut { events, next_pos, truncated }`.
- The protocol's ordering/gap/resync model is authoritative: per-room positions are the dense rank over `(lamport, event_id)`, strictly increasing and gap-free; `stream.resync` is the authoritative recovery (positions exclusive on the low side, resend `next_pos`); `resync_required` names a position to discard back to; `peer` carries the connection generation for stale-teardown discard (dependent on upstream U1, whose conformance cases are `blocked_on_upstream`); `stream.subscribe`/`stream.resync`/`stream.unsubscribe` are connection-scoped and `op_id`-ignored, so they are re-issued per generation and never rely on kernel replay.
- The crate's MSRV is **1.91** and the CI toolchains are 1.96.0 (primary) + 1.91.0 (MSRV) with `wasm32-unknown-unknown` available; the reconciler adds no dependency and no wasm-hostile code.
- The orchestrator performs all git/gh/PR actions; this document is the only artifact the planning phase produces, and no production code is written for #169 by the planning phase.

## 14. Acceptance-criteria checklist (traceability)

- [ ] **Every gap reason is observable** — §R3 (`ResyncReason` emitted at reconciliation start); test in §8.
- [ ] **Reconciliation is serialized and coalesced** — §R9 (single-flight + `rerun`); test in §8.
- [ ] **Overflow cannot permanently deduplicate an undelivered event** — §R5/§R7/§R10 (loss-not-suppression + forced re-baseline); test in §8.
- [ ] **Baseline and buffered pushes converge by event ID and signed timestamp** — §R6 (`pos`/`event_id`/`at`); test in §8.
- [ ] **Peer state is replaced from authoritative reads** — §R8 (wholesale replacement + double presence fence); test in §8.
- [ ] **DirectClient resume uses the same outcome without pretending a socket reconnected** — §R11 (`Input::Resume`, no synthetic `StateChanged`); test in §8.
