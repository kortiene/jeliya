# Dioxus/Web — People, Agents & Runs, invitations, Fleet, Settings, and diagnostics (#180)

**Issue:** #180 `[Dioxus][Web]: Port People, Agents and Runs, invitations, Fleet, Settings, and diagnostics`
**Program:** #156 (Dioxus clean-slate). **Milestone:** M3 (shared web foundation).
**Blocked by / depends on:** #178 (shell/routing/prefs — **merged**, `crates/jeliya-ui/src/{app.rs,shell/,components/}`), #177 (CSS/l10n/a11y foundations — **merged**), #175 (typed / full fault-injected client adapter coverage — the read/mutation surface and the reference-mock scenarios #180 renders against). Authoritative presence qualification remains **#79** (`ConnEvent` generation + typed offline reason).
**Authoritative product contract:** `docs/product-behavior-contract.md` — §"Required destinations", §"Status vocabulary and truthful states", §"Retained product invariants" (rows 1, 2, 4, 5), §"Recency, unread, and attention", §"Identity, aliases, and self label", §"Room identity and homonyms", §"No-fake-state rules" (rules 1–7).
**Protocol authority:** `docs/protocol-v2.md`; the typed operation surface is `crates/jeliya-api/src/{ops.rs,shared.rs,types.rs}`.
**Architecture record:** `docs/dioxus-architecture.md` (Decision-3 no-`cfg`-in-shared-components, Decision-6 semantic primitives, Decision-7 secret boundary).
**Status:** SPEC — not yet implemented. This document is a build plan; it changes no production code.

---

## 1. Outcome and scope

#178 shipped the global shell with **skeleton** panes for every non-Rooms destination. #180 fills those skeletons with truthful product content: the room **People** roster with invitation issue/replace/revoke and destructive member/leave actions, the room **Agents & Runs** view, the global **Agent Fleet**, the **Settings** local-alias editor and the diagnostics card, and the shared **destructive-confirmation** primitive — each routeable, focus-safe, single-submit, capability-gated, and rendered under the six truthful states with an evidence-backed, distinct status vocabulary.

The client transport remains the deterministic **mock** `ClientHandle` until #171's `WsWeb` adapter lands (as in #176/#178); every read and mutation #180 issues is transport-agnostic and is exercised against the mock's scripted programs and, where the behavior is a daemon guarantee (#46/#47), against committed **real-daemon** regressions at the core/conformance layer (§13.4).

### In scope

- **People (room):** the signed roster (`room.members`), outstanding/expired invitations (`invite.list`) as a **separate fact** from roster standing, per-member role/standing, the device-local alias, presence as a **distinct** fact (`room.peers`, live rooms only), and the capability-gated actions: **Invite** (`invite.mint`), **Re-invite after expiry** (`invite.mint` for the same identity + `invite.revoke` of the stale one), **Revoke** (`invite.revoke`), **Remove member** (`member.remove`, destructive), **Leave room** (`room.leave`, destructive).
- **Agents & Runs (room):** the agents *in this room* (derived: members that have authored a `status.post`) and their latest signed status + run history (`status.history`) — a different destination from the global Fleet.
- **Agent Fleet (global):** `fleet.list` across every authorized room, the closed actionable-attention set, the **Live** filter, **polled only while the destination is active and the document visible, resuming once**.
- **Settings:** extend #178's Settings with the **device-local alias** editor (per identity id, `PreferenceKey::Aliases`) and the **diagnostics card** (bounded, redacted, actionable) beside the existing identity / locale / self-label surfaces.
- **Diagnostics:** a bounded, secret-redacted, actionable surface reachable from Settings and the `StatusFooter` disclosure.
- **Destructive/sensitive confirmation primitive:** one accessible dialog (built on `components/dialog.rs`) that repeats the room disambiguator, defaults initial focus to the abandoning control, and is single-submit.
- **Truthful status display seam:** localized labels for every closed vocabulary (`Standing`, `Role`, `Liveness`, `Reachability`, `Link`, `Redeemability`, `Severity`, `StatusLabel`) plus the room-session Open/Closed fact and a safe localized fallback for absent/unknown values.
- **Error/retry/loading/empty/offline/stale/unauthorized states** on every new read surface.

### Explicitly out of scope (non-goals, from the issue)

- **Agent spawning or execution.** #180 renders agent runs; it never starts one. There is no `status.post` authoring UI here.
- **Files and Pipes** panes (#181) — their room-destination routes and strip tabs already exist (#178); their content is not built here.
- **Activity timeline and composer** (#179).
- **Treating membership as live connectivity.** Membership (`room.members`) and presence (`room.peers`) are rendered as separate facts; one is never inferred from the other.
- **Forwarding secrets or raw internal errors into diagnostics.** The minted invite `capability`, the self-label, and full identities are never placed in diagnostics; raw error text is secret-scrubbed at the display seam.
- **Preserving React/Flutter state or v1 payload behavior**, or reading any legacy key.
- **Desktop/Android surfaces** (#184/#193) and their qualification (#189/#194).

### Platform applicability

Shared UI, **web-qualified first**. Every unit of logic is written target-agnostic (view-model folds, the status display seam, the polling controller, the confirmation policy) so #184/#193 reuse it behind their own `PlatformServices` and the same `ClientHandle` seam.

---

## 2. What already exists vs. what #180 builds

| Concern | Already exists | #180 builds |
|---|---|---|
| Destination routes | `Route::{Room{room_id,dest},Fleet,Settings}`, `RoomDest::{Activity,People,Agents,Files{item},Pipes{item}}` with round-trip `parse`/`to_path` (`crates/jeliya-platform/src/navigation.rs`) | The pane **content** switched on `RoomDest::People` / `RoomDest::Agents` inside `RoomShell`, and on `Route::Fleet` / `Route::Settings`; item deep links for People/Agents (a member/agent selection is route state — §9) |
| Room skeletons | `RoomShell` (header + tab strip + `room-pane-skeleton`), `RoomUnavailable` (recoverable state), `FleetPane`/`SettingsPane` skeletons (`components/{room_shell,fleet,settings}.rs`) | People/Agents panes replacing the skeleton; Fleet content; Settings aliases + diagnostics card |
| Client seam | `ClientHandle::call::<O>(input, Dedup)` (compile-time paired), `subscribe`, `state`, `start`/`stop`; deterministic `MockScript`/`MockController` (`crates/jeliya-client`) | Per-pane reads (`room.members`, `invite.list`, `room.peers`, `fleet.list`, `status.history`) and mutations (`invite.mint`, `invite.revoke`, `member.remove`, `room.leave`) with the Ready-gated, retry-honest dispatch pattern `app.rs` established for `room.list` |
| Typed ops | All 33 ops + closed vocabularies (`crates/jeliya-api`) | No new op, no new wire value — #180 consumes the existing surface only |
| Status display | `l10n/wire.rs` (`role_label`, `member_status_label`, `peer_path_label`, `status_for`) with raw passthrough; `l10n/error.rs` (`ErrorDisplay`, `scrub_secrets`) | Display functions for the remaining closed vocabularies (`Liveness`, `Reachability`, `Link`, `Redeemability`, `Severity`, `StatusLabel`, room-session Open/Closed) + absent/unknown fallbacks |
| Primitives | `Dialog` (modal, focus-safe, Escape), `Field` (labelled control), `NavLandmark`, `Heading`, live regions, `DiagnosticsDialog` | The `ConfirmDialog` destructive primitive (built on `Dialog`), the alias-map editor, the Fleet/roster list markup |
| Capabilities | `RoomRow.capabilities: Vec<CapabilityToken>`, `RoomActivateOut.capabilities`; `CapabilityToken` closed on the 33 op names | The affordance gate: an action control is rendered **only** when its `CapabilityToken` is present (§8, contract invariant 5) |
| Preferences | `PreferenceKey::Aliases` (caller-serialized map), session-scoped `WebPreferences` (#178) | The alias serialization format + editor; alias resolution at every self/peer rendering site |
| Lifecycle | `Lifecycle`/`LifecycleBus` with `Resumed`/`Backgrounded` (browser `visibilitychange`) via `WebLifecycle` (#178) | The Fleet polling controller that consumes visibility + active-route state (§10) |

---

## 3. Owning modules and crate layout

All new logic lives in `crates/jeliya-ui` behind the `ui` feature, mirroring #178's split of **pure host-testable folds** from **thin RSX components**. No `web-sys` and no platform `cfg` enter these modules (Decision-3); the transport is the injected `ClientHandle`, and platform authority is the injected `PlatformServices`.

```
crates/jeliya-ui/src/
  view/                         # pure, host-testable view-model folds (no dioxus render, no web-sys)
    mod.rs
    roster.rs                   # fold room.members (+ room.peers when live) -> RosterView: distinct membership & presence columns
    invites.rs                  # fold invite.list -> InviteView; expiry/redeemability classification; re-invite target derivation
    agents.rs                   # filter fleet.list by room_id -> room AgentsView; status.history -> RunHistoryView
    fleet.rs                    # fold fleet.list -> FleetView; the Live filter; attention (severity) grouping
    poll.rs                     # FleetPoll: pure state machine {active, visible} -> {Idle, Polling, resume-once}
    capability.rs               # typed affordance gate: which CapabilityTokens authorize which action control
    load.rs                     # LoadState<T>: the six truthful states (Loading/Loaded/Empty/Offline/Stale/Failed/Unauthorized) shared by every read
  status/
    mod.rs                      # the display seam for every closed vocabulary + absent/unknown fallback (extends l10n/wire.rs' pattern)
  components/
    people.rs                   # People pane: roster + invites + presence + capability-gated actions
    agents.rs                   # Agents & Runs pane (room-scoped) + a run-history disclosure
    fleet.rs                    # (extend) FleetPane content + polling wiring + Live filter
    settings.rs                 # (extend) alias editor + diagnostics card
    confirm.rs                  # ConfirmDialog: destructive/sensitive confirmation (single-submit, focus-safe, disambiguator)
    invite_form.rs              # the accessible invite-issue form (subject id + role + expiry) — single-submit
  l10n/{mod.rs,en.rs,fr.rs}     # (extend) Catalog trait: all new copy, EN+FR parity compiler-enforced
```

`view/*`, `status/*`, and `poll.rs` are pure and unit-tested on the host (`cargo test -p jeliya-ui`). The RSX components are rendered against the mock and asserted in the wasm/Playwright suite. `l10n/wire.rs`'s existing functions are kept; `status/mod.rs` is their sibling for the remaining vocabularies (kept separate only to avoid one 400-line file — the pattern is identical).

**Data-flow shape.** Each pane owns its read lifecycle through a `use_future` that follows the exact discipline `app.rs` established for `room.list` (§5.3): subscribe-before-check, dispatch only in `State::Ready`, retry a `Disconnected` idempotent read after recovery, record every other error once (terminal), and never render a spurious zero before the first answer. Results land in a pane-local `Signal<LoadState<T>>`; there is **no** second global state store, so a route change unmounts a pane and its in-flight read future with it. The route **is** the state (Decision-1, retained from #178).

---

## 4. Key design decisions

- **D1 — Membership and presence are two reads, rendered as two facts, never folded into one (contract invariant 4, §"No-fake-state" rule 2/4).** The roster comes from `room.members` (signed standing + role, available whether or not the room is live). Presence comes from `room.peers` (observed transport links, **live rooms only**). `RosterView` carries them in **separate fields**; a member with no peer link renders membership `Member` and presence `Offline`/absent side by side, never a single blended "online member" badge. When the room is not live (`RoomRow.live == false`, session **Closed**), presence is *unknown* (not *zero*): the People pane shows the roster and an honest "presence unavailable — room session is Closed" note, never "no one is here." Presence is **authoritative** (from `room.peers`), and the deeper generation-fenced qualification is #79's — #180 renders exactly what `room.peers` states and claims no more.

- **D2 — Invitations are a separate fact from roster standing (contract §"Status vocabulary", roster row).** `invite.list` is its own read and its own list in the People pane; an outstanding invite is **never** rendered as a roster standing (there is no "Invited" member). `Redeemability::{Outstanding,Expired,Revoked,Redeemed}` classifies each invite; a `Redeemed` invite maps to a member that appears in `room.members` on its own evidence. The retired v1 `invited` member-status word (`l10n/wire.rs::member_status_label` still maps it for raw passthrough safety) is **not** produced by any #180 view.

- **D3 — Re-invitation after expiry is mint-fresh + revoke-stale for the same identity (contract invariant 2, #47).** The People pane offers a **Re-invite** affordance on an `Expired` invite row: it mints a fresh capability bound to the same `subject_id` (`invite.mint`) and revokes the stale one (`invite.revoke`) so the old ticket cannot be presented. The minted `capability` string is displayed **once** for the operator to hand off (it is returned only by `invite.mint`, never by `invite.list`), copied via the clipboard capability, and **never** written to preferences, diagnostics, or logs (§11). The durability of the replacement across daemon restart is a **daemon** guarantee proven by the real-daemon regression (§13.4), not the UI.

- **D4 — Destructive and sensitive actions go through one primitive that is single-submit and focus-safe (contract §"No-fake-state" rule 6, §"Accessibility").** `ConfirmDialog` (built on `Dialog`, so `role="dialog"`, `aria-modal`, focus containment, and Escape are inherited): it **repeats the room disambiguator** (short id) in its body even when no homonym exists; its **initial focus lands on the abandoning control** (Cancel), never on the confirming one (`Dialog` already focuses the panel `tabindex=-1`, and `ConfirmDialog` orders Cancel before Confirm in the DOM/tab order); and it is **single-submit** — the confirm control disables the moment it is pressed and stays disabled until the mutation settles, so a double-press or a re-render cannot issue the mutation twice. Member removal, leaving a room, and revoking an invite all route through it.

- **D5 — Single-submit is a UI in-flight guard; cross-reconnect replay is opt-in only for operations with a tested v2 dedup guarantee (contract §"No-fake-state" rule 7).** A mutation's submit control is disabled while an `in_flight` signal is set (the double-submit guard). *Separately*, whether the client may **auto-replay** the mutation across a reconnect is decided by `Dedup`: `invite.redeem` carries `joined: bool` (`false` on replay) and is the one join operation with an observable replay guarantee, so join uses `Dedup::Key`; **`member.remove`, `room.leave`, `invite.mint`, `invite.revoke` use `Dedup::None`** and, on an ambiguous `CallError::Disconnected { Unknown }`, surface a truthful "couldn't confirm — reload to check" state rather than silently re-issuing a signed, irreversible event. The client kernel (#168) already refuses to replay a non-`op_id` mutation; #180 simply never passes a `Dedup::Key` it cannot honestly defend. (If protocol-v2 documents an explicit tested dedup guarantee for `member.remove`/`invite.revoke`, a `Dedup::Key` may be added there in a later slice; the corpus #161 is the authority, not this UI.)

- **D6 — Action affordances are gated by typed capability tokens, not by scattered `disabled` checks (contract invariant 5).** `view/capability.rs` maps each action to the `CapabilityToken` that authorizes it (`Invite → InviteMint`, `Revoke → InviteRevoke`, `Remove → MemberRemove`, `Leave → RoomLeave`). The pane renders an action control **only** when the room's capability list (`RoomRow.capabilities`, refreshed by `RoomActivateOut.capabilities` when live) contains that token. An unauthorized user sees **no** affordance (not a disabled button), so the UI never presents an action the daemon would refuse, and the "authorized" set is a single typed lookup rather than role logic duplicated per control.

- **D7 — The Fleet polls only while active and visible, and resumes exactly once (issue "Fleet polls only when active/visible and resumes once", §"Recency" "holding rooms open … is rejected").** `view/poll.rs` is a pure `{active, visible} → PollAction` machine: it emits a poll tick only while the route is `Route::Fleet` **and** the document is visible (from the `Lifecycle` `Resumed`/`Backgrounded` bus), and when it transitions back into the polling condition it fetches **once** immediately (resume-once) before resuming the interval — it never replays a backlog of missed ticks and never keeps polling in the background. Leaving the destination or backgrounding the tab stops the loop.

- **D8 — The status display seam is exhaustive over closed enums (compiler-total), with a localized fallback only for the *absent* arms; a forward-compat unknown wire value fails the read honestly (contract §"No-fake-state" rule 3/5).** Every closed API enum (`Standing`, `Role`, `Liveness`, `Reachability`, `Link`, `Redeemability`, `Severity`, `StatusLabel`) is mapped by an **exhaustive `match`** in `status/mod.rs`, so `rustc` guarantees no arm is unlabelled — that is the structural form of AC-7 for the typed surface. The **absent** variants (`LatestStatus::Absent`, `LastSeen::Absent`, a member with no peer row) map to a localized "no status yet" / "never seen" / presence-absent label — never a blank and never a fabricated liveness. Because the API enums are **closed** (protocol v2 "never silently reclassify"; deserialization of an out-of-vocabulary value fails), a daemon value newer than this build fails the typed read and surfaces as **Failed** (a truthful error with a way forward), not a silent drop. Surfaces that still carry a *raw wire string* (the existing `l10n/wire.rs` role/status/path functions) keep their raw-passthrough fallback for forward compatibility. This split is the honest reconciliation of "unknown statuses have safe localized fallback" with "closed vocabularies never reclassify."

- **D9 — Agents in a room are `fleet.list` filtered by `room_id`; the global Fleet is the whole list (contract §"Required destinations": Agents & Runs is "a different destination from the global Agent Fleet").** Both destinations read the same `fleet.list` projection (agent-ness is derived and served — a member that authored ≥1 `status.post`); the room view is `FleetView` filtered to the open room, the global view is the whole list grouped by attention severity. Per-agent **run history** is `status.history { room_id, subject_id, page }`, opened on demand from an agent row (a bounded first page). This avoids inventing a second "agents in room" op and keeps agent-ness a single derived fact.

- **D10 — Liveness and last-posted status are two separately announced facts (contract §"Accessibility": "a 'Stale' agent whose last label was 'Working' must not sound like it is working now").** A fleet/agent row renders **liveness** (`Liveness`, dot + label) and **latest status** (`LatestStatus.label`) as two labelled facts with distinct accessible names; the row's accessible name never blends them into "Working" when the agent is `Stale`. Severity (`Severity`, served) drives the attention grouping and the dot tone, never re-derived from the label.

---

## 5. Operations, reads, and the dispatch discipline

### 5.1 The read/mutation map

| Destination | Reads | Mutations | Notes |
|---|---|---|---|
| **People** | `room.members` (roster), `invite.list { page }` (invites), `room.peers` (presence — live rooms only) | `invite.mint`, `invite.revoke`, `member.remove`, `room.leave` | membership ≠ presence (D1); invites ≠ standing (D2) |
| **Agents & Runs (room)** | `fleet.list` filtered by `room_id`, `status.history { room_id, subject_id, page }` on demand | — (no spawning/authoring, non-goal) | agent-ness derived (D9) |
| **Agent Fleet (global)** | `fleet.list` (polled, D7) | — | Live filter, attention grouping |
| **Settings** | (none new; identity from the connection snapshot #178) | — | alias editor writes `PreferenceKey::Aliases`; diagnostics reads client `state()`/last error |
| **Presence liveness needs a live room** | `room.activate` (to make a Closed room live before `room.peers`) is offered explicitly, never automatic | `room.activate` / `room.deactivate` | activation is a user action with its own capability; the People pane never silently activates a room to fabricate presence |

Paging (`invite.list`, `status.history`) uses a **bounded** first page (`Cursor::Start`, `Direction::Backward` for history / `Forward` for invites, `limit` a fixed small constant, e.g. 50) and renders a "show more" continuation only when `Truncated::More` is returned. No surface reads unbounded (AC-6 boundedness applies to diagnostics and equally to every list here).

### 5.2 Mutation `Dedup` policy (D5)

| Op | `Dedup` | Ambiguous-disconnect behavior |
|---|---|---|
| `invite.redeem` (join; owned by #178/#179) | `Dedup::Key(op_id)` | replay-safe (`joined:false` on replay) |
| `invite.mint` | `Dedup::None` | show "couldn't confirm the invite was created" + reload-to-check; do not re-mint |
| `invite.revoke` | `Dedup::None` | show "couldn't confirm the revoke" + reload-to-check |
| `member.remove` | `Dedup::None` | show ambiguous-outcome (a signed removal may or may not have committed); reload to read the roster truth |
| `room.leave` | `Dedup::None` | same — a signed departure cannot be taken back, so never auto-replay |

`OpId` values (for `invite.redeem`) are minted by the caller as stable per-attempt keys (the same discipline #179 uses for join); #180 does not introduce a new id source.

### 5.3 The dispatch discipline (reused from `app.rs`)

Every pane read follows `crates/jeliya-ui/src/app.rs`'s `room.list` future exactly (it is the reference the reconciler audit hardened): (1) `subscribe()` before the first `state()` check so a transition between them is buffered, not missed; (2) dispatch only when `handle.state() == State::Ready` (a real adapter refuses calls while `Connecting`); (3) on `CallError::Disconnected` for an **idempotent read**, park on a fresh subscription until the next `Ready` and retry (leave-and-re-enter proven from post-failure evidence); (4) every other error recorded **once** as terminal; (5) `rooms_loaded`-style "answered" flags gate Empty vs. Loading so a booting pane shows **Loading**, never a spurious Empty. `LoadState<T>` (`view/load.rs`) encodes this as `{Loading, Loaded(T), Empty, Offline, Stale, Failed(FriendlyError), Unauthorized}` and every pane renders all arms.

---

## 6. Truthful status vocabulary and the display seam (`status/mod.rs`)

The contract's six status vocabularies are the authority; #180 renders each fact with exactly one word, distinct on its surface (AC-2). The display seam maps the typed enum to catalog copy:

| Fact | API type | Vocabulary (EN) | Rendering |
|---|---|---|---|
| Room session | `RoomRow.live: bool` | **Open** / **Closed** | room header + People presence note |
| Signed membership / roster standing | `Standing::{Active,Left,Removed}` | **Member** / **Left** / **Removed**; absent → localized **Unknown** (D8) | roster row standing column |
| Role | `Role::{Authority,Member}` | **Authority** / **Member** (translatable; `l10n/wire.rs::role_label` exists) | roster row role column |
| Peer reachability (aggregate) | `Reachability::{Connecting,Connected,Alone,Offline}` | **Connecting** / **Connected** / **Alone** (rendered "No peers connected") / **Offline** | People presence summary |
| Per-device link | `Link::{Direct,Relay,NotConnected{reason}}` | **direct** / **relay** (Tier-2, verbatim) / not connected + `LinkReason` | per-peer line |
| Agent liveness | `Liveness::{Working,OnlineIdle,Offline,Stale}` | **Working** / **Online** / **Offline** / **Stale**; **Live** = filter over Working+Online | fleet/agent row (dot + label) |
| Latest status label | `StatusLabel::{Online,Idle,Claiming,Working,Done,Failed,Blocked}` | mapped copy; **Blocked** reads "needs a person" | agent row, run history |
| Attention severity | `Severity::{Ok,Failed,Review}` | drives dot tone + grouping; served, never re-derived (D10) | Fleet attention grouping |
| Invite redeemability | `Redeemability::{Outstanding,Expired,Revoked,Redeemed}` | **Outstanding** / **Expired** / **Revoked** / **Redeemed** | invite row |

**Retired words held retired (contract):** no "Active" display label (the enum arm `Standing::Active` renders **Member**), no "Alone in this room" (absence of a link is not solitude — `Reachability::Alone` renders "No peers connected"), no "N active" header count, and a connected peer of unknown path is **Connected**, never **Relay**. **Presence never implies read/receipt** — no "seen"/"delivered" copy anywhere (§"Recency" rule; the People and Agents panes carry none).

Each mapping is an exhaustive `match` (D8), so a new enum arm is a compile error until it is given a label — the compiler-total form of AC-7.

---

## 7. Destinations in detail

### 7.1 People (room) — `components/people.rs`

Rendered when `RoomShell` receives `RoomDest::People`. Structure (one `<main>`/`<h2>` under the room shell; the roster and the invites are separate named regions):

1. **Presence summary** (top): the room-session fact (**Open**/**Closed**) and, for a live room, the `Reachability` aggregate from `room.peers`. For a Closed room, the honest "presence unavailable — session Closed" note plus, when the `room.activate` capability is present, an explicit **Activate** action (never automatic — D1).
2. **Roster** (`room.members`): one row per member — the **alias-resolved name** (`alias(subject_id) ?? shortId(subject_id)`, self as `alias(selfId) ?? "You"` with the distinct "this device" marker — §"Identity, aliases"), the **role** (Authority/Member), the **standing** (Member/Left/Removed/Unknown), the **joined_at** date (formatting-locale), the **agent** marker when the member appears in `fleet.list` for this room (derived classification, never a role), and — as a *separate* line — that member's **per-device presence** from `room.peers` (Direct/Relay/not-connected + reason), or presence-absent when no peer row exists. Capability-gated **Remove** action (D6, opens `ConfirmDialog`).
3. **Invitations** (`invite.list`, D2): one row per invite — the bound `subject_id` (alias-resolved), `role`, `expires_at` (formatting-locale), and `Redeemability`. Capability-gated actions: **Revoke** (Outstanding → `ConfirmDialog` → `invite.revoke`), **Re-invite** (Expired → mint-fresh + revoke-stale, D3). The minted capability from an issue/re-invite is shown once in a copyable, non-persisted disclosure (§11).
4. **Issue an invitation** (`invite_form.rs`, capability-gated on `InviteMint`): an accessible form — a `subject_id` field (**starts empty** with example/help, never pre-seeded with the user's own id — §"Identity" rule), a role selector (**member only** today; `authority` is `role_not_grantable`, so the selector offers member and states the limitation, never a broken option), and an expiry input (mapped to an absolute `Timestamp`). Single-submit (D4/D5). On success the new invite appears in the list and the capability is disclosed once.
5. **Leave room** (capability-gated on `RoomLeave`): a destructive `ConfirmDialog` repeating the disambiguator; on confirm, `room.leave` (D5, `Dedup::None`); on success the shell routes to Rooms and the room now renders its departed state (contract invariant 5 floor; the archive surface is #91/#179).

### 7.2 Agents & Runs (room) — `components/agents.rs`

Rendered for `RoomDest::Agents`. The agents *in this room* = `fleet.list` filtered by the open `room_id` (D9). Each row: alias-resolved agent name, **liveness** and **latest status** as two separate facts (D10), `last_seen` (or "never seen"), and a **run history** disclosure that reads `status.history { room_id, subject_id, page }` (bounded, D9/§5.1) and lists entries chronologically with label + severity + progress. Empty state: "no agents in this room yet" (only after the answer). This destination authors nothing (spawning/execution is a non-goal).

### 7.3 Agent Fleet (global) — `components/fleet.rs`

Rendered for `Route::Fleet`, replacing the #178 skeleton. Reads `fleet.list` and renders every agent across authorized rooms, grouped by **attention severity** (the closed set: failed work `Severity::Failed`, needs-a-person `Blocked`→`Severity::Review`; `Stale`/`Offline` are liveness, **not** attention reasons — §"Recency"). Each row carries the room (room name + short-id disambiguator, so homonymous rooms are distinguishable — §"Room identity"), liveness, latest status, and last-seen (D10). A **Live** filter spans `Working`+`OnlineIdle`. Selecting a row deep-links to that room's Agents destination (`Route::Room { room_id, RoomDest::Agents }`). Polling per D7/§10.

### 7.4 Settings — `components/settings.rs` (extend)

Add beside the existing identity / language / self-label sections (#178):

- **Device-local aliases** editor: the per-identity-id alias map (`PreferenceKey::Aliases`, caller-serialized). One editable alias per identity id, **including the self id** (self falls back to "You", peers to `shortId`). Copy states "on this device, never sent" (contract §"Identity"). Validation mirrors the self-label (trim; empty clears; soft cap). Excluded from diagnostics (§11).
- **Diagnostics card** (§7.5): the bounded, redacted, actionable diagnostics surface.

### 7.5 Diagnostics — the Settings card + `StatusFooter` disclosure

Diagnostics is **bounded, redacted, actionable** (AC-6). It surfaces: the client lifecycle `State`, the last recorded (secret-scrubbed) error detail, and structured connection facts (from the #178 connection snapshot when present) — a **bounded** set (a fixed field list, no growing log). It is **redacted**: every value passes through `ErrorDisplay::diagnostic_detail` / `scrub_secrets` (Decision-7), and it **excludes the self-label, the alias map, minted invite capabilities, and full identities** (identities shortened; contract §"Settings", §"Localization" "diagnostics redact full identities and never contain the self label"). It is **actionable**: a "copy diagnostics" action (clipboard capability) produces the same redacted text for pasting into an issue. The existing `DiagnosticsDialog` (`components/diagnostics.rs`) is the disclosure form; the Settings card is the same content inline.

---

## 8. Capability-gated affordances (`view/capability.rs`, D6)

```
Action        -> authorizing CapabilityToken
Invite        -> InviteMint
Revoke        -> InviteRevoke
Remove member -> MemberRemove
Leave room    -> RoomLeave
Activate room -> RoomActivate  (for presence)
```

`fn authorized(action: Action, caps: &[CapabilityToken]) -> bool` is a pure lookup; a control is rendered only when authorized. The capability list is the room's own (`RoomRow.capabilities`, refreshed by `RoomActivateOut.capabilities` when a room is activated). This is the typed-capability suppression the contract requires (invariant 5): unauthorized actions are **absent**, not disabled, so the a11y tree never advertises an action the daemon will refuse and the "who may act" logic is one lookup, not role checks scattered through render code.

---

## 9. Routing and deep links (AC-1)

Every destination and item deep link is **stable** (round-trips through `Route::parse`/`to_path`):

- People / Agents destinations already have stable routes (`/rooms/:id/people`, `/rooms/:id/agents`).
- **Item deep links.** `RoomDest::Files`/`Pipes` already carry an `item` (#178/#67). #180 needs a stable deep link to a **selected member** (People) and a **selected agent / open run history** (Agents). Two options (Open Question Q1): **(a)** carry the selection in the route as a new item on `RoomDest::{People,Agents}` (a typed `item: Option<SubjectId>`), matching the Files/Pipes precedent and giving a genuinely stable deep link and a Back-truthful open/close; **(b)** keep the selection as pane-local signal state (no deep link, simpler, but a selected member/agent is not linkable and Back does not close it). **Recommendation: (a)** — the AC says "every destination **and item** deep link is stable," and the Files/Pipes item precedent is exactly this shape; it requires an additive `navigation.rs` change (a new `item` field on the People/Agents arms + parse/`to_path` round-trip + the strict-parse tests), coordinated as a small #178-navigation follow-up. The Fleet→room deep link (§7.3) is already expressible with the existing route.
- Malformed/unknown paths keep #178's fail-safe (strict `parse` Err → Rooms + replace). A deep link to a member/agent not in the answered list renders the recoverable "not found here" state within the pane, never a blank panel (contract rule 6 analogue).

---

## 10. Fleet polling controller (`view/poll.rs`, D7, AC-5)

Pure state machine, no timer of its own (the interval is driven by the composition/`use_future`, the machine decides whether a tick is allowed and whether a resume-fetch is owed):

```
struct FleetPoll { active: bool, visible: bool, was_running: bool }
enum PollAction { Idle, FetchOnceThenPoll, Poll, Stop }
// running := active && visible
// on state change: running && !was_running -> FetchOnceThenPoll (resume-once)
//                  running &&  was_running -> Poll
//                  !running               -> Stop (was_running:=false)
```

- `active` = the current route is `Route::Fleet` (read from the router mirror signal).
- `visible` = the document is visible, folded from the `Lifecycle` bus (`Resumed`/`Backgrounded`, wired by `WebLifecycle` in #178).
- On entering the running condition (destination opened **or** tab refocused), the machine returns `FetchOnceThenPoll` — **one** immediate fetch (resume-once), then the interval. It never queues missed ticks and never polls while backgrounded or off-destination. The `use_future` in `FleetPane` owns the interval and cancels with the route change (the pane unmounts). A host unit test drives the four transitions (activate, background, refocus, deactivate) and asserts exactly one resume-fetch per re-entry.

---

## 11. Security and correctness (issue "Allow no …")

- **No fake liveness.** Presence renders only what `room.peers` states, only for a live room; a Closed room shows presence *unknown*, never zero, and never a fabricated "online" (D1). Membership never implies connectivity. Liveness and last-posted status are two facts (D10).
- **No token leakage.** The minted invite `capability` is shown once, copyable, and **never** persisted (not to `PreferenceKey::*`, which is session-scoped anyway), never logged, and never in diagnostics. The daemon token never enters browser state (#178 §K5 stands). Diagnostics are secret-scrubbed (Decision-7) and exclude the self-label, aliases, and full identities. A source-scan test (mirroring #177's literal gate and #178's no-legacy-reader gate) asserts no diagnostics/log path receives a `capability`, a self-label, or a full identity.
- **No duplicate submissions.** Every mutation is single-submit (in-flight guard, D4/D5); the confirm control disables on press. No auto-replay of a non-idempotent signed mutation (D5); ambiguous disconnects surface truthfully.
- **No unsafe initial focus.** `ConfirmDialog` lands initial focus on the panel/Cancel, never the destructive control (D4); it repeats the disambiguator (contract rule 6). Inherited from the audited `Dialog` primitive.
- **No unauthorized action affordances.** Affordances are typed-capability-gated (D6); an unauthorized action is absent, not disabled.
- **No restoration of expired bearer tickets.** A `Redeemability::Expired` invite is rendered as expired and its capability is not re-offered; re-invitation mints a **fresh** capability and revokes the stale one (D3). Preferences (session-scoped, #178) hold no ticket that could survive to be replayed; the browser session credential dies with the tab.
- **Homonym safety.** Every room reference in Fleet and in destructive dialogs carries the short-id disambiguator (contract §"Room identity"), homonym or not.

---

## 12. l10n catalog additions (`l10n/{mod.rs,en.rs,fr.rs}`)

Add `Catalog` trait methods (declared once, implemented in both `En` and `Fr` so `rustc` enforces key/placeholder parity — #177's mechanism) for: the roster (heading, role labels reuse the existing `wire_role_*`, standing labels — some exist as `wire_status_*`, the "this device" marker, the agent marker, the presence-summary and per-link copy, `LinkReason` reasons, `Reachability` words); the invitations list (redeemability words, expiry/help copy, the issue-form labels — subject-id field + example/help, role, expiry, submit, the "member-only / authority not grantable" note, the once-shown capability disclosure copy); Agents & Runs (heading, liveness words, `StatusLabel` copy incl. "Blocked → needs a person", run-history labels, progress copy, empty state); Fleet (attention group headings, the **Live** filter label, per-row room + liveness + latest + last-seen labels, empty/loading/offline/stale/failed/unauthorized copy); Settings (aliases heading + per-alias help + "on this device, never sent", diagnostics card heading + copy-diagnostics action + the redaction note); and every destructive-confirm copy (remove/leave/revoke titles + bodies that name the room disambiguator + the abandon/confirm actions + the ambiguous-outcome "couldn't confirm" copy). The node-side #177 gates (empty value, `fr==en`, French typography per `docs/glossary-fr.md`, literal scan) apply unchanged. Tier-2 tokens (`direct`, `relay`) stay verbatim in both locales (existing `wire_path_*`).

---

## 13. Test strategy

The canonical gate is `cargo` + the wasm/Playwright web suite established in #176/#177/#178 (`crates/jeliya-ui/e2e/*`, the design-token/l10n gates, the wasm-graph guard). Add:

### 13.1 Host unit tests (`cargo test -p jeliya-ui`, pure `view/`, `status/`, `poll.rs`)
- **Roster fold:** membership and presence stay distinct columns; a Closed room yields presence-unknown, not zero; a member with no peer row renders presence-absent; agent classification is derived from the fleet list; alias resolution (self→alias/"You", peer→alias/shortId).
- **Invite fold:** redeemability classification; an outstanding invite is never a roster standing; the re-invite target is the same `subject_id`; expired invites offer Re-invite, outstanding offer Revoke.
- **Capability gate:** each action authorized iff its token is present; unauthorized → no affordance.
- **Status seam:** every closed-enum arm has a non-empty distinct label in EN and FR (exhaustive-match totality is compile-time; the test asserts distinctness and non-emptiness and the absent-arm fallbacks); Tier-2 `direct`/`relay` verbatim; retired words absent ("Active"/"Alone"/"N active" produced by no view).
- **Fleet poll machine:** exactly one resume-fetch per re-entry; no polling while backgrounded or off-destination; the four transitions.
- **LoadState:** each read renders Loading before the answer, Empty only after a zero answer, Failed with a way forward, Unauthorized as itself (never an empty room).
- **Dedup policy:** the mutation→`Dedup` table (D5) holds; an ambiguous disconnect yields the "couldn't confirm" state, not a re-issue.

### 13.2 Component/render tests (against the mock + `WebPlatform` in-memory)
- People roster + invites render from scripted `room.members`/`invite.list`/`room.peers` replies; membership and presence are two visible facts; the issue form is single-submit (a second press while in-flight issues no second `invite.mint` — assert one scripted dispatch consumed).
- `ConfirmDialog` initial focus lands on Cancel; Escape restores focus to the opener; the room disambiguator is present; confirm disables on press.
- Re-invite mints a fresh capability and revokes the stale invite (assert both scripted calls); the capability is shown once and is not written to preferences.
- Settings alias editor writes `PreferenceKey::Aliases` and re-resolves names live; the diagnostics card is redacted and omits the self-label/aliases/capability.

### 13.3 Browser e2e (Playwright, `crates/jeliya-ui/e2e/`, offline against `dist/`)
Extend the marker-gated fixture pattern (`compose.rs` `?boot=`/`?rooms=`/`?onboard=`, armed by the `jeliya-e2e-*` `localStorage` marker) with `?members=`, `?invites=`, `?fleet=`, `?peers=` scripts so the offline suite drives each pane against the deterministic mock:
- **Deep links + item selection:** `/rooms/:id/people` and `/rooms/:id/agents` render; a selected member/agent deep link is stable and Back closes it (if Q1(a) is taken); a member/agent absent from the answered list shows the recoverable "not found here" state.
- **Invite / destructive single-submit + focus-safe:** the invite form and the remove/leave/revoke confirmations are single-submit and land focus on Cancel; French first (overflow shows there first).
- **Fleet polls only when active/visible, resumes once:** drive route + a scripted `visibilitychange`; assert `fleet.list` is issued only while `/fleet` is active and the tab visible, and exactly once on re-entry (count scripted dispatches).
- **Unauthorized affordances absent:** a room whose capability list lacks `MemberRemove`/`InviteMint` shows no Remove/Invite control.
- **Unknown/absent status fallback:** an absent latest-status / never-seen agent, and a Closed-room presence, render their safe localized labels, no blank, no fabricated liveness.
- **Diagnostics bounded/redacted:** the card shows a fixed field set, no self-label/alias/capability, secret-scrubbed detail.

### 13.4 Real-daemon regressions (AC-4) — the honest boundary
The web `ClientHandle` is the **mock** until #171's `WsWeb`, so the "real-daemon" AC-4 regressions do **not** run through the web UI in this slice. They are committed at the **core/conformance** layer, where a real `jeliyad`/`jeliya-core` loopback exists:
- **Late invite + join after established history** (#46, contract invariant 1): a committed v2 conformance-corpus case (#161) + a core loopback integration test — an identity invited into a room that already carries multi-author history joins and receives that history.
- **Ticket expiry → fresh invite same identity → successful join** (#47, contract invariant 2): a committed v2 case + core test — replacing an expired ticket invalidates the old one, the fresh ticket redeems, and the replacement survives a daemon restart.
These are the "committed v2 real-daemon regressions" the issue names; they are owned jointly with the conformance corpus (#161) and the runtime (#168), and referenced by the web qualification gate (#182) which re-proves them end-to-end once `WsWeb` lands. The spec records this split so the AC is not falsely claimed as a passing **web** e2e in a mock-only slice (R1).

### 13.5 Focused-first guidance
Run the pure host tests (`cargo test -p jeliya-ui`) and the relevant e2e spec first; reserve the full web build + Playwright matrix + wasm-graph/l10n/token gates for the review gate. `web-sys` binding shims (none new in #180) stay browser-only.

---

## 14. Acceptance-criteria traceability

| AC (issue) | Where satisfied |
|---|---|
| Every destination and item deep link is stable | §9 (routes round-trip; item deep links via Q1(a)); e2e §13.3 |
| Status, role, membership, and liveness copy are evidence-backed and distinct | §6 display seam (D8), D1 (membership≠presence), D10 (liveness≠latest status); §13.1/§13.3 |
| Invite/destructive flows are single-submit and focus-safe | D4/D5, `ConfirmDialog` + `invite_form`; §13.2/§13.3 |
| Late-join and expired-ticket re-invitation real-daemon regressions pass | §13.4 (committed corpus #161 + core; UI flows in §7.1/D3); re-proven web-side by #182/#171 |
| Fleet polls only when active/visible and resumes once | D7, §10 `view/poll.rs`; §13.1/§13.3 |
| Diagnostics are bounded, redacted, and actionable | §7.5, §11 (Decision-7 scrub, fixed field set, copy action); §13.2/§13.3 |
| Unknown statuses have safe localized fallback | D8 (exhaustive match = compile-total; absent-arm fallbacks; forward-unknown → honest Failed); §13.1 |

---

## 15. Risks

- **R1 — Mock-only transport makes AC-4 a core/conformance deliverable, not a web e2e.** The web UI has no real daemon until #171. Mitigation: build the invite/join/re-invite UI flows and prove them against the mock now; commit the real-daemon #46/#47 regressions at the core/conformance layer (§13.4); state plainly in the PR that the *web* end-to-end proof is #182/#171. Do not mark AC-4 as a passing web scenario here.
- **R2 — Item deep-link route extension (Q1).** A stable member/agent deep link wants an additive `item` on `RoomDest::{People,Agents}` in `jeliya-platform::navigation` (a merged crate). Mitigation: coordinate the additive field + parse/`to_path`/strict-parse tests as a small navigation follow-up; if it slips, ship selection as pane-local state and record the deep-link gap honestly (the destination route is still stable; only the item is not linkable).
- **R3 — Presence requires a live room; #79 is the deeper qualification.** `room.peers` needs an activated room and its authoritative generation fencing is #79's. Mitigation: render exactly what `room.peers` states, never activate silently, show presence-unknown for a Closed room, and defer generation-fenced presence correctness to #79 (contract invariant 4 floor).
- **R4 — #175 coverage.** #180 renders against the reference mock and the fault-injected adapter suite (#175); if a needed op's mock scenario is not yet in #175, #180 scripts it locally (as #178 scripts `?onboard=`) and the shared scenario migrates into #175. State any locally-scripted scenario in the PR.
- **R5 — Scope creep into #179/#181/#91.** Activity/composer (#179), Files/Pipes content (#181), and the full departed-room archive (#91) are out. Guard against implementing a timeline, a file list, or the read-only archive here; #180 ships the leave *action* and the departed-state floor, not the archive surface.
- **R6 — Closed-enum forward-compat.** A daemon newer than this build sending a new `Liveness`/`StatusLabel` fails the typed read → Failed state. Mitigation: this is the honest behavior (D8, contract rule 3); the safe-fallback AC is met by the absent-arm labels and the raw-string passthrough seam, and the corpus (#161) is where new vocabulary is introduced in lockstep.
- **R7 — Double-submit under re-render.** A Dioxus re-render must not re-fire a mutation. Mitigation: the in-flight guard is a signal read in the click handler, and the confirm control's `disabled` is bound to it; a render test asserts a second press issues no second scripted dispatch.

## 16. Open questions

- **Q1 (owner: #180 + #178-navigation):** carry a selected member/agent as a typed `item` on `RoomDest::{People,Agents}` (stable deep link, Back closes it — **recommended**, matches Files/Pipes) vs. pane-local selection (simpler, not linkable)? Resolves the "item deep link is stable" AC.
- **Q2 (owner: #180 + #161):** do `member.remove` / `invite.revoke` carry an explicit tested v2 dedup guarantee that would justify a `Dedup::Key` (auto-replay-safe) rather than the conservative `Dedup::None` + ambiguous-outcome surface (D5)? Default to `None` until the corpus proves otherwise.
- **Q3:** the expiry input UX for `invite.mint` — a duration picker mapped to an absolute `Timestamp`, or an absolute date/time field? Recommendation: a small set of durations (e.g. 1h/1d/7d) mapped to `expires_at`, formatting-locale-aware, since operators think in "how long," and the wire needs an absolute instant.
- **Q4:** should the room-scoped Agents view and the People roster share one `fleet.list` read (agent-ness is derived from it) to avoid two projections per room open, or read independently? Recommendation: one `fleet.list` per room open, shared between People (agent marker) and Agents (the list), cached in a pane-shared signal for the room's lifetime.
- **Q5:** the alias-map serialization format under `PreferenceKey::Aliases` (a JSON `{ subject_id: label }`), and whether it is shared verbatim with the desktop store (#185). Recommendation: a small versioned JSON object under the #178 envelope; #185 reuses the format, each platform its own namespace.

## 17. Non-goals (restated, to bound the work)

Agent spawning/execution and any `status.post` authoring; Files/Pipes content (#181); Activity/composer (#179); the full departed-room read-only archive (#91); treating membership as connectivity; forwarding secrets or raw internal errors into diagnostics; preserving React/Flutter state or v1 payloads; reading any legacy key; desktop/Android surfaces and their qualification (#184/#193/#189/#194); the web real-daemon end-to-end proof (deferred to #171/#182).
