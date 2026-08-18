# Dioxus/Web — Bootstrap, onboarding, global shell, routing, and fresh browser preferences (#178)

**Issue:** #178 `[Dioxus][Web]: Implement bootstrap, onboarding, shell, routing, and fresh browser preferences`
**Program:** #156 (Dioxus clean-slate). **Milestone:** M3 (shared web foundation).
**Blocked by / depends on:** #176 (shared `jeliya-ui` crate + reproducible web build — merged), #177 (CSS/l10n/a11y foundations — merged), #174 (`jeliya-platform` `PlatformServices` — merged), #171 (`WsWeb` browser session adapter — the real `ClientHandle` transport).
**Authoritative product contract:** `docs/product-behavior-contract.md` §"Required destinations", §"Canonical routes", §"Responsive shells and viewports", §"Bootstrap and onboarding", §"Preferences and device-local persistence", §"Status vocabulary and truthful states". #58 is the information-architecture issue that fixes the destination topology captured verbatim in that contract.
**Architecture record:** `docs/dioxus-architecture.md` (Decision-3 layering/no-`cfg`-in-shared-components, Decision-5 target composition, Decision-6 semantic primitives).
**Status:** SPEC — not yet implemented. This document is a build plan; it changes no production code.

---

## 1. Outcome and scope

Deliver the Dioxus **global shell** for the browser target: from clean daemon bootstrap and first-run onboarding through the three global destinations (**Rooms**, **Agent Fleet**, **Settings**), with the canonical extensionless route family, browser back/forward, three responsive shells (compact / medium / wide), and a **fresh, namespaced, versioned browser preference schema** with deterministic first-run defaults and visible recovery for corrupt or unsupported new-format state.

The shell is rendered by injecting **two separate inputs at the root** — a `jeliya_client::ClientHandle` and a `jeliya_platform::PlatformServices` — exactly as `AppRoot` already requires (`crates/jeliya-ui/src/app.rs`). #178 supplies the **real browser `PlatformServices`** (History-API navigation, browser lifecycle, session-scoped preferences and secrets) in place of the deterministic fake, and wires the shell shell/router/bootstrap on top of the already-typed capability contracts.

### In scope

- Bootstrap that narrates daemon truth (the six truthful states), never an infinite spinner.
- First-run onboarding: create-or-connect identity, then create-or-join the first room (onboarding terminus is a room the user holds).
- Global shell: Rooms list + room-shell **skeleton** (room header + room-destination nav strip), Agent Fleet destination skeleton, Settings destination (identity display, live text/formatting-locale switchers, device-local self-label editor).
- Canonical routing over the browser History API, with canonicalizing redirects, fail-safe parsing, back/forward, deep links, and once-per-launch last-room restore.
- Three responsive shells with the `#58`/contract navigation topology.
- A new browser preference **namespace + schema version + envelope**, deterministic defaults, corrupt/unsupported-version recovery, and an explicit enumerated legacy-key purge (never a reader).

### Explicitly out of scope (non-goals, from the issue)

- Room **Activity** timeline and composer (later slice, #179).
- **Files** and **Pipes** panes (#181).
- People roster / Agents-in-room / Agent Fleet **content** (#180) — the destinations and routes exist; their panes are skeletons here.
- Reading or migrating any legacy React `localStorage` key, `?tab=` query, draft, alias, or profile. There is **no** legacy reader.
- Maintaining React routes.

### Platform applicability

Web only. Every unit of logic is written target-agnostic where it can be (route model, preference schema, bootstrap state machine, shell selection) so the desktop (#184/#185) and Android (#190–#193) shells reuse it behind their own `PlatformServices`.

---

## 2. What already exists vs. what #178 builds

The platform-authority **contracts** are already merged (#174) and are the seams #178 fills. #178 does **not** redefine them:

| Concern | Already defined (contract) | #178 builds (browser implementation + shell) |
|---|---|---|
| Route model | `jeliya_platform::navigation::{Route, RoomDest, RouteParseError}` with `parse`/`to_path` round-trip, strict fail-closed parsing, `encodeURIComponent`-identical encoding | The Dioxus router hook + History-API `Navigation` impl that drives it |
| Navigation capability | `trait Navigation { route(); navigate(); hand_back_to_platform() }` | `WebNavigation` (pushState/replaceState/popstate) + additive `navigate_replace` (see D4) |
| Lifecycle capability | `trait Lifecycle` + `LifecycleBus` + `LifecycleEvent::{BackRequested, NavigationRequested{route}, ProcessRestored, Window, Resumed, Backgrounded}` | `WebLifecycle` mapping browser events (`popstate`, `visibilitychange`, `pagehide`) onto the bus |
| Preferences | `trait Preferences` keyed by typed `PreferenceKey`, `WriteOutcome`, `Durability::SessionScoped` | `WebPreferences` — session-scoped, namespaced, versioned envelope over an in-memory backend + a schema layer with corrupt/version recovery |
| Secrets | `trait SecretStore`, `Secret`, `SecretKey::{SessionCredential, InviteTicket}` | `WebSecretStore` — session-scoped, in-memory, dies with the tab (holds the browser session credential only) |
| Facade | `PlatformServices` (one `Arc<dyn Platform>`, cloneable, opaque) | `WebPlatform: Platform` assembling the above + honest `Unavailable` stubs for out-of-scope capabilities |
| Shell surface | `AppRoot`, `NavLandmark`, `SkipLink(s)`, `BootScreen`, `RoomListItem`, `EmptyCenter`, `StatusFooter`, live regions, the resolved-locale context (#176/#177) | Global-destination nav, room-shell skeleton, Fleet/Settings shells, the router-driven pane switch, the locale switch UI |
| Identity truth | `Hello.subject: SubjectState::{Present{subject_id,device_id}, Absent}`, `Hello.storage_generation`; `SubjectEnsure {} -> SubjectEnsureOut { subject_id, device_id, created }` | The bootstrap/onboarding state machine that consumes them (see D2 for the seam gap it must close) |

**The single largest new integration risk (D2):** the daemon's identity-presence and storage-generation truth arrives on the `Hello` frame, but the current `ClientHandle` seam surfaces **only** lifecycle `State` and `ClientEvent::{StateChanged, Gap, ResyncRequired, Lagged}` (`crates/jeliya-client/src/handle.rs`, `event.rs`). `Hello.subject` / `Hello.storage_generation` are **not** exposed to the UI. Closing that gap is D2.

---

## 3. Owning modules and crate layout

### 3.1 New crate: `crates/jeliya-platform-web`

The browser `Platform` implementation lives in its own wasm-only crate, mirroring the future `#185` desktop and `#173` Android target crates and matching the architecture record's rule that *target implementations live in their own crates* (`crates/jeliya-platform-implementation/src/lib.rs` states this explicitly). It depends on `jeliya-platform` (contract) and, because Files/Pipes are out of scope for #178, does **not** need `jeliya-platform-implementation` (the factory door for `PickedSource`/`ExportTarget`/… tokens) — its `Files` accessor is an honest `Unavailable` stub until #181.

```
crates/jeliya-platform-web/
  Cargo.toml                # cdylib-free lib; wasm32-only deps: web-sys, wasm-bindgen, js-sys
  src/
    lib.rs                  # WebPlatform: Platform, and its constructor(s)
    navigation.rs           # WebNavigation: Navigation  (History API)
    lifecycle.rs            # WebLifecycle: Lifecycle     (browser events -> LifecycleBus)
    preferences.rs          # WebPreferences: Preferences (schema layer over in-memory backend)
    secrets.rs              # WebSecretStore: SecretStore (in-memory, tab-scoped)
    stubs.rs                # Unavailable Files/Clipboard/Share/UrlLauncher/WindowActions/PrivateDirectory
  tests/
    navigation.rs           # push/replace/popstate round-trips against a jsdom-free host? (see note)
```

> **web-sys-in-tests note.** `web-sys` APIs cannot run under a plain `cargo test` host. The *pure* logic — route canonicalization policy, preference envelope parse/version-gate/reset, the legacy-key allowlist, bootstrap state derivation — is factored into **host-testable** modules (see §3.2) with `web-sys` confined to thin binding shims. The `web-sys` shims themselves are exercised only in the Playwright/`wasm-bindgen-test` browser suite (§12.4), never on the host.

**Alternative considered (and rejected as the primary):** a `#[cfg(feature = "web")] mod platform_web` inside `jeliya-ui`, following the #176/#177 precedent of web-sys living behind the `web` feature. Rejected because it re-entangles target platform authority with the shared UI crate, whereas the architecture wants target impls as siblings of `jeliya-ui`; the new crate keeps `jeliya-ui`'s wasm boundary test (`crates/jeliya-ui/tests/boundaries.rs`) checking a shared graph that names no `web-sys` platform code. If the team prefers to defer crate proliferation, the module form is acceptable **provided** the pure schema/route/bootstrap logic still lives in host-testable, web-sys-free modules.

### 3.2 New host-testable modules in `crates/jeliya-ui` (behind the `ui` feature)

These are pure, renderer-and-web-sys-free, and unit-tested on the host. They hold the *decisions*; the web crate holds only the *bindings*.

```
crates/jeliya-ui/src/
  shell/
    mod.rs                  # Shell enum {Compact, Medium, Wide} + shellFor(width) + breakpoint constants
    router.rs               # use_route(services) -> (Signal<Route>, Navigate); canonicalization + fail-safe
    bootstrap.rs            # Boot state machine: {Booting, Onboarding(step), Ready, Recovered, Failed} from client + subject truth
  prefs/
    mod.rs                  # PreferenceSchema: namespace, version, envelope, defaults, corrupt/version recovery
    backend.rs              # trait KeyValueBackend (get/set/remove/keys) + InMemoryBackend
    legacy.rs               # the enumerated legacy-key allowlist + purge policy (data only; no reader)
  components/
    global_nav.rs           # NavLandmark-backed global-destination navigation (rail / bottom bar)
    room_shell.rs           # room header (context always visible) + room-destination nav strip (skeleton panes)
    settings.rs             # Settings destination: identity display, locale switchers, self-label editor
    fleet.rs                # Agent Fleet destination skeleton
    onboarding.rs           # identity step + rooms step
    recovery.rs             # visible reset/recovery banner for corrupt/unsupported new-format state
```

`WebPreferences` (in `jeliya-platform-web`) is a thin `Preferences` impl = `prefs::PreferenceSchema` over an `InMemoryBackend`, plus the boot-time legacy purge (which calls `web_sys::Storage::remove_item` for each key in `prefs::legacy::LEGACY_WEB_KEYS`).

### 3.3 Composition changes in `crates/jeliya-ui`

`compose.rs` / `bin/web.rs` (both `web`-gated) depend on `jeliya-platform-web` and inject `WebPlatform` where `web_composition()` currently injects `PlatformServices::fake_browser()`. The `ClientHandle` remains the deterministic mock until #171's `WsWeb` adapter lands behind the same handle (§5.F). No shared component changes shape: they still name `jeliya_ui::PlatformServices`.

---

## 4. Key design decisions

- **D1 — The route *is* the navigation state; there is exactly one state machine.** The shell reads `Route` from the injected `Navigation` capability and never keeps a second, divergable copy (contract §"Canonical routes"). The already-shipped `jeliya_platform::navigation::Route` model is reused verbatim; #178 adds no second route type. The Dioxus `use_route` hook holds a `Signal<Route>` that is a *mirror* of the URL, synchronized on every navigation, not an independent source.

- **D2 — Bootstrap truth is a read, not a mutation: surface the `Hello` connection snapshot through the `ClientHandle` seam (additive, coordinated with #270).** Deriving "does an identity exist?" from a mutating `subject.ensure` conflates *"is there an identity"* with *"create one,"* and `storage_generation` (needed by the fresh-state/reset policy) has no operation at all. #178 therefore consumes a **read-only connection snapshot** — `{ subject: SubjectState, storage_generation: u64, resume: Resume }` captured from `Hello` — exposed on `ClientHandle` as (a) a current-value accessor `connection() -> Option<ConnectionSnapshot>` and (b) a `ClientEvent::Connected { .. }` emitted on each (re)connect. This is additive (a new event variant matched with a fresh arm; a new accessor) and is the natural companion to #270, which already reshapes the kernel's connected path (`Input::Connected`, `Core.last_incarnation`, `on_connected`). **Coordinate the exact shape with #270 rather than inventing a parallel `Connected`.** Against the deterministic mock (the reference backend for M3), the snapshot is scripted; the real value arrives from `Hello` via #168/#171. **Fallback if the seam extension cannot be scheduled with #178:** the onboarding identity step calls the idempotent `SubjectEnsure {}` and reads `created` to distinguish first run (the exact discipline the retiring React `IdentityStep` used with `identity.create` + `identity_exists`), and `storage_generation`-driven reset is deferred to the platform that owns a data directory (desktop/Android), since the browser owns no data directory. This fallback is honest but weaker; D2's additive snapshot is the recommendation.

- **D3 — The browser stores nothing that survives the tab.** Per the first-release distribution boundary and contract §"Preferences", the ordinary browser is `Durability::SessionScoped`: preferences and the session credential live in memory and die with the tab. `WebPreferences`/`WebSecretStore` back onto an **in-memory** store, **never** `localStorage` and **never** `sessionStorage` (which survives reload and would make a reload wrongly restore state). Every write reports `WriteOutcome::SessionOnly`; `durability()` returns `SessionScoped`; the UI honestly says "applies this session, not saved" where it matters. A browser reload is a fresh session by construction.

- **D4 — Push vs. replace is a first-class navigation intent (additive `Navigation::navigate_replace`).** Canonicalizing redirects (`/`→`/rooms`), last-room restore, and legacy-URL rewrites must use **replace** so Back never walks through states the user never performed (contract rule 4/5; the retiring `useRoute` had `NavigateOptions.replace`). The current `Navigation` trait has only `navigate`. #178 adds a defaulted `fn navigate_replace(&self, route: Route) { self.navigate(route) }` (fully additive; existing fakes keep working; only `WebNavigation` overrides it with `history.replaceState`). The `jeliya-platform` fake records it as a distinct `RecordedEffect` for test assertions.

- **D5 — A fresh, versioned, namespaced preference schema; legacy keys are never interpreted.** #178 fixes the browser namespace (a prefix no retiring client ever wrote), a schema version, and a per-key envelope. Unknown/malformed/unsupported-version envelope values are **not** interpreted as state — they trigger a documented reset-to-defaults with a **visible** recovery affordance. Legacy React keys are **removed** (an enumerated, unconfirmed purge) or ignored; there is no code path that *reads* a legacy key as new state.

- **D6 — No platform `cfg` fork in shared components (Decision-3).** All `web-sys` lives in `jeliya-platform-web` and the `web`-gated parts of `compose.rs`/`bin/web.rs`. The shared shell reads breakpoints, routes, lifecycle, and preferences only through injected capabilities / injected reactive inputs. The one element-identity fork CSS cannot make (compact app bar vs. wide header) is driven by an injected `Shell` value, not by `cfg` (§9).

- **D7 — Fail safe and fail visible.** A malformed route resolves to the recoverable **Rooms** state with the URL canonicalized by *replace*; a route naming an unreachable room resolves to a recoverable state (never a blank panel); corrupt/unsupported preference state resets to defaults behind a visible banner. Nothing fails silently, and nothing old-generation is reinterpreted as new (contract §"Bootstrap", §"Canonical routes" rule 6).

---

## 5. Implementation workstreams

### 5.A — Browser `PlatformServices` (`jeliya-platform-web`)

1. **`WebNavigation: Navigation`.**
   - `route()` → `Route::parse(window.location.pathname).unwrap_or(Route::Rooms)`.
   - `navigate(route)` → `history.pushState(null, "", url_for(route))`, where `url_for` preserves `search` and `hash` (query/fragment are not navigation state; a redirect that dropped `?daemon=`/`?mock…`/`?boot=` would repoint the daemon or unfixture the e2e suite — the retiring `history.ts` contract).
   - `navigate_replace(route)` → `history.replaceState(...)` (D4).
   - Re-navigating to the path already shown pushes no duplicate entry (Back must not become a double-press no-op), but a replace still runs.
   - `hand_back_to_platform()` → on web, delegate to the browser (`history.back()` if there is in-app depth, else allow default). Web Back is the browser's native popstate; the platform-Back binding is Android's (#193).
   - On `popstate`, emit `LifecycleEvent::NavigationRequested { route: Route::parse(path).unwrap_or(Route::Rooms) }` on the shared `LifecycleBus`, so the router folds back/forward reactively (pushState/replaceState do not fire popstate, so in-app navigation updates the mirror signal directly — §5.C).

2. **`WebLifecycle: Lifecycle`.** Owns one `LifecycleBus`. Wires browser events:
   - `popstate` → `NavigationRequested{route}` (above).
   - `visibilitychange`/`pagehide`/`focus`/`blur` → `Resumed` / `Backgrounded{phase}` (best-effort mapping; the browser has no `ProcessRestored` and no window events — those stay native/Android/desktop).
   - Deep-link / external route changes arrive as `popstate`/initial load, not a bespoke event.
   Closures are leaked or held for the tab lifetime exactly as the existing e2e hook does (`compose.rs::install_e2e_connection_hook`).

3. **`WebPreferences: Preferences`** = `prefs::PreferenceSchema` over an `InMemoryBackend` (§6). `durability()` → `SessionScoped`; every `set`/`remove` returns `WriteOutcome::SessionOnly`. On construction, runs the **legacy purge** (§6.5).

4. **`WebSecretStore: SecretStore`** — in-memory `BTreeMap<SecretKey, Secret>`, `SessionScoped`. Holds only the browser session credential and invite tickets (never the daemon token — §K5). Dies with the tab.

5. **`stubs.rs`** — `Files`, `Clipboard`, `Share`, `UrlLauncher`, `WindowActions`, `PrivateDirectory` return `Availability::Unavailable` / `CapabilityError::Unavailable` for out-of-scope capabilities. `UrlLauncher`/`Clipboard` may be given real minimal impls if a Settings link/copy needs them (identity-id copy uses the clipboard — implement `Clipboard::write` via `navigator.clipboard`; keep `Share`/`Files`/window stubbed). Each stub matches the shape's fact (`Shape::Browser` facts in `jeliya-platform`).

6. **`WebPlatform: Platform`** assembles the accessors and is wrapped by `PlatformServices::new(Arc::new(WebPlatform::new(...)))`.

### 5.B — Compose/injection wiring (`jeliya-ui`, `web` feature)

- `web_composition()` injects `jeliya_platform_web::WebPlatform` instead of `PlatformServices::fake_browser()`.
- Keep the deterministic **mock `ClientHandle`** until #171. The shell is transport-agnostic; when `WsWeb` lands, only this line changes (`compose.rs` already documents the swap point).
- `platform_locale()` and `on_locale_lang` stay as they are (#177). The router and bootstrap read `services.navigation()` / `services.lifecycle()` / `services.preferences()`.

### 5.C — Router (`shell/router.rs`, host-testable)

`use_route(services: PlatformServices) -> (Signal<Route>, Navigate)`:

1. Seed `Signal<Route>` from `services.navigation().route()`.
2. **Canonicalize once at mount** using *replace*: if the raw pre-parse path is `/` or fails to parse, `navigate_replace(Route::Rooms)`; if it parsed to a `/rooms/:id` bare room, the parse already normalized to Activity, and `to_path` gives the canonical spelling — replace to it so the address bar shows the canonical form. Because canonicalization is *replace*, Back never lands on a non-canonical URL the user never typed intent for.
3. Subscribe to `services.lifecycle()`; fold each `NavigationRequested { route }` (browser Back/Forward/deep-link) into the signal.
4. `Navigate(route, replace: bool)` calls `navigate`/`navigate_replace` and updates the signal (pushState/replaceState do not fire popstate, so the mirror must be updated by the caller — the retiring `useRoute` invariant).
5. **Last-room restore (contract rule 4, §"Preferences" row 1):** at launch only, and only when the raw path is the **bare root** `/` (not an explicit `/rooms`), read `Preferences::LastRoom`; if present, `navigate(Route::Room{ id, Activity })` **pushed on top of** the already-replaced `/rooms`, so Back leaves the room for the list. An explicit route always wins (no restore off `/rooms` or any deeper path). On the browser, `LastRoom` is `SessionScoped` and empty on a fresh tab, so restore is a no-op in production but is fully implemented and tested with a seeded preferences fake.

Fail-safe rule (D7): `Route::parse` is strict and returns `Err` for malformed/unknown paths; the router maps `Err` → `Route::Rooms` + a replace to `/rooms`. This is the "unknown URL is a recoverable state, not an error page" contract, expressed through the stricter typed parser rather than the retiring React total parser.

### 5.D — Bootstrap state machine (`shell/bootstrap.rs`, host-testable)

A pure fold from `(client lifecycle State, connection snapshot, room.list result)` to a `BootView`:

```
enum BootView {
  Booting,                       // client not yet Ready OR snapshot not yet known -> route's LOADING state
  Onboarding(OnboardStep),       // Ready + subject Absent (identity) OR subject Present + zero rooms (rooms)
  Shell,                         // Ready + subject Present + >=1 room (or user advanced past onboarding)
  Failed(FailureView),           // Stopped/Failed -> real error + way forward (retry/diagnostics/reset)
  Recovered(RecoveryView),       // corrupt/unsupported new-format preference state was reset (visible banner)
}
enum OnboardStep { Identity, Rooms }
```

Rules (contract §"Bootstrap and onboarding"):
- **Booting is *unknown*, not *zero*.** While `State` is `Idle`/`Connecting` or the snapshot is unknown, render the route's loading state (the existing `BootScreen` cover / per-destination loading), never an empty timeline or "0 rooms".
- **First run creates or connects an identity** and offers the optional device-label field beside it; the cryptographic `subject_id` is present and copyable, described as the unrecoverable P2P identity (from `SubjectEnsureOut.subject_id` or the snapshot's `SubjectState::Present.subject_id`).
- **First-room creation is the terminus**: onboarding ends with the user holding a room they created (`room.create`) or joined (join-by-ticket, when the join flow lands — for #178 the create path is the terminus; the ticket field is present and validated but its full ret«`joinRoomWithRetry`» flow is #179-adjacent — see Open Questions Q3).
- **A failed bootstrap surfaces the real failure with a way forward** — retry, diagnostics (the existing `StatusFooter`→`DiagnosticsDialog`), or the reset path — never an infinite spinner. The terminal `Failed`/`Stopped` covers already exist (`app.rs` `boot_target`); #178 adds the reset/retry affordances.
- **Fresh-state/reset policy, user-facing half:** on the browser, the "known legacy preference key" case is the enumerated React `localStorage` keys — removed unconfirmed by the boot-time purge (§6.5). The "unverified old data directory" case is **not a browser concern** (the browser owns no data directory); it belongs to desktop/Android bootstrap (#184/#190, spike #160). The browser's analogue reset is the **preference-schema** reset (§6.4): corrupt/unsupported new-format preference state → reset to defaults behind a visible banner, never auto-interpreted.

### 5.E — Onboarding (`components/onboarding.rs`)

Ported behavior from the retiring `ui/src/components/Onboarding.tsx`, re-expressed against v2 ops and the catalog:
- **Identity step:** a full-page landmarked surface (`<main class="onboarding">`), the wordmark as the `h1`, an optional self-label field (writes `PreferenceKey::SelfLabel`; validation: trim, empty clears, soft 40-char max — contract §"Identity, aliases, and self label"), and a "create identity" action calling `SubjectEnsure {}`. On `created:false`/`subject_exists`, just advance (someone/another tab already created it). The `subject_id` is shown shortened, fully copyable, described as unrecoverable.
- **Rooms step:** two equal tasks — **create a room** (`room.create`, non-whitespace name required; a local name collision warns and proceeds — contract §"Room identity and homonyms") and **join with a ticket** (the ticket field present, split/validated; the retrying join flow is deferred — Q3). Onboarding advances to the room the user now holds.
- Copy is catalog-only (no literal strings — the #177 literal-copy gate).

### 5.F — Global shell and destinations (`components/global_nav.rs`, `room_shell.rs`, `fleet.rs`, `settings.rs`)

- **Global-destination nav:** Rooms / Agent Fleet / Settings, rendered through the `NavLandmark` primitive (Decision-6), with the active destination derived from `Route`. Placement is shell-dependent (§9): a persistent rail (wide/medium) vs. a compact bottom bar carrying **only** global destinations.
- **Rooms:** the existing room list (`RoomListItem`) becomes navigable — selecting a room calls `Navigate(Route::Room{ id, Activity })`. The empty/loading/failed states already follow the truthful-state rules (`app.rs`).
- **Room shell skeleton:** when the route is `Route::Room{..}`, render the room header (room context — name + short-id disambiguator, always visible on every room-scoped surface) and the room-destination **nav strip** (Activity / People / Agents / Files / Pipes) whose items navigate to the corresponding routes. The pane **content** is a per-destination skeleton/loading placeholder (Timeline/People/Files land in #179–#181). A route naming an unreachable/departed room renders the recoverable state (Rooms as the way out; the signed left/removed fact stated plainly — contract rule 6 / invariant 5 floor), not a blank panel.
- **Agent Fleet:** the `/fleet` destination renders a skeleton with the truthful loading/empty states; its actionable-attention content is #180.
- **Settings:** the `/settings` destination renders (a) the **identity** surface (`subject_id` shortened + copyable, described as unrecoverable; self-label editor sharing onboarding's validation and stating "on this device, never sent"); (b) **language settings** — the live text-locale and formatting-locale switchers that assign the resolved-locale context signal (`use_locale_context`, already provided by `app.rs`; #178 is the "later slice" that adds the switch UI). Selecting a locale writes `PreferenceKey::TextLocale` / `FormattingLocale` and applies live (no reload), and honestly reflects `WriteOutcome` durability ("applies this session, not saved" on the browser). Diagnostics remains the `StatusFooter` disclosure.

### 5.G — l10n catalog additions (`crates/jeliya-ui/src/l10n/{mod.rs,en.rs,fr.rs}`)

Add `Catalog` trait methods (declared once in the trait, implemented in both `En` and `Fr`, so `rustc` enforces key/placeholder parity — #177's mechanism) for: global-destination names (Rooms/Fleet/Settings) and their `aria-label`s; onboarding copy (identity title/body/actions, rooms create/join labels, ticket help/example, self-label help); settings section headings; the reset/recovery banner copy; the room-destination strip labels (reuse existing where present). Node-side #177 gates (empty value, `fr==en`, French typography, literal scan) apply unchanged.

---

## 6. Fresh browser preference schema (`prefs/`)

### 6.1 Namespace and version

- **Namespace:** a single constant prefix that **no retiring client ever wrote** — e.g. `jeliya.dx.v1` (the retiring React keys were bare `jeliya.lastRoom`, `jeliya.aliases.v1`, `jeliya.roomFlags`, `jeliya.lastSeen`, `jeliya.locale`, drafts; the Flutter store was `app_prefs.json`). The concrete string is fixed here and verified by a test to be disjoint from `legacy::LEGACY_WEB_KEYS`.
- **Schema version:** an integer `SCHEMA_VERSION` (start at `1`). It gates the envelope, not the namespace: a future breaking schema bumps `SCHEMA_VERSION` and either migrates within the new generation or resets (never reads a foreign version as valid).

> This fixes the **web** namespace only. #185 fixes the desktop store and *its* version key; #173 fixes the Android data directory. The envelope format below is a shared *pattern* the persisting platforms may reuse, but each names its own namespace (contract §"Preferences", final paragraph).

### 6.2 Key derivation

Each typed `PreferenceKey` maps to a concrete backend string key under the namespace, e.g.:

| `PreferenceKey` | Backend key (illustrative) |
|---|---|
| `LastRoom` | `jeliya.dx.v1.lastRoom` |
| `Draft{room_id}` | `jeliya.dx.v1.draft.<encoded room_id>` |
| `Aliases` | `jeliya.dx.v1.aliases` |
| `SelfLabel` | `jeliya.dx.v1.selfLabel` |
| `Pinned{room_id}` | `jeliya.dx.v1.pinned.<encoded room_id>` |
| `Archived{room_id}` | `jeliya.dx.v1.archived.<encoded room_id>` |
| `LastSeen{room_id}` | `jeliya.dx.v1.lastSeen.<encoded room_id>` |
| `TextLocale` | `jeliya.dx.v1.textLocale` |
| `FormattingLocale` | `jeliya.dx.v1.formattingLocale` |

Room-id embedding uses the same percent-encoding discipline as routes (`encodeURIComponent`-identical) so an exotic id cannot break the key or collide with another. The mapping is total and closed over the `PreferenceKey` enum — a component cannot express a legacy or arbitrary key.

### 6.3 Envelope

Each stored value is a versioned envelope (a small JSON object) so a bare or foreign value is detectable:

```
{ "v": <SCHEMA_VERSION>, "value": <string> }
```

`get` reads the raw backend string, parses the envelope, checks `v == SCHEMA_VERSION`, and returns `value`. `set` writes the envelope. On the browser the backend is in-memory, so the envelope roundtrips within a session; the envelope's purpose is the recovery gate (§6.4) and forward-compatibility for persisting platforms.

### 6.4 Corrupt / unsupported-version recovery (visible)

For any key whose raw value:
- **fails to parse** as an envelope (corrupt), or
- carries `v > SCHEMA_VERSION` (**unsupported new-format** — written by a future app the user downgraded from), or
- carries `v < SCHEMA_VERSION` with no defined migration,

the schema layer:
1. does **not** interpret the value as state (returns the deterministic default for that key),
2. **removes** the offending key from the backend (so it cannot poison a later read),
3. records a `SchemaAnomaly` fact (which key(s), why) surfaced to the boot layer, which renders a **visible recovery banner** (`components/recovery.rs`) offering a plain "your local preferences were reset" explanation and an explicit "reset local preferences" action that clears the whole namespace to defaults.

Because the shipped browser backend starts empty each tab, production almost always takes the pure-defaults path; the corrupt/unsupported path is deterministically reachable in tests by seeding an `InMemoryBackend` with corrupt/future-version bytes, and is the exact recovery the persisting platforms (#185/#173) will exercise for real. Recovery **never** deletes or migrates an *unverified old data directory* — that is a native concern the browser does not have (D2/§5.D).

### 6.5 Legacy-key purge (removal, never a reader)

`legacy::LEGACY_WEB_KEYS` enumerates the known retiring-client `localStorage` keys (the React families: `jeliya.lastRoom`, `jeliya.aliases.v1`, `jeliya.roomFlags`, `jeliya.lastSeen`, `jeliya.locale`, per-room draft keys by known prefix, and the legacy `?tab=` affordance which is a *query* not a storage key and is simply never read). On `WebPlatform` construction, `WebPreferences` **removes** each enumerated legacy key from `localStorage` (best-effort; `Storage::remove_item`). This is the contract's "a known, enumerated legacy preference key is ignored or explicitly removed — removal needs no confirmation." Constraints:
- The purge removes **only** keys in the closed allowlist; it never enumerates-and-deletes arbitrary keys, and it never touches non-`jeliya`-prefixed keys.
- No value is ever **read** — the purge is write-only (`remove`). There is no code path anywhere that constructs a *reader* of a legacy key, a legacy query format, a legacy draft, or a legacy profile (AC-6). A test asserts the string `getItem`/read of any legacy key appears nowhere in the shipped shell (a source-scan gate, mirroring #177's literal gate).

### 6.6 Deterministic first-run defaults (documented)

| Preference | Fresh default | Resolution |
|---|---|---|
| `LastRoom` | unset → no restore | Restored once per launch, only from bare root, always loses to explicit route |
| `Draft{room}` | `""` | Composer empty (composer content is #179) |
| `Aliases` | unset → self renders localized **"You"**; peers render `shortId(id)` | Per identity id; never signed; excluded from diagnostics |
| `SelfLabel` | unset → **"You"** | Trim; empty clears; soft 40-char max |
| `Pinned{room}` | `false` | "on this device" |
| `Archived{room}` | `false` | "on this device" |
| `LastSeen{room}` | unset → seeded to the room's recency when it first appears with events | One timestamp/room; advanced on view; never on the wire |
| `TextLocale` | unset → follow platform preferred languages → **English** | Applies live; `LocaleState::resolve` (already implemented) |
| `FormattingLocale` | unset → follow platform locale → resolved text locale | Independent of text locale from day one; applies live |

These are the "documented defaults" of AC-4. The locale rows reuse the already-implemented `LocaleState::resolve` precedence (`app.rs`); #178 adds the switch UI that writes the two keys.

---

## 7. Routing details (traceable to contract rules)

| Contract rule | #178 implementation |
|---|---|
| `/`→`/rooms`; `/rooms/:id`→`/rooms/:id/activity` | `Route::parse` already canonicalizes; router replaces the URL to the canonical `to_path` at mount |
| `:roomId` is the protocol `room_id`, verbatim + percent-encoded | `Route::to_path` uses `encodeURIComponent`-identical encoding (already implemented, byte-tested) |
| Query/fragment are not navigation state; redirects preserve the query | `url_for` preserves `search` + `hash` (§5.A) |
| An explicit route always wins over a restored room; restore once per launch, only from bare root; pushed on top of Rooms | §5.C step 5 |
| Back is truthful: room dest → Activity → Rooms → leave app; Back never mutates unseen state | Correct push/replace discipline + browser history depth; restore pushes the room on top of Rooms so Back leaves the room |
| A route naming an unreachable room → recoverable state, never blank/error | Room shell renders the recoverable states (not-on-device → Rooms; signed left/removed fact; failed open → real error + Retry + Rooms; still-booting → loading) |
| Malformed/unknown path → recoverable Rooms | Strict `Route::parse` Err → `Route::Rooms` + replace |
| No `?tab=` affordance, no legacy persistence reader | The query is never read; §6.5 |

---

## 8. Responsive shells and navigation (matches #58 / contract §"Responsive shells")

- **Breakpoints (single source, mirrored):** `COMPACT_MAX = 899.98`, `WIDE_MIN = 1280` (fractional `899.98` so a scaled/zoomed fractional width cannot fall between the compact and medium media queries — the retiring `shell.ts` invariant). `shell::shellFor(width) -> Shell` and the two media-query strings live in `shell/mod.rs`; a test parses `ui/src/styles.css` and fails if the CSS breakpoints and these constants disagree (the retiring `shell.test.ts` guard, ported).
- **CSS owns the layout.** The three-shell grid (which panes show, and where) is CSS in the single canonical `ui/src/styles.css` (consumed byte-identically per #176/#177). #178 reuses the `.app` grid and its compact/medium/wide media queries; it does not fork layout in Rust.
- **The one Rust fork (Decision-6, injected — D6):** the compact global-destination **bottom bar** vs. the wide/medium **rail/header** are different elements (rendering both and hiding one would put two nav landmarks / two room titles in the a11y tree). `AppRoot` receives the current `Shell` as an injected reactive input (the web target subscribes `matchMedia` in `jeliya-platform-web`/compose and pushes updates; other targets provide their own), and selects the element — never a `cfg`.

| Shell | Width | Nav topology |
|---|---|---|
| Compact | `< 900px` | One pane at a time; global destinations in a bottom bar (global only); room destinations via nested nav inside Rooms |
| Medium | `900–1279px` | Room rail + workspace; room-destination inspector opens as a dismissible drawer (drawer content is #180/#181; #178 wires the strip + route) |
| Wide | `≥ 1280px` | Room rail + workspace + inspector as a third column |

Binding rules retained: connection status reserves layout space and is announced once through one live region (already true — `app.rs`); the compact bottom bar carries only global destinations; room context stays visible on every room-scoped surface at every width; 44px touch floor / 58px tab-bar minimum (grow with text scale) unchanged (#177 a11y foundation).

---

## 9. Security and correctness

- **No token, no unnecessary identity, in browser storage.** `WebSecretStore` holds only the tab-scoped session credential + tickets, in memory, dying with the tab; the daemon token never enters it (§K5). `WebPreferences` holds no credential material. Diagnostics exclude the self-label and redact full identities (existing rule).
- **Legacy storage is never trusted as Dioxus state.** No reader of any legacy key/query/draft/profile exists (§6.5); the only legacy interaction is a write-only, allowlisted removal. A source-scan test enforces the absence of a reader (AC-6).
- **Fail safe + visible.** Malformed routes and corrupt/unsupported new-format state resolve to recoverable, visible states (§6.4, §7); nothing old-generation is silently reinterpreted (contract §"Bootstrap").
- **Bootstrap reflects daemon truth, not a guess.** Identity presence and (if D2 lands) storage generation come from `Hello`/the connection snapshot; booting is rendered as *unknown*, never as *zero* (no empty timeline, no "you are alone", no "0 rooms" before an answer).
- **Preference-write honesty.** Every browser write reports `SessionOnly`; the UI says "applies this session, not saved" rather than implying persistence (`WriteOutcome`, `Durability` are read, not guessed).
- **The legacy purge is bounded and destructive-by-authorization.** Removing enumerated legacy keys is explicitly authorized by the contract ("removal … needs no confirmation") and is strictly limited to the closed allowlist; it never enumerates-and-nukes arbitrary storage and never reads values.

---

## 10. Test strategy

The canonical gate is `cargo` + the wasm/Playwright web suite already established in #176/#177 (`crates/jeliya-ui/e2e/*`, `scripts/check-jeliya-ui-wasm-graph.sh`, the design-token/l10n gates). Add:

### 10.1 Host unit tests (pure modules, `cargo test`)
- **Route/router:** canonicalization (`/`→`/rooms` replace; `/rooms/:id`→Activity), fail-safe (malformed/unknown → Rooms + replace), push-vs-replace intents, no-duplicate-entry on same-path navigate, last-room restore (seeded prefs fake): restores only from bare root, loses to explicit routes, pushes on top of Rooms. (The `Route::parse`/`to_path` round-trip and encoding are already covered in `jeliya-platform`.)
- **Preference schema fixtures:** key derivation totality/closedness; envelope roundtrip; corrupt value → default + removal + `SchemaAnomaly`; `v > SCHEMA_VERSION` → recovery; namespace disjoint from `LEGACY_WEB_KEYS`; deterministic first-run defaults table; `set`/`remove` report `SessionOnly`.
- **Legacy purge:** removes exactly the allowlist, touches nothing else, never reads; a source-scan test asserts no legacy-key reader exists in the shipped shell.
- **Bootstrap state machine:** the fold `(State, snapshot, room.list) → BootView` — booting→loading (unknown≠zero), Absent→Identity step, Present+0 rooms→Rooms step, Present+≥1→Shell, Stopped/Failed→Failed with way forward, corrupt-prefs→Recovered banner.
- **Shell selection:** `shellFor` at 360/899/899.98/900/920/1280 (fractional-breakpoint coverage) and the CSS-vs-constant parity test against `ui/src/styles.css`.

### 10.2 Component/render tests (against the mock + `WebPlatform` in-memory)
- Onboarding identity/rooms steps drive `SubjectEnsure`/`room.create` against the mock; the subject id renders shortened + copyable.
- Settings locale switch writes the two keys and applies live (the resolved-locale context changes with no reload; `<html lang>` tracks it — already wired).

### 10.3 Browser e2e (Playwright, `crates/jeliya-ui/e2e/`)
- **Deep links + back/forward:** load `/rooms/:id/people`, assert the route renders; browser Back/Forward walk Activity→Rooms→leave; a malformed path recovers to `/rooms` (URL replaced).
- **Deterministic first-run defaults:** a fresh tab (empty in-memory store) shows English (or the platform language), no restored room, no drafts, no unread dots.
- **Corrupt-new-state recovery:** seed the store (via a marker-gated fixture, like the existing `?boot=`/`?rooms=` fixtures in `compose.rs`) with a corrupt/future-version envelope; assert the visible recovery banner and reset-to-defaults.
- **Responsive fractional breakpoints:** at 360/899/900/920/1280 (and 899.98 fractional), in **English and French**, assert the correct nav topology (compact bottom bar = global only; medium/wide rail), the 44px/58px floors, and no overflow (French first).
- **No-legacy-reader / purge:** pre-seed `localStorage` with legacy React keys; assert they are removed on boot and never read as state (the shell shows fresh defaults, not restored legacy state).

### 10.4 Focused-first guidance
Run the pure host tests (`cargo test -p jeliya-ui`, `cargo test -p jeliya-platform-web` host portions) and the relevant e2e spec first; reserve the full web build + Playwright matrix for the review gate. `web-sys` binding shims are proven only in-browser (`wasm-bindgen-test`/Playwright), never on the host.

---

## 11. Acceptance-criteria traceability

| AC (issue) | Where satisfied |
|---|---|
| Bootstrap/onboarding reflect daemon truth | §5.D, §5.E, D2 (`Hello.subject`/snapshot + `SubjectEnsure`); booting = unknown≠zero |
| Canonical extensionless routes and browser history work | §5.A, §5.C, §7 (`Route` model + History-API `Navigation` + popstate) |
| Wide/medium/compact navigation matches #58 | §8 (breakpoints, CSS layout, injected `Shell` fork, nav topology) |
| Fresh preferences initialize documented defaults | §6.6 (defaults table) + tests §10.1 |
| Corrupt/unsupported new-format state offers visible safe recovery | §6.4 + `components/recovery.rs` + tests §10.1/§10.3 |
| No legacy key/query/profile reader exists | §6.5 (write-only enumerated purge; source-scan test) + non-goals |

---

## 12. Risks

- **R1 — The `ClientHandle` seam does not surface `Hello.subject`/`storage_generation` (D2).** Highest-impact dependency. Mitigation: coordinate the additive snapshot with #270; ship #178's shell/router/prefs against the mock now (snapshot scripted); fall back to idempotent `SubjectEnsure` for identity presence if the seam extension slips, and defer storage-generation reset to the native platforms that own a data directory.
- **R2 — #171 `WsWeb` not merged.** The shell is transport-agnostic and renders against the mock (as #176 does); only the compose line changes when `WsWeb` lands. #178 can be built and reviewed without #171, but the *live* "reflect daemon truth" AC is only fully exercised once #171 provides the real `Hello`. State this honestly in the PR.
- **R3 — `Navigation` trait extension (D4).** Additive defaulted method keeps every existing impl/fake compiling; the `jeliya-platform` fake must record the new effect and its own tests updated. Low risk, cross-crate.
- **R4 — New crate footprint.** `jeliya-platform-web` adds a manifest and must stay out of the `jeliya-ui` shared graph and the MSRV/`--all-targets` host jobs (wasm-only). The wasm-graph guard (`scripts/check-jeliya-ui-wasm-graph.sh`) and `jeliya-platform`'s boundary tests must be extended to cover it. If the team prefers, the module-in-`jeliya-ui` fallback (§3.1) avoids the new manifest.
- **R5 — Scope creep into #179–#181.** The room/Fleet/Settings *content* is out of scope; #178 must ship *skeletons* + routing only. Guard against accidentally implementing the timeline/roster/files here.
- **R6 — Session-scoped honesty regressions.** Any accidental use of `localStorage`/`sessionStorage` for new-format preferences would make a reload wrongly restore state and violate D3. A test asserts the new schema backend is in-memory and that no new-format key is written to `localStorage`/`sessionStorage`.

## 13. Open questions

- **Q1 (owner: #178 + #270):** exact shape and placement of the connection snapshot on the `ClientHandle` seam (`ClientEvent::Connected { subject, storage_generation, resume }` + `connection()` accessor). Resolve jointly with #270 to avoid a parallel `Connected`.
- **Q2:** the concrete namespace string (`jeliya.dx.v1` vs. another spelling) and whether `SCHEMA_VERSION` lives inside the namespace or the envelope only. Recommendation: namespace carries a generation marker (`.v1`) for a hard reset lever; the envelope `v` carries the fine-grained schema version. Both must be disjoint from every retiring key.
- **Q3:** how much of join-by-ticket lands in #178's onboarding rooms step. Recommendation: present + validate the ticket field and wire `room.join`/split, but the full retrying join UX (`joinRoomWithRetry` equivalent) may be an #179-adjacent slice; the create-a-room path is the guaranteed terminus for #178.
- **Q4:** whether the injected `Shell` value flows as a Dioxus context signal from `AppRoot` or as an explicit prop like `platform_locale`/`on_locale_lang`. Recommendation: a context signal (many nested consumers), seeded by an injected initial value + an injected subscription callback, matching the #177 locale-context pattern.
- **Q5:** does the compact drawer for room destinations (medium shell) need any #178 wiring beyond the route + strip, given its content is #180/#181? Recommendation: wire the drawer open/close *as route state* (destination ≠ Activity ⇒ inspector open, per `inspectorDest`), with placeholder content, so the position-preserving open/close contract is testable now.

## 14. Non-goals (restated, to bound the work)

Timeline/composer; Files/Pipes; People/Agents/Fleet *content*; reading or migrating any legacy React key, `?tab=` query, draft, alias, or profile; maintaining React routes; desktop/Android preference persistence (their own namespaces/version keys are #185/#173).
