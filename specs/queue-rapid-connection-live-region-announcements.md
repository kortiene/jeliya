# Queue rapid connection live-region announcements so each is heard

- **Issue:** #276 — [Rust][jeliya-ui] Queue rapid connection live-region announcements so each is heard
- **Owning crate/module:** `crates/jeliya-ui` — `src/components/live_region.rs` (the `Announcer` seam) and `src/app.rs` (the connection announcer that feeds it); e2e in `crates/jeliya-ui/e2e/a11y.spec.ts`.
- **Split from:** #177 (CSS / i18n / a11y foundations), round 32 review. Complementary to rounds 25/26 (`coalesced_through_problem`, which keeps the RECOVERY audible after a coalesced problem window).
- **Status:** planning / specification only. No production code is changed by this document.

## 1. Outcome

When a CONNECTION drop is followed immediately by a recovery — `Interrupted → Ready` events arriving already-buffered, processed by the consume loop without a render in between — a screen-reader user must hear **both** states: the DROP ("Connection status: Reconnecting") *and* the RECOVERY ("Connection status: Connected"). Today only the recovery is announced; the drop is silently overwritten before it reaches the DOM.

The fix must:

1. **Preserve pending DISTINCT announcements** until each has reached the live region — one committed render per message — instead of storing only the latest string.
2. **Keep the announce-once coalescing for REPEATED identical announcements** (a re-rendering list still announces once — the "announce-once seam").
3. **Drive the draining from the render loop without busy-looping** (self-terminating when the backlog empties).
4. **Not regress the room-count announce-once e2e.** A prior "defer content" attempt (#177 round 24) was reverted because it broke the room-count witness; the draining mechanism here must be validated against BOTH the connection drop/recovery e2e AND the room-count announce-once e2e.

## 2. Background: how announcements work today

### 2.1 The `Announcer` seam (`crates/jeliya-ui/src/components/live_region.rs`)

`Announcer` wraps a **single** `Signal<String>`. `announce(message)` coalesces against the current value: if the region already says exactly `message`, nothing changes and assistive tech is not re-triggered (`live_region.rs:24-32`). `message()` reads the signal so `LiveRegion` re-renders when it changes (`live_region.rs:36-38`).

There are two INDEPENDENT regions, provided once at the root by `use_announce_context()` (`live_region.rs:64-71`) and read via `use_announce()` (`live_region.rs:76-91`):

- `connection` — connection-lifecycle announcements (interruption / recovery).
- `content` — settled room count / terminal room-list failure.

Two regions exist so a content announcement and a connection announcement that fire in the SAME render do not clobber each other (`live_region.rs:41-53`). `LiveRegion` (`live_region.rs:97-112`) renders one stable, `visually-hidden`, `role="status"`, `aria-live="polite"`, `aria-atomic="true"` node whose text is the current message. Both nodes live OUTSIDE the boot/shell lifecycle conditional in `app.rs` (`app.rs:476-485`) so each is the SAME DOM node across boot↔shell transitions.

### 2.2 The connection announcer (`crates/jeliya-ui/src/app.rs`)

The `consume` async block (`app.rs:260-303`) folds every `ClientEvent` and, for a `StateChanged`, decides via `announces_connection_change()` (`app.rs:50-59`) whether the transition is a DROP (to `Interrupted`/`Failed`/`Stopped`) or a RECOVERY (back to `Ready` after such a drop, incl. the coalesced-through-problem case). If so, it calls `announcers.connection.announce(...)` with the localized `conn_announcement(status_word)` string (`app.rs:293-300`). English strings (`l10n/en.rs:100-118`, `l10n/wire.rs:22-31`):

- `Interrupted` → `"Connection status: Reconnecting"`
- `Ready` (recovery) → `"Connection status: Connected"`

The room-count announcer is a render effect (`app.rs:314-324`): it re-runs only when the room list or locale changes and relies on `announce`'s coalescing so a re-rendering list still announces once.

### 2.3 The mock event path that batches the pair

The e2e drives transitions through the marker-gated hook `window.__jeliyaE2eConnState(state)` (`compose.rs:250-284`), which calls `MockController::set_state` → `MockInner::transition` → `EventBus::broadcast(StateChanged{..})` (`mock/mod.rs:201-209`). Each subscription has its own bounded `VecDeque` buffer (`event.rs:240`, `event.rs:335-344`). So two rapid `set_state` calls both land in the subscriber's buffer, and the consume loop's `events.next().await` yields them back-to-back.

### 2.4 The exact bug

When `Interrupted` and `Ready` are both already buffered, the consume loop runs:

```
announce("Connection status: Reconnecting")  // sets the single signal
announce("Connection status: Connected")     // immediately replaces it
```

with no render (DOM commit) between them. The signal ends at `"…Connected"`. When Dioxus finally renders, the connection region only ever shows the recovery. The drop is never in the DOM, so it is never announced.

The existing connection e2e (`a11y.spec.ts:267-347`) sidesteps this by **explicitly waiting** for the drop text to render (`await expect(region).not.toHaveText("")`, `a11y.spec.ts:322`) before driving `ready`. This issue removes that crutch.

### 2.5 Why the room-count witness is the guardrail

The room-count e2e (`a11y.spec.ts:232-265`) asserts, via a mutation-**record**-based witness installed before boot (`a11y.spec.ts:30-72`), that the content region:

- **mounts exactly once and EMPTY** (no content present at node insertion — polite content present at insertion is not reliably announced), and
- receives `"0 rooms"` as **exactly one TEXT update**.

The reverted #177 round-24 "defer content" attempt broke this witness (see §5). Any draining mechanism MUST keep: node stable (mount == 1), mount empty, and each distinct message arriving as exactly one text update.

## 3. Root cause and design goal

**Root cause:** the announcement pipeline has a buffer depth of exactly one (the single `Signal<String>`). Two distinct writes between renders lose the first.

**Design goal:** give each region an ordered backlog of DISTINCT pending messages and advance the visible signal by exactly **one message per committed render**, so every distinct message occupies the live region for its own render frame. Preserve coalescing for consecutive identical messages so a re-rendering caller still announces once.

Invariant to maintain (the "announce-once seam", generalized):

> The sequence `displayed :: pending` never contains two equal consecutive entries.

This is what makes coalescing correct (a repeated announce is dropped) and makes every drain step change the visible text (so every step is its own DOM mutation).

## 4. Chosen approach: a per-region backlog drained by a render-loop effect

### 4.1 Data model (`live_region.rs`)

Replace each region's single `Signal<String>` with two signals:

- `displayed: Signal<String>` — the text `LiveRegion` renders (what `message()` returns). Starts empty.
- `pending: Signal<VecDeque<String>>` — DISTINCT messages waiting to reach `displayed`, in FIFO order. Starts empty.

`std::collections::VecDeque` is `std` (no new dependency; `unsafe_code` remains forbidden). `Announcer` stays `Clone + Copy + PartialEq` (both fields are `Signal<_>`, which are `Copy`).

```rust
#[derive(Clone, Copy, PartialEq)]
pub struct Announcer {
    displayed: Signal<String>,
    pending: Signal<VecDeque<String>>,
}
```

### 4.2 `announce` — enqueue distinct, coalesce consecutive

```rust
pub fn announce(&self, message: impl Into<String>) {
    let message = message.into();
    // Coalesce against the TAIL of the pipeline: the last still-pending message,
    // or the currently displayed text when nothing is pending. `peek` reads
    // without subscribing (announce is called from effects). A consecutive
    // identical announce (a re-rendering list) is dropped — the announce-once
    // seam; a DISTINCT message is enqueued so it is not lost when a later
    // announce follows before the first has rendered.
    let tail_is_message = {
        let pending = self.pending.peek();
        match pending.back() {
            Some(last) => *last == message,
            None => *self.displayed.peek() == message,
        }
    };
    if tail_is_message {
        return;
    }
    self.pending.write().push_back(message);
}
```

This preserves the `displayed :: pending` no-equal-consecutive invariant.

### 4.3 `drain_one` — advance by one, render-loop driven

```rust
/// Advance the region by ONE pending message. Reads `pending` (SUBSCRIBES) so
/// the effect re-runs when `announce` enqueues; writing `displayed` dirties the
/// scope that reads `message()`, which forces a render + DOM commit BEFORE this
/// effect can run again (Dioxus runs effects at lowest priority and returns from
/// the task pump as soon as an effect dirties a scope). So each pending message
/// reaches the DOM in its own render. When nothing is pending this writes no
/// signal, so the effect chain is self-terminating — never a busy loop.
fn drain_one(&self) {
    if self.pending.read().is_empty() {   // read -> subscribe (wake on enqueue)
        return;
    }
    if let Some(next) = self.pending.write().pop_front() {
        self.displayed.set(next);
    }
}
```

`message()` is unchanged in signature; it now reads `displayed`:

```rust
pub fn message(&self) -> String {
    self.displayed.read().clone()
}
```

### 4.4 Register the drains once, at the provider (`live_region.rs::use_announce_context`)

`use_announce_context()` is called exactly once, unconditionally, in `AppRoot` — the only place `LiveRegion` is rendered against these providers. Register one drain effect per region there (Rules of Hooks satisfied: unconditional, stable order):

```rust
pub fn use_announce_context() -> Announcers {
    let connection = use_context_provider(|| ConnectionRegion(Announcer::new_signals())).0;
    let content = use_context_provider(|| ContentRegion(Announcer::new_signals())).0;
    let announcers = Announcers { connection, content };
    // Render-loop-driven backlog drains: each re-runs when its region enqueues,
    // advances by exactly one, and forces a render before the next step.
    use_effect(move || announcers.connection.drain_one());
    use_effect(move || announcers.content.drain_one());
    announcers
}
```

Adjust the private newtypes `ConnectionRegion`/`ContentRegion` (`live_region.rs:57-60`) to carry an `Announcer` (two signals) instead of a bare `Signal<String>`; keep the distinct-type keying so Dioxus context does not alias the two regions.

`use_announce()` fallback (`live_region.rs:76-91`) — isolated component tests with no provider: build local signals with `use_signal` and register the two `drain_one` effects there too, unconditionally, so a component that renders its own `LiveRegion` and calls `announce` still drains. (This is a small, hook-order-stable addition; see §11 Risk R4 for the alternative of leaving the fallback undrained.)

### 4.5 Why this yields one message per render (Dioxus 0.7.9 scheduling)

Confirmed against `dioxus-core-0.7.9`:

- `use_effect` runs its callback via `queue_effect`, and re-runs it when a reactive read changes — but always **queued for the next render**, deduplicated (`dioxus-hooks-0.7.9/src/use_effect.rs:16-50`).
- In `VirtualDom::poll_tasks` (`dioxus-core-0.7.9/src/virtual_dom.rs:506-538`), effects are the LOWEST-priority work. After each `effect.run()`, the loop calls `queue_events()` and, **if any scope is now dirty, returns immediately** — before running further queued effects. `wait_for_work` then renders (commits the DOM) before the pump runs again.

So one `drain_one` run: pops the drop, writes `displayed` → dirties the scope reading `message()` → `poll_tasks` returns → **render commits the drop text** → next cycle re-runs `drain_one` (its `pending` read was invalidated by the pop) → pops the recovery → commit → next cycle finds `pending` empty → no write → chain terminates. Two distinct commits, drop then recovery. Because §3's invariant guarantees each popped value differs from the current `displayed`, every step changes the text and therefore gets its own commit.

### 4.6 What does NOT change

- `LiveRegion` component body and its DOM node — untouched. The node stays stable (mount once) and empty at insertion (because `displayed` starts empty). This is the property the room-count witness checks.
- `app.rs` call sites (`announce`, `message()`) — signatures unchanged; the consume loop and the room-count/terminal effects keep calling `announce`.
- The two-region separation and their `id`s (`live-region`, `connection-live-region`).
- `announces_connection_change` and the `coalesced_through_problem` handling — unchanged (rounds 25/26 are orthogonal).

## 5. Why the reverted round-24 "defer content" attempt broke room-count — and why this design does not

The round-24 attempt deferred content inside `live_region.rs` and was reverted for breaking the room-count witness. The witness fails if ANY of these happen: the region **remounts** (mount > 1), content is present **at insertion** (mount carries text), or the count arrives as **more than one** text update. Deferral schemes typically break one of these by seeding the visible value at mount, by toggling the node, or by emitting an empty→value flicker.

This design avoids all three by construction:

- `displayed` starts empty and `LiveRegion` is unchanged → **mount is empty, mount count stays 1**.
- The content region enqueues `"0 rooms"` once (subsequent identical announces coalesce) and the drain writes it exactly once → **one text update**, never re-announced.
- No node is toggled or re-keyed; only text content mutates.

**Mandatory guardrail:** the room-count e2e (`a11y.spec.ts:232-265`) MUST pass unchanged. Treat any change to its assertions as a red flag that this mechanism has regressed the announce-once seam.

## 6. Domain / API / security / observability impact

- **API surface:** internal to `jeliya-ui`. `Announcer`'s public methods (`announce`, `message`) keep their signatures. The struct's private fields change; `use_announce_context`/`use_announce` return types are unchanged (`Announcers`). No cross-crate or wire impact.
- **Data model:** a per-region in-memory `VecDeque<String>` bounded in practice by how many distinct transitions fire between two renders (a handful). See §11 R3 for an optional hard cap.
- **Validation / authorization:** none — presentation-only.
- **Security / privacy:** no new inputs, no `web-sys`/`cfg` added to shared components (Decision-3 preserved). The e2e hook stays marker-gated and inert in production (`compose.rs:257-266`).
- **Performance:** one extra `VecDeque` per region and at most one extra render per DISTINCT queued message. Bounded and self-terminating; no polling, no timers, no `requestAnimationFrame`.
- **Reliability:** the drain is quiescent when idle (parks on the `pending` subscription); no busy loop.
- **Migration / rollout:** none required; behavior-compatible except that rapid distinct connection announcements now each render.

## 7. Implementation steps

1. **`live_region.rs` — data model.** Add `use std::collections::VecDeque;`. Change `Announcer` to hold `displayed: Signal<String>` and `pending: Signal<VecDeque<String>>`. Add a private constructor (e.g. `Announcer::new_signals()` or inline in the providers) creating both signals.
2. **`live_region.rs` — `announce`.** Implement the tail-coalescing enqueue (§4.2) using `peek` (no subscription).
3. **`live_region.rs` — `message`.** Read `displayed` (§4.3).
4. **`live_region.rs` — `drain_one`.** Add the private render-loop drain (§4.3): subscribe to `pending`, pop one into `displayed`, no-op when empty.
5. **`live_region.rs` — newtypes + providers.** Update `ConnectionRegion`/`ContentRegion` to carry an `Announcer`. In `use_announce_context`, build both regions and register `use_effect(move || …drain_one())` for each (§4.4).
6. **`live_region.rs` — `use_announce` fallback.** Build local `displayed`/`pending` via `use_signal` and register the two `drain_one` effects unconditionally (§4.4), so isolated LiveRegion tests still drain.
7. **No `app.rs` change required** for the fix. (Optionally tighten comments at `app.rs:260-303` to note that distinct rapid connection announcements are now each rendered.)
8. **Unit tests** (`#[cfg(test)]` in `live_region.rs`): pure, signal-free tests of the pipeline invariant where practical (see §8.2). If signal-backed tests need a running scope, gate them behind the crate's existing test harness for hooks; otherwise extract the queue-decision logic into a pure helper (e.g. `fn coalesce_decision(tail: Option<&str>, displayed: &str, msg: &str) -> Enqueue|Skip`) and unit-test that helper directly.
9. **New e2e** in `a11y.spec.ts` (§8.1): batched drop→recovery WITHOUT a render wait, record-based witness.
10. **Verify** both e2e tests (§9) and the Rust unit suite.

## 8. Test strategy

### 8.1 New e2e — batched drop then recovery, no wait (the primary proof)

Add to `crates/jeliya-ui/e2e/a11y.spec.ts`, keeping the existing stepped test (§2.4) as the non-batched proof.

- **Witness:** install (before boot, on `document`) a mutation-**record**-based witness for `#connection-live-region`, mirroring the room-count witness (`a11y.spec.ts:44-72`) — record each `characterData` edit and each replacement text node as a separate `text` entry, and each region-node insertion as a `mount`. This is deliberately NOT the textContent-read witness the existing connection test uses (`a11y.spec.ts:283-299`): reading `textContent` inside the observer callback can miss an intermediate value if two commits fall in one observer microtask batch, which would make the test unable to detect the very drop it is meant to prove.
- **Arm** the marker (`localStorage['jeliya-e2e-boot-fixture']='1'`) and `gotoReadyShell(page)` so the shell reaches Ready and the connection hook installs.
- **Drive both transitions in ONE `page.evaluate`, synchronously, with no render wait between:**

  ```js
  await page.waitForFunction(() => typeof window.__jeliyaE2eConnState === "function");
  await page.evaluate(() => {
    window.__jeliyaE2eConnState("interrupted");
    window.__jeliyaE2eConnState("ready");
  });
  ```

  Both `set_state` calls broadcast before the consume loop yields, so the pair is buffered — the exact batched scenario.
- **Assert:** the region node is stable (`mount` count == 1) and the record witness logged **exactly two non-empty `text` announcements, in order**: the drop (`"Connection status: Reconnecting"`) then the recovery (`"Connection status: Connected"`). Use `expect.poll` on the log so the two renders have time to commit. A design that overwrites the drop yields ONE announcement (fails); a re-announce yields > 2 (fails).

Run under all four viewport projects (wide/medium/compact/narrow) with reduced motion forced, consistent with the file's other tests.

### 8.2 Rust unit tests (`live_region.rs`)

- **Coalescing / enqueue decision:** consecutive identical → skip; distinct → enqueue; identical to `displayed` when `pending` empty → skip; identical to `pending.back()` → skip; A,B,A sequence → all enqueue. (Test via the extracted pure helper per step 8, or via signal-backed hooks if the harness supports it.)
- **Invariant:** after any announce sequence, `displayed :: pending` has no equal consecutive entries, so every drain step changes `displayed`.
- Keep the existing `announces_connection_change` tests (`app.rs:500-565`) green — they are orthogonal.

### 8.3 Regression guard (mandatory)

- The room-count announce-once e2e (`a11y.spec.ts:232-265`) MUST pass with no edits (mount == 1, empty at insertion, exactly one `"0 rooms"` text update).
- The existing stepped connection e2e (`a11y.spec.ts:267-347`) MUST still pass.

### 8.4 Commands

From `crates/jeliya-ui/`: build the reproducible `dist/` per the crate's canonical pinned-wasm-bindgen build (see `specs/dioxus-web-jeliya-ui-crate-and-reproducible-build.md`), then run Playwright (`e2e/`, `npx playwright test a11y.spec.ts`). Run `cargo test -p jeliya-ui` for the unit suite. State explicitly in the PR which build/test commands were run and their results (honesty-first workflow).

## 9. Acceptance criteria

1. A new e2e drives `Interrupted` then `Ready` in a single synchronous `page.evaluate` **without waiting** for the interrupted text to render, and observes BOTH the drop and the recovery announcements (exactly two, in order) in `#connection-live-region`, with the region node stable (mount == 1).
2. The existing announce-once room-count e2e (`a11y.spec.ts:232-265`) still passes with no changes to its assertions (no regression of the announce-once seam).
3. The existing stepped connection e2e (`a11y.spec.ts:267-347`) still passes.
4. Repeated identical announcements still coalesce to a single render (unit test + the room-count e2e).
5. The drain is render-loop driven and self-terminating: no busy loop, no timer/`requestAnimationFrame`, quiescent when the backlog is empty.
6. `LiveRegion`'s DOM node stays stable and empty at insertion; no new `web-sys`/`cfg` in shared components; `unsafe_code` still absent; MSRV 1.91.
7. `cargo test -p jeliya-ui` green; `jeliya-ui` build reproducible/byte-identical as before.

## 10. Risks and mitigations

- **R1 — Two commits land in one observer batch, so the witness can't see the drop.** The mechanism forces a render (DOM commit) between pops via the `poll_tasks` early-return-on-dirty (§4.5); the record-based witness (§8.1) logs each characterData/text mutation separately even within one observer microtask. Mitigation is the specific reason §8.1 forbids the textContent-read witness.
- **R2 — Reintroducing the round-24 room-count regression.** Keep `LiveRegion` and its node untouched; start `displayed` empty; drain writes each distinct message exactly once. The room-count e2e is the fixed guardrail (§5, §8.3).
- **R3 — Unbounded backlog under a pathological flap storm.** In practice the queue holds only the distinct transitions fired between two renders (a handful). Optional hard cap: bound `pending` (e.g. keep the last N distinct, N≈8) and, on overflow, drop from the FRONT so the newest states survive — but only if a test demonstrates a realistic overflow; otherwise leave unbounded for simplicity and document the reasoning.
- **R4 — Effect self-retrigger becomes a synchronous drain (all messages in one render).** Guarded by Dioxus semantics: `drain_one` writes `displayed`, dirtying a scope, so `poll_tasks` returns before re-running the effect (§4.5). If a future Dioxus upgrade changes this, the batched e2e (§8.1) fails loudly (one announcement instead of two). Pin the behavior with that test; do not rely on undocumented timing.
- **R5 — Fallback path (`use_announce` with no provider) left undrained.** Registering the drains in the fallback (step 6) keeps isolated LiveRegion tests correct. If that proves noisy (an effect per consumer), the alternative is to document that the fallback provides the announce seam only and that DOM-asserting tests must go through a provider; choose the fallback-drain unless a test shows it harmful.
- **R6 — Locale-switch mid-backlog.** Messages are pre-localized strings captured at `announce` time; a locale switch does not retranslate queued items. This matches today's behavior (the single signal also held a pre-localized string) and is acceptable for transient connection notices.

## 11. Alternatives considered

- **Async drain task (`use_future` + a render-flush await).** A per-region task that pops one, sets `displayed`, then awaits the next render. Rejected: Dioxus 0.7.9 exposes no stable public `flush_sync`/`wait_for_next_render` in this crate's feature set (the only `flush_sync` references are internal to `dioxus-core`), and simulating "wait for commit" from a task is more fragile than the effect-driven chain, which the runtime already sequences correctly (§4.5).
- **Timer / `requestAnimationFrame` pacing.** Rejected: violates "no busy-looping / render-loop driven", adds `web-sys` to shared code, and couples announcement pacing to frame timing.
- **A single combined drain effect for both regions.** Workable (writing both `displayed`s in one run still commits both, on distinct nodes), but couples the regions; one effect per region is clearer and keeps each region independently self-terminating.
- **Store a `Vec` and render all pending at once in the node.** Rejected: `aria-atomic="true"` reads the whole node; concatenating messages would speak them as one utterance and breaks the one-message-per-announcement contract.

## 12. Rollout / rollback

- **Rollout:** presentation-only, no flags, no migration. Ship with the crate.
- **Rollback:** revert to the single-`Signal<String>` `Announcer`; the batched e2e (§8.1) is removed in the same revert. Because `announce`/`message` signatures are unchanged, callers in `app.rs` are unaffected either way.

## 13. Open questions

1. **Hard cap on `pending`?** Default recommendation: leave unbounded unless a test demonstrates a realistic overflow (R3). Confirm with the reviewer whether a defensive N-cap is wanted now.
2. **Fallback drain (R5).** Confirm the fallback should register drains (recommended) vs. documenting it as a non-DOM seam.
3. **Extract the coalescing decision to a pure helper?** Recommended for unit-testability without a running Dioxus scope (step 8 / §8.2). Confirm this is acceptable versus signal-backed hook tests only.
4. **Should the stepped connection e2e (`a11y.spec.ts:267-347`) be kept as-is or folded into the new batched test?** Recommendation: keep both — they prove the non-batched and batched paths respectively.
