# Dioxus/Web — Room Activity: timeline, composer, drafts, and pending-send state (#179)

**Issue:** #179 `[Dioxus][Web]: Port room Activity, timeline, composer, drafts, and pending-send state`
**Program:** #156 (Dioxus clean-slate). **Milestone:** M3 (shared web foundation), the first room-content slice on top of the #178 shell.
**Blocked by / depends on:** #178 (bootstrap/onboarding/shell/routing — merged; the room-shell skeleton this fills), #161 (typed protocol contract — the `jeliya-api` view models this renders), #169 (authoritative resync — the `Reconciler` + `RoomView`/`RoomUpdate` this consumes). Classifies retries through #168 (the bounded kernel's `CallError::execution`). The real browser transport is #171 (`WsWeb`); until it lands the surface renders against the deterministic mock exactly as #176/#178 do.
**Coordinated (not a prerequisite):** #91 (departed-room read-only archive) reuses this Activity surface; #179 ships the base Activity surface + the capability-gated read-only floor, and #91 owns the full archive contract.
**Authoritative product contract:** `docs/product-behavior-contract.md` §"No-fake-state rules", §"Required destinations" (Activity row), §"Status vocabulary and truthful states", §"Recency, unread, and attention", §"Retained product invariants" (invariant 5), §"Room identity and homonyms", §"Identity, aliases, and self label". `docs/room-workbench.md` and `docs/room-attention.md` are the retained data-model records.
**Architecture record:** `docs/dioxus-architecture.md` (Decision-3 no platform `cfg` in shared components, Decision-5 target composition, Decision-6 semantic primitives). Signed events are source truth; folding/grouping/filtering are reversible view state.
**Status:** SPEC — not yet implemented. This document is a build plan; it changes no production code.

---

## 1. Outcome and scope

Deliver the room **Activity** destination for the browser target: the signed timeline and the composer, rendered under `/rooms/:roomId/activity`, replacing the `RoomShell` "Activity" skeleton (`crates/jeliya-ui/src/components/room_shell.rs`) with a real pane. The pane projects the reconciler's authoritative `RoomView` into a truthful, gap-free, position-ordered timeline; folds agent-status runs and offers the view-only activity filters; renders a robust scroll anchor + "new activity" affordance that survives resize and route changes; drives a composer with autosize, platform-correct keyboard behavior, per-room drafts, and attachment handoff; and models each send as an **evidence-backed** `pending → syncing → synced` / `failed` lifecycle classified through the kernel's `CallError::execution`, never an invented "delivered".

The Activity surface is the room's canonical landing surface (contract §"Required destinations"). It reuses the already-shipped seams: `jeliya_client::ClientHandle` (the injected client), `jeliya_client::reconcile::Reconciler` (the authoritative per-room view, #169), and `jeliya_platform::PlatformServices` (drafts through `Preferences`, attachment picking through `Files` when #181 lands). No shared component gains a platform `cfg`; DOM measurement uses Dioxus's renderer-agnostic mounted element API.

### In scope

- **Timeline projection:** a total, exhaustive projection over the closed 10-kind signed event set into renderable rows — messages, agent-status (single + folded runs), membership syslines (created/joined/left/removed/invite-revoked), file-shared and pipe-published references, pipe-revoked syslines — with **no `return null`** silent drop of any signed fact. A kind without a bespoke card renders as an inspectable generic row.
- **Folding / grouping projections (reversible view state):** maximal same-author `agent_status` run folding (latest card + honest run evidence + expand/collapse), the five view-only activity filters (multi-select, empty = everything), day dividers, and 5-minute same-sender message compacting — all layered *on top of* the raw event list so the counter and scroll accounting keep counting the unfolded, unfiltered items.
- **Live pushes / reconcile:** consume `RoomUpdate::{Resyncing, Converged, Lagged}` from the `Reconciler`; render the authoritative converged timeline; surface the resync notice and local-loss marker honestly.
- **Scroll anchors + new-activity affordance:** stick-to-bottom, deliberate reading-position preservation across route change and remount, a "N new messages / N new activity" control that words itself by what the new items actually are, and resize/rotation/`display:none`-reveal robustness — via the Dioxus mounted element API.
- **Composer:** autosize (1 line → cap), desktop Enter-to-send / Shift+Enter newline vs. compact Enter-newline + explicit send button, per-room draft persistence, attachment handoff (paste/drop/pick) that **preserves the typed text on attachment failure**, and a send that clears the draft optimistically and **restores it on failure**.
- **Per-room drafts + composer height across route changes:** drafts keyed per room in the fresh `jeliya.dx.v1` browser preference namespace (session-scoped); composer height re-derived on remount.
- **Send state machine:** `pending` (call in flight) → `syncing` (daemon authored the event; awaiting the reconciler's committed row) → dropped when reconciled; or `failed` sub-classified by `CallError::execution()` (never-sent vs. may-have-executed), with a per-send Retry that reuses a stable `op_id` so the daemon deduplicates.
- **Room archive restrictions (capability-gated floor):** the composer and send are gated on the room's typed `MessageSend` capability, so a departed room (`Standing::Left`/`Removed`, no `message.send` capability) renders the signed timeline read-only with the signed left/removed fact stated plainly — the retained floor for invariant 5, with #91 owning the full archive contract.
- **l10n:** all copy through the #177 typed catalog (compile-enforced EN/FR parity), including the ported timeline/composer strings.

### Explicitly out of scope (non-goals, from the issue)

- **Inventing delivery/read receipts** — no "delivered", "seen", "read", or any queued/pending affordance that reads as confirmation (contract no-fake-state rule 1).
- **Hiding signed events permanently** — folding and filtering are reversible view state; nothing signed is deleted from the projection.
- **Files/Pipes implementation** (#181): file-shared and pipe-published events render as inspectable references with a disabled/absent "open in …" affordance until #181; the fetch/serve flow, `file.list` availability, and the Pipes pane are #181. The composer's attachment control is wired to the `Files` capability seam but degrades honestly to `Unavailable` until #181.
- **People / Agents-in-room / Fleet content** (#180) — Activity does not render the roster; it resolves author display names through the alias rules only.
- **Fixing unrelated network bugs**, inventing presence, or a second gap/catch-up path (the reconciler is the only resync path).
- **Reading or importing any legacy React storage** — no legacy draft key, no old signed log, no `?tab=` query. Fresh Dioxus draft + view state only (clean-slate cutover).
- **Cross-reconnect send-replay idempotency** at the daemon ledger level — that depends on #270's stable principal + incarnation fence; §10 states the honest boundary.

### Platform applicability

Shared room UI, **web-qualified first**. Every projection/fold/send-classification unit is written renderer- and web-sys-free (host-testable pure modules), so desktop (#184) and Android (#193) reuse it behind their own composition; DOM measurement rides Dioxus's renderer-agnostic mounted API, not `web-sys`.

---

## 2. What already exists vs. what #179 builds

| Concern | Already merged | #179 builds |
|---|---|---|
| Authoritative per-room view | `Reconciler::{new, activate_room, deactivate_room, subscribe, resume, run}`, `RoomView { room_id, generation, timeline: Vec<Event>, members, peers, reachability }`, `RoomUpdate::{Resyncing, Converged, Lagged}` (#169) | Wiring the reconciler into `AppRoot`, driving `run()`, activating the routed room, folding `RoomUpdate` into a per-room view signal |
| Typed events | `jeliya_api::Event { pos, event_id, at, author, kind: EventKindContent }`, closed 10-kind `EventKindContent`, `Author::{Resolved{subject_id,role,standing}, Unresolved}` (#161) | The exhaustive projection from `EventKindContent` → renderable rows |
| Send seam + classification | `ClientHandle::call::<MessageSend>` → `Result<MessageSendOut{room_id,event_id,pos,at}, CallError>`; `CallError::execution() -> Execution::{DefinitelyNot, Unknown, Definitely}`; `Dedup::{None, Key(OpId)}`; `message.send` in the op_id-deduplicated set (#168) | The pending/syncing/synced/failed model, op_id minting, capability-gated availability, Retry |
| Room shell frame | `RoomShell` (header + destination strip + per-destination skeleton), `RoomUnavailable`, routing (#178) | Replacing the Activity skeleton with the real pane; keeping the strip/header frame |
| Preferences | `Preferences` keyed by `PreferenceKey::Draft{room_id}` / `LastSeen{room_id}`, session-scoped `WebPreferences`, `jeliya.dx.v1` namespace (#174/#178) | Reading/writing per-room drafts and the last-seen mark through these keys |
| Alias / self display | contract §"Identity, aliases, and self label"; `PreferenceKey::{Aliases, SelfLabel}`, `shortId` discipline | Author display name resolution in timeline rows (self → `alias(selfId) ?? "You"`, peers → `alias(id) ?? shortId(id)`) |
| l10n catalog | `Catalog` trait, EN/FR parity gate, literal-copy scan (#177) | The ported timeline/composer catalog methods |
| Mock backend | `MockScript`/`Program::{reply_ok, reply_err, emit_then_reply, hang, local}`, `MockController::{deliver_next, pending_call, emit, set_state, drop_connection}` (#167) | Extending the compose fixtures to script the reconciler's baseline reads + `message.send` for the offline suite |

**The single largest new integration:** `AppRoot` today issues one `room.list` read and folds only lifecycle events (`crates/jeliya-ui/src/state.rs::apply_event` explicitly ignores `Push`/`Gap`/`ResyncRequired`/`Lagged`). #179 introduces the **reconciler** as the room-view source and a **per-room Activity view state** the Activity pane subscribes to. The seam's public event vocabulary does **not** change (the reconciler sits above it and owns its own `RoomUpdate` fan-out).

---

## 3. Owning modules and crate layout

Following the #178 split: pure, renderer-/web-sys-free **decision modules** unit-tested on the host, plus thin **Dioxus components** that render them and own DOM measurement through Dioxus's mounted element API.

```
crates/jeliya-ui/src/
  room/
    mod.rs               # RoomActivityState: the per-room view + pending model the pane renders
    projection.rs        # Event (10 kinds) -> RenderUnit rows; day dividers; compacting; sides
    runs.rs              # maximal same-author agent_status run folding (RunSummary) + activity filters
    send.rs              # SendEntry + SendPhase + classify(CallError) -> SendPhase; op_id minting
    reconcile.rs         # fold RoomUpdate -> RoomView signal + resync/loss notices; pending reconciliation
    scroll.rs            # pure scroll math: stick-to-bottom, restore, new-item accounting (no DOM)
  components/
    activity.rs          # ActivityPane: timeline list + new-activity control + composer; owns onmounted/onscroll/onresize
    timeline_row.rs       # the per-kind row renderers (message, agent status, run, syslines, file/pipe refs, generic)
    composer.rs          # Composer: autosize, keyboard, drafts, attach, send/retry
  l10n/{mod.rs,en.rs,fr.rs}  # +timeline/composer catalog methods (declared once, implemented in EN and FR)
```

- **`room/` is pure** (`jeliya-api` view models + std only): no `dioxus`, no `web-sys`, no `cfg`. It holds the *decisions* (what row a kind becomes, when two units group, how a pending entry classifies, how the new-item count is derived from counts/offsets). It is the host-tested core.
- **`components/activity.rs` and `components/composer.rs`** hold the *rendering and the DOM I/O*. Scroll offsets/sizes and autosize measurement flow through Dioxus's `MountedData` element API (`onmounted`, `onscroll`, `onresize`, `scroll_to`, and the element's scroll/size/rect accessors), which every Dioxus renderer implements — so these components stay free of `web-sys`/`cfg` (Decision-3). The `mounted` feature is already declared in `crates/jeliya-ui/Cargo.toml`.

### 3.1 Reconciler ownership and drive

The `Reconciler` is **derived from the injected `ClientHandle`**, not a new external dependency, so `AppRoot`'s "two separate injected inputs" seam (Decision-5) is preserved:

1. `AppRoot` constructs one `Reconciler::new(handle.clone(), ReconcileConfig::default())` via `use_hook` (once per app) and provides it (and a `use_context` accessor) to the subtree.
2. A `use_future` drives `reconciler.run()` for the app's lifetime (the driver never spawns; a single polled future is the contract). On the mock this is polled by the Dioxus runtime exactly as the existing `WebRoot` mock-drive future is; under `WsWeb` (#171) the adapter's event loop polls it.
3. The Activity pane, when mounted for `Route::Room { dest: Activity }`, calls `reconciler.activate_room(room_id, from_pos)` on mount and `deactivate_room(room_id)` on unmount, and holds its own `reconciler.subscribe()` `RoomUpdateSubscription` folded into the per-room view signal.

**Alternative considered (compose-injected reconciler as a third `AppRoot` prop):** rejected as the primary because the reconciler is a pure client-side construct over the same handle and adding a third top-level input widens the root seam without need; the `use_hook`+context form keeps the injection surface at two. If a target's event loop must own the drive future explicitly (e.g. #171/#173), composition may instead construct+drive the reconciler and pass its handle in — the pane code is identical either way because it only needs a `Reconciler` handle and a `RoomUpdateSubscription`.

---

## 4. Key design decisions

- **D1 — Signed events are source truth; every projection is reversible view state.** The reconciler's `RoomView.timeline: Vec<Event>` (gap-free, dedup-by-`event_id`, position-ordered) is the *only* history authority. Run-folding, the activity filter, day dividers, and 5-minute compacting are computed *on top of* it and never mutate or drop a signed fact — clearing a filter restores every row, collapsing a run hides nothing permanently (contract non-goal "Hiding signed events permanently"; issue "do not hide signed events permanently"). The counter and scroll accounting count the **unfolded, unfiltered** items, so folding/filtering can never rewrite the honest activity total (the exact React invariant in `buildRenderUnits`).

- **D2 — The projection over the closed 10-kind set is exhaustive and total; nothing signed is silently dropped.** The React `EventCard` had a `default: return null` — a silent drop. #179 replaces it with a `match` over `EventKindContent` that the Rust compiler forces to be exhaustive: adding an 11th kind to the protocol will not compile until the view decides how to render it. Any kind for which the design has not written a bespoke card renders as an **inspectable generic row** (author + signed time + a localized "kind" label + the safe metadata), never `null`. This is the honest reading of "render all known/unknown timeline events" within a *typed, closed* protocol: the view is total over the decodable set. Forward-compat for a genuinely undecodable future kind is a **codec/reconciler-boundary** concern — `EventKind` fails deserialization on an unknown kind, so such an event never reaches the view; the reconciler's `Input::DecodeFailed` routes it to an authoritative resync, not a silent hole (§10). #179 does not add an "unknown wire kind" view arm because the type system makes one unreachable; §12 O-Q1 records the boundary explicitly.

- **D3 — A send's state is exactly what the seam proves, and nothing more.** There is no optimistic "delivered". A `SendEntry` moves `Pending` (the `call` future is unresolved) → `Syncing` (the `MessageSendOut` returned: the daemon authored the event, `event_id`+`pos` known, but the reconciler has not yet surfaced the committed row) → **dropped** the instant the converged timeline contains that `event_id`. A failed call becomes `Failed(execution)` where `execution` is `CallError::execution()`: `DefinitelyNot` ("not sent", clean retry), `Unknown` ("may have sent", honest ambiguous copy, retry offered but never auto-taken), `Definitely` (the daemon ran it, only the local reply decode failed — treated as `Syncing`, since a committed row will arrive). This is the contract's "distinguishes never-sent work from work that may have executed" made a type-level fact through `Execution`.

- **D4 — Retry is idempotent by construction (stable `op_id`), and never automatic for may-have-executed work.** Each send mints one stable `OpId` (derived from the local `SendEntry` client id) and issues `message.send` with `Dedup::Key(op_id)`. `message.send` is in the protocol's op_id-deduplicated set, so a retry under the **same** `op_id` returns the daemon ledger's original `MessageSendOut` (the same `event_id`) and performs no second effect. The Retry affordance re-issues with that same `op_id`; pressing it after an `Unknown` failure therefore cannot create a duplicate on the same connection — and, once it returns the original `event_id`, the pending entry reconciles against the committed row (§10). The client **never auto-replays** a send (contract rule 7 / issue "no automatic replay of may-have-executed sends"); only the user's Retry re-issues, and only under the same op_id. Cross-reconnect ledger continuity depends on #270 (§10, R3).

- **D5 — Drafts and view state are fresh Dioxus state; no legacy import.** The per-room draft is `PreferenceKey::Draft{room_id}` in the `jeliya.dx.v1` namespace through the injected `Preferences` capability — **not** React's `localStorage['jeliya.draft.<roomId>']`. On the browser it is `Durability::SessionScoped` (dies with the tab, honest to D3 of #178). The reading position, expanded-run set, and activity filter are per-room session state living in the pane's signals, keyed by room so a room switch resets folding (a view, never a mutation). No legacy draft/log/view key is ever read (clean-slate cutover; a source-scan gate mirrors #178 §6.5's).

- **D6 — DOM measurement is a renderer-agnostic capability, not `web-sys`.** All scroll/size/rect reads and the autosize measurement go through Dioxus's `MountedData` element API surfaced by `onmounted`/`onscroll`/`onresize` and the returned element handle. The *math* (what to do with the offsets — stick, restore, count new items, cap autosize height) lives in the pure `room/scroll.rs`; the component only feeds it measured numbers and applies the result. This keeps `activity.rs`/`composer.rs` free of `web-sys`/`cfg` and lets desktop/Android reuse them unchanged (Decision-3/-6).

- **D7 — Fail safe, fail visible, fail truthful.** A room whose open failed shows the daemon's real (translated) error + Retry + Rooms, never a blank timeline. A still-loading room shows the route's Loading state, never an empty timeline (booting is *unknown*, not *zero* — contract §"Bootstrap"). A reconciler `Resyncing` notice is surfaced (non-blocking) so every resync cause is observable; a `Lagged` marker is surfaced honestly and recovered by the next converged view. A departed room states the signed left/removed fact plainly and permanently.

- **D8 — Capabilities gate actions, not disabled buttons scattered through UI.** Composer/send availability reads the room's typed `capabilities: Vec<CapabilityToken>` (`MessageSend` present ⇒ composer live). Absence suppresses the composer as a typed capability (invariant 5 floor), so a read-only archive is a capability outcome, not an `if departed { disable() }` sprinkled across the tree.

---

## 5. Event projection and grouping contract (`room/projection.rs`, `room/runs.rs`)

The projection turns `RoomView.timeline` (+ the pending sends) into ordered render rows. The React logic in `ui/src/components/Timeline.tsx` and `ui/src/lib/timelineRuns.ts` is the behavior reference; #179 re-expresses it against the typed v2 `Event`.

### 5.1 Render units

```
enum RenderUnit {
    Event(Event),        // one standalone signed event
    Run(AgentRun),       // >=2 consecutive same-author agent_status events, folded
    Pending(SendEntry),  // a local send not yet reconciled (always shown, never filtered)
}
```

`AgentRun` holds the ordered `Vec<Event>` (all `AgentStatus`, same resolved author, contiguous) plus a derived `RunSummary { count, first_at, last_at, latest: Event }`. `buildRenderUnits(view.timeline, pending, active_filters)`:

1. Filter the signed events to the active categories (empty set = everything). Filtering is a view; pending are **never** filtered (Retry must stay reachable).
2. Fold maximal same-author `AgentStatus` runs (`groupRuns` equivalent) over the filtered events.
3. Merge the always-shown pending entries back in by author-dated instant (`at`), stable-sorted.

The five activity categories and their kind membership (contract §"Recency" order): **conversation** (`Message`), **agent-runs** (`AgentStatus`), **membership** (`RoomCreated`, `MemberJoined`, `MemberLeft`, `MemberRemoved`, `InviteRevoked`), **files** (`FileShared`), **pipes** (`PipePublished`, `PipeRevoked`). The categories are declared once as a closed set so a new kind must be classified (compile-enforced).

### 5.2 Per-kind rendering (`components/timeline_row.rs`) — exhaustive, total

| `EventKindContent` | Row |
|---|---|
| `Message { body }` | Message bubble; own vs. remote side by author subject vs. self; sender name via alias rules; agent chip when the author role is agent; signed `at` as clock. Subject to 5-minute same-sender compacting. |
| `AgentStatus { label, progress }` | Agent-status card: sender, signed time, the closed-vocabulary `StatusLabel` routed through the wire→display seam (never translated; unknown label passes raw), real `Progress` only. Foldable into a run. **No `severity`** is rendered (it is not in signed content; attention severity is a projection #180 owns). |
| `RoomCreated { name }` | Sysline "room created by …". |
| `MemberJoined { subject_id, role }` | Sysline "… joined as {role}". |
| `MemberLeft { subject_id }` | Sysline "… left". |
| `MemberRemoved { subject_id, by }` | Sysline "… removed … by …". |
| `InviteRevoked { invite_id }` | Sysline "an invitation was revoked" (id disambiguated, never leaked as copy). |
| `FileShared { file_id, name, bytes, digest }` | File reference tile (name, size); "Open in Files" is present-but-inert until #181 (honest, not a fake action). |
| `PipePublished { pipe_id, target, audience }` | Pipe reference tile; "Open in Pipes" inert until #181. |
| `PipeRevoked { pipe_id }` | Sysline "a pipe was revoked". |

`Author::Unresolved` renders with no attribution asserted (the localized unresolved sender), never a fabricated role or name (contract: "nothing is asserted"). A future kind added to `EventKindContent` (compile error until handled) gets the **generic inspectable row**: sender + signed time + a localized kind label + safe metadata — never dropped.

### 5.3 Day dividers, compacting, sides

Ported verbatim from React: a day divider precedes the first unit of each new calendar day (in the resolved formatting locale); 5-minute same-sender consecutive `Message` (and own-pending) units render compact (avatar/meta suppressed); side is `own` (self author or pending), `remote` (other author of a message/agent-status/file/pipe), or `system` (syslines, runs never group). Runs and non-message events never group and never let a neighbor group into them (the React `unitMessageSender = null` rule).

---

## 6. Send state machine (`room/send.rs`, `components/composer.rs`)

```
struct SendEntry { client_id: SendId, op_id: OpId, body: String, at: Timestamp, phase: SendPhase }
enum SendPhase {
    Pending,                  // message.send future in flight
    Syncing { event_id },     // MessageSendOut returned; awaiting the committed row (or DecodeReply => Definitely)
    Failed { execution, code } // CallError; execution = DefinitelyNot | Unknown; code = optional translated wire code
}
```

Flow:

1. **Compose → send.** On send, mint `client_id` (a fresh local id) and a stable `op_id` derived from it. Trim the body; refuse empty/whitespace, refuse when the composer is disabled or the room lacks `MessageSend` capability. Optimistically clear the draft (and its `Preferences` key), push a `SendEntry{ Pending }`, and dispatch `handle.call::<MessageSend>(MessageSend{ room_id, body }, Dedup::Key(op_id))`.
2. **On `Ok(MessageSendOut{ event_id, .. })`** → set `Syncing { event_id }`. The entry stays visible (a spinner-free "sending…/syncing…" honest label, **never** a checkmark implying delivery) until the reconciler's converged timeline contains `event_id`, at which point the entry is **dropped** (§10). A `Local(DecodeReply)` failure (`execution() == Definitely`) is also treated as `Syncing` with an unknown `event_id` — the daemon ran it, so the committed row will arrive and the entry reconciles by absence (§10 fallback).
3. **On `Err(e)`** → set `Failed { execution: e.execution(), code: e.as_wire()… }`. Restore the draft **only if** the composer is still empty and the user has not typed a new draft (never clobber fresh input). Render the honest failure copy: `DefinitelyNot` → "not sent" + Retry; `Unknown` → "may not have sent" + Retry (with the ambiguity stated); a `Wire` refusal → the translated daemon code + a way forward.
4. **Retry.** Re-issue `message.send` with the **same** `op_id` (D4). Move the entry back to `Pending`. Each failed send is retried individually (its signed `at` names which send, exactly as the React `aria-label={s.timelineRetryMessageAt(time)}`), because several sends can fail in one timeline.

Pending entries are per-room session state (dropped on room switch is **not** acceptable if still unresolved — see §10 O-Q3 for the room-switch policy; the default is to keep unresolved sends in a per-room map so returning to the room still shows them).

---

## 7. Scroll anchoring and the new-activity affordance (`room/scroll.rs`, `components/activity.rs`)

The React `Timeline` scroll machine is ported as **pure math** fed by Dioxus-measured numbers. `room/scroll.rs` owns a `ScrollModel` with the state the React refs held — `stick_to_bottom`, `last_scroll_top`, `restore_pending: Option<SavedView>`, `reload_baseline: Option<usize>`, `new_item_count` — and pure transitions:

- `on_scroll(offset, scroll_height, client_height) -> ScrollAction` — sets `stick_to_bottom` when within the 140px bottom threshold; clears `new_item_count` at the bottom.
- `sync(measure) -> ScrollAction` — the single place a target scroll offset is decided: restore a saved reading position once the reloaded backlog is in, stick to bottom, or reinstate the reading spot after a `display:none` reveal zeroed the measurement (the compact-pane case).
- `on_items_changed(count, loading) -> ScrollAction` — live-delta new-item accounting, with the **reload baseline** so a wholesale backlog reload on reconnect is not announced as new activity, and the restore path deriving the count from the saved view.
- `save_view() -> Option<SavedView>` — the deliberate reading position (`scroll_top`, `item_count` seen) persisted across the room switch; a room left at the bottom re-opens at its newest event.

`components/activity.rs`:
- Uses `onmounted` to capture the scroller element handle, `onscroll` to feed `on_scroll`, and `onresize` (the Dioxus element resize event, the `ResizeObserver` equivalent) to re-run `sync` on layout changes (hidden→visible, rotation, composer growth).
- Applies each `ScrollAction` by calling the element handle's `scroll_to`.
- Persists `save_view()` into a per-room map (a `use_context` App-level signal, so it survives the keyed remount on room switch and the compact `display:none` hide) and re-seeds `restore_pending` on return.
- Renders the "**N new messages**" / "**N new activity**" control (worded by what the trailing `new_item_count` items actually are — messages-only vs. mixed) that jumps to the newest event with reduced-motion honored.

**Resize/navigation survival (AC):** the saved reading position and the new-activity count are keyed by room in App-level state, not in the pane's local signals, so a resize (which may swap the shell and remount the pane) and a route change both restore the same position and count. The composer height is re-derived on remount from the restored draft (autosize is a measurement, not stored geometry), so it "survives" by reconstruction.

---

## 8. Composer (`components/composer.rs`)

Ported from `ui/src/components/Composer.tsx`, re-expressed against the seams:

- **Draft:** on mount, read `Preferences::get(Draft{room_id})`; on every edit, write it (empty clears). Session-scoped on the browser; the composer honestly does not imply cross-tab persistence.
- **Autosize:** on `oninput` and on `onresize` (width change only, to avoid the height-feedback loop), measure the element's scroll height via the mounted handle and apply `min(scroll_height, cap)`; inside a `display:none` pane (measurement 0) fall back to the stylesheet's one-line height and let the resize event re-measure on reveal — the exact React fallback.
- **Keyboard (platform-correct):** the injected `Shell` value (already in `AppRoot`, #178 §8) decides: on **non-compact** (desktop-like) Enter sends and Shift+Enter is a newline; on **compact** Enter is always a newline and the explicit send button is the only send (the soft-keyboard newline key). The "Enter to send" hint is withheld on compact where the claim is false. This uses the injected `Shell`, never a `cfg`/user-agent sniff.
- **Attachment handoff (text-preserving):** paste-with-files, drag-drop, and the pick button route to the `Files` capability (#181). On the web target until #181 the control degrades to an honest `Unavailable`/absent affordance. Critically (issue security note): **an attachment failure never clears or corrupts the typed text** — sharing and sending are independent; a share-in-flight never blocks a send nor the reverse (the React `sharing` vs. `sending` separation).
- **Send/Retry:** as §6. On send failure the draft is restored (guarded against clobbering fresh input).
- **Capability gate (D8):** when the room lacks `MessageSend`, the composer renders its read-only/suppressed state (the signed left/removed fact for a departed room), not a disabled textarea.

---

## 9. Reconcile wiring and the last-seen / unread mark (`room/reconcile.rs`)

- **Activate/subscribe/fold.** On Activity mount: `activate_room(room_id, from_pos)` (from_pos = 0 for a first open — the reconciler pages history to Complete; the last converged `pos` on a re-open). Hold a `RoomUpdateSubscription`; fold into a `RoomActivityState { view: Option<RoomView>, resyncing: bool, lost: Option<u64> }`. `Converged(view)` (coalesced per room, latest wins) replaces the view; `Resyncing` sets a non-blocking notice (so every resync cause is observable, #169 AC-1); `Lagged` sets an honest local-loss marker recovered by the next converged view.
- **No-duplicate guarantee (AC).** The reconciler timeline is already gap-free and dedup-by-`event_id`. #179's only new dedup is the **pending↔committed** reconciliation: a `SendEntry` in `Syncing { event_id }` is dropped when `view.timeline` contains `event_id`. Because the committed event is authoritative and positioned, a reconnect/resync that re-baselines the room re-delivers the same committed row (same `event_id`) — the pending was already dropped, so no duplicate appears (§10 covers the ambiguous-execution case).
- **Last-seen / unread (device-local, never on the wire).** The Activity pane advances `PreferenceKey::LastSeen{room_id}` to the room's newest signed `at` on view (contract §"Recency, unread, and attention"); it is seeded to the room's recency when the room first appears with events; it never implies "seen/delivered/read". The room-list dot (not a count) is #179's Rooms surface concern already; Activity owns advancing the mark. Live activity moves recency only forward and never seeds the baseline.

---

## 10. Reconnect/resync correctness and the ambiguous-send boundary

The "no duplicate messages" and "no automatic replay of may-have-executed sends" ACs meet here; the honest boundary is stated explicitly.

- **Clean send, same connection.** `message.send` returns `MessageSendOut{event_id}`; the pending goes `Syncing`; the reconciler's converged timeline surfaces `event_id`; the pending drops. One row. No duplicate on reconnect (the committed row is authoritative and re-delivered under the same id).
- **Ambiguous send (`Disconnected/Timeout` ⇒ `Unknown`).** The client cannot tell whether the daemon authored the event. #179 does **not** auto-replay (contract rule 7). The entry shows `Failed{Unknown}` ("may not have sent") with Retry. Two truthful sub-cases:
  - *It did not execute.* Retry (same `op_id`) authors the event; pending → `Syncing` → reconciled. One row.
  - *It did execute.* On the **same** connection, the reconciler surfaces the committed row (with an `event_id` the pending never learned) — so momentarily the honest state is the committed message **plus** a "may have sent — retry?" affordance. Retry (same `op_id`) hits the daemon ledger, returns the **original** `event_id`, the pending learns it and reconciles against the already-shown committed row → the affordance resolves to the single real row. This is the contract's "distinguishes never-sent from may-have-executed": we surface the ambiguity rather than fake a resolution, and the op_id retry is the deterministic way to collapse it. #179 may additionally auto-resolve an `Unknown` pending when a committed `Message` row from **self** with the same `body` and a close `at` appears before retry; this heuristic is **optional and conservative** (O-Q2) — the op_id retry is the guaranteed path.
- **Cross-reconnect ledger continuity (#270).** The daemon dedup ledger is keyed `(session principal, op_id)`. Until #270 lands a stable principal + incarnation fence, a retry issued on a **new** connection may not match the ledger and could re-execute. Therefore: #179 keeps the retry available, but the *guaranteed* idempotency is same-connection; cross-reconnect idempotency is a #270-gated qualification (R3). The kernel's own bounded auto-replay is `ReplayPolicy::Never` today for the same reason (`stable_principal=false`), so #179 must not assume the kernel silently retried a send.
- **DecodeFailed / undecodable future kind.** A wire event whose kind this build cannot decode never reaches the view (typed closed set); the reconciler's `Input::DecodeFailed` path re-baselines authoritatively. #179 surfaces the resulting `Resyncing` notice; it does not attempt to render an undecodable event (O-Q1).

---

## 11. Security and correctness

- **No fabricated delivery/read state.** No send ever shows a "delivered"/"seen"/checkmark; `Syncing` copy states the honest fact (the daemon authored it; it is not a receipt). Unread never implies anyone read anything (contract §"Recency").
- **No silent drop of signed facts.** The projection is exhaustive; folding/filtering are reversible; a source-scan test asserts no `return null`/silent-drop path exists for a signed kind.
- **No auto-replay of may-have-executed work.** Only user-initiated Retry re-issues, only under the same op_id; the client never resubmits a send on the caller's behalf (contract rule 7).
- **Attachment failure preserves text.** Share and send are independent; a failed/`Unavailable` attach never clears or mutates the composed draft (issue security note).
- **Identifier hygiene.** Event/file/pipe/invite ids never enter localized copy or error strings (the `CallError` display already redacts payloads §K15); ids shown to the user are shortened, mono, copyable, described as opaque — never assumed hex.
- **Bounded by construction.** The reconciler is byte- and count-bounded (`ReconcileLimits`); the pending map, expanded-run set, and per-room saved-view map are keyed by the bounded active-room set; the projection allocates from the already-bounded `RoomView.timeline`. No unbounded UI growth.
- **Fresh state only.** No legacy draft/log/view/query is read; drafts and view state are session-scoped fresh Dioxus state (clean-slate cutover).
- **Truthful failure surfaces.** Failed open → real translated code + Retry + Rooms; departed room → signed fact stated plainly, composer suppressed as a capability; booting → Loading, never an empty timeline.

---

## 12. Test strategy

Canonical gate: `cargo` host tests + the wasm/Playwright web suite established in #176/#177/#178 (`crates/jeliya-ui/e2e/*`, the wasm-graph guard, the l10n/token gates). Focused-first: run the pure `room/` host tests and the one relevant e2e spec before the full matrix.

### 12.1 Host unit tests (pure `room/` modules, `cargo test -p jeliya-ui`)
- **Projection:** each of the 10 kinds → the expected `RenderUnit`/row shape; `Author::Unresolved` asserts nothing; a synthetic "future kind" (added behind a test-only enum shim, or asserted structurally) proves the match is exhaustive and yields the generic row — no `null`.
- **Runs/filters:** maximal same-author run folding (2+, boundary at author change and non-status neighbor); `RunSummary` count + first/last span; activity filter membership (empty = all; each category; pending always survive filtering); folding/filter never change the unfolded/unfiltered count.
- **Day dividers / compacting / sides:** divider at each day boundary; 5-minute same-sender compacting; runs and syslines never group; side classification (own/remote/system) including pending = own.
- **Send classification:** `classify(CallError)` → `Failed{DefinitelyNot}` for `Wire`/`QueueFull`/`EncodeRequest`/never-sent `Cancelled`/`Disconnected`; `Failed{Unknown}` for `Timeout`/sent-`Disconnected`/`Cancelled{Unknown}`/`Backend`; `Syncing` (Definitely) for `DecodeReply`; `MessageSendOut` → `Syncing{event_id}`. op_id stable across a retry of the same entry.
- **Pending reconciliation:** a `Syncing{event_id}` entry drops when a converged timeline contains `event_id`; two failed sends retry independently; an `Unknown` entry co-existing with a committed self-row resolves on op_id retry (and, if enabled, the optional body+at heuristic).
- **Scroll math:** stick-to-bottom threshold; restore-once-loaded; reload-baseline suppresses whole-backlog "new" on reconnect; new-item live delta; `display:none`-reveal reinstatement; `save_view`/restore round-trip. (Pure numbers, no DOM.)
- **Last-seen/unread:** baseline seeding on first-appearance-with-events; advance-on-view; `absent` last-event → no unread/attention.

### 12.2 Component/render tests (mock + `WebPlatform` in-memory)
- Activity pane renders a scripted converged timeline; a `message.send` against the mock drives `Pending → Syncing`, and a scripted converged view containing the returned `event_id` drops the pending (echo-before-response ordering via `Program::emit_then_reply`).
- A scripted `message.send` error drives `Failed{execution}` and restores the draft.
- Composer draft round-trips through `Preferences`; keyboard behavior differs by injected `Shell`.

### 12.3 Browser e2e (Playwright, `crates/jeliya-ui/e2e/`)
Extend the marker-gated fixture protocol in `compose.rs` (the `?boot=`/`?rooms=`/`?onboard=` family) with a **timeline fixture** that scripts the reconciler's baseline reads (`stream.subscribe`, `room.timeline`, `room.members`, `room.peers`) and `message.send`, so the offline suite drives Activity deterministically:
- **Ordering/grouping contract:** a scripted mixed timeline renders the required order, run folding, day dividers, compacting, and filters (clear-filter restores all).
- **Echo-before-response:** the daemon's committed push arrives before the `message.send` reply; assert exactly one row and no duplicate (`emit_then_reply`).
- **Reconnect/resync no-duplicate:** drive `drop_connection` + a re-baseline; assert the message set is unchanged and no row duplicates; the `Resyncing` notice is observable.
- **Long timeline:** a large scripted backlog scrolls, sticks to bottom, and shows the new-activity control on a live push while scrolled up; the count words correctly for messages-only vs. mixed.
- **Resize/navigation survival:** scroll up, resize across the 900/1280 breakpoints and navigate away and back; assert the reading position and new-activity count survive; assert the per-room draft and composer height survive a route change (EN and FR — French copy first for overflow).
- **Keyboard/mobile newline:** desktop Enter sends / Shift+Enter newline; compact Enter newline + explicit send.
- **Failure recovery:** a scripted send error preserves the text and offers Retry; an attachment failure preserves the text.

### 12.4 Real-daemon (supervised `jeliyad`, once #171 lands)
Echo-before-response, reconnect, long-timeline, resize, and retry against a real daemon — the issue's Verification list. Until #171, these run against the mock (offline suite) and are re-qualified live under #182; the PR states this honestly (R2).

---

## 13. Acceptance-criteria traceability

| AC (issue) | Where satisfied |
|---|---|
| Event ordering/grouping matches the behavior contract without dropping signed facts | §5 (exhaustive projection, folding as reversible view state), D1/D2; tests §12.1/§12.3 |
| Pending/synced/failed states are evidence-backed | §6, D3 (`CallError::execution`), §10; no fabricated delivery (§11); tests §12.1/§12.2 |
| Reconnect/resync produces no duplicate messages | §9 (reconciler dedup + pending↔committed reconciliation), §10; tests §12.3 |
| Scroll position and new-activity behavior survive resize/navigation | §7 (pure scroll model + App-level per-room saved view + Dioxus resize/mount events); tests §12.3 |
| Per-room drafts and composer height survive route changes | §8, D5 (`Draft{room_id}` prefs + autosize re-derivation); tests §12.3 |
| Keyboard/mobile newline behavior and failure recovery match existing tests | §8 (injected `Shell` keyboard fork; text-preserving failure/attach); tests §12.3 |

---

## 14. Risks

- **R1 — Reconciler integration surface.** #179 introduces the reconciler as the room-view source and must extend the mock fixtures to script its baseline reads, or the offline suite cannot render Activity. Mitigation: a dedicated timeline fixture (§12.3) mirroring the `?boot=`/`?rooms=` marker-gated pattern; keep the reads scripted and event-driven (`pending_call`/`deliver_next`) as `compose.rs` already does.
- **R2 — No `WsWeb` (#171) yet.** The "real-daemon echo/reconnect/retry" verification is only fully exercised once #171 provides the live `Hello`/pushes. #179 builds and reviews against the mock; the live re-qualification is #182. State this in the PR.
- **R3 — Cross-reconnect send idempotency needs #270.** The op_id retry is guaranteed idempotent same-connection; across a reconnect the daemon ledger match depends on #270's stable principal + incarnation. Mitigation: keep Retry available with honest ambiguity copy; do not claim cross-reconnect idempotency; gate the live cross-reconnect-retry test on #270 (§10).
- **R4 — Dioxus mounted element API coverage.** The scroll/autosize logic relies on the `MountedData` element handle (scroll offset/size/rect + `scroll_to`) and the `onresize` element event under `dioxus = 0.7.9` (`mounted` feature). If a needed accessor is missing on the web renderer, fall back to a narrowly-scoped injected measurement capability (a `PlatformServices`-style seam) rather than `web-sys` in the shared component. O-Q4.
- **R5 — Scope creep into #180/#181.** Files/Pipes panes, the roster, and attention severity are out of scope; file/pipe events render as inert references and the attach control degrades to `Unavailable`. A review checklist item guards against implementing the fetch/serve flow here.
- **R6 — "Unknown event" language vs. the typed closed set.** The issue's "known/unknown events" can be misread as needing an unknown-wire-kind view arm, which the typed model makes unreachable. Mitigation: D2 states the exhaustive-total interpretation and routes true forward-compat to the codec/reconciler boundary; a test proves the projection is total and drops nothing.

## 15. Open questions

- **O-Q1 (owner: #161/#207 + #179):** the exact behavior when the daemon emits an event kind this build cannot decode. Recommendation: the reconciler's `DecodeFailed` → authoritative resync is the mechanism; #179 surfaces the `Resyncing` notice and does not render an undecodable event. Confirm no resync-loop on a persistently-undecodable event (a codec/version concern, not the view's).
- **O-Q2:** whether to enable the optional heuristic that resolves an `Unknown` pending against a matching committed self-`Message` (same `body`, close `at`) before the user retries. Recommendation: keep it **off** by default (the op_id retry is the guaranteed path); if enabled, make it conservative and reversible, never collapsing two genuinely distinct sends.
- **O-Q3:** the room-switch policy for unresolved pending sends. Recommendation: keep unresolved sends in a per-room App-level map (like the saved view), so returning to the room still shows a `Pending`/`Failed{Unknown}` entry — dropping them on switch would hide may-have-executed work.
- **O-Q4:** whether all needed scroll/measurement accessors exist on the Dioxus 0.7.9 web renderer, or whether a small injected measurement capability is required (R4). Resolve by a spike against the `mounted` API before building `activity.rs`.
- **O-Q5:** how much of the departed-room read-only archive #179 renders vs. #91. Recommendation: #179 ships the capability-gated floor (signed timeline read-only, composer suppressed via absent `MessageSend`, the signed left/removed fact stated plainly); #91 owns the full archive contract (historical roster, suppressed Invite/Leave/file/pipe as typed capabilities, "no live networking" guarantee). The base Activity surface must not block on #91.
- **O-Q6:** `from_pos` for `activate_room` on a first open vs. re-open. Recommendation: 0 on first open (reconciler pages to Complete), the last converged `pos` on re-open; confirm against the reconciler's bootstrap paging (#169 "bootstrap pages to Complete, recovers the subscribe→activate race").

## 16. Non-goals (restated, to bound the work)

Delivery/read receipts or any confirmation-implying pending affordance; permanently hiding signed events; Files/Pipes implementation (fetch/serve/list, the Pipes pane) — file/pipe events are inert references; People/Agents-in-room/Fleet content and attention severity; fixing unrelated network bugs; reading or importing any legacy React draft, signed log, view state, or `?tab=` query; cross-reconnect send-replay idempotency (a #270-gated qualification); desktop/Android qualification of this surface (#184/#193, qualified #182/#189/#194).
