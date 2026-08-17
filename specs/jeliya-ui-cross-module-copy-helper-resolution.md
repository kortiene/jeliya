# Spec — Cross-module copy-helper resolution for the jeliya-ui literal-copy gate (#275)

- **Issue:** kortiene/jeliya#275 — `[Rust][jeliya-ui] Cross-module copy-helper resolution for the literal-copy gate`
- **Split out of:** #177 (`specs/dioxus-web-l10n-tokens-a11y-foundations.md`). Round 26 of #177 landed a first-cut cross-file index (`copyReturningFnNames` + a name-only `crossFileCopyFns` set); round 27 review showed it inadequate and it was **reverted**. This issue does it correctly.
- **Surface owned by this issue:** `scripts/lib/jeliya-ui-catalog.mjs` (the shared gate implementation) and `scripts/check-jeliya-ui-i18n.test.mjs` (its companion tests). No Rust production code changes; the crate is only fixture material.
- **Gate this feeds:** the `ui-literal-copy` CI context (`node scripts/check-jeliya-ui-i18n.mjs --only=literals`), one of the three independently-required jeliya-ui i18n contexts.
- **Owner role:** UI/foundations maintainers (the #177 owners).
- **Status of this document:** planning/spec only. **No production code is to be written for this issue by the planning phase.**

> Where this spec and the actual behavior of `scripts/lib/jeliya-ui-catalog.mjs` disagree, the code is authoritative and this spec has a bug — say which in the PR. The gate reads the Rust catalogs and components **as text** on purpose (a gate that imports the crate it gates can be argued out of its own findings, and `scripts/` has no Rust build step); this issue keeps that property.

---

## 1. Outcome

Make the jeliya-ui literal-copy gate resolve copy helpers **across modules**, so copy hidden in a helper in one file and invoked via a qualified path from another —

```rust
// crates/jeliya-ui/src/app.rs
div { {crate::state::hardcoded()} }

// crates/jeliya-ui/src/state.rs
pub fn hardcoded() -> &'static str { "Delete account" }   // must be FLAGGED
```

— cannot bypass the gate. The literal `"Delete account"` lives in `state.rs`, outside any `rsx!` block and never assigned to an interpolated binding, so today it is invisible to the gate: the per-file scan of `app.rs` collects the call `crate::state::hardcoded()` as a copy helper but can only resolve `fn` bodies **inside `app.rs`**, and the per-file scan of `state.rs` sees a literal that is neither in RSX nor in a locally-invoked copy-helper body. The result is a real catalog bypass.

The cross-module resolver must match the fidelity the per-file scanner already has **within** a file — qualified, receiver-scoped helper resolution (so `A::label` and `B::label` do not collide) and transitive helper tracing (`outer() { inner() }`, `inner() { "…" }`) — extended across the whole `crates/jeliya-ui/src` module tree.

---

## 2. What this issue is, and is not

**In scope**

1. A cross-module index that reads **every production module** under `crates/jeliya-ui/src` (`state.rs`, `l10n/*`, `compose.rs`, `components/*`, `app.rs`, …), not only the component roots.
2. Keying resolved helpers by **qualified module path** (a file-path → Rust-module-path heuristic), never by terminal name, so a catalog-backed `crate::labels::title()` is not conflated with an unrelated `fn title()` elsewhere.
3. **Transitive** reachability computed through calls **across files**, not just directly-literal-bearing functions.
4. A driver-level regression test through `checkJeliyaUiI18n` for the documented case, the collision case, and the transitive case.

**Explicitly not in scope (non-goals, per the issue)**

- Full Rust name resolution. **Re-exports** (`pub use`), **glob imports** (`use other::*;` bringing a helper into bare scope), and **macro-generated modules/functions** are not resolved. A file-path → module-path heuristic that covers the common `crate::<file>::<fn>` shape is acceptable; residual gaps are documented (§7).
- Changing **how** a literal inside a resolved helper body is classified. Cross-module resolution changes only **where** helper bodies are found (this file vs. any file); the body-literal judgement (bare-letters test, `i18n-exempt`, attribute/child classification) is reused **byte-for-byte** from the existing per-file path, so a helper behaves identically whether it is local or cross-module. This is what "match that fidelity" means and it also means any later refinement of the body classifier benefits both paths at once.
- Following helper calls that reach copy only through a **binding** (`let w = helper(); "{w}"`). Today the copy-helper roots are the calls that sit **directly** in an RSX copy position; that root set is unchanged. Binding-flow into a helper call remains out of scope (it is not what #275 is about, and widening it would change the clean-tree baseline).
- The catalog locale modules `l10n/en.rs` / `l10n/fr.rs`. They **are** the catalog; their literals are governed by the catalog rules, and they stay excluded from the copy index (§4.2), exactly as `componentFiles` already excludes them from the literal scan.

---

## 3. Current behavior (what exists today)

All line references are to `scripts/lib/jeliya-ui-catalog.mjs` at the branch head.

- **`scanRustSource(source)`** → `{ skeleton, literals, comments }`. `skeleton` is the source with string/char **contents** and comments blanked (newlines preserved, code-unit offsets stable), so structural look-behind cannot trip over a brace/colon inside copy. Byte-string/char literals are masked but not recorded. This is the one tokenizer both paths use.
- **`scanComponentLiterals(file, source)`** (≈L1493) scans **one** file. Its copy-helper machinery (≈L1628–L1734):
  - `copyHelpers: Map<"receiver\0name", {name, receiver}>` — calls collected from **RSX copy positions only**: expression children `{ helper() }` / `{ Recv::helper() }` (`childCallRe`) and copy-bearing attribute values `label: helper()` (`attrCallRe`, gated by `COPY_ATTRS`). Structural positions (`id: id_for()`) are deliberately not collected.
  - `parsePath(path)` splits a `::` path into `{ name = last segment, receiver = second-to-last }`, folding the context-relative qualifiers `Self`/`self`/`crate`/`super` (`PSEUDO_RECEIVERS`) to `receiver = null` so they resolve globally like a bare call.
  - `scopeMatches(fnPos, receiver)` walks out from a candidate `fn` to its enclosing `impl`/`mod` header and confirms it names `receiver` — this is the receiver scoping that keeps `A::label` from resolving `B::label`.
  - Resolution loop: for each helper, `defRe = /\bfn <name>\s*\(/g` is run **against this file's skeleton only**; each scope-matching definition's body span `[open, close]` is pushed to `helperBodies`, and the body's own calls are pushed to `pending` for **transitive** tracing (bounded by `visited`).
  - `inCopyHelperBody(pos)` is `helperBodies.some(...)`. In the main literal loop (≈L1760), a literal **outside** RSX that is `inCopyHelperBody && bareLetters && !exempt` is reported as `rust-text` (“copy returned by a helper is not in the catalog”).
- **`componentFiles(repoRoot)`** (≈L2256) walks `crates/jeliya-ui/src`, returning every `*.rs` **except** `l10n/en.rs` and `l10n/fr.rs`.
- **`checkJeliyaUiI18n({ repoRoot, only, allowlist })`** (≈L2296) is the driver. For the `literals` group it calls `scanComponentLiterals(file, readFileSync(...))` for each `componentFiles` entry and returns the sorted union.

**The gap.** `defRe` is scoped to the single file being scanned. A helper defined in another module is never found, so its literal is never marked as copy — neither from the calling file (can't see the body) nor from the defining file (nothing local invokes it in RSX). Requirements 1–3 of the issue are the three ways the reverted name-only fix failed to close this while staying honest.

**Two facts that constrain the fix** (verified against the current tree, and why the clean-tree test stays green):

- The only cross-module `crate::l10n::...::fn(...)` call in the crate is `crate::l10n::wire::status_for(...)` at `app.rs:296`, and it sits in a **plain Rust `let` binding inside an async closure — not an RSX copy position** — so it is not (and must not become) a copy-helper root. The `wire::*` label helpers are likewise called only as `let status_word = wire::status_for(...)`. No cross-module helper is invoked directly in an RSX copy position today, so a correct resolver adds **zero** findings to the current tree.
- `l10n/wire.rs` helpers carry **match-scrutinee** literals (`"authority" => …`, `"direct" => …`) that are input values, not rendered copy, and return `strings.<method>()` catalog calls. The existing per-file body-literal loop does **not** distinguish a match-pattern (LHS of `=>`) from a returned literal. Because §2 forbids changing that classifier, the resolver must not special-case these either; parity with the per-file scanner is the contract. This is safe today (nothing renders those helpers cross-module in a copy slot) and is called out as a shared, pre-existing limitation (§7), not something this issue introduces or fixes.

---

## 4. Design

The driver becomes **two-phase**: build a cross-module copy index and resolve the reachable copy-fn set **once**, then run the existing per-file scan with each file **seeded** by the reachable helpers defined in it. All new logic lives in `scripts/lib/jeliya-ui-catalog.mjs`.

### 4.1 The guiding principle — one classifier, two body-finders

The per-file scanner already knows how to (a) find a helper `fn`'s body in a file given `{name, receiver}` and (b) judge the literals inside it. The cross-module feature must **not** duplicate (b). It only widens (a): a helper body may now be found in **another** module. Concretely, `scanComponentLiterals` gains an optional seed of `{name, receiver}` helpers that this file must treat as copy-helper roots **in addition to** the ones it collects from its own RSX. Everything downstream (`scopeMatches`, `helperBodies`, `inCopyHelperBody`, the `rust-text` finding) is reused unchanged, so local and cross-module helpers are judged by identical rules and reported against the file where the literal actually lives.

### 4.2 The module set and the file-path → module-path map

- **Module set.** Reuse `componentFiles(repoRoot)` as the exact set of files indexed and scanned, so the index and the scan never disagree about which files exist. `en.rs`/`fr.rs` remain excluded (they are the catalog).
- **`moduleForFile(repoRelPath) -> string | null`** — a new pure helper mapping a repo-relative `*.rs` path to its Rust module path:
  1. Require the path to be under a crate `src` root of the form `<crate-dir>/src/` (derive the prefix from `LITERAL_SCAN_ROOTS`; today `crates/jeliya-ui/src`). A path not under such a root → `null` (out of scope for cross-module resolution).
  2. Strip the `src/` prefix and the `.rs` suffix, split on `/`.
  3. Drop a trailing `mod` segment (directory module: `l10n/mod` → `l10n`).
  4. A top-level `lib` or `main` segment is the **crate root**: map to the sentinel `crate` (empty module path).
  5. Otherwise join the remaining segments with `::` and prefix `crate::`.
  6. **`bin/<name>.rs`** is a **separate binary crate root**, not part of the library module tree (it depends on the lib as the external crate `jeliya_ui::…`, and its own `crate::` refers to the bin). Give each `bin/<name>.rs` an isolated namespace (e.g. module key `bin:<name>`) so a `crate::…` inside a bin resolves only within that bin and never collides with the library's `crate`. This is faithful to Rust and keeps bins from cross-contaminating lib resolution (§7).

  Worked results for the current tree: `state.rs → crate::state`, `app.rs → crate::app`, `compose.rs → crate::compose`, `components/mod.rs → crate::components`, `components/dialog.rs → crate::components::dialog`, `l10n/wire.rs → crate::l10n::wire`, `lib.rs → crate` (root), `bin/web.rs → bin:web`.

### 4.3 The index

**`buildCopyModuleIndex(files) -> Index`** where `files` is `[{ file, source }]` (repo-relative path + text). For each file:

1. `moduleForFile(file)`; skip files that map to `null`.
2. `scanRustSource(source)` → `{ skeleton, literals, comments }`.
3. **Function definitions.** Enumerate every `fn <name>(` in the skeleton (reuse the balanced param-and-body walk already used for catalog methods / helper resolution — factor it into a shared `enumerateFnDefs(skeleton)` returning `{ name, paramsStart, bodyOpen, bodyClose }`). For each definition record:
   - `module` (from step 1), `name`, and `receiver` — the enclosing `impl <Type>` / `mod <name>` header token, computed with the **existing** `scopeMatches` walk generalized to *return* the receiver name (or `null` for a free/module-level fn). A file-level `mod <name> { … }` nesting also contributes to the module path; for the common one-module-per-file shape this is just the file's module, and deeper inline `mod`s are a documented partial (§7).
   - `bodyOpen`, `bodyClose`, and the **outgoing call paths** in the body: every `((?:[A-Za-z_]\w*\s*::\s*)*[A-Za-z_]\w*)\s*\(` match (the same body-call regex the per-file transitive tracer uses), captured as raw path strings for later resolution with caller context.
   - Store under a key `defKey(module, receiver, name)` (e.g. `` `${module} ${receiver ?? ''} ${name}` ``). Keep a **multimap** (a name may be defined in several arms/impls) — reachability marks the key, and §4.1's per-file body-finder re-locates every matching body in the defining file.
4. **RSX copy-helper roots.** Collect the calls that sit in an RSX copy position — **exactly** the `childCallRe` + `attrCallRe`/`COPY_ATTRS` collection the per-file scanner performs. Factor that collection into a shared `collectRsxCopyHelperCalls(file, source) -> [{ path, callerModule }]` used by **both** `scanComponentLiterals` (for its local roots) and the index (for the global roots), so the fiddly in-RSX/in-test/copy-attribute detection has one implementation and cannot drift.

`Index` exposes: `defs` (the multimap above), `roots` (all `{ path, callerModule }` across files), and the set of known module paths (for longest-prefix qualifier matching).

### 4.4 Resolution — a call path + caller module → a `defKey` set

**`resolveCall(path, callerModule, index) -> defKey[]`.** Split `path` on `::` into segments; `name` = last; `qualifier` = the rest.

- **No qualifier** (bare `name()`): resolve within `callerModule` (a free fn or any impl method in that module) → keys `defKey(callerModule, *, name)`. (Bare calls to `use`-imported cross-module items are a non-goal; see §7.)
- **Context-relative head** `crate` / `self` / `super`:
  - `crate::…` → resolve the remaining qualifier as an **absolute** module path from the crate root.
  - `self::…` → relative to `callerModule`; `super::…` → relative to `callerModule`'s parent (drop the last `::` segment).
  - After rebasing, apply the absolute rule below to the rebased qualifier.
- **Absolute / concrete qualifier** (`a::b::…::Last`): find the **longest prefix** of the qualifier segments that is a **known module path** in the index. The remaining tail (0 or 1 segment in the supported shape) is the **receiver**:
  - tail empty → `defKey(module, null, name)` (free fn in that module — the `crate::state::hardcoded` case).
  - tail one segment `Recv` → `defKey(module, Recv, name)` (an inherent method — the `crate::state::Widget::label` shape; a supported partial).
  - No prefix is a known module → treat the last qualifier segment as a **receiver in the caller module** (`defKey(callerModule, Recv, name)`), matching the current same-file `Recv::helper()` behavior; if that yields nothing it is an unresolved no-op (harmless).

Resolution returns **all** matching `defKey`s (multimap). Unresolvable paths return `[]` and are harmless — the gate only ever adds findings for a helper it can both reach **and** locate a body for.

### 4.5 Reachability (transitive, across files)

**`resolveCopyReachableFns(index) -> Set<defKey>`.** Seed a worklist from `index.roots`: for each `{ path, callerModule }`, push every `resolveCall(path, callerModule, index)` key. Then BFS: pop a `defKey`, add to the reachable set (dedup = `visited`), look up its definition(s) in `index.defs`, and for each outgoing call path in each body call `resolveCall(callPath, thatDef.module, index)` and push the results. Terminate when the worklist drains (bounded by the finite `defKey` space; `visited` breaks cycles). The result is the closed set of copy-returning functions — the direct RSX roots **and** everything transitively reachable from them, across module boundaries.

This subsumes the per-file transitive tracer for cross-file edges; within-file edges are still followed by `scanComponentLiterals` itself (§4.6), so the two together cover every edge.

### 4.6 Wiring it into the scan

- **`scanComponentLiterals(file, source, options = {})`** — add an optional third argument `options.seedCopyHelpers`: an array of `{ name, receiver }` that this file must treat as copy-helper roots **in addition to** those collected from its own RSX. Implementation: initialize the existing `copyHelpers` map / `pending` worklist with these seeds before the resolution loop runs. Everything else is unchanged. Default `[]` ⇒ **byte-identical to today's behavior**, so every existing one/two-argument test call is unaffected.
  - The seed for a file is derived from the reachable set: `{ receiver, name }` for each `defKey` in `resolveCopyReachableFns(index)` whose `module === moduleForFile(file)`.
- **`checkJeliyaUiI18n`** — in the `literals` branch:
  1. Read every `componentFiles(root)` entry into `[{ file, source }]` (one read each; reused by both index and scan).
  2. `index = buildCopyModuleIndex(files)`; `reachable = resolveCopyReachableFns(index)`.
  3. Precompute `seedsByModule: Map<module, {name,receiver}[]>` from `reachable`.
  4. For each file, call `scanComponentLiterals(file, source, { seedCopyHelpers: seedsByModule.get(moduleForFile(file)) ?? [] })` and collect findings.
  - The catalog/typography branches are untouched.

Because the reachable set is computed before any per-file scan, a downstream file (e.g. `state.rs`) is seeded with `hardcoded` even though nothing **in `state.rs`** invokes it — that is exactly what makes the literal in `state.rs` get flagged, and the finding is attributed to `state.rs:<line>` where the literal lives.

### 4.7 Optional in-memory driver seam (recommended for testability)

To let driver-level tests run without a temp directory, extend `checkJeliyaUiI18n` with an optional `files` override: `checkJeliyaUiI18n({ only, files })` where `files` is a `Map<repoRelPath, source>`. When provided, `componentFiles`/`readFileSync` are bypassed and the literal scan iterates `files` directly (catalog/typography still read `LOCALE_FILES` from `files` when present). This is additive and backward-compatible (absent ⇒ today's disk behavior) and keeps the required test "through `checkJeliyaUiI18n`" while staying hermetic. If the reviewers prefer not to widen the driver signature, fall back to the `mkdtempSync` fixture harness in §6.1 — both satisfy the verification.

---

## 5. Worked cases (the three required behaviors)

1. **Documented cross-module case (must flag).** `app.rs` RSX: `div { {crate::state::hardcoded()} }`; `state.rs`: `pub fn hardcoded() -> &'static str { "Delete account" }`. Root `resolveCall("crate::state::hardcoded", "crate::app")` → `defKey(crate::state, null, hardcoded)`; reachable. `state.rs` seeded with `{name:"hardcoded", receiver:null}` ⇒ its body literal `"Delete account"` (bare-letters, not exempt, outside RSX) → **`rust-text`** finding at `crates/jeliya-ui/src/state.rs`.

2. **Collision case (must NOT flag).** `app.rs` RSX: `div { {crate::labels::title()} }`, where `crate::labels::title` is catalog-backed (`fn title(strings) -> String { strings.title() }`, no bare literal); a different module (say `state.rs`) has an unrelated `fn title() -> &str { "selected-state" }`. `resolveCall("crate::labels::title", …)` resolves **only** `defKey(crate::labels, null, title)` — the qualified module path. `crate::state`'s `title` is **not** reached (no RSX call qualifies it), so `"selected-state"` is **not** flagged. A name-only index (the reverted approach) would have marked every `fn title` body as copy and wrongly flagged `"selected-state"`.

3. **Transitive cross-file case (must flag).** `app.rs` RSX: `div { {crate::a::outer()} }`; `a.rs`: `pub fn outer() -> String { inner() }`; `b.rs` (reached from `a.rs` via `crate::b::inner`, or same-module `inner`): `pub fn inner() -> &'static str { "Delete account" }`. BFS: root `outer` reachable → its body call resolves `inner` → reachable. The file defining `inner` is seeded ⇒ `"Delete account"` → **`rust-text`** finding at that file.

---

## 6. Test plan

All tests in `scripts/check-jeliya-ui-i18n.test.mjs`. Every new assertion must fail **before** the implementation (red-before-green) — verify by writing the test against the current code and observing the miss/false-flag first.

### 6.1 Driver-level tests (the issue's required verification)

Use the in-memory `files` seam (§4.7) if adopted, else a `mkdtempSync(tmpdir())` fixture that writes the files under `<tmp>/crates/jeliya-ui/src/…` and calls `checkJeliyaUiI18n({ repoRoot: tmp, only: ['literals'] })` (the `literals` group needs no catalogs). Cleanup with `rmSync(tmp, { recursive: true })`.

1. **`hardcoded()` cross-module is flagged.** Files: `app.rs` (component with `div { {crate::state::hardcoded()} }` inside `rsx!`), `state.rs` (`pub fn hardcoded() -> &'static str { "Delete account" }`). Assert a finding with `code === 'rust-text'`, `/Delete account/`, and `file` ending `state.rs`. **Fails today** (no cross-module resolution).
2. **Collision is not falsely flagged.** Files: `app.rs` (`div { {crate::labels::title()} }`), `labels.rs` (catalog-backed `title`, no bare literal), `state.rs` (unrelated `fn title() -> &str { "selected-state" }`). Assert **no** finding matches `/selected-state/`. Must be green with the qualified-path index (and would be **red** under a name-only index — keep a comment saying so).
3. **Transitive cross-file is flagged.** Files: `app.rs` (`div { {crate::a::outer()} }`), `a.rs` (`outer` calls `crate::b::inner`), `b.rs` (`inner` returns `"Delete account"`). Assert a `rust-text` / `/Delete account/` finding at `b.rs`. **Fails today**.

### 6.2 Unit tests for the new pure helpers

- **`moduleForFile`**: `state.rs → crate::state`, `l10n/mod.rs → crate::l10n`, `components/dialog.rs → crate::components::dialog`, `lib.rs → crate`, `bin/web.rs → bin:web`, a path outside a `src` root → `null`.
- **`resolveCall`**: `crate::state::hardcoded` from `crate::app` → `[defKey(crate::state,null,hardcoded)]`; `self::inner`/`super::x` rebasing; `A::label` vs `B::label` do not cross; a bare `helper()` resolves only within the caller module; an unknown module → `[]`.
- **`resolveCopyReachableFns`**: a two-hop chain across three files is fully reached; a cycle terminates; an unrelated same-named fn in another module is **not** reached.

### 6.3 Regression / non-regression

- **Backward-compat of `scanComponentLiterals`.** The existing `scanComponentLiterals('x.rs', source)` (two-arg) calls must be untouched — assert the new third arg defaults to no seeds by keeping the whole existing suite green.
- **Clean-tree invariant.** `test('the real jeliya-ui tree is clean across all groups')` (≈L2186) must still return `[]`. This is the load-bearing check that the resolver adds no false positives to real code (§3 explains why it holds: no cross-module helper is invoked in an RSX copy slot today).
- **`i18n-exempt` still silences** a cross-module helper-body literal (the body-literal loop already honors `exempt`; add one fixture proving an exempted cross-module literal is not flagged).

### 6.4 Local gate

From `scripts/`-adjacent root: `node --test scripts/check-jeliya-ui-i18n.test.mjs`. The full canonical gate (`npm run verify` in `adw_sdlc/`) is unrelated to this repo path; the relevant CI contexts here are the jeliya `ui-literal-copy` / `ui-catalog` / `ui-french-typography` checks driven by `scripts/check-jeliya-ui-i18n.mjs`.

---

## 7. Documented residual gaps (non-goals made explicit)

These are acceptable per the issue's non-goals; record them as comments at the new index/resolver and, if a `docs/` decision note for #177 tracks gate limitations, add a line there.

1. **Re-exports / glob imports.** `pub use other::helper;` and `use other::*;` that bring a helper into bare or renamed scope are not resolved; only qualified `crate::<mod>::<fn>` (and same-module bare/`Recv::`) calls resolve. A helper laundered through a re-export can still hide copy — a known ceiling, not a regression (today nothing resolves cross-module at all).
2. **Macro-generated modules/functions.** Functions or modules produced by macros are invisible to the text scan.
3. **Inline nested `mod` blocks** deeper than one-module-per-file contribute only a partial module path (the file's module); a call qualified past an inline submodule may under-resolve. The common jeliya-ui shape is one module per file, so this is rarely hit.
4. **Inherent-method receivers across modules** (`crate::state::Widget::label`) are supported for the single-receiver tail shape only; trait-method dispatch and multi-segment type paths are not modeled.
5. **Binary crates** (`bin/*.rs`) are isolated namespaces; a bin's `crate::` never resolves into the library and vice-versa. Correct for Rust, but means bin-only copy helpers are scanned per-file only (as today).
6. **Match-pattern literals in a resolved helper body** are judged exactly as the per-file scanner judges them (§2, §3) — a shared pre-existing limitation, not introduced here. If a future issue teaches the body classifier to skip LHS-of-`=>` scrutinee literals, both the local and cross-module paths inherit it for free.

---

## 8. Acceptance criteria

1. A driver-level test **through `checkJeliyaUiI18n`** proves `crate::state::hardcoded()` invoked in a component file, resolving to a copy literal in `state.rs`, is flagged (`rust-text`) against `state.rs`.
2. The collision case (`crate::labels::title()` catalog-backed vs. an unrelated `fn title()` returning a literal elsewhere) is **not** flagged, and the test documents that a name-only index would have failed it.
3. The transitive cross-file case (`outer() { inner() }`, `inner() { "…" }` across files) is flagged.
4. Resolution keys by **qualified module path** via a file-path → module-path heuristic (`moduleForFile`), never by terminal name; unit tests cover the map and `resolveCall`.
5. `scanComponentLiterals`'s existing two-argument call sites are unchanged in behavior (the whole existing suite stays green), and the new seed argument defaults to a no-op.
6. The real-tree cleanliness test stays green (no new false positives on production code).
7. Residual gaps (§7) are documented in-code (and in the #177 gate-limitations note if one exists).
8. Every new test fails before the implementation and passes after (red-before-green), verified explicitly.

---

## 9. Risks and mitigations

- **False positives on real code breaking the required CI context.** The one realistic vector is a legitimate cross-module helper (e.g. `l10n/wire::*`) becoming a copy-helper root. Mitigation: roots are unchanged (direct RSX copy positions only), and §3 verifies none exist today; the clean-tree test (6.3) is the guard, and any future legitimate cross-module render of a catalog-backed helper will resolve to a body with no bare literal.
- **Collapsing distinct helpers (the reverted bug).** Mitigation: qualified-module keying with longest-prefix module matching and receiver scoping; the collision test (6.1.2) locks it in.
- **Runaway/backtracking scans.** Mitigation: reachability is a BFS over a finite `defKey` space with a `visited` set; all body/param walks reuse the existing bounded balanced-brace scanners.
- **Silent under-resolution (a gap hiding copy).** Accepted per non-goals and enumerated in §7; the honest position is that this issue **narrows** the bypass surface (qualified cross-module calls now covered) without claiming full name resolution.
- **Index/scan divergence.** Mitigation: both consume the same `componentFiles` set, the same `scanRustSource` tokenizer, and the shared `collectRsxCopyHelperCalls` / `enumerateFnDefs` extractors — one implementation, no drift.

---

## 10. Implementation checklist (for the executing agent)

1. Add `moduleForFile(repoRelPath)` (+ crate-src-root derivation from `LITERAL_SCAN_ROOTS`).
2. Factor `enumerateFnDefs(skeleton)` and `collectRsxCopyHelperCalls(file, source)` out of the existing per-file logic; re-point `scanComponentLiterals` at them (no behavior change).
3. Add `buildCopyModuleIndex(files)`, `resolveCall(path, callerModule, index)`, `resolveCopyReachableFns(index)` (all pure, all exported for unit tests).
4. Add `options.seedCopyHelpers` to `scanComponentLiterals`; seed `copyHelpers`/`pending` before the resolution loop; default `[]`.
5. Two-phase the `literals` branch of `checkJeliyaUiI18n`; optionally add the `files` in-memory seam (§4.7).
6. Tests per §6 (driver-level first — they are the acceptance evidence), red-before-green.
7. In-code residual-gap comments (§7); update the #177 gate-limitations note if present.
8. Run `node --test scripts/check-jeliya-ui-i18n.test.mjs`; confirm the clean-tree test and the full suite are green.
