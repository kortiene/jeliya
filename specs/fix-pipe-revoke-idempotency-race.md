# Fix: `pipe.revoke` idempotency race — a second revoke authors a new withdrawal instead of replaying the original

- **Issue:** #271 `[Daemon][Flaky]: pipe.revoke idempotency race — a second revoke occasionally authors a new withdrawal instead of replaying the original`
- **Priority / labels:** flaky · daemon · reliability
- **Owning module (Defect A):** `crates/jeliya-core/src/supervisor.rs` — `RoomSupervisor::pipe_close`
- **Boundary consumer:** `crates/jeliya-core/src/typed.rs` — `TypedSupervisor::pipe_revoke`
- **Owning module (Defect B):** `crates/jeliyad/src/serve.rs` — test `websocket_file_share_progress_resets_stall_but_not_absolute_deadline`
- **Conformance surface:** `conformance/v2/pipes.json` case `pipe_revoke_of_an_already_revoked_pipe_returns_the_original_withdrawal` (case index 26); companion `pipe_revoke_withdraws_the_publication_as_a_converged_room_fact` (index 25)
- **Type:** current-stack reliability maintenance; two independent defects fixed in one sweep; no new dependencies; MSRV 1.91, `unsafe_code` forbidden (both fixes are safe Rust)

---

## 0. Scope and non-goals

This spec fixes two **pre-existing** flakes surfaced during wave-2's #207 corpus A/B verification (2026-08-10). Neither is caused by any wave-2 PR; both predate the wave.

- **Defect A (primary):** the daemon's `pipe.revoke` is **not idempotent** at the room-fact layer. A second, genuinely distinct revoke of an already-revoked pipe authors a *second* signed `pipe.closed` (→ committed `pipe_revoked`) event rather than replaying the original withdrawal. This is a correctness defect that *manifests* as flake.
- **Defect B (secondary sweep):** the serve-crate unit test `websocket_file_share_progress_resets_stall_but_not_absolute_deadline` drives its timers off a **real wall clock** (`tokio::time::sleep`) while its two siblings use a paused virtual clock, making it timing-sensitive under concurrent CI load.

**Non-goals:** no change to the wire protocol, the `pipe.closed` event schema, the op_id dedup ledger, or `pipe.connect`/`pipe.list`/`pipe.release` semantics. No durable persistence of the ledger. No change to the conformance corpus fixtures (the fixture already encodes the correct expectation; the daemon must meet it). Do not "fix" the flake by loosening the harness assertion.

---

## 1. Problem statement — Defect A (daemon idempotency race)

### 1.1 What the corpus case requires

`conformance/v2/pipes.json` → `pipe_revoke_of_an_already_revoked_pipe_returns_the_original_withdrawal` runs, all on `subject:A` (the publisher):

1. `pipe.publish` (op_id `$op1`) → saves `$pid`.
2. `pipe.revoke {room_id, pipe_id:$pid}` with op_id **`$op2`** → saves `revoked_at = out.revoked_at`.
3. `pipe.revoke {room_id, pipe_id:$pid}` with op_id **`$op3_distinct`** — a *different* op_id — and asserts:
   ```json
   "expect": { "ok": true, "out": {
       "room_id": "$rid", "pipe_id": "$pid",
       "revoked_at": "$revoked_at",      // MUST equal step-2's instant
       "event_id": "<event_id>", "pos": "<pos>" } }
   ```
4. `pipe.list` → `pipes: []`, plus `{ "observe": "no_event_authored", "scope": "case" }`.

The intent line is explicit: *"a second, genuinely distinct withdrawal request succeeds idempotently and returns the FIRST revocation instant rather than authoring a second withdrawal."* Because step 3 carries a **distinct op_id**, the op_id dedup ledger (`is_dedup_op` includes `pipe.revoke`, `crates/jeliya-core/src/engine.rs:307`) does **not** cover it — idempotency here must come from the **domain state** (the pipe is already revoked), not from op_id replay. (Case 25 exercises the op_id-replay path with the *same* op_id `op-revoke-1`; that path is already correct and must stay correct.)

### 1.2 The daemon path today

`TypedSupervisor::pipe_revoke` (`crates/jeliya-core/src/typed.rs:1991-2019`) delegates to `RoomSupervisor::pipe_close` for the event id, then derives `pos`/`revoked_at` from that id via `committed_pos_and_instant` (`typed.rs:2043+`):

```rust
let event_id = self.sup.pipe_close(room_id, pipe_id).await …?;
let (pos, revoked_at) = self.committed_pos_and_instant(&ctx.room_id, &event_id)?;
```

`RoomSupervisor::pipe_close` (`crates/jeliya-core/src/supervisor.rs:3190-3241`) does, in order:

1. `open_pipe(&store, …, pipe_id)` — **unknown-pipe guard**; `None` → `PipeUnknown` (`supervisor.rs:4132`, scans `PipeOpened` rows only; a *closed* pipe still has its `PipeOpened` row, so this still finds it).
2. **Ownership guard**: `opened.owner_id != self_id` → `PipeDenied` (→ `PipeNotPublisher`).
3. **Unconditionally** `session.node.pipe_close(…, now_ms())`.
4. `release_pipe_connections(...)`.
5. `find_pipe_event(room, EventType::PipeClosed, pipe_id)` and returns that id.

The upstream `node.pipe_close` (`iroh-rooms-net/src/node.rs:1099`) **builds and publishes a fresh `pipe.closed` unconditionally** — it never checks whether the pipe is already closed:

```rust
let wire = build_pipe_closed(…, created_at);   // created_at = now_ms()
self.publish(wire.to_bytes()).await …?;
self.pipe_registry.remove(&pipe_id);           // dedup happens AFTER the author, uselessly
```

And the projection commits **every** `pipe.closed`: `is_committed` (`crates/jeliya-core/src/projection.rs:195-201`) is true for any `pipe.closed` with a valid `created_at` (its `kind_content` maps to `PipeRevoked`, `projection.rs:434`). There is **no fold-level dedup** of a second close for the same pipe — both closed events are committed and each consumes a timeline position.

**Consequence:** step 3 always authors a *second* committed `pipe_revoked` event. There is no idempotency guard anywhere in the jeliya-owned path.

### 1.3 Why it usually passes on a quiet machine, and races under load

Two harness checks straddle the second author; each fails on a different sub-timing, which is why the flake rate varies (measured 1/5 on a PR build, 2/5 on baseline main@1b44321 under concurrent cargo load; deterministic-pass in isolation on a quiet machine):

- **`revoked_at: "$revoked_at"` equality.** `find_pipe_event` (`supervisor.rs:3247-3286`) returns **whichever matching row `store.by_type` yields *last*** (`found = Some(...)` overwrites each match). With two closed events present, the returned event id — and therefore the `revoked_at`/`event_id`/`pos` that `committed_pos_and_instant` derives — is whichever the store orders last. On a quiet machine the two authors land in the **same `now_ms()` millisecond**, so both events carry an identical `revoked_at` and the equality holds by coincidence; under load the two authors straddle a millisecond boundary, the second event's `revoked_at` differs, and if `by_type` yields it last the equality assertion fails.
- **`no_event_authored` (scope: case).** The harness (`conformance/v2/harness/runner.mjs`) snapshots the room's committed-event total via `room.timeline` (`#roomEventTotal`, `runner.mjs:1177`) and, after each **successful** step, refreshes the baseline (`runner.mjs:145-160`); the observation throws iff the total has grown past the last baseline (`#evalObserve` → `no_event_authored`, `runner.mjs:1264+`). Step 3 is `ok:true`, so its baseline refresh *should* absorb the extra event — but that refresh reads `room.timeline` through a subscribed session **immediately** after step 3's reply, racing the just-authored event's WAL visibility. Under load the refresh reads the *stale* (pre-second-event) total; the later `no_event_authored` observation reads the *settled* total and sees growth → throw: `no_event_authored violated: room events grew N -> N+1`.

Both symptoms have the **same root cause**: a second withdrawal is authored at all. Remove the second author and both symptoms vanish deterministically.

### 1.4 Why the sequential case is deterministically fixable (and the residual concurrent race)

By the time step 3 runs, step 2's reply has already returned — and `pipe_revoke`'s reply is produced only after `find_pipe_event` **polls the local store until the `pipe.closed` row is visible** (up to 20 × `POLL_INTERVAL` = 2 s, `supervisor.rs:128,3253`). So when step 3 begins, the original closed event is guaranteed present in the same local store an idempotency lookup would read. The **sequential** case is therefore closed *deterministically* by a read-before-author guard, with no locking.

A **concurrent** pair of distinct-op_id revokes of the same pipe (legal, and what the issue title's word "race" also covers) is *not* closed by the read alone: both could read "not yet closed" and both author. Closing that window requires serializing the check-then-author per `(room, pipe)`. This spec includes that serialization (§2.3) so the fix honestly matches the title, while noting the corpus flake is resolved by §2.1 alone.

---

## 2. Fix design — Defect A

Three parts, in `crates/jeliya-core/src/supervisor.rs`, entirely behind the existing `RoomSupervisor::pipe_close` seam (no signature change; `typed.rs` and the wire are untouched).

### 2.1 Idempotent replay: return the original withdrawal, author nothing (primary; sufficient for the corpus case)

Add a helper that finds the **original** (canonically earliest) committed `pipe.closed` for a pipe, returning its bare event-id hex:

```rust
/// The bare event-id hex of the ORIGINAL `pipe.closed` for `pipe_id` in
/// canonical timeline order, or `None` if the pipe has never been closed
/// in the local log. "Original" = the first committed close by the same
/// `(lamport, event_id)` rank the timeline/resync/pos derivation uses, so a
/// replayed revoke serves the exact instant/pos the first revoke served.
fn original_pipe_closed(
    store: &EventStore,
    room_id: &RoomId,
    pipe_id: [u8; SHORT_ID_LEN],
) -> CoreResult<Option<String>>
```

Implementation notes:
- Scan the canonical tail (`store.room_tail(room_id, u32::MAX)`), skipping non-committed rows via `proj::is_committed`, and return the **first** row whose decoded content is `Content::PipeClosed(p)` with `p.pipe_id == pipe_id`. Do **not** use `store.by_type` "last wins" — canonical-order-first is what makes "the original" well-defined and consistent with `committed_pos_and_instant`, which ranks the same canonical order.
- This scan mirrors the ordering already used in `committed_pos_and_instant` (`typed.rs:2043+`) and `positioned` (`projection.rs:174`); keep it read-only and sync-scoped (no `!Sync` store borrow across an await), matching the surrounding code.

Rewrite `pipe_close` to consult it **after** the unknown-pipe and ownership guards and **before** authoring:

```rust
// (unchanged) unknown-pipe guard + ownership guard on open_pipe(...)

// Idempotent replay: an already-withdrawn pipe returns its ORIGINAL
// withdrawal and authors nothing further. This must run only after the
// publisher relation is confirmed, so a non-publisher re-revoking a closed
// pipe still gets pipe_not_publisher, never a laundered success.
{
    let store = self.open_store()?;
    if let Some(event_id) = original_pipe_closed(&store, &room_id, pipe_id)? {
        return Ok(event_id);
    }
}

// First withdrawal: author exactly one signed pipe.closed.
session.node.pipe_close(…, now_ms()).await …?;
self.release_pipe_connections(&room_id, pipe_id_hex)?;   // idempotent; harmless on replay
let event_id = self.find_pipe_event(&room_id, EventType::PipeClosed, pipe_id).await?;
Ok(event_id)
```

Ordering rationale (do not reorder):
- **Unknown-pipe** first — preserves `pipe_revoke_of_an_unknown_pipe_is_pipe_unknown` (case 27) and the record's "never confirm existence via an auth error" rule.
- **Ownership** second — preserves `pipe_close_refuses_a_room_authority_that_did_not_publish` (`supervisor.rs:6281`); a non-publisher never reaches the idempotent-replay branch, so re-revoking a closed pipe as a non-publisher still yields `PipeNotPublisher`.
- **Idempotent replay** third — only a confirmed publisher of an already-closed pipe gets the original event id back with no new author.

`release_pipe_connections` on the replay path is intentionally **skipped** (the first revoke already released them; the pipe is closed, so no live connection should exist). It is idempotent, so if a future change moves it onto the replay path it stays harmless — but keeping the replay branch a pure read is preferred.

### 2.2 Deterministic selection on the first-author path (defense in depth)

Change `find_pipe_event` (or introduce a dedicated `find_first_pipe_event`) to select the **canonically earliest** matching row rather than the last `by_type` row, so that even if two closed rows ever coexist (e.g. a legacy store, or a lost race before §2.3 lands) the returned id is stable and equals "the original." Concretely: break out of the scan on the first canonical-order match instead of letting later rows overwrite `found`. `find_pipe_event` is also used for `PipeOpened` lookups on the publish path; keep that call site's behavior equivalent (a pipe has exactly one `PipeOpened`, so first-vs-last is identical there). Preserve the existing 20-poll WAL-visibility retry.

> With §2.1 in place, the first-author path can produce at most one `pipe.closed` per pipe per daemon, so §2.2 is belt-and-suspenders; include it because it is cheap, removes the last nondeterministic selector on this path, and makes any future regression fail loudly rather than flake.

### 2.3 Per-`(room, pipe)` serialization of check-then-author (closes the concurrent-distinct-op_id window)

Add a lightweight, bounded guard map to `RoomSupervisor` so the read-existing-close → author critical section is atomic per pipe:

```rust
/// Serializes concurrent revokes of the SAME pipe so a check-then-author
/// cannot interleave into two withdrawals. Keyed per (room, pipe); distinct
/// pipes never contend. In-memory, like the op_id ledger.
revoke_guards: StdMutex<HashMap<(RoomId, [u8; SHORT_ID_LEN]), Arc<TokioMutex<()>>>>,
```

In `pipe_close`, after the ownership guard, acquire (get-or-insert) the per-key `Arc<TokioMutex<()>>` and hold it across the §2.1 replay-lookup + the first-author sequence:

```rust
let guard_cell = {
    let mut guards = self.revoke_guards.lock().expect("revoke guard map poisoned");
    guards.entry((room_id.clone(), pipe_id)).or_default().clone()
};
let _authoring = guard_cell.lock().await;   // per-pipe; other pipes/rooms unaffected
// … §2.1 replay lookup, else author + find …
```

Constraints:
- The `StdMutex` around the *map* is released before `.await` (only the per-key `Arc<TokioMutex>` is held across the await), so no `std::sync::Mutex` guard crosses an await point.
- Do **not** reuse the supervisor's global `structural` `TokioMutex` (`supervisor.rs:324`): it serializes room open/teardown, and holding it across `node.pipe_close` + the up-to-2 s `find_pipe_event` poll would stall unrelated room opens. A dedicated per-pipe map keeps contention to concurrent revokes of the *same* pipe only.
- Map growth is bounded by the number of distinct pipes ever revoked on the daemon (in-memory, per the record's "no durable ledger" stance). Cleanup is optional; if added, remove the entry only while holding the map lock and only when no waiters remain. Leaving entries is acceptable for the MVP (a pipe id is 16 bytes; the count is tiny).

> If review judges §2.3 out of scope for a "flaky" ticket, it may be split into a follow-up, leaving §2.1 + §2.2 as the shippable fix for the observed corpus flake. State that decision explicitly in the PR; do not silently drop it, since the issue title names a race.

---

## 3. Fix design — Defect B (flaky serve test)

`crates/jeliyad/src/serve.rs::websocket_file_share_progress_resets_stall_but_not_absolute_deadline` (`serve.rs:4899-5010`) is `#[tokio::test]` and drives timing with a **real** `tokio::time::sleep(800 ms)` (`serve.rs:4927`), then relies on the daemon's real `Instant`-based stall (1 000 ms) and absolute deadline (~1 600 ms) timers firing in a specific order to produce the CREDIT → ABORT → (ACK) → terminal-Text sequence it asserts. Under concurrent CI load the 800 ms real sleep and the daemon timers drift, reordering records and intermittently failing the assertions. Its two siblings avoid this by pausing the clock:

- `websocket_file_share_daemon_abort_ack_timeout_replies_then_closes_4007` — `tokio::time::pause()` + `advance()` (`serve.rs:4751,4781,4787`).
- `websocket_file_share_no_progress_stalls_and_survives_exact_ack` — `tokio::time::pause()` + `advance()` (`serve.rs:4833,4834`).

The stall/deadline timers are `Instant`-based (`crates/jeliyad/src/transfer.rs:174-196`: `stall_deadline`/`deadline` add `Duration`s to a start `Instant`), so a paused virtual clock advanced with `tokio::time::advance` drives them deterministically — the pattern is already proven in this file.

### 3.1 Conversion

Convert the test to the paused-clock pattern:
1. Call `tokio::time::pause()` at the top (before `socket_pair`, matching the siblings).
2. Replace `tokio::time::sleep(800 ms)` with `tokio::time::advance(Duration::from_millis(800))` to reach the mid-transfer progress point *before* sending the single DATA byte.
3. After sending DATA, **await the CREDIT record** (as today) before any further `advance` — awaiting the CREDIT proves the byte was durably accepted and the stall timer was reset; do not advance the clock until it arrives (otherwise the reset has not yet happened and the stall/deadline race re-enters).
4. `advance` the remaining time to cross the **absolute** deadline (~1 600 ms from OPEN) while staying **below** the reset stall boundary (~1 800 ms), then await the ABORT (`OperationError`), matching the current assertions and the test's stated arithmetic (`serve.rs:4906-4911`).
5. Keep the exact-ACK → terminal `TransferDeadlineExceeded { transferred_bytes:1, total: Known{bytes:1}, budget_ms:1600 }` and the `room.list` survival exchange unchanged; these are message round-trips, not timer-driven, so they need no `advance`.
6. Preserve the post-conditions: `state.transfer_pool.usage() == (0,0)` and empty `protocol-v2-stream-staging`.

Interleaving rule (the one subtlety): every `advance` must be **bracketed** by the message exchange it is meant to trigger — advance to a boundary, then `await` the record the daemon emits at that boundary — so the virtual clock never jumps past two timer boundaries in one step. The siblings demonstrate exactly this cadence.

### 3.2 Fallback / honesty clause

If, after the paused-clock conversion, the test *still* flakes, that indicates a genuine daemon ordering nondeterminism (ABORT vs CREDIT) rather than harness timing — a separate defect. In that case, stop and file/annotate it; do **not** paper over it by widening timing tolerances. The expectation is that the conversion fully deterministically fixes it, consistent with #244's characterization of this as a stall/deadline *timing* flake.

---

## 4. Implementation steps (ordered, red-before-green)

1. **Reproduce Defect A red first.** Add a `jeliya-core` unit test (near `pipe_close_refuses_a_room_authority_that_did_not_publish`, `supervisor.rs:6281`) named e.g. `pipe_close_of_an_already_closed_pipe_replays_the_original_and_authors_nothing`:
   - Single-daemon: create room, open, publish a pipe to a bound `TcpListener`, `pipe_close` → capture `event_id_1`.
   - Assert the room's committed timeline contains exactly **one** `pipe_revoked` for the pipe (count `by_type(PipeClosed)` decoded matches == 1), and record its rank/instant.
   - `pipe_close` **again** → assert the returned `event_id_2 == event_id_1`, and that the committed `pipe_revoked` count for the pipe is **still 1** (no new author). This test fails on `main` (count becomes 2 and/or ids differ).
2. **Add `original_pipe_closed` (§2.1)** and rewire `pipe_close` to replay-before-author. Re-run the test from step 1 → green.
3. **Add a concurrency test (§2.3):** spawn two `pipe_close` futures for the same pipe with `tokio::join!` (distinct logical requests) and assert the committed `pipe_revoked` count is exactly 1 and both return the same id. Confirm it fails without the guard and passes with it. (If §2.3 is deferred, mark this test `#[ignore]` with a reason referencing the follow-up rather than deleting it.)
4. **Deterministic selection (§2.2):** change `find_pipe_event` to first-canonical-match; add/extend a test proving that with two synthetic closed rows the earliest is returned. Verify the publish-path `PipeOpened` lookup is unaffected.
5. **Focused core gate:** `cargo test -p jeliya-core pipe_close` and the pipe-related tests; then `cargo test -p jeliya-core`.
6. **Defect B:** convert the serve test to paused-clock (§3). Run it in a tight loop under load to confirm determinism, e.g.:
   ```
   cargo test -p jeliyad --lib serve::tests::websocket_file_share_progress_resets_stall_but_not_absolute_deadline
   ```
   Optionally wrap in a shell loop (×50) while a `cargo build` churns in parallel to simulate CI load; expect 0 failures.
7. **Conformance A/B (Defect A proof):** run the v2 harness case `pipe_revoke_of_an_already_revoked_pipe_returns_the_original_withdrawal` repeatedly under load, before vs after, and confirm the after-build is deterministically green. Per repo memory, the *authoritative* proof for a daemon change is a full 341-case v2 corpus A/B diff (CI live-gates only ~22/341), so also run the full corpus once to confirm **no regression** on cases 25/27 and the other `pipe.*` cases. Commands per `conformance/v2/harness` (`npx`/node harness); consult `conformance/v2/manifest.json` for the runner entrypoint.
8. **Workspace safety net:** `cargo test --workspace` (or the crates touched: `jeliya-core`, `jeliyad`) plus `cargo clippy --workspace -- -D warnings` and `cargo fmt --check`; `unsafe_code` remains absent.
9. **Docs/changelog:** add a `CHANGELOG.md` entry under the daemon/reliability section noting the idempotent `pipe.revoke` fix and the deterministic serve test. No `docs/` profile page changes are required (no capability-status change — the behavior was always *specified* to be idempotent; only the implementation is corrected). If a maintainer judges the idempotency guarantee worth surfacing, update `docs/capability-status.md` for `pipe.revoke` and keep it reachable from `docs/index.md` (validated by `node scripts/check-docs.mjs`).

> Per the session working rules: this is a targeted daemon fix; run the **focused** tests above during development. Reserve a full `npm run verify` for the ADW finalize gate — do not re-run the whole suite every phase. This spec touches Rust crates and the conformance harness only; there is no prompt-pack (`.adw/pack.profile.json`) change.

---

## 5. Acceptance criteria

**Defect A**
- [ ] `RoomSupervisor::pipe_close` authors **at most one** committed `pipe_revoked` event per `(pipe_id)` per daemon: a second (or Nth) revoke of an already-closed pipe returns the **original** event id and authors nothing.
- [ ] A distinct-op_id second revoke of an already-revoked pipe returns `ok:true` with `revoked_at`/`event_id`/`pos` **equal to the first revoke's** (`committed_pos_and_instant` over the original event).
- [ ] Ownership and unknown-pipe semantics are unchanged: `PipeUnknown` for an unknown pipe, `PipeNotPublisher` for a non-publisher — including a non-publisher revoking an *already-closed* pipe.
- [ ] Concurrent distinct-op_id revokes of the same pipe converge on one withdrawal (§2.3), or that residual window is explicitly deferred in the PR with a tracked follow-up and an `#[ignore]`'d proof test.
- [ ] `find_pipe_event` returns the canonically-earliest match; publish-path `PipeOpened` lookup unchanged.
- [ ] Conformance case `pipe_revoke_of_an_already_revoked_pipe_returns_the_original_withdrawal` is deterministically green under concurrent load (≥ 20 consecutive runs, 0 failures), and case `pipe_revoke_withdraws_the_publication_as_a_converged_room_fact` (op_id-replay + member-B convergence) and case `pipe_revoke_of_an_unknown_pipe_is_pipe_unknown` remain green.
- [ ] Full 341-case v2 corpus shows no regression attributable to this change.

**Defect B**
- [ ] `websocket_file_share_progress_resets_stall_but_not_absolute_deadline` uses a **paused** virtual clock (`tokio::time::pause()` + `advance`), no real `sleep` for timer progression.
- [ ] The test asserts the same observable sequence and post-conditions as today (CREDIT `{accepted_through:1, send_through:2}`, ABORT `OperationError`, exact-ACK → `TransferDeadlineExceeded{…, budget_ms:1600}`, `room.list` survival, `transfer_pool.usage()==(0,0)`, empty staging) and passes deterministically under load (≥ 50 consecutive runs, 0 failures).

**Both**
- [ ] `cargo clippy --workspace -- -D warnings`, `cargo fmt --check` clean; no `unsafe`.
- [ ] No change to public wire/schema, the op_id ledger, or unrelated daemon paths.

---

## 6. Risks, security, and rollback

- **Correctness of "original" selection.** If the earliest-canonical scan disagreed with `committed_pos_and_instant`'s ranking, a replay could serve a `pos`/`revoked_at` inconsistent with the timeline. Mitigation: both derive from the *same* canonical `(lamport, event_id)` order over `room_tail` filtered by `proj::is_committed`; the §4 tests assert the replayed `pos`/`revoked_at` equal the first revoke's.
- **Ordering regressions in `pipe_close`.** Inserting the replay/guard must not move the unknown-pipe or ownership guards. Mitigation: explicit ordering in §2.1 and the dedicated refusal tests (case 27, `supervisor.rs:6281`).
- **Deadlock / await-safety of the guard (§2.3).** Holding a `std::sync::Mutex` across `.await` is forbidden (and would be non-`Send`). Mitigation: the map's `StdMutex` is released before the per-key `TokioMutex` is awaited; only the async mutex is held across the author/poll. `cargo clippy` and the `Send` bound on the spawned serve future catch violations.
- **Guard-map growth.** Bounded by distinct revoked pipes (in-memory, MVP-acceptable). Optional cleanup under the map lock; documented in §2.3.
- **Convergence unchanged.** Authoring exactly once is *more* correct for multi-member convergence (case 25's member-B `pipe.list: []`), not less. No push/resync path changes.
- **Security/privacy.** No new data crosses the trust boundary; the idempotent path is a local read. The unknown-pipe-before-auth ordering (which avoids confirming a pipe's existence to a non-publisher) is preserved.
- **Serve-test conversion risk.** Mis-bracketing an `advance` past two timer boundaries would change which record arrives first. Mitigation: §3.1 interleaving rule + the sibling tests as the reference cadence; the §3.2 honesty clause forbids masking a residual real race.
- **Rollback.** Each defect is independent and revertible in isolation: Defect A is contained to `pipe_close` + one helper (+ optional guard field) in `supervisor.rs`; Defect B is a single test function in `serve.rs`. Reverting either restores prior behavior with no schema/state migration.

---

## 7. Assumptions

- The upstream `iroh-rooms-net` `node.pipe_close` remains "author unconditionally"; the fix lives entirely in the jeliya-core layer and does not depend on an upstream change. (If upstream later adds idempotency, §2.1 becomes a redundant fast-path, still correct.)
- The op_id dedup ledger continues to cover *same-op_id* replays (case 25); this spec only adds *domain-state* idempotency for *distinct-op_id* re-revokes.
- The conformance fixtures for cases 25/26/27 are correct as written and must not be edited; the daemon is what changes.
- Stall/deadline timers are `Instant`-based and thus controllable by `tokio::time::pause()`/`advance()` (confirmed at `transfer.rs:174-196` and the two sibling tests).
- The v2 replay harness is single-subject/single-daemon for the primary daemon; case 26 is entirely `subject:A` on one daemon, so it is faithfully reproducible locally (unlike the multi-actor gap noted in repo memory).

---

## 8. Open questions

1. **Should §2.3 (concurrent-distinct-op_id serialization) ship with this ticket or as a follow-up?** The observed corpus flake is sequential and fully fixed by §2.1+§2.2; §2.3 closes the residual race the *title* names. Recommendation: ship it (small, contained); fall back to a tracked follow-up only if review pushes back.
2. **Guard-map cleanup:** leave entries (bounded, tiny) vs. reference-count and evict. Recommendation: leave for the MVP; revisit only if a stress test shows unbounded distinct-pipe churn.
3. **`docs/capability-status.md` surfacing:** is "revoke is idempotent" a capability-status-worthy guarantee, or purely an implementation-correctness fix? Recommendation: changelog only unless a maintainer wants it in the capability matrix.
4. **`find_pipe_event` rename:** keep the shared helper and change selection semantics (§2.2), or introduce a dedicated `find_first_pipe_event` and leave `find_pipe_event` for the single-`PipeOpened` case? Recommendation: rename/generalize to first-match; the `PipeOpened` call site is unaffected (one opened event per pipe).
5. **Corpus-run command of record:** confirm the exact `conformance/v2/harness` invocation and any load-simulation wrapper the maintainer wants cited as the acceptance evidence (the issue's measurements were "machine under concurrent cargo load").
