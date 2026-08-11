# Spec — Shared `jeliya-ui` crate and reproducible Dioxus web build (#176)

- **Issue:** kortiene/jeliya#176 — `[Dioxus][Web]: Scaffold the shared UI crate and reproducible Dioxus web build`.
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M3 (web replacement).
- **Records / tests against:** `docs/dioxus-architecture.md` — Decision 1 (one renderer, system WebView), Decision 3 (layering + allowed dependency direction), Decision 5 (one embedded artifact), and the "Feature graphs" section.
- **Depends on (all landed):** #158 (feasibility spike — `spikes/dioxus-web/`), #163 (`crates/jeliya-api`), #167 (`crates/jeliya-client`, `ClientHandle` + deterministic mock).
- **Adjacent / soft dependency (NOT landed):** #174 (`PlatformServices`, "Planned"). See §6 and Open Question O-1.
- **Hands off to:** #183 (content-addressed sealed artifact + legacy-consumption-fails), #184/#159 (token boundary), #200 (React removal / release-line cutover), #186–#189 (per-platform system-WebView packaging = M4), #177 (design-token gate replacement).
- **Owner role:** web maintainer (per the architecture layering table).
- **Status of this document:** planning/spec only. **No production code is to be written for this issue by the planning phase.** The orchestrator owns all git/gh work.

> Where this spec and `docs/dioxus-architecture.md`, `docs/protocol-v2.md`, or `docs/first-release-distribution.md` disagree, those records are authoritative and this spec has a bug — say which in the PR, exactly as the architecture record requires of every slice that tests against it.

---

## 1. Outcome

Deliver the **production-shaped shared UI crate** `crates/jeliya-ui` (Dioxus 0.7, system-WebView-targeted) and **one reproducible web build** that emits a single deterministic artifact the daemon embeds. Concretely:

1. `crates/jeliya-ui` exists as a workspace member, consuming `jeliya-api` (typed operations/outputs/pushes/view models), `jeliya-client::ClientHandle` (the lifecycle-aware seam), a `PlatformServices` injection seam, and the shared CSS/design tokens — with **target composition selected at the crate root**.
2. Dioxus **and the `dx` CLI are version-pinned**; the WASM (browser) build is reproducible from a clean checkout.
3. The `wasm32-unknown-unknown` dependency graph **excludes Iroh and every native crate** (`jeliya-core`, `jeliyad`, `jeliya-ffi`, `quinn`, `rustls`, `tokio`, `wry`, `openssl-sys`, `hickory`), asserted by CI, not by inspection.
4. The build emits **one deterministic asset tree** (byte-identical across two clean builds) that the daemon embeds through its existing `embed-ui` seam.
5. CI **cannot fetch an unpinned `dx`** and the canonical build **does not depend on React/Vite/Flutter tooling**; accidental consumption of React (`ui/dist`) output as the canonical artifact **fails closed**.
6. **Development and production commands are documented** and runnable.

This is the M3 foundation. It is *web-first*: it produces the artifact that the desktop/Android system-WebView shells (M4/M5) later render unchanged. It is **not** feature-complete UI, and it does not build the native desktop shell.

## 2. Scope, non-goals, and the boundary with #183 / #200

### In scope

- The `crates/jeliya-ui` crate: layout, feature graph, root composition seam, shared-asset consumption.
- Pinned Dioxus/`dx`; the reproducible browser (`wasm32`) build pipeline and its deterministic output.
- Daemon embedding of the Dioxus artifact and a guard that rejects React/Vite output.
- CI: wasm-graph assertion, build determinism assertion, `dx`-pin/no-unpinned-fetch assertion, no-React/Flutter-tooling assertion, and a no-network render smoke.
- Developer + production command documentation, and the `docs/dioxus-architecture.md` slice-table update.

### Explicit non-goals (from the issue and the architecture record)

- **WGPU / Blitz** renderers (excluded as experimental).
- **Feature-complete UI** — the Room Workbench port and global flows are the rest of M3 (later slices), not this one.
- **React/Vite interoperability or a dual build.** The Dioxus build becomes canonical; React source may be *consulted*, but no React artifact/build is *required*.
- **Pulling core/Iroh into browser WASM.**
- **The native desktop/Android shells and their packaging** (M4/M5: #186–#189, #160/#194).
- **The v2 protocol, kernel semantics, or reconnect/queue behavior** — those are `jeliya-api`/`jeliya-client`/#168 and are consumed here, not redefined.

### The boundary this spec draws with #183 and #200 (read carefully)

The issue AC and Decision 5 are in tension: the AC says "make React output fail" and "one canonical Dioxus artifact," while Decision 5 says the **released** `v0.6.x` line keeps shipping the React `ui/dist` archive **until #200**. This spec resolves the tension as follows and the PR must state it:

- **#176 makes the Dioxus build the canonical, CI-gated, daemon-embeddable artifact** and flips the *development/CI* embed + the release **build step** onto it (so "React tooling is no longer required" becomes true).
- **#176 delivers deterministic assets and a reproducible build**; it does **not** author the *content-addressed sealed manifest* (renderer + source SHA + toolchain versions + digest) nor the *runtime legacy-artifact-rejection* — **those are #183**. #176's "React output fails" guard is a **build-time** guard (the wrong tree cannot be embedded), which #183 later hardens into a sealed, content-addressed manifest check.
- **#176 must not delete the React tree** (`ui/`) or its per-client gates. React removal is #200; retiring the token/i18n/a11y gates is #177/#197. `ui/src/styles.css` and `assets/design-tokens.json` remain in place and are *consumed* by `jeliya-ui`, not moved or deleted.

If the maintainer decides during implementation that the release-line cutover (rewriting `scripts/check-release.mjs`'s `embedded_ui` npm contract and the release `Build UI` job wholesale) is too large for one slice, the fallback is to add the Dioxus build as a **parallel, CI-canonical** path and leave `release.yml`'s tagged-artifact job on React until #200, while still satisfying every #176 AC in CI. This spec specifies the full cutover (§9–§10) and marks the release-line rewrite as the one part that may be deferred to #200 by explicit PR note — see Open Question O-2.

## 3. Owning crate, layout, and workspace membership

Add one new **workspace member** `crates/jeliya-ui` and add it to the single `members = [...]` line in the root `Cargo.toml` (the lane convention every new-crate issue follows; #167 added `crates/jeliya-client` the same way). Do **not** give it a private `[workspace]` table — it is a production crate, unlike the disposable `spikes/*`.

Proposed layout (mirrors the documentation posture of `jeliya-api`/`jeliya-client`):

```
crates/jeliya-ui/
  Cargo.toml
  Dioxus.toml            # dx web build config (index template, asset dir, output dir, wasm-opt policy)
  README.md              # dev + prod commands (AC-6); links to docs/dioxus-web-build.md
  index.html             # the Dioxus web index template (replaces ui/index.html's React <script>)
  src/
    lib.rs               # crate docs, re-exports, boundary invariants; the shared component library
    app.rs               # app_root(props) — the composed application root (target-agnostic)
    compose.rs           # target-selected composition wiring (see §6); no business-logic cfg forks
    services.rs          # the PlatformServices injection seam consumed here (see §6 / O-1)
    state.rs             # UI-facing state derived from ClientHandle events (view models from jeliya-api)
    components/          # shared RSX components (renderer-agnostic; no platform cfg forks)
      mod.rs
    bin/
      web.rs             # feature = "web": dioxus::launch(app_root) for the browser (wasm32)
  assets/
    styles.css           # canonical stylesheet ASSET reference (see §7 — one source, not a copy)
  tests/
    boundaries.rs        # dependency-tree + no-forbidden-crate assertions (mirrors jeliya-client)
  e2e/                   # no-network render smoke (Playwright, pinned; mirrors spikes/dioxus-web/e2e)
    render.spec.ts
```

**Boundary invariants (asserted, not merely intended), mirroring `jeliya-api`/`jeliya-client`:**

- Crate-level `#![forbid(unsafe_code)]`. `#![deny(missing_docs)]` on the public library surface.
- **`jeliya-ui` reaches platform authority only through injected services** (`PlatformServices`), never directly. It depends on `jeliya-api` and `jeliya-client`, and on Dioxus; it must **not** depend on Iroh, `jeliya-core`, `jeliyad`, a WebSocket crate, or a native transport.
- **No platform business-logic `cfg` forks in shared components** (architecture Decision 3 / #174). Target differences live only in `compose.rs`/the per-target `bin` and in the injected services, never scattered through `components/`.
- `ClientHandle` and `PlatformServices` are **injected separately** (#174) — never entangled into one object.

## 4. Feature graph, dependency direction, and the MSRV/OpenSSL crux

This is the highest-risk part of the slice. The repository has three hard constraints that the feature graph must respect simultaneously:

1. **MSRV job compiles the whole workspace.** `.github/workflows/ci.yml` runs `cargo check --locked --workspace --all-targets` on **rustc 1.91.0**. Any dependency that is *non-optional* in a workspace member is compiled there.
2. **`dioxus-desktop` 0.7.9 links OpenSSL non-optionally.** It declares `tungstenite` with `features = ["native-tls"]` for every non-Android target, so `openssl-sys` (needing system OpenSSL headers) enters any graph that includes `dioxus-desktop`. This is why the desktop spike (`spikes/dioxus-desktop/`) and the local toolchain gap exist.
3. **Browser WASM must be Iroh-free and native-free** (Decision 3, and #158 AC-1).

The `jeliya-client` crate already solved (1) for its Dioxus dependency by keeping `dioxus` **optional and feature-gated** (`example = ["dep:dioxus"]`), so `--workspace --all-targets` never compiles a renderer. **`jeliya-ui` must follow the same discipline.**

### Feature design (recommended)

```toml
[features]
default = []                 # thin, host-compilable, MSRV-1.91-safe, renderer-free, OpenSSL-free
web    = ["dep:dioxus", "dioxus/web"]     # browser DOM renderer; built ONLY for wasm32
native = ["dep:dioxus", "dep:dioxus-desktop"]  # system-WebView shell; M4 (#186-#189); OpenSSL-bearing
```

- **`default` (no features)** compiles to the crate's non-Dioxus helpers (or nothing) and is what the MSRV `--workspace` job, the `1.96.0` clippy/test `--workspace` job, and `dependency-security` see. It must stay **renderer-free and OpenSSL-free** so no existing host-target CI job regresses or needs `libssl-dev`.
  - Consequence: the shared RSX components and `app_root` live behind `#[cfg(any(feature = "web", feature = "native"))]` (or a `ui` feature that both enable), exactly as `jeliya-client`'s verification component lives behind `example`. The `bin/web.rs` target is `required-features = ["web"]`.
- **`web`** enables `dioxus/web` and is built **only** for `wasm32-unknown-unknown` in a dedicated job. It must never pull `dioxus-desktop`/`wry`/`tokio`/`openssl-sys`.
- **`native`** is defined so the layering is real, but is **out of scope to build/ship here** (M4). CI does not build it in this slice; a comment records that it carries OpenSSL and belongs to #186–#189.

**Why not make the components default-on?** Because dioxus `minimal`/`web` on rustc 1.91.0 is unverified, and turning it on by default forces the MSRV `--workspace` job to compile Dioxus. If the maintainer instead wants default-on components, the fallback (Open Question O-3) is to set a crate-local `rust-version` above the workspace MSRV and `--exclude jeliya-ui` from the MSRV `--workspace` check — a waiver this spec prefers to avoid.

**Dioxus/`dx` pinning.** Pin `dioxus = "=0.7.9"` (already resolved in the root `Cargo.lock`: `dioxus 0.7.9`, `wasm-bindgen 0.2.126`, `web-sys`). Pin the `dx` CLI to the exact matching version (see §8). `--locked` on every build keeps the lockfile authoritative; `dependency-security` (cargo-audit) already covers the graph.

## 5. Reproducibility model (what "deterministic" concretely requires)

The artifact is deterministic when two clean-checkout builds on the same pinned toolchain produce a **byte-identical** asset tree. Requirements:

- **Pinned inputs:** rustc (record the exact version used to build the wasm; the workspace floor is 1.91 but the release build should pin a single stable, e.g. the CI `1.96.0` already in use — the manifest must record whichever is chosen), `dioxus`/`dx` (=0.7.9 line), `wasm-bindgen` (0.2.126, must match the `wasm-bindgen-cli`/`dx`-bundled bindgen exactly or the build fails), and `wasm-opt`/Binaryen (pinned version, or **disabled** for determinism — see below).
- **Reproducible wasm:** build with `--locked`; set `RUSTFLAGS="--remap-path-prefix=$PWD=."` (or `CARGO_BUILD_RUSTFLAGS`) so absolute paths don't leak into the binary; set `SOURCE_DATE_EPOCH` to a fixed value; ensure no build timestamp is embedded.
- **Deterministic asset names/hashes:** dx content-hashes asset filenames; that hash must be a pure function of file bytes (no mtime, no build id). Fix locale/collation for any directory listing the pipeline performs (`LC_ALL=C`), and sort inputs.
- **`wasm-opt` policy:** `wasm-opt` output can vary across Binaryen versions. Either **pin the Binaryen version** and record it in the build doc, or **disable `wasm-opt`** in `Dioxus.toml` for the reproducible artifact and record the size caveat (the #158 spike measurements were explicitly *not* `wasm-opt`'d; size budgets are #198, not this slice). Recommendation: **pin Binaryen** so production keeps the size win, and have the determinism check run against the pinned-`wasm-opt` output.
- **Verification:** a CI step builds twice into two directories and asserts `sha256` equality of the sorted file list and of each file (see §11, `check-web-determinism.sh`).

## 6. Application root: consuming `jeliya-api`, `ClientHandle`, and `PlatformServices`

The crate root composes the application from three injected inputs and selects composition per target.

### 6.1 `ClientHandle` (from `jeliya-client`, landed)

- `app_root` receives a `ClientHandle` (the cloneable seam). Components call `handle.call::<Op>(req)` for typed request/output pairs and `handle.subscribe()` for `ClientEvent` streams (`StateChanged`, pushes, `Gap`, `ResyncRequired`). Replies never travel on the event stream (the seam guarantees this).
- UI-facing `state.rs` folds `ClientEvent`s into Dioxus signals holding **`jeliya-api` view models** (never raw JSON — the seam already forbids `serde_json::Value` in its public surface).
- The **browser binding is `WsWeb`**, which is **#168's** adapter and is *not* built in this slice. For #176, the root is written against `ClientHandle` and driven in tests/smoke by the **deterministic mock** (`jeliya-client` `mock` feature) — the reference behavior. The real `WsWeb` transport slots in behind the same handle later. State this in the PR: #176 renders against the mock; it does not open a socket.

### 6.2 `jeliya-api` (landed)

- All request/output/push/error/view-model types come from `jeliya-api`. `jeliya-ui` does **not** define a second spelling of any wire type. `jeliya-api` is already `wasm32`-clean (no Iroh/WebSocket/Dioxus, no `serde_json::Value` in public types), so it links in the browser graph by construction.

### 6.3 `PlatformServices` (#174, **NOT landed** — the one real dependency gap)

`PlatformServices` is the injectable boundary for files, persistence, lifecycle, URLs, clipboard/share, navigation, and window actions. **#174 is "Planned," so the trait does not exist yet.** #176 is *not* formally blocked by #174 (its blockers are #158/#163/#167), yet the issue says to "consume PlatformServices." Resolution (Open Question O-1, recommended answer):

- **Define the injection *shape* here, minimally, without fabricating #174's contract.** Introduce a small local `PlatformServices` seam in `services.rs` containing **only** the members `app_root` actually needs for the M3 web foundation (at most: persistence get/set for UI preferences, URL/clipboard, navigation), each behind a trait with a **deterministic in-process test implementation** (mirroring #174's own requirement that "every service has a deterministic test implementation").
- **Inject it separately from `ClientHandle`** (never merged), so when #174 lands, `jeliya-ui` adopts the canonical trait by replacing the local seam with a re-export — a mechanical change, not a redesign.
- Do **not** implement real file/lifecycle/window authority here (that is native, M4). The web foundation uses the browser-appropriate deterministic implementations.
- Record in the PR and in `docs/dioxus-architecture.md` that `jeliya-ui`'s `PlatformServices` is a **provisional seam pending #174**, so the architecture record's "Planned" row and this crate stay honest.

### 6.4 Target composition at the root (Decision 3, #174)

- `compose.rs` is the **only** place a target choice is made. For this slice there is one real target (browser via `bin/web.rs`) plus the native seam stub. Composition selects the concrete `PlatformServices` implementation and the `ClientHandle` source; it must contain **no product/business-logic `cfg`** — only wiring.
- Shared `components/` are **cfg-free**. A component that needs a platform capability takes it as a prop/injected service, never via `cfg`.

## 7. Shared CSS / assets / tokens — consumed canonically

The AC requires "Shared CSS/assets/tokens are consumed canonically." Today:

- `ui/src/styles.css` is the stylesheet; `scripts/check-design-tokens.mjs` reads it against `assets/design-tokens.json`.
- The #158 spike proved `ui/src/styles.css` drives Dioxus RSX **byte-identically** (design-system CSS survives the renderer swap), while noting it says nothing about what enforces the *tokens* once that file retires (that is #177).

For #176:

- **One stylesheet, referenced — not copied.** `jeliya-ui` consumes the shared stylesheet through Dioxus's `asset!()` mechanism so there is exactly one source of truth, not a divergent duplicate. Because `ui/` must stay intact until #200, the cleanest canonical arrangement is: keep `ui/src/styles.css` as the physical source and have `jeliya-ui/assets/styles.css` be a **build-time reference** to it (an `asset!` pointing at the repo-relative path, or a symlink/checked-in generated copy validated by a byte-equality CI check — pick one and document it). **Recommendation:** reference the existing file via an explicit relative `asset!` path; if the asset pipeline cannot reach outside the crate, add a CI byte-equality assertion (`cmp`) between `crates/jeliya-ui/assets/styles.css` and `ui/src/styles.css` so a copy cannot silently drift (the same "no divergence" property the token fixture enforces today).
- **Design tokens** stay sourced from `assets/design-tokens.json`. #176 does **not** replace `scripts/check-design-tokens.mjs` (that gate still reads `ui/src/styles.css`, which is unchanged) and does **not** author the Dioxus-side token gate — that verification-loss replacement is #177. Record the deferral.
- **Static assets** (favicon, `og.png`, `site.webmanifest`, etc., currently in `ui/public/` and `assets/`) that the shell needs are declared as Dioxus assets so they participate in the deterministic hashed output. Only assets the shell actually references are pulled in.

## 8. Pinning `dx` and forbidding an unpinned fetch (AC-5)

The `dx` CLI (dioxus-cli) historically pulls `openssl-sys`, which is why the #158 spike deliberately used `cargo build --target wasm32 + wasm-bindgen` instead. #176 must pin `dx` and make CI unable to fetch an unpinned one. Two acceptable strategies; **primary recommendation is (A)**:

**(A) Pinned prebuilt `dx` binary, verified by checksum (avoids compiling OpenSSL).**
- Install `dx` from the dioxus-cli release matching `=0.7.x` via a **version-pinned** `cargo binstall dioxus-cli@0.7.x` **or** a direct download of the pinned release asset, then verify the downloaded binary against a **recorded `sha256`** before use. Record `dx --version` and the checksum in the build doc/manifest.
- CI guard `check-dx-pin.sh`: assert the installed `dx --version` equals the pinned version string exactly, and that the install command in CI carries an explicit version + checksum. Fail if any workflow invokes `cargo install dioxus-cli` / `cargo binstall dioxus-cli` **without** a pinned `@version` (grep the workflow files), and fail if `dx` is fetched from a non-pinned source. This is the executable form of "CI cannot fetch unpinned `dx`."

**(B) Pinned-from-source `dx`.**
- `cargo install --locked --version =0.7.x dioxus-cli` with `libssl-dev pkg-config` installed on the CI runner (`ubuntu-latest` can `apt-get install`). Reproducible via `--locked`, but compiles OpenSSL and is slower; it also reintroduces the local-machine OpenSSL-header gap for developers. Acceptable for CI, worse for contributors.

**Determinism-first fallback (documented, not primary):** the #158 spike's `cargo build --target wasm32-unknown-unknown` + **pinned** `wasm-bindgen-cli` (0.2.126) path is already proven in this repo and is fully deterministic without `dx`. Because the AC explicitly names `dx`, this spec makes `dx` the canonical tool, but records the pure-cargo path as the escape hatch if the pinned `dx` binary proves unreproducible or unavailable — the two must produce equivalent asset trees, and whichever is canonical is the one the determinism check runs against.

**No React/Flutter/Vite tooling in the canonical build (AC-5).** The Dioxus build job must not run `npm`, `vite`, `tsc`, `flutter`, or `dart`, and must not read `ui/dist`. A guard (`check-web-build-toolchain.sh`) asserts the build recipe references none of those and that `node_modules`/`ui/dist` are absent from the build's inputs.

## 9. Daemon embedding and the "React output fails" guard

Today `crates/jeliyad/src/serve.rs` embeds `#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]` under the `embed-ui` feature. #176 makes the daemon embed the **Dioxus** artifact.

**Recommended approach:**

- The Dioxus build emits to a **new canonical output directory** (e.g. `crates/jeliya-ui/dist/` or a repo-level `web/dist/`) — **not** `ui/dist` — so the React and Dioxus outputs can never be confused, and `ui/dist` (React) has no path to the daemon.
- Repoint the `embed-ui` `#[folder = ...]` to the Dioxus output. Keep the feature name `embed-ui` (the release wiring, `check-release.mjs` feature list, and `--ui-dir` override all key off it) so the change is surgical.
- **Build-time guard (the "React output fails" AC):** add a marker the Dioxus build writes into its output (e.g. `dist/.dioxus-artifact` containing the renderer id + pinned versions) and a compile-time/`build.rs`-time assertion in `jeliyad` that the embedded folder contains that marker and an `index.html` that loads the wasm module — and does **not** contain a Vite/React signature (e.g. a `<script type="module" src="/src/main.tsx">` or a Vite `assets/*.js` bundle map). If the marker is absent or a React signature is present, the `embed-ui` build **fails**. This is the #176 build-time form; #183 later replaces the marker with a content-addressed sealed manifest and adds the *runtime* legacy-rejection.
- The daemon's dev path (`--ui-dir <dir>`) continues to work for iterating on the Dioxus `dist/` without a daemon rebuild (the #158 spike used exactly this).

## 10. CI and release wiring

Extend the existing jobs; do not invent a new required check name without confirming the branch-protection contract (the required checks are load-bearing — see the repo memory on required gates).

### 10.1 New/extended CI steps (in `ci.yml`, the Rust+wasm job that already installs `wasm32-unknown-unknown`)

1. **WASM graph exclusion (AC-2):** `crates/jeliya-ui/../../scripts/check-wasm-graph.sh`-style check, generalized to run against `-p jeliya-ui --features web --target wasm32-unknown-unknown`. Reuse the proven `spikes/dioxus-web/check-wasm-graph.sh` logic (forbidden prefixes: `iroh jeliya-core jeliyad jeliya-ffi quinn rustls tokio-rustls hickory` **plus** `tokio`, `wry`, `tao`, `openssl-sys`, `native-tls`). Promote it to `scripts/check-jeliya-ui-wasm-graph.sh`.
2. **`jeliya-ui` web build (AC-1):** build the artifact with the pinned `dx` (§8) into `dist/`, `--locked`.
3. **Determinism (AC-3):** `scripts/check-web-determinism.sh` — build twice into `dist-a`/`dist-b`, assert byte-identical (`diff -r` + per-file `sha256`).
4. **`dx` pin / no-unpinned-fetch (AC-5):** `scripts/check-dx-pin.sh` (§8) + `scripts/check-web-build-toolchain.sh` (no `npm`/`vite`/`tsc`/`flutter`/`dart`, no `ui/dist` input).
5. **Shared-asset consumption (AC-4):** byte-equality assertion between the canonical stylesheet source and the crate's referenced copy (§7), plus a grep asserting the tokens source is `assets/design-tokens.json`.
6. **No-network render smoke (Verification):** headless Chromium (pinned Playwright, `--offline`/no network) loads the built `dist/` served statically and asserts the shell renders (a known root element / a computed non-transparent `.msg-bubble` background, as the #158 spike did). Mirror `spikes/dioxus-web/e2e/`.
7. **MSRV safety:** confirm the existing MSRV `--workspace` job stays green because `jeliya-ui`'s default features pull no renderer (§4). No MSRV job edit should be needed; if one is, it is the O-3 waiver.

### 10.2 Release (`release.yml`) — the flip, with the deferral escape hatch

- The current **"Build the embedded web UI once"** job runs `npm ci && npm run build` and uploads `ui/dist`. #176's target state replaces its build step with the pinned `dx` Dioxus build emitting the canonical `dist/`, and the matrix jobs embed that. This is what removes the "React tooling required" fact.
- `scripts/check-release.mjs` currently asserts `build.embedded_ui.built_from_source` + `package_lock_sha256` and expects `npm ci`/`npm run build` in the recorded commands. Those assertions are React/npm-specific. **#176 updates them to the Dioxus toolchain** (pinned `dx`/rustc/wasm-bindgen versions + the artifact digest) — **but the content-addressed sealed manifest (renderer, source SHA, digest) is #183's owned schema.** Coordinate: #176 may land the minimal manifest fields it needs and leave the sealed-manifest hardening to #183, or #183 may own the whole `embedded_ui` block. **Open Question O-2** records this; the safe default is for #176 to do the *build-step* flip + build-time guard and let #183 own the manifest schema, keeping #176's release edit small.

## 11. Verification strategy and concrete commands

Run focused checks first (the orchestrator runs the canonical gate at finalize). From the repo root unless noted.

```bash
# Crate compiles on host with default features (MSRV-shaped; no renderer, no OpenSSL)
cargo check --locked -p jeliya-ui

# Browser build graph excludes Iroh/native (AC-2)
scripts/check-jeliya-ui-wasm-graph.sh          # generalized from spikes/dioxus-web/check-wasm-graph.sh

# Reproducible web build (AC-1) + determinism (AC-3)
scripts/build-web.sh                            # pinned dx -> crates/jeliya-ui/dist
scripts/check-web-determinism.sh                # builds twice, asserts byte-identical

# dx pinned, no unpinned fetch, no React/Flutter tooling (AC-5)
scripts/check-dx-pin.sh
scripts/check-web-build-toolchain.sh

# Shared CSS/tokens consumed canonically (AC-4)
cmp crates/jeliya-ui/assets/styles.css ui/src/styles.css   # or the referenced-asset assertion

# Daemon embeds the Dioxus artifact; React output cannot embed (AC + §9)
cargo build --locked -p jeliyad --features embed-ui        # succeeds only with a Dioxus dist/marker
# (negative test) point embed at a React ui/dist and assert the build fails

# No-network render smoke (Verification)
cd crates/jeliya-ui/e2e && npm ci --ignore-scripts && npx playwright test   # offline

# Boundary tests (mirror jeliya-client/tests/boundaries.rs)
cargo test --locked -p jeliya-ui --features web --target wasm32-unknown-unknown  # if wasm-runnable, else build-only
```

The full canonical gate remains `npm run verify` in `adw_sdlc/` for the control plane and `cargo`/CI for the Rust workspace; reserve it for the final review.

## 12. Acceptance-criteria → work mapping

| # | Acceptance criterion | Where satisfied |
|---|---|---|
| AC-1 | Pinned Dioxus 0.7 web/system-WebView renderer builds reproducibly | §4 (pin `dioxus =0.7.9`), §8 (pin `dx`), §5 (reproducibility model), §11 (`build-web.sh`) |
| AC-2 | WASM graph excludes Iroh/native crates | §4 (feature graph, native default-off), §10.1(1) (`check-jeliya-ui-wasm-graph.sh`) |
| AC-3 | One canonical Dioxus artifact has deterministic assets | §5, §9 (single output dir), §10.1(3) (`check-web-determinism.sh`); sealed manifest deferred to #183 |
| AC-4 | Shared CSS/assets/tokens consumed canonically | §7 (one stylesheet referenced, tokens from `design-tokens.json`), §10.1(5) |
| AC-5 | CI cannot fetch unpinned `dx` or depend on React/Flutter tooling | §8, §10.1(4) (`check-dx-pin.sh`, `check-web-build-toolchain.sh`) |
| AC-6 | Development and production commands documented | §13, `crates/jeliya-ui/README.md`, `docs/dioxus-web-build.md` |
| (context) | One artifact embedded by the daemon | §9 (repoint `embed-ui`, build-time React-output guard) |

## 13. Documentation deliverables (AC-6)

- **`crates/jeliya-ui/README.md`** — the runnable commands:
  - **Development:** `dx serve --features web` (or the pinned equivalent) for hot-reload against `--ui-dir`; and the daemon dev path (`jeliyad --ui-dir crates/jeliya-ui/dist`, as the #158 spike used) for a real-daemon loop.
  - **Production:** `scripts/build-web.sh` → deterministic `dist/`; then `cargo build --release -p jeliyad --features embed-ui` to embed it.
- **`docs/dioxus-web-build.md`** (new page, must pass the docs profile gate — exactly 10 frontmatter fields, valid `status`/`implementation_status`/`verification_status`/`release_status`, reachable from `docs/index.md`; see the docs-profile contract in repo memory). Records: pinned versions (rustc, `dioxus`/`dx`, `wasm-bindgen`, Binaryen/`wasm-opt` policy), the determinism recipe (`SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `LC_ALL=C`), and the output/embedding contract.
- **`docs/dioxus-architecture.md`** — update the slice table: add a `#176` row and mark it landed when implemented; note that `jeliya-ui`'s `PlatformServices` is a **provisional seam pending #174**; keep Decision 5's "React remains shipped until #200" statement consistent with the §2 boundary (do not claim React is removed).
- **`docs/known-gaps-roadmap.md`** — if it tracks M3 progress, note the web foundation landed and that the design-token gate replacement (#177) and the sealed artifact manifest (#183) remain open.
- Update `crates/jeliya-ui`'s own crate docs (`lib.rs`) with the boundary-invariant list, mirroring `jeliya-api`/`jeliya-client`.

## 14. Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| **MSRV 1.91 `--workspace` job compiles a renderer and breaks** if `jeliya-ui` deps aren't default-off | High if ignored | §4: keep `dioxus`/renderer **optional + feature-gated** exactly like `jeliya-client`; verify `cargo +1.91.0 check --workspace --all-targets` stays green. Fallback: crate-local `rust-version` + MSRV `--exclude` (O-3). |
| **`dx` drags OpenSSL** and can't install on the toolchain-gapped local/CI machine | High | §8(A): install a **pinned prebuilt `dx` binary** verified by checksum, sidestepping the OpenSSL compile. Documented pure-cargo+`wasm-bindgen` fallback (proven by #158). |
| **Build is not byte-reproducible** (`wasm-opt`/Binaryen drift, path/timestamp leakage) | Medium | §5: pin Binaryen or disable `wasm-opt`; `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `LC_ALL=C`; the determinism check fails the PR if two builds differ. |
| **Release line breaks** by flipping `Build UI`/`check-release.mjs` off npm prematurely (Decision 5 keeps React shipped until #200) | Medium | §2 + O-2: keep the manifest-schema rewrite in #183/#200; #176 does the build-step flip + build-time guard, or defers the release-job flip and stays CI-canonical only. Never delete `ui/`. |
| **`PlatformServices` (#174) not landed** but AC says to consume it | Medium | §6.3 + O-1: define a minimal provisional seam with deterministic test impls, injected separately; adopt #174's trait mechanically when it lands; record it as provisional in the architecture doc. |
| **Adding a new required CI check** without matching branch protection stalls merges | Medium | Fold assertions into existing jobs/steps; if a new required check is needed, confirm the branch-protection contract first (repo memory: required checks are load-bearing). |
| **Token/i18n/a11y verification loss** from consuming CSS without the React gate | Low (scoped out) | §7: `check-design-tokens.mjs` still reads the unchanged `ui/src/styles.css`; the Dioxus-side gate is explicitly #177, recorded as a deferred gap. |
| **Design-system CSS doesn't fully survive** the renderer swap in real components (spike found a compact-viewport blank-screen and a missing composer rule) | Low–Medium | The no-network render smoke asserts **computed** style (non-transparent bubble, `resize:none` composer) and runs compact viewports, reproducing the spike's regression guards rather than trusting class presence. |

## 15. Open questions

- **O-1 — `PlatformServices` shape.** Which members does the M3 web foundation genuinely need before #174 lands, and does the web maintainer prefer a provisional local seam (recommended) or to block #176 on #174? Recommended: provisional minimal seam with deterministic impls, injected separately, adopted mechanically when #174 lands.
- **O-2 — Release-line cutover ownership.** Does #176 rewrite `scripts/check-release.mjs`'s `embedded_ui` block and the release `Build UI` job onto the Dioxus toolchain now, or does #183/#200 own that so #176 only flips the build step + adds the build-time guard? Recommended: #176 flips the build step and adds the guard; #183 owns the sealed-manifest schema.
- **O-3 — MSRV posture for `jeliya-ui`.** Keep renderer deps optional/feature-gated so the crate's default build compiles on 1.91 (recommended, no waiver), or set a crate-local `rust-version` above the workspace MSRV and `--exclude jeliya-ui` from the MSRV `--workspace` check? Needs a one-line measurement: does `cargo +1.91.0 check -p jeliya-ui --features web --target wasm32` succeed? (For the default-off design, this is only run in the dedicated wasm job on the pinned stable, not on MSRV.)
- **O-4 — `dx` vs pure-cargo as canonical.** Confirm a pinned prebuilt `dx@0.7.x` binary exists for the CI runner architecture and is byte-reproducible; if not, adopt the pure-cargo+`wasm-bindgen` path as canonical (still satisfies AC-1/AC-3; AC-5's "no unpinned `dx`" is then trivially met because `dx` isn't used, but the check must still forbid an unpinned fetch). Decide which is the canonical, determinism-checked recipe.
- **O-5 — Output directory.** `crates/jeliya-ui/dist/` vs a repo-level `web/dist/`. Either works; the choice affects the `embed-ui` `#[folder]` path and `.gitignore`. Recommended: `crates/jeliya-ui/dist/` (co-located with the crate, ignored in git).
- **O-6 — `wasm-opt` policy.** Pin Binaryen (keep the size win, more moving parts) or disable `wasm-opt` for the reproducible artifact (simplest determinism, larger wasm; size budgets are #198). Recommended: pin Binaryen and record the version in `docs/dioxus-web-build.md`.

## 16. Rollout / rollback

- **Rollout:** additive. The crate is new; the daemon change is a single `#[folder]` repoint plus a guard; CI adds steps to existing jobs; the release build-step flip is the one externally visible change and is gated behind the §2 boundary decision. `ui/` (React) stays intact, so the current (already-degraded) release path is not deleted.
- **Rollback:** revert the `embed-ui` `#[folder]` to `ui/dist` and drop the new CI steps; `jeliya-ui` can remain in the tree unbuilt (default-off features) without affecting other crates. Because nothing is content-migrated and no data schema changes, rollback is a code revert with no state implications.
- **No data/state migration** is involved; this is a build/packaging and crate-scaffold slice.

---

### Appendix A — Evidence base (verified against the tree at spec time)

- Workspace members today: `jeliya-core`, `jeliyad`, `jeliya-api`, `jeliya-codec`, `jeliya-client`; `jeliya-ffi` excluded (root `Cargo.toml`). Root `Cargo.lock` already resolves `dioxus 0.7.9`, `wasm-bindgen 0.2.126`, `web-sys` (via `jeliya-client`'s example dep); `wry`/`tao`/`openssl-sys` are **not** in the root lock.
- Daemon embed seam: `crates/jeliyad/src/serve.rs` — `#[derive(rust_embed::RustEmbed)] #[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]`, `embed-ui` feature; `--ui-dir` overrides embedded assets.
- CI precedents: `.github/workflows/ci.yml` installs `wasm32-unknown-unknown` and builds `jeliya-client`'s shared component on native + wasm32; MSRV job runs `cargo check --locked --workspace --all-targets` on **1.91.0**; clippy/test `--workspace` on **1.96.0**; `check-design-tokens.mjs` reads `ui/src/styles.css` against `assets/design-tokens.json`.
- Release precedent: `.github/workflows/release.yml` "Build the embedded web UI once" runs `npm ci && npm run build` → `ui/dist`; `scripts/check-release.mjs` asserts `embedded_ui.built_from_source` + `package_lock_sha256` and expects `npm ci`/`npm run build` in recorded commands.
- Spike evidence (#158, `spikes/dioxus-web/`): `cargo build --target wasm32-unknown-unknown` + `wasm-bindgen 0.2.126` (no `dx`, deliberately — `dx` pulls OpenSSL); 124 wasm crates, no Iroh/core; `ui/src/styles.css` reused byte-identical; `check-wasm-graph.sh` and the compact-viewport e2e regressions are directly reusable here.
- `dioxus-desktop` 0.7.9 links OpenSSL non-optionally for non-Android targets (`spikes/dioxus-desktop/Cargo.toml` and `check-native-graph.sh`) — the reason the `native` feature must stay out of default/CI-workspace builds in this slice.
