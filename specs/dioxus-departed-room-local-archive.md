# Dioxus — Open departed rooms as local read-only historical archives (#91)

**Issue:** #91 `[UX][Dioxus][v2]: Open departed rooms as local read-only historical archives`
**Program:** #156 (Dioxus clean-slate). **Milestone:** shared Dioxus room surface (surfaces on #182 web; re-qualified #189 desktop, #194 Android).
**Blocked by / depends on:** behavior contract #162 (`docs/product-behavior-contract.md`); typed projections/engine #165/#166 (which already ship the `room.archive` operation and the departed-room `capabilities` rule); the applicable room surface beginning with **#179** (`RoomShell`, the Activity timeline row, the room header). #171 (`WsWeb`) supplies the real `ClientHandle` transport that replaces the deterministic mock; until then this surface renders against the mock, exactly as #178–#181 do.
**Downstream verification:** #195 consumes this issue's completed archive contract and per-platform evidence. **#195 is not a prerequisite for implementing or closing #91.**
**Authoritative product contract:** `docs/product-behavior-contract.md` — Required-behavior rule **5** ("Departed rooms can be opened as explicit local read-only historical archives"), Retained-invariant **3** (the deliberate widening of the Room Workbench "state the signed fact, do not open" floor), §"Membership, presence, file-provider availability, and Pipe reachability are distinct facts".
**Authoritative protocol:** `docs/protocol-v2.md` — `room.archive`, `room.members`, `room.list`, the `capabilities` "present iff not refused" rule, and the **validation-order step-5 standing exemption** (only `room.archive` and `room.list` survive a departed standing).
**Architecture record:** `docs/dioxus-architecture.md` — Decision-3 (layering / no platform `cfg` in shared components), Decision-5 (target composition selected only at the crate root), Decision-6 (semantic primitives).
**Status:** SPEC — not yet implemented. This document is a build plan; it changes no production code.

---

## 1. Outcome and scope

Let a user open a room they **left** or were **removed from** as an explicit, local, **read-only historical archive** — the signed timeline and a historical roster reconstructed from that timeline, the signed left/removed fact stated plainly and permanently, and every live action suppressed **by absent capability** — **without starting any networking** and without implying current access.

This is the deliberate widening of the retained Room Workbench floor ("a departed room states the signed fact and does not open"). #91 designs the surface that lets it open truthfully.

### In scope

- Select a **standing-driven archive composition** for a departed room, distinct from the live room surface: a departed room dispatches **only** the typed `room.archive` read and never `room.activate`, `stream.subscribe`, `room.timeline`, `room.members`, `room.peers`, `file.*`, or `pipe.*`.
- Render the locally held **signed timeline** (reusing #179's Activity event rows) and a **historical roster reconstructed from the signed membership events** in that timeline.
- State the signed **left/removed** fact **plainly and permanently** — a non-dismissable banner that also explains that rejoining requires a fresh invite.
- Suppress the composer, **Invite**, **Leave**, **file share/fetch**, and **Pipe** actions as **typed capabilities** — affordances **absent**, not disabled buttons scattered through UI code.
- Start **no** Iroh node, peer dialing, discovery, sync, heartbeat, or peer-hint mutation, and perform **no** durable mutation.
- Preserve truthful archive behavior across **direct routes / deep links** and **same-install restart**.
- Keep every unit of logic **target-agnostic** so the web (#182), desktop (#189), and Android (#194) shells inherit identical facts behind their own `PlatformServices`; #195 records the platform rows.

### Explicitly out of scope (non-goals, from the issue)

- Importing or opening **legacy v1** archives. Only archives created by the new protocol/storage generation are supported; **no** old log reader and **no** migration path.
- Continuing **live synchronization** after departure.
- Reusing the normal **writable** room surface with disabled actions scattered through UI code.
- Implying that historical peer state is **current**.
- **Automatically deleting** the local archive.
- Defining a **new** protocol operation. `room.archive` already exists (#165/#166); #91 is the client seam + Dioxus surface that consumes it.

### Platform applicability

Shared Dioxus behavior; implemented per applicable platform. The archive projection, roster fold, capability gate, and pane are pure Rust in `jeliya-ui`; the surface first ships on the web target (#182) and is re-qualified on desktop (#189) and Android (#194) with no per-platform logic fork.

---

## 2. What already exists vs. what #91 builds

The typed **command** and the daemon **capability rule** are already defined by #165/#166 and the protocol. #91 does **not** redefine them; it consumes them.

| Concern | Already defined (do not redefine) | #91 builds |
|---|---|---|
| Archive command | `jeliya_api::RoomArchive { room_id, page }` → `RoomArchiveOut { room_id, standing, events, truncated }`, `MUTATING = false`, "normatively **zero network activity and zero durable mutation**", errors `room_still_active` on an active room (`crates/jeliya-api/src/ops.rs`; `docs/protocol-v2.md §room.archive`) | A `ClientHandle::room_archive` convenience wrapper + the paged read loop |
| Capability token | `CapabilityToken::RoomArchive` (the token **is** the op name) | The UI gate that renders each affordance **iff** its token is present |
| Departed-room capabilities | Protocol rule: a token is present **iff** the op would not be refused on membership/standing/lifecycle/liveness. For a departed room every standing-gated op is refused, so the served array is effectively `["room.archive"]` (`docs/protocol-v2.md §room.list`) | A conformance test that fixes this expectation and a gate that trusts it |
| Standing | `RoomRow.standing: Standing::{Active, Left, Removed}` on every `room.list` row (`crates/jeliya-api/src/shared.rs`) | The composition selector: `Active` → live room surface (#179), `Left`/`Removed` → the archive pane |
| Signed events | `jeliya_api::Event` + closed `EventKindContent` (ten kinds, incl. `MemberJoined{subject,role}`, `MemberLeft{subject}`, `MemberRemoved{subject,by}`, `RoomCreated{name}`) (`crates/jeliya-api/src/types.rs`) | The **roster fold** over these events + the departure-fact reader |
| Room surface | `RoomShell` + the room header + the Activity timeline row (#179) | The archive pane that reuses the timeline row and replaces the live nav strip |
| Route model | `Route::Room { room_id, dest }` / `RoomDest` (`crates/jeliya-platform/src/navigation.rs`) | Standing-driven, **dest-agnostic** archive selection in the `Route::Room` arm of `AppRoot` |

**The one hard design constraint (D3 below):** the historical roster **cannot** come from `room.members`. Per the protocol's normative validation order (`docs/protocol-v2.md`), the standing stage refuses every room-scoped op with `membership_ended` **except** `room.archive` and `room.list`. `room.members` and `room.timeline` are **not** exempt, so a departed caller reading either gets `membership_ended`. The archive's roster is therefore **reconstructed from the signed membership events carried in `RoomArchiveOut.events`** — authoritative signed data, honestly historical, never presented as current.

---

## 3. Owning modules and crate layout

All new logic lands in the shared `jeliya-ui` crate (Iroh-free, native-free wasm graph; renderer behind the `ui`/`web` features). No new crate.

```
crates/jeliya-ui/src/
  room/                         NEW module (pure, host-testable — no Dioxus)
    mod.rs                      re-exports; the module doc
    archive.rs                  the archive projection: ArchiveView, the room.archive→ArchiveView fold
    roster.rs                   the historical-roster fold over signed membership events
    capability.rs               `fn grants(caps, token) -> bool` + the forbidden-token set + gate asserts
  components/
    room_archive.rs             NEW component: RoomArchivePane, DepartureBanner, HistoricalRoster
    room_shell.rs               (from #178/#179) — the live surface; unchanged except the selector note
  app.rs                        the `Route::Room` arm branches on the found row's standing
  l10n/{mod.rs,en.rs,fr.rs}     NEW archive strings (compile-enforced EN/FR parity)
```

`room/` is deliberately **pure** (no `dioxus::prelude`), mirroring `shell/` and `prefs/`: the projection, fold, and gate are host-tested without a renderer, and `components/room_archive.rs` is a thin RSX view over them. This keeps the decisions on the MSRV/host jobs and the renderer optional (Decision-3).

---

## 4. Design decisions

**D1 — Standing selects the composition; the command is the "explicit typed path."**
`AppRoot`'s `Route::Room` arm already finds the `RoomRow` for the routed id (it is in `snapshot.rooms`, because `room.list` enumerates left/removed rooms). The arm branches on `row.standing`:
- `Standing::Active` → the live `RoomShell` (#179), which may `room.activate` / subscribe.
- `Standing::Left | Standing::Removed` → `RoomArchivePane`, which dispatches **only** `room.archive`.

"Explicit typed archive path" (AC-1) is satisfied by two facts: the departed room reaches a **different composition**, and that composition issues a **different, compile-time-paired command** (`room.archive`, `MUTATING=false`) — never the live `room.activate`/`room.timeline`. There is **no** new `Route` variant; selection is standing-driven so it is dest-agnostic (see D6).

**D2 — Capability absence is structural, not decorative.**
The archive pane's RSX contains **no** composer, Invite, Leave, file, or Pipe affordance at all — they are not present-and-disabled. `room/capability.rs` defines the forbidden set and a `grants()` predicate; a host test asserts (a) a departed `RoomRow`'s served `capabilities` never contain any forbidden token, and (b) the archive composition's set of dispatchable ops ⊆ `{room.archive}`. This is the memory-recorded #180 invariant ("capability-gated affordances ABSENT not disabled") applied to the whole live-action surface at once.

**D3 — The historical roster is a fold over the signed timeline, not `room.members`.**
Because `room.members` is refused for a departed caller (§2), `roster.rs` reconstructs the roster from `RoomArchiveOut.events`:
- `RoomCreated{name}` seeds the room name and (via its `author`) the authority.
- `MemberJoined{subject_id, role}` adds/updates a member at `active`.
- `MemberLeft{subject_id}` sets that subject's standing to `left`.
- `MemberRemoved{subject_id, by}` sets that subject's standing to `removed`.
The result is labeled **historical / as of your departure**, never "current" (satisfies the non-goal "Implying that historical peer state is current"). The fold consumes only the events the archive has loaded; because paging reads forward from `Start`, the roster-bearing genesis/join events arrive first, and the roster refines as more pages load. The roster carries **no presence and no reachability** — those are live facts a departed archive has no basis to assert.

**D4 — The departure fact is authoritative from two agreeing sources.**
The banner states the caller's own ended standing from `RoomArchiveOut.standing` (the daemon's authoritative answer) and, when the loaded events include it, the signed `MemberLeft`/`MemberRemoved` event that names the caller (for `Removed`, the `by` subject). The two must agree; a disagreement (or a `room_still_active` reply, D8) is surfaced as an honest error, never silently reconciled.

**D5 — The banner is permanent and explains rejoin.**
The `DepartureBanner` is rendered first inside the archive `<main>`, is **not** dismissable, and carries the closed-vocabulary explanation that this is a local read-only archive and that **rejoining requires a fresh invite** (AC-5). There is no rejoin affordance: no self-service rejoin op exists — `invite.redeem` needs a capability the departed user does not hold — so offering one would be a button that fails, exactly what the security section forbids.

**D6 — Deep links and restart are truthful because selection is standing-driven and preference-free.**
Any `/rooms/:roomId/<dest>` deep link to a departed room lands on the archive (selection ignores `dest`; the archive is not one of the five live destinations). It does **not** fall through to `RoomUnavailable`, because a departed room **is** present in `room.list` (reachable). Same-install restart re-reads standing from local evidence via `room.list` (zero network), so the archive re-opens identically with no device-local flag involved (AC-6).

**D7 — Zero network, zero mutation, by the command's own contract, re-asserted at the seam.**
`room.archive` is normatively network-free and mutation-free at the daemon. The client seam adds no side effects: the pane subscribes to no room stream, calls `room_archive` with `Dedup::None` (a pure read takes no `op_id`), and never calls `start()`-gated live ops. A test asserts the archive path emits exactly the `room.archive` op(s) and nothing else (AC-2).

**D8 — Races fail honestly.** If `room.archive` returns `room_still_active` (the room was re-activated between the `room.list` read and the archive open), the pane shows an honest "this room is active again" notice with **Rooms** as the way out — never a silent redirect into a live surface the user did not ask for.

---

## 5. The archive projection (pure; `room/archive.rs`)

```rust
/// The read-only view a departed room renders. Pure; no Dioxus, no transport.
pub struct ArchiveView {
    pub room_id: RoomId,
    pub room_name: Option<String>,          // from a loaded RoomCreated event, else None
    pub my_standing: Standing,              // Left | Removed — from RoomArchiveOut.standing
    pub departure: DepartureFact,           // D4
    pub events: Vec<Event>,                 // the signed timeline, oldest→newest
    pub roster: Vec<HistoricalMember>,      // D3
    pub more: Option<Cursor>,               // Truncated::More → the continuation cursor
}

pub enum DepartureFact {
    Left,                                   // caller authored member_left
    Removed { by: Option<SubjectId> },      // an authority authored member_removed{by}
}

pub struct HistoricalMember {
    pub subject_id: SubjectId,
    pub role: Role,
    pub standing: Standing,                 // active | left | removed, as of the loaded log
}
```

- `ArchiveView::fold(prev, out: RoomArchiveOut, me: SubjectId) -> ArchiveView` merges a freshly read page into the accumulated view: appends events in position order, refines `roster` via `roster::apply`, sets `room_name` if a `RoomCreated` is present, and records `more` from `out.truncated`. `my_standing` comes from `out.standing`; `departure` is derived by scanning for the caller's own `MemberLeft`/`MemberRemoved` (falling back to `DepartureFact::from(out.standing)` when that event is not in the loaded window).
- The fold is **idempotent by position**: re-reading a page never double-appends (dedupe on `Event.pos`), so a retry after a transient failure is safe even though the read itself is network-free.

`room/roster.rs` holds `apply(&mut Vec<HistoricalMember>, &Event)` implementing D3, plus `authority_of(&[Event])`. `room/capability.rs`:

```rust
pub const FORBIDDEN_IN_ARCHIVE: &[CapabilityToken] = &[
    CapabilityToken::MessageSend, CapabilityToken::StatusPost,
    CapabilityToken::RoomLeave, CapabilityToken::RoomActivate, CapabilityToken::RoomDeactivate,
    CapabilityToken::InviteMint, CapabilityToken::InviteList, CapabilityToken::InviteRevoke,
    CapabilityToken::MemberRemove,
    CapabilityToken::FileShare, CapabilityToken::FileList, CapabilityToken::FileFetch,
    CapabilityToken::FileRead, CapabilityToken::TransferCancel,
    CapabilityToken::PipePublish, CapabilityToken::PipeList, CapabilityToken::PipeConnect,
    CapabilityToken::PipeRelease, CapabilityToken::PipeRevoke,
    CapabilityToken::RoomTimeline, CapabilityToken::RoomMembers, CapabilityToken::RoomPeers,
    CapabilityToken::StatusHistory,
    CapabilityToken::StreamSubscribe, CapabilityToken::StreamUnsubscribe, CapabilityToken::StreamResync,
];
pub fn grants(caps: &[CapabilityToken], token: CapabilityToken) -> bool { caps.contains(&token) }
```

The archive pane needs no per-affordance `grants()` checks (the affordances are simply absent); the set and predicate exist so a test can prove the served capabilities and the composition agree.

---

## 6. Client seam (`crates/jeliya-client/src/handle.rs`)

`ClientHandle::call::<RoomArchive>` already works via the generic entry point. Add a one-line convenience forwarder for symmetry with `room_timeline`/`room_list` (none may erase the output type):

```rust
/// `room.archive` — open a left/removed room as a local read-only archive.
/// A pure, network-free read; takes no `op_id`.
pub fn room_archive(&self, input: RoomArchive, dedup: Dedup)
    -> impl Future<Output = Result<RoomArchiveOut, CallError>> + '_ {
    self.call::<RoomArchive>(input, dedup)
}
```

The pane reads with `Dedup::None`, `Cursor::Start`, `Direction::Forward`, and `limit = timeline_page_max` (the same paging bound the timeline uses), then follows `Truncated::More { cursor }` for further pages. Use `dispatch_typed_bounded`-equivalent ceilings if the pane is wired through the bounded read path #179 established; otherwise the plain `call` is acceptable for a read whose page size is already bounded by the protocol. **No** `subscribe()` for the room and **no** `start()`-gated live call is issued on this path.

---

## 7. UI composition (`components/room_archive.rs`)

`RoomArchivePane { room_id, my_standing, navigate, handle }`:

1. On mount, run the paged `room.archive` read into an `ArchiveView` signal (loading → loaded → error states mirror #179's timeline pane; **offline reconciler-read scripting is deferred to the tests phase** exactly as #179 deferred it — the pane parks on Loading when the read cannot complete, never fabricates content).
2. Render, inside the room `<main>` (the room header from #178/#179 stays above):
   - **`DepartureBanner`** first — the permanent, non-dismissable fact (`role="status"` region, not an alert; it is steady-state, not an interruption) with left-vs-removed copy, the "local read-only archive" statement, and the "rejoining requires a fresh invite" explanation (D5).
   - A **read-only timeline** of `view.events` using #179's `TimelineRow` component (identical rendering; no composer beneath it). A "load older/more" control appears only while `view.more.is_some()` and triggers the next page.
   - **`HistoricalRoster`** listing `view.roster` under a heading that names it historical ("Members when you left"), each row showing subject, role, and ended standing — **no** presence dot, **no** reachability, **no** live/agent affordances.
3. Render **no** composer, Invite, Leave, file, or Pipe affordance, and **no** live room-destination nav strip (the five-tab strip belongs to the live surface; the archive is a single read-only view). This is D2 enforced by construction.

The pane replaces the live `RoomShell` for departed rooms; the live `RoomShell` is otherwise unchanged. `AppRoot`'s `Route::Room` arm (`crates/jeliya-ui/src/app.rs`) changes from "render `RoomShell` iff reachable, else `RoomUnavailable`" to:

```
if !reachable            -> RoomUnavailable            (route names a room not in room.list)
else match row.standing {
    Active               -> RoomShell { .. }           (#179 live surface)
    Left | Removed       -> RoomArchivePane { .. }     (this spec)
}
```

`reachable`/unknown handling is preserved: before `room.list` answers, the shell shows the loading room frame rather than flashing "unavailable" or the archive.

---

## 8. Localization

Add compiler-enforced EN/FR `Catalog` methods (parity gates in `l10n/mod.rs`, `en.rs`, `fr.rs`; French avoids the typography the node gate rejects). Draft copy:

| Method | EN | FR |
|---|---|---|
| `archive_banner_left_title` | "You left this room" | « Vous avez quitté ce salon » |
| `archive_banner_removed_title` | "You were removed from this room" | « Vous avez été retiré de ce salon » |
| `archive_banner_body` | "This is a local, read-only archive. Its history is shown as it was; it is not live and receives no new activity." | (FR equivalent) |
| `archive_banner_rejoin` | "To rejoin, you need a new invite." | (FR equivalent) |
| `archive_timeline_label` | "Archived timeline" | (FR) |
| `archive_roster_heading` | "Members when you left" | (FR) |
| `archive_load_more` | "Show earlier activity" | (FR) |
| `archive_still_active` | "This room is active again. Open it from Rooms." | (FR) |
| `archive_empty` | "No archived activity." | (FR) |

Copy must never say or imply "seen", "delivered", "online", or "current" for the roster (contract §unread/presence honesty). No free-text interpolation of subject ids beyond the existing short-id/subject display primitives.

---

## 9. Accessibility

- The `DepartureBanner` is a landmarked `role="status"` region (steady-state fact, not an alert), announced once on mount through the existing content live region; it is not focus-trapping and not dismissable.
- The archived timeline and roster are ordinary reading content with headings; the "Show earlier activity" control is a real `<button>` with a 44px target and visible focus (the #177 foundations).
- Because the composer and every action are **absent**, there are no disabled controls for assistive tech to announce as unavailable — the surface reads as a document, which is what it is.
- The room header's short-id disambiguator (from #178) remains, so the archive is unambiguously the same room.

---

## 10. Security and correctness

- **No network activity.** The archive path issues exactly `room.archive` (a normatively network-free daemon read) and nothing else. It starts no Iroh node and no room subscription; it calls no liveness-gated op. Enforced by D7's op-set test and by the pane holding no `stream.subscribe`/`room.activate` call site.
- **No mutation, no hint change.** `room.archive` is `MUTATING=false` and durably side-effect-free; the pane writes no preference tied to opening an archive (opening one changes no device-local state), so nothing mutates peer hints or storage.
- **No unavailable content, no failing affordances.** Every action that would return `not_a_member`/`membership_ended` is **absent**, not offered. The capabilities that authorize the surface are authoritative (served by the daemon, trusted by the gate, fixed by the conformance test), never inferred by the client.
- **Historical ≠ current.** The roster is explicitly labeled historical and carries no presence/reachability, so no live peer state is implied.
- **No auto-deletion.** Opening or closing the archive deletes nothing; the archive persists in `room.list` until the storage generation is discarded.

---

## 11. Test strategy

**Host unit tests (no renderer) — `crates/jeliya-ui/src/room/`:**
- `roster.rs`: genesis→join→leave→remove folds produce the correct `HistoricalMember` standings; authority is read from the `RoomCreated` author; unresolved authors assert nothing.
- `archive.rs`: `ArchiveView::fold` is idempotent by `pos` across a re-read page; `departure` reads `Left` vs `Removed{by}` from the caller's own event and falls back to `out.standing` when that event is outside the loaded window; `more` tracks `Truncated`.
- `capability.rs`: `FORBIDDEN_IN_ARCHIVE` contains every live/mutating room token and excludes `room.archive`; `grants()` is a pure membership check.

**Mock-scripted seam tests — `crates/jeliya-client` / `jeliya-ui` component tests:**
- Script `MockScript::on("room.archive", Program::reply_ok::<RoomArchive>(&out))` with a Left and a Removed fixture (fresh v2 rooms, per the issue's verification list) and assert the pane loads the timeline, reconstructs the roster, and shows the correct banner.
- Assert the archive path dispatches **only** `room.archive`: script every other op to `Program::reply_err`/`local(Backend)` and assert none is called (an unscripted op deterministically errors, so a stray call is caught).
- Script `room.archive` → `room_still_active` and assert the honest "active again" notice + Rooms escape (D8).
- Paging: script two pages (`Truncated::More` then `Complete`) and assert "Show earlier activity" loads the second and then disappears.

**Boundary / graph gates (unchanged, must stay green):**
- `crates/jeliya-ui/tests/boundaries.rs` and `scripts/check-jeliya-ui-wasm-graph.sh`: the new `room/` module and `room_archive.rs` add **no** Iroh/native/WebSocket edge to the wasm graph.
- `l10n` parity + French-typography + `fr != en` node gates cover the new strings.
- `cargo check --locked --workspace --all-targets` (MSRV, renderer-free) still compiles `jeliya-ui` to nothing without the `ui` feature (`room/` is pure but still `#[cfg(feature = "ui")]`-gated behind the module, matching `shell/`).

**Conformance mapping (owned by #165/#166, referenced here):**
- The daemon-side facts this surface trusts — `room.archive` returns the ended standing + events with zero network/mutation, an active room answers `room_still_active`, and a departed room's `room.list` capabilities are `["room.archive"]` — are protocol conformance, verified in the 341-case corpus, not re-implemented here. #91 asserts the **UI** honors them; it does not re-prove the daemon.

**Live re-qualification:** with the real `WsWeb` transport (#171) the web surface is re-qualified against a live daemon (fresh v2 rooms covering voluntary leave and removal, direct route/restart, absent network, every forbidden action, and new-invite rejoin) as part of #182; desktop #189 and Android #194 repeat the same script behind their transports.

---

## 12. Downstream verification (#195)

#195 records one ledger row per **applicable Dioxus platform** proving the AC facts against a fresh v2 room in both the voluntary-leave and removal cases: archive opens read-only, no Iroh/dial/sync/heartbeat/hint activity is observed, timeline + historical roster + permanent departure banner render, every forbidden action is absent, a fresh invite rejoins, and direct-route + same-install-restart preserve the behavior. #195 consumes this contract; it is **not** a prerequisite for closing #91.

---

## 13. Acceptance-criteria mapping

| # | Acceptance criterion | Where satisfied |
|---|---|---|
| 1 | Left and removed rooms open through an explicit typed archive path | D1 (standing selector + `room.archive` command); §7 `AppRoot` arm |
| 2 | No Iroh session, dialing, sync, heartbeat, or peer-hint mutation occurs | D7; §10; the op-set test |
| 3 | Local signed timeline and historical roster render with an unambiguous departure banner | D3, D4, D5; §5, §7 |
| 4 | Composer, Invite, Leave, file operations, and Pipes are unavailable by capability | D2; §5 `FORBIDDEN_IN_ARCHIVE`; §7 (affordances absent) |
| 5 | Rejoining requires and explains a fresh invite | D5; §8 `archive_banner_rejoin` |
| 6 | Direct routes and same-install restart preserve truthful archive behavior | D6 |
| 7 | Required Dioxus platform rows are present in #195 | §12 (this issue delivers the contract + web evidence; #195 records the rows) |

---

## 14. Risks and open questions

- **R1 — Roster completeness across paging.** The roster is only as complete as the loaded events. For a large archive the genesis/join events (which carry membership) arrive in the first forward page(s), so the roster is correct early; but a room that churned membership deep in history needs all membership-bearing pages before the roster is final. **Decision:** show the roster refining as pages load, label it "as of your departure," and never claim completeness the loaded window cannot support. Revisit a daemon-served historical roster (a `room.archive` extension carrying the closing roster) with #165/#166 if the fold proves insufficient — **out of scope here.**
- **R2 — Entry affordance in the Rooms list.** Should a departed room show a standing chip ("Left"/"Removed") in the Rooms list so selecting it is a conscious act? Recommended yes (honest, and makes the archive entry explicit), but it touches #180's People/room-list surface. **Open:** confirm the chip lands here or in #180; the archive itself does not depend on it.
- **R3 — `room.archive` bounded-read path.** Whether to route the read through #179's `dispatch_typed_bounded` ceilings or the plain `call`. Recommended: reuse whatever #179 standardized for `room.timeline`, since the page bound is identical. **Open** until #179 lands in this branch.
- **R4 — Prerequisite ordering.** #179's `RoomShell`, timeline row, and room header are **not yet in this branch** (only the #178 skeleton is). This spec targets the room surface as #179 defines it; if #179's timeline-row API differs, §7's reuse adjusts to it. No archive logic depends on #179 internals beyond the event-row component.
- **R5 — Removal without a loaded `MemberRemoved{by}`.** When the caller's own removal event is outside the loaded window, `by` is unknown; the banner then states "removed" without naming the remover rather than guessing. Acceptable and honest.

---

## 15. Rollout / rollback

Pure additive surface behind the existing `ui`/`web` features. Rollback is removing the `Route::Room` standing branch (departed rooms revert to the retained floor — the room does not open) and deleting `room/` + `room_archive.rs`; nothing else depends on them. No migration, no data change, no persisted flag — opening an archive is a stateless local read.
