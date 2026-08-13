# Dioxus web foundation: CSS, design tokens, EN/FR localization, formatting, and accessibility

**Issue:** #177 — `[Dioxus][Web]: Establish CSS, design-token, EN/FR, formatting, and accessibility foundations`
**Program:** #156 (Dioxus clean-slate), milestone **M3 — Web replacement**.
**Blocked by (all landed):** #176 (shared `jeliya-ui` crate + reproducible build), #162 (product behavior contract), #174 (`PlatformServices`).
**Downstream:** #197 (release-evidence owner consumes this foundation's enforced evidence).
**Status of this document:** implementation specification only. No production code is changed by the phase that writes it. The ADW orchestrator owns all git/gh work.

---

## 1. Summary

Make localization, formatting, design tokens, responsive CSS, and accessibility **structural inputs** to the Dioxus port rather than retrofit work. Concretely, for the shared `crates/jeliya-ui` crate:

1. Establish **one canonical Rust-facing EN/FR catalog** with compiler-enforced key/placeholder parity and gate-enforced plural parity, empty-value, and untranslated-value checks.
2. Establish **independently switchable text and formatting locales**, with a single formatting seam (`Formats`) that splits vocabulary (text locale) from numeric/calendar conventions (formatting locale).
3. Enforce **French typography** (U+202F / U+00A0 / U+2019 / U+2026 / guillemets, `octets`, `42 %`) as a gate.
4. Keep **Dioxus consuming the canonical `ui/src/styles.css` and `assets/design-tokens.json` without divergent copies** (already byte-identical from #176) and add the one **Rust-facing token source that CSS cannot express** — the deterministic identity-palette hash — with a cross-client parity fixture.
5. Ship **shared semantic accessibility primitives** (landmarks, headings, focus management, live regions, modal dialog, reduced motion, touch targets) and make them the **only path** for dialogs, navigation, status, and forms.
6. Prove **no critical/serious axe violations** plus keyboard and reduced-motion behavior on the foundation routes at **1440, 920, 390, and 320** widths.
7. Make **catalog, literal-copy, French-typography, and accessibility-foundation checks required** branch-protection contexts (not advisory), and **prove** that configuration.

The Dioxus catalog, token, CSS, semantic-component, and required-check sources become authoritative. React (`ui/`) and Flutter (`app/`) are **requirements-mining input only**; neither is a parity or compatibility authority, and pixel identity is a non-goal (architecture record, Decision-7 / Rejected alternatives).

---

## 2. Current state (verified in-tree at spec time)

- **Shared crate:** `crates/jeliya-ui` exists (#176). Feature-gated: `ui` (renderer-agnostic surface + mock + `jeliya-platform/fake`), `web` (`dioxus/web`, wasm32), `native` (M4 seam stub). Components live in `src/components/mod.rs`; app root in `src/app.rs`; target selection only in `src/compose.rs` + `src/bin/web.rs`. `PlatformServices` is re-exported from `jeliya_platform` (#174) and injected **separately** from `ClientHandle`.
- **Canonical CSS:** `ui/src/styles.css` (4480 lines). Consumed byte-identically by the Dioxus build via `scripts/build-web.sh`; the `jeliya-ui-web` CI job asserts `cmp crates/jeliya-ui/dist/styles.css ui/src/styles.css` and that `dist/.dioxus-artifact` carries `renderer=dioxus-web`. Breakpoints: `min-width:1280px` (wide), `900–1279.98px` (medium), `max-width:899.98px` (compact), `max-width:480px` (narrow), `@media (prefers-reduced-motion: reduce)`. Viewport/safe-area/z tokens (`--vh-full`, `--safe-*`, `--tabbar-h`, `--z-*`) are CSS-only.
- **Design tokens:** `assets/design-tokens.json` is the shared fixture (colors, alpha companions, radii, contrast floors 4.5/3.0, elevation vocabulary, gradient ceiling). `scripts/check-design-tokens.mjs` validates `ui/src/styles.css` against it (declared-value parity, no undeclared `var()`, shadow/side-stripe/gradient absolutes). **It reads only CSS + JSON — no React code — so it already gates the CSS the Dioxus stack renders.**
- **React l10n (retiring, requirements-mining source):** `ui/src/l10n/` — `catalog.ts` (the `Catalog` interface, ~440 keys), `en.ts` (source of truth), `fr.ts` (typed to `Catalog`; missing key = compile error), `formats.ts` (`Formats` class; two-locale split; `Intl`-backed), `locale.ts` (two independent prefs `jeliya.textLocale` / `jeliya.formattingLocale`, unset = follow platform), `wireDisplay.ts`, `errorDisplay.ts`, `destinations.ts`, `tokens.ts` (never-translate), `template.tsx`, `strings.tsx`. Gate `scripts/check-ui-i18n.mjs` (+`.test.mjs`) enforces key parity, empty values, `fr==en` untranslated, French typography, and a component literal scan.
- **React a11y (retiring, requirements-mining source):** `ui/e2e/a11y.spec.ts` (one visible `main`/`h1` per route, named landmarks, skip links that move focus, target-size floors + spacing exception, reduced-motion, document title) and `ui/e2e/a11y-matrix.spec.ts` (axe critical/serious sweep across every destination × 4 viewport projects, using tags `wcag2a wcag2aa wcag21a wcag21aa wcag22aa best-practice`).
- **Dioxus render smoke:** `crates/jeliya-ui/e2e/` (Playwright) serves `dist/` offline (no network, no WebSocket) and asserts the shell mounts, the shared CSS paints (`getComputedStyle(body).backgroundColor` non-transparent), and the mock drives to `Ready`. Config has desktop + one compact (390) project.
- **CI (`.github/workflows/ci.yml`):** `docs-ui` runs `check-design-tokens.mjs` and the React `check-ui-i18n.mjs`; `ui-e2e` (Playwright, **not required**); `jeliya-ui-web` (Dioxus wasm foundation — runs on every PR, **not yet a required check**, per its own comment and the accessibility-checklist "Known gaps"). `main` is PR-only, linear-history, admin self-merge.
- **Component copy today is hardcoded English:** `"Jeliya"`, `"connecting to the local daemon…"`, `"stopping — draining accepted work…"`, `"stopped"`, `"the client failed and will not retry"`, `"No rooms yet"`, `"Loading rooms…"`, `"Choose a room"`, `"Untitled room"`, `"client · {lifecycle}"`, and the raw diagnostic `"room.list: {error:?}"`. There is **no** `<main>`/`<h1>` landmark, skip link, live region, dialog primitive, or formatting seam in the crate yet.

---

## 3. Goals and non-goals

### Goals
- One authoritative EN/FR catalog for the Dioxus stack with key/placeholder/plural parity.
- Independently switchable text and formatting locales, applied live (no restart).
- French typography enforced as a gate.
- No divergent copy of tokens/CSS; one Rust-facing token source for the palette hash CSS cannot express.
- Shared semantic primitives that are the only path for dialogs/navigation/status/forms.
- No critical/serious axe violations, plus keyboard and reduced-motion correctness, at 1440/920/390/320.
- Catalog, literal-copy, French-typography, and accessibility-foundation jobs are **required** checks, proven.

### Non-goals (verbatim from the issue, expanded)
- Pixel-identical rendering vs React/Flutter.
- **A third hand-maintained translation catalog.** The Dioxus catalog is *the* authoritative catalog for the surviving stack; it does not add a third parallel catalog to maintain alongside React + Flutter (which retire under #200/#201/#202).
- Claiming WCAG certification (results are **enforced evidence**, not certification — architecture Decision-6, accessibility-checklist).
- Porting every product surface. Scope is the **foundation routes** the mock renders plus the primitives, not the full Room Workbench (that is later M3 slices).
- Depending on legacy React/Flutter jobs after the Dioxus replacements pass.

---

## 4. Key design decisions

### D1 — Canonical catalog: a typed, hand-authored Rust catalog (recommended), EN source-of-truth, FR compile-checked

**Recommendation:** author the canonical EN/FR catalog as **typed Rust** inside `crates/jeliya-ui/src/l10n/`, mirroring the proven React `catalog.ts`/`en.ts`/`fr.ts` shape:

- A `Catalog` **trait** (or a struct-of-fn-pointers) declares every message once. Plain messages are `&'static str`; parameterized messages are methods with **typed arguments** so a missing/mistyped argument is a compile error (the Rust analogue of `MessageFn<[room: string]>`).
- `en` is the source of truth (reviewed as product copy). `fr` implements the same `Catalog`; **a missing key does not compile** → key parity and placeholder parity are enforced by `rustc`, exactly as `tsc` enforces the React side.
- No runtime i18n dependency. This honors the repo's explicit culture ("`ui/` ships exactly two runtime dependencies… adding one is a decision needing its own rationale"; the QR encoder was hand-vendored to avoid a dep) and the "missing key = compile error" contract the React catalog is praised for.

**Why not codegen from a data file (the "generate" alternative, D1-alt):** a canonical `.ftl`/JSON data file + generated Rust (Flutter-ARB style, drift-checked) is viable and would be **translator-tool-friendly (Weblate)** — which matters for the deferred Bambara/N'Ko roadmap (`docs/i18n.md` §5–6). It is recorded as the leading alternative and the main open question (§14, Q1). For the *foundation* (EN/FR only, no external translators yet), hand-authored typed Rust is lower-machinery and idiomatic; if the maintainer commits to Weblate before Bambara, switch the canonical source to a data file **now** to avoid a later migration.

**Consequence for gates:** because the compiler already enforces parity, the node gates below are **defence-in-depth for the failure modes types cannot see** (empty string, `fr` left in English, plural-category coverage, French typography, hardcoded component literals) — the same rationale `check-ui-i18n.mjs` states for existing behind `tsc`.

### D2 — One shared parser, exposed as separate required jobs
Implement all catalog/literal/typography rules in **one** node module (`scripts/lib/jeliya-ui-catalog.mjs`) with a unit-tested rule set, and expose them as **separate required CI contexts** via a `--only=<rule>` selector on a thin entrypoint `scripts/check-jeliya-ui-i18n.mjs`. This satisfies AC-7's "catalog, literal, typography… jobs" as independently-named required checks while keeping one maintainable implementation. (Rationale for reading Rust source **as text** rather than importing it: a gate that evaluates the thing it gates can be argued out of its own findings — the same decision `check-ui-i18n.mjs` and `check-docs.mjs` already make.)

### D3 — Design tokens: consume CSS unchanged; add a Rust palette source; do not re-encode CSS values in Rust
Dioxus renders through CSS custom properties in the byte-identical `ui/src/styles.css`; it therefore needs **no** Rust duplicate of colors/radii (that would be the exact drift `assets/design-tokens.json` exists to prevent). The **only** token concept CSS cannot express is the deterministic identity-palette **hash** (`colorForId`/`avatarBg`/`tileBg`/`fileTint`), which must produce byte-identical colors across clients "or the same person gets a different avatar colour per device" (`docs/design-tokens.md`). Port those pure functions to Rust and pin them with a **shared cross-client fixture** (`assets/identity-palette-fixture.json`: id → expected color) read by both a Rust test and (while it exists) a React test.

### D4 — Formatting backend: minimal pure-Rust for EN/FR conventions; no `Intl`, no heavy dep in the foundation
`Intl` is unavailable to portable Rust and would force a `cfg`/web-sys fork (forbidden in shared components). For the foundation, implement locale-aware number/date formatting for the **two supported conventions (en, fr)** with a small, tested internal table (decimal separator, group separator incl. fr U+202F, `42 %` narrow-no-break-space, `octets` units, Today/Yesterday, clock). The `Formats` seam **accepts any BCP-47 formatting tag** but resolves unsupported tags to a documented fallback within {en, fr}. Broad CLDR coverage for arbitrary third formatting locales (e.g. `de-CH`, which React got free from `Intl`) is **explicitly deferred to a product-surface slice** and will require a real formatting library (`icu4x`) as a separate, rationale-bearing dependency decision (§14, Q2). Text=fr/format=en and text=en/format=fr are fully expressible, so "independently switchable" holds for the foundation.

### D5 — Locale resolution: platform language injected at composition; persistence via `PlatformServices`
Shared components read only the *resolved* locale from a Dioxus context; they never detect the platform language or touch storage directly (Decision-3: no `cfg` in shared components; platform authority only via injected services). The per-target composition (`compose.rs` / `bin/web.rs`) resolves the initial platform language (browser: `navigator.languages`; native: the OS locale) and injects it; the two persisted preferences are read/written through the injected `Preferences` capability. **Storage key names are deferred to #178's browser namespace** (Decision-2 forbids stating a replacement key name until #178 lands); the spec uses the injected capability and treats the concrete key as a #178 coordination point (§14, Q3).

### D6 — Semantic primitives are the only path
Dialogs, navigation, status, and forms are reachable **only** through the shared primitives (`dialog.rs`, `nav.rs`, `status.rs`, `form.rs`, plus `a11y.rs`/`live_region.rs`). A literal/structure scan forbids raw `role="dialog"`, bare `<dialog>`, ad-hoc `aria-live`, and unmanaged focus outside those primitives, so the accessible behavior cannot be bypassed.

### D7 — Diagnostics carry raw values; primary copy never does; nothing secret reaches the DOM
Unknown wire enum values render through a `WireDisplay` map with **raw passthrough** for forward compatibility (never a fabricated label). Errors render friendly catalog copy in primary UI; the **raw** code/detail lives only in a **Diagnostics dialog** disclosure (which also doubles as the foundation's real exercise of the `dialog` primitive). Diagnostics strings are **scrubbed of secret-bearing fields** (daemon token and any credential material never enter user copy or DOM attributes — architecture Decision-7 boundary #159; reinforced by `scripts/check-secret-storage.mjs`).

---

## 5. Detailed design

### 5.1 Module layout (new)
```
crates/jeliya-ui/src/
  l10n/
    mod.rs        # Catalog trait, Locale enum, Strings/Formats context + hooks (use_strings/use_formats),
                  #  live-switching signal, SUPPORTED_LOCALES, FALLBACK_LOCALE
    en.rs         # source of truth
    fr.rs         # impl Catalog; compile-enforced key/placeholder parity
    plural.rs     # Plural category selection per locale (en: one/other; fr: 0&1→one, else other)
    format.rs     # Formats: text-locale vocabulary + formatting-locale conventions (bytes/clock/day/count/percent/relTime)
    wire.rs       # WireDisplay: enum -> localized label, raw passthrough for unknown values
    error.rs      # ErrorDisplay: friendly copy (+ structured raw detail for the diagnostics disclosure)
    tokens.rs     # never-translate constants (brand, endonyms, shell/wire examples) — outside Catalog by design
    palette.rs    # colorForId/avatarBg/tileBg/fileTint (Rust-facing token source) + fixture-backed parity test
  components/
    mod.rs        # existing exports + the new primitives below
    a11y.rs       # SkipLink, Main(landmark), Heading(order-aware), VisuallyHidden, focus helpers
    dialog.rs     # Dialog primitive: role=dialog + aria-modal, focus trap, Escape->opener, scrim, safe initial focus
    live_region.rs# LiveRegion + use_announce (stable node, announce-once/coalesced)
    nav.rs        # Navigation landmark (named), tablist behavior seam
    status.rs     # Status vocabulary (dot + text label; never color-only; two separate facts)
    form.rs       # Field primitive: label association + optional marker as label fragment
    diagnostics.rs# Diagnostics dialog: raw lifecycle/error detail, secret-scrubbed
```

### 5.2 Catalog contract (AC-1)
- **Keys** follow the React/Flutter `<area><Key>` lowerCamel scheme so a reviewer can line the catalogs up during the migration and words cannot drift.
- **Message kinds:** plain (`&'static str`) and parameterized (typed method args). A sentence with styled/interactive segments is **one** message with slot markers, rendered by a `Template` helper — never fragments concatenated in RSX (i18n.md rule 2).
- **Plurals (`plural.rs`):** a plural message takes the `count` and selects the CLDR category for its locale. English: `n==1 → one`, else `other` (0 → other). French: `0` and `1 → one`, else `other`. A Rust unit test asserts each locale maps `{0,1,2,5}` to the expected category, and the catalog gate asserts every plural key exists in both locales (plural parity).
- **Introspection for gates:** because Rust has no field reflection, the catalog is declared via a small `catalog!` macro (or an explicit `KEYS` slice) that also emits an `entries(&self) -> &'static [(&'static str, Rendered)]` surface so a Rust `#[test]` and the node text-scanner agree on the key set. The node gate reads `en.rs`/`fr.rs` as text (per D2).
- **Foundation copy to migrate now (from §2's hardcoded list):** boot/lifecycle labels, `No rooms yet`, `Loading rooms…`, `Choose a room`, `Untitled room`, the status-footer label, and friendly forms of the room-load failure. `"room.list: {error:?}"` is reclassified as diagnostics (§5.8), not primary copy.

### 5.3 Locale + formatting independence (AC-2)
- `SUPPORTED_LOCALES = [en, fr]`, `FALLBACK_LOCALE = en`.
- Two persisted preferences via injected `Preferences`: text locale (must have a catalog; unset = follow platform primary language, then fallback) and formatting locale (any tag; unset = follow platform, resolved into the supported convention set with fallback to the text locale's conventions).
- A Dioxus context publishes `{ text: Locale, formatting: FormattingLocale, textFollowsSystem: bool }`; `use_strings()` returns the text catalog, `use_formats()` returns `Formats` bound to (text vocabulary, formatting conventions). Every consumer resolves **per render**, so switching either preference applies **live** with no restart.
- Accepted cross-client deviation (carried from React/Flutter so clients agree): byte-unit words (`octets`) and Today/Yesterday/"ago" phrases follow the **text** locale (vocabulary); only numeric/calendar conventions follow the **formatting** locale.
- `<html lang>` tracks the resolved text locale.

### 5.4 French typography (AC-3)
Enforce over every `fr` value (the `localeTag` field excepted), reusing the React rule set exactly:
- U+202F (narrow no-break space) before `;` `!` `?` `%` and inside guillemets `« »`.
- U+00A0 (no-break space) before `:`.
- U+2019 apostrophe (no straight `'`), U+2026 ellipsis (no `...`), guillemets instead of `"`.
- Checks run on the **rendered sentence with slots collapsed to a sentinel**, so a break straddling a `{slot}` boundary is still caught.
- `octets` byte units (`o/Ko/Mo/Go`) and `42 %` spacing live in the catalog/formatter, not inline.
An `IDENTICAL_ALLOWLIST` (key → reason) and a `NEVER_TRANSLATE` lexicon (brand + Tier-2/3 glossary words) permit legitimately identical `fr==en` values; **stale allowlist entries are themselves reported** (a stale exemption hides the next real one).

### 5.5 Design tokens / CSS (AC-4)
- **No divergent copy:** keep the #176 byte-identical consumption of `ui/src/styles.css`; the `jeliya-ui-web` `cmp` check already proves no copy diverges.
- **Token conformance:** keep `scripts/check-design-tokens.mjs` (CSS ↔ fixture) and make it required. It is not React-specific. (Note: when React `ui/` retires under #200 the canonical `styles.css` relocates; update the gate path **in that change**, not here — recorded as a #200 coordination point.)
- **Rust palette source (D3):** `palette.rs` ports the pure hash functions; `assets/identity-palette-fixture.json` pins id→color; a Rust `#[test]` asserts conformance, and (while React exists) a mirrored TS assertion pins the same fixture, guaranteeing identical avatars cross-client. Wiring avatars into product surfaces is a later slice; the foundation ships the function + fixture + test.

### 5.6 Semantic accessibility primitives (AC-5)
The foundation converts the existing shell into a properly landmarked page and adds the primitives, matching the React `a11y.spec.ts` contracts:
- **Landmarks & headings:** exactly one visible `<main>` and one `<h1>` per foundation route; named `nav`/complementary landmarks ("Room rail", inspector names) so landmark navigation can tell panes apart; heading order (h1→h2…) enforced.
- **Skip links:** the first tab stops, invisible until focused, that **move focus** (not just scroll) to `main` / composer; not offered where the target does not exist.
- **Focus:** a visible focus ring everywhere focus can land; no focus trap **outside** a dialog; destructive actions never take initial focus; document title names the destination.
- **Live regions (`live_region.rs`):** a **single stable** polite region for connection transitions and one for new-content announcements, updated by `use_announce` which **coalesces** so a rebuilding list announces **once**, not per re-render (the exact failure mode the checklist warns cannot be caught by automation — so it is designed out structurally and covered by a manual-checklist row).
- **Dialog (`dialog.rs`):** `role="dialog"` + `aria-modal`, focus trapped inside, `Escape` returns focus to the opener, scrim, and initial focus never on a destructive control.
- **Reduced motion:** honor `prefers-reduced-motion` (CSS media query already present); programmatic scroll lands instantly rather than animating.
- **Touch targets:** 44px compact floor (24px documented spacing exception paired with a ≥24px neighbor-spacing rule) via the shared classes; measured by hit-testing in e2e.

**Dioxus API note:** focus movement, focus trap, and instant scroll use Dioxus 0.7.9 portable mounted/element APIs (available in both browser and system-WebView targets), not raw `web-sys` (which would be `web`-only and force a fork). Confirm the exact 0.7.9 surface during implementation (§14, Q4).

### 5.7 Wire/error display (AC security/correctness)
- `WireDisplay` maps known protocol enum values (roles, member statuses, peer paths, daemon/connection states) to localized labels; **unknown values pass through raw** for forward compatibility, and a display label is never the same constant as its wire value.
- `ErrorDisplay` returns friendly catalog copy for known `{code, message, hint}` shapes and preserves the raw structured detail separately for the diagnostics disclosure. Unknown codes get a localized generic fallback while the raw code is preserved in diagnostics.

### 5.8 Diagnostics + secret boundary (AC security/correctness)
- The status footer exposes a **Diagnostics dialog** (built on `dialog.rs`) that shows the raw lifecycle state and the raw failure detail (`CallError` debug, raw wire code) — the correct home for `"room.list: {error:?}"`, which leaves primary copy.
- A **secret-scrub** step guarantees no daemon token or credential material enters user copy or any DOM attribute (title/aria-*), aligned with `check-secret-storage.mjs` and Decision-7 (#159). The catalog literal gate additionally forbids interpolating raw wire/secret values into user-visible copy outside the diagnostics disclosure.

---

## 6. CI gates and required checks (AC-7)

### 6.1 New/updated gate scripts
- `scripts/lib/jeliya-ui-catalog.mjs` — shared parser + rules (parity defence-in-depth, empty value, `fr==en` identical + allowlist staleness, plural-key parity, French typography, component literal scan). Unit-tested by `scripts/check-jeliya-ui-i18n.test.mjs` (mirroring `check-ui-i18n.test.mjs`: fixture trees, rule-level coverage, exemptions passed as parameters).
- `scripts/check-jeliya-ui-i18n.mjs` — thin entrypoint with `--only=catalog|literals|typography` (and default = all), exit 1 on findings.
- `assets/identity-palette-fixture.json` + `palette.rs` test + a TS mirror assertion.

### 6.2 Jobs and required contexts
Add/adjust jobs in `.github/workflows/ci.yml` so each AC-7 concern is an **independently-named required context**:

| Required context (job) | Runs | Cost |
|---|---|---|
| `ui-catalog` | `node scripts/check-jeliya-ui-i18n.mjs --only=catalog` + `node --test scripts/check-jeliya-ui-i18n.test.mjs` | seconds |
| `ui-literal-copy` | `node scripts/check-jeliya-ui-i18n.mjs --only=literals` | seconds |
| `ui-french-typography` | `node scripts/check-jeliya-ui-i18n.mjs --only=typography` | seconds |
| `ui-a11y-foundation` | Playwright axe + keyboard + reduced-motion at 1440/920/390/320 against the offline `dist/` | minutes |

Additionally: keep `check-design-tokens.mjs` (+ the palette-parity assertion) and the `jeliya-ui-web` byte-identical `cmp` as required (AC-4 token/CSS consistency). Compiler-enforced key/placeholder parity rides along in `jeliya-ui-web`'s `cargo test -p jeliya-ui --features ui`.

The three text gates are near-instant, so four separate contexts are cheap; a maintainer who prefers fewer contexts may fold `ui-catalog`/`ui-literal-copy`/`ui-french-typography` into one job — **the acceptance bar is that each concern is inside a required context** (§14, Q5).

### 6.3 Branch-protection change and proof (out of PR diff)
Adding a context to required checks is a repository setting, **not** part of any PR's file diff (the `jeliya-ui-web` job comment and the accessibility-checklist "Known gaps" both state this). The maintainer/orchestrator must:
1. Merge the PR that introduces the jobs (so the context names exist on `main`).
2. Add the four **check-RUN names** to `main` branch protection (and confirm `jeliya-ui-web`). GitHub matches a required status check by its check-run name — each job's `name:` value, **not** the job id — so require these exact contexts (job id in parentheses):
   - `jeliya-ui catalog parity (EN/FR)` (job `ui-catalog`)
   - `jeliya-ui literal copy (no hardcoded UI strings)` (job `ui-literal-copy`)
   - `jeliya-ui French typography` (job `ui-french-typography`)
   - `jeliya-ui accessibility matrix (axe + keyboard)` (job `ui-a11y-foundation`)

   Requiring the job ids (`ui-catalog`, …) instead would never bind — no run publishes those names (see the note at `.github/workflows/ci.yml`).
3. **Prove it:** `gh api repos/kortiene/jeliya/branches/main/protection/required_status_checks` (with `env -u GITHUB_TOKEN` per the local-dev gotcha) must list those four check-run names; capture that output as the required-check evidence #197 consumes.

This phase (spec) and the implementing phase **do not** run git/gh; the spec documents the procedure and the proof command so the maintainer can complete and evidence it.

---

## 7. Accessibility e2e matrix (AC-6)

Extend `crates/jeliya-ui/e2e/`:
- Add `@axe-core/playwright` to `crates/jeliya-ui/e2e/package.json` (audited by the existing `dependency-security` job, which already `npm ci`/`npm audit`s this lockfile).
- Add four Playwright **projects**: `wide` 1440×900, `medium` 920×1000, `compact` 390×844, `narrow` 320×568. Force `reducedMotion: 'reduce'` by default with a `no-preference` override block to prove the two branches differ (as the React suite does).
- `a11y.spec.ts` (structural contracts): exactly one visible `main`/`h1` per foundation route; skip links are the first tab stops and move focus; the Diagnostics dialog traps focus, closes on Escape to its opener, and never initial-focuses a destructive control; the connection-transition live region announces once; compact target-size floors (44px, 24px exception with neighbor spacing) by hit-testing; instant scroll under reduced motion; document title names the destination.
- `a11y-matrix.spec.ts` (sweep): axe with tags `wcag2a wcag2aa wcag21a wcag21aa wcag22aa best-practice`, **failing on any critical/serious** violation, moderate/minor attached as advisory, over each foundation route at all four projects. An empty documented-false-positive list with the "every entry needs a linked rationale" guard test.
- **Foundation routes exercised** (so the sweep is not vacuous): the boot cover, the landmarked rooms shell (empty + loaded-empty states), the empty center, and the Diagnostics dialog. These render real landmarks, an `h1`, skip links, a live region, and a dialog — enough for axe and keyboard rules to mean something.

Runs offline against the mock-driven `dist/` (no network/WebSocket), reusing the existing serve/no-network harness.

---

## 8. Implementation steps (PR-sliceable, ordered)

Each slice keeps `crates/jeliya-ui`'s boundary tests green and adds no Iroh/native crate to the wasm graph.

1. **l10n scaffold + migrate existing copy.** Add `l10n/{mod,en,fr,plural,tokens}.rs`; define `Catalog`, `Locale`, `use_strings`, live-switch context; migrate every hardcoded component string (§2) into the catalog; wire `<html lang>`. Compiler now enforces EN/FR parity.
2. **Formatting seam.** Add `format.rs` with the EN/FR convention table (bytes/clock/day/count/percent/relTime) and the text-vs-formatting split; `use_formats`. Unit tests for `1 234,56`/`1,234.56`, `42 %` with U+202F, `octets`, Today/Yesterday.
3. **Locale resolution + persistence.** Resolve platform language at composition (`compose.rs`/`bin`), inject it; read/write the two prefs via injected `Preferences`; prove live switching (text=fr/format=en and the reverse) in a component test. Coordinate the storage key with #178.
4. **Semantic primitives.** Add `a11y.rs`, `dialog.rs`, `live_region.rs`, `nav.rs`, `status.rs`, `form.rs`; convert the shell to `main`+`h1`+named landmarks+skip links; add the Diagnostics dialog; route all dialog/nav/status/form through the primitives.
5. **Wire/error/diagnostics + secret scrub.** Add `wire.rs`, `error.rs`, `diagnostics.rs`; move the raw failure string into the Diagnostics dialog; friendly primary copy; secret-scrub assertion.
6. **Rust palette token source.** Add `palette.rs` + `assets/identity-palette-fixture.json` + Rust test + TS mirror.
7. **Gates.** Add `scripts/lib/jeliya-ui-catalog.mjs`, `scripts/check-jeliya-ui-i18n.mjs`, `scripts/check-jeliya-ui-i18n.test.mjs`; wire the four rules; make the design-token gate cover the palette fixture.
8. **e2e a11y matrix.** Add `@axe-core/playwright`, the four projects, `a11y.spec.ts`, `a11y-matrix.spec.ts`.
9. **CI jobs.** Add `ui-catalog`, `ui-literal-copy`, `ui-french-typography`, `ui-a11y-foundation`; ensure token/CSS consistency stays required.
10. **Docs.** Update `docs/dioxus-web-build.md` / add an l10n+a11y note; amend `docs/i18n.md`, `docs/design-tokens.md`, and `docs/accessibility-checklist.md` "what CI covers" rows to point at the Dioxus surfaces (keeping the OKF-profile frontmatter and index reachability the docs gate enforces). Record the branch-protection procedure + proof for #197.

---

## 9. Test strategy

- **Compiler:** EN/FR key + placeholder parity (`cargo test -p jeliya-ui --features ui`).
- **Rust unit/component tests:** plural categories, formatter conventions, live locale switching, WireDisplay raw passthrough, ErrorDisplay fallback, palette fixture conformance, secret-scrub.
- **Node gate + gate self-tests:** empty value, `fr==en` untranslated + allowlist staleness, plural-key parity, French typography, component literal scan.
- **Playwright (offline `dist/`):** landmarks/headings, skip-link focus movement, dialog trap/Escape/return, announce-once, target-size hit-testing, reduced-motion, document title, and the axe critical/serious sweep — all at 1440/920/390/320.
- **Boundary tests:** unchanged `tests/boundaries.rs` + `scripts/check-jeliya-ui-wasm-graph.sh` keep the wasm graph Iroh-free/native-free.
- **Focused-first:** run the specific `cargo test`/`node`/`npx playwright test` for the slice; reserve full `verify`/matrix for the final review.

---

## 10. Acceptance-criteria mapping

| AC | Satisfied by |
|---|---|
| One canonical EN/FR source with key/placeholder/plural parity | §4-D1, §5.2 (rustc parity) + §6.1 catalog gate (plural parity, empty, defence-in-depth) |
| Text and formatting locale independently switchable | §4-D4/D5, §5.3 (two prefs, per-render resolution, live switch) |
| French spacing/typography gates pass | §5.4 + `ui-french-typography` job |
| Dioxus consumes canonical tokens/CSS without divergent copies | §5.5 (byte-identical `cmp`, token gate, no Rust CSS duplicate) |
| Shared semantic primitives meet landmark/focus/live-region rules | §5.6 + primitives-only path (D6) |
| Axe: no critical/serious on foundation routes | §7 `a11y-matrix.spec.ts` at 4 viewports |
| Catalog/literal/typography/a11y jobs are required checks | §6.2–6.3 (four required contexts + proof) |

Security/correctness: unknown wire values → localized fallback with raw preserved in diagnostics (§5.7–5.8); no secret in copy/DOM (§5.8). Platform applicability: shared UI, first exercised on web.

---

## 11. Risks and mitigations

- **Formatting scope creep / dependency pressure.** Deferring arbitrary formatting locales keeps the foundation dep-light; the `icu4x` decision is isolated to a later slice with the seam already shaped to accept any tag. *Mitigation:* the two-locale table plus documented fallback; open question Q2.
- **Vacuous a11y gate.** If the foundation renders too little, axe/keyboard rules prove nothing. *Mitigation:* the foundation renders real landmarks, an `h1`, skip links, a live region, and the Diagnostics dialog (§7).
- **Announce-once regressions.** Automation cannot hear duplicate announcements. *Mitigation:* structural design (stable node + coalescing `use_announce`) plus a manual-checklist row; do not claim automation covers it.
- **Required-check illusion.** A job that runs but is not in branch protection does not block merges. *Mitigation:* explicit proof step (§6.3) captured as #197 evidence.
- **Storage-key collision with #178.** Hardcoding a namespaced key now could conflict. *Mitigation:* use the injected `Preferences` capability and defer the concrete key to #178 (Q3).
- **Docs gate.** New/edited docs must keep exactly the OKF-profile frontmatter and index reachability. *Mitigation:* run `node scripts/check-docs.mjs` in the slice.
- **CSS relocation under #200.** The token gate path (`ui/src/styles.css`) changes when React retires. *Mitigation:* documented #200 coordination point; not changed here.
- **Adversarial-review load.** In-repo experience is that review finds real defects in just-written spec/gate text; budget a fix round and verify self-referential counts with a script.

## 12. Rollout / rollback
- Additive and behind the existing `ui`/`web` features; the default and MSRV builds stay renderer-free. No release-line change (that is #200); `ui/` and its React gates remain intact during coexistence.
- **Rollback:** the new jobs and gates can be dropped from branch protection without touching the retiring React gates; the crate changes are self-contained in `crates/jeliya-ui` + `scripts/` + `assets/`.

## 13. Out of scope (deferred)
Full Room Workbench product surfaces; arbitrary third formatting locales / `icu4x`; Bambara/N'Ko catalogs and Weblate; avatar wiring beyond the palette function + fixture; the sealed content-addressed manifest (#183); the release-line cutover and React removal (#200); the system-WebView security/lifecycle/a11y matrix (#189/#196).

## 14. Open questions
1. **Q1 — Canonical source shape.** Hand-authored typed Rust (recommended) vs a data-file (`.ftl`/JSON) + generated Rust. If Weblate/external translators are committed before Bambara (i18n.md §5), prefer the data file **now** to avoid a later migration.
2. **Q2 — Formatting backend.** Is the EN/FR-only convention table acceptable for the foundation, with `icu4x` (or equivalent) deferred to the first product-surface slice that needs an arbitrary formatting locale?
3. **Q3 — Storage keys.** Which persisted preference keys, given #178 owns the browser namespace and forbids naming a replacement key until it lands? Coordinate ordering with #178.
4. **Q4 — Dioxus 0.7.9 a11y APIs.** Confirm portable focus (`set_focus`), focus-trap, and instant-scroll surfaces exist for both browser and system-WebView targets without a `web-sys`/`cfg` fork.
5. **Q5 — Required-context granularity.** Four separate required contexts (recommended, cheap) vs folding the three text gates into one context. Acceptance requires each concern to sit inside *a* required context.
6. **Q6 — Palette scope.** Ship the palette hash + fixture + test now (recommended, guards cross-client identity early) or defer entirely until an avatar-rendering surface exists?

## 15. Assumptions
- #176, #162, #174 are landed (architecture record confirms); this issue is unblocked.
- `PlatformServices::preferences()` exposes a get/set string store with a durability distinction usable for the two locale prefs (confirm the exact `Preferences` API during slice 3).
- `main` remains PR-only, linear-history, admin self-merge; branch-protection edits are a maintainer/orchestrator action outside any PR diff.
- The canonical `ui/src/styles.css` remains the single stylesheet source until #200; its breakpoints (1280 / 900–1279.98 / 899.98 / 480) and reduced-motion media query are the responsive base the foundation ports.
- `@axe-core/playwright` in `crates/jeliya-ui/e2e` is covered by the existing `dependency-security` audit of that lockfile.
