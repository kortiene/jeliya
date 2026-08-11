---
type: "Guide"
title: "Dioxus web build and reproducibility"
description: "The reproducible-build contract for the shared jeliya-ui crate (#176): the pinned toolchain, the deterministic wasm recipe, the single canonical artifact the daemon embeds, the build-time guard that rejects React output, and the development and production commands."
tags: ["dioxus", "web", "build", "reproducibility", "clean-slate"]
timestamp: "2026-08-10T20:00:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "release-engineers"]
---

# Dioxus web build and reproducibility

This records how the shared `jeliya-ui` crate (#176) produces **one
reproducible web artifact** and how the daemon embeds it. It tests against
`docs/dioxus-architecture.md` — Decision 1 (one renderer, system WebView),
Decision 3 (layering and the dependency direction), and Decision 5 (one
embedded artifact). Where they disagree, the architecture record is
authoritative and this page has a bug.

This is the M3 *web foundation*. It is **not** feature-complete UI and does not
build the native desktop shell. It renders against the deterministic mock and
opens no socket; the real browser transport (`WsWeb`) is #168.

## Pinned inputs

| Input | Pin | Where |
|---|---|---|
| `dioxus` | `=0.7.9` (renderer crates resolve to the 0.7.x line) | `crates/jeliya-ui/Cargo.toml`, `Cargo.lock` |
| `wasm-bindgen` / `wasm-bindgen-cli` | `0.2.126` — the CLI version **must equal** the locked library version or the build fails | `Cargo.lock`; asserted by `scripts/build-web.sh` |
| rustc | a single pinned stable (CI uses `1.96.0`); the workspace MSRV floor is `1.91` | `.github/workflows/ci.yml` |
| `wasm-opt` / Binaryen | **disabled** for the reproducible artifact (size budgets are #198); pin Binaryen if it is ever enabled for the size win (O-6) | `crates/jeliya-ui/Dioxus.toml`, `scripts/build-web.sh` |

`dx` (dioxus-cli) is **not** used by the canonical build. `dx` historically
links OpenSSL non-optionally, which the clean-slate toolchain avoids, so the
canonical, determinism-checked recipe is `cargo build --target
wasm32-unknown-unknown` + the pinned `wasm-bindgen-cli` — the path the #158
spike proved in this repo (Open Question O-4). CI still forbids an unpinned `dx`
fetch (`scripts/check-dx-pin.sh`).

## Determinism recipe (§5)

The artifact is deterministic when two clean-checkout builds on the same pinned
toolchain produce a **byte-identical** asset tree. `scripts/build-web.sh` sets:

- `SOURCE_DATE_EPOCH` to a fixed value (no embedded build timestamp).
- `RUSTFLAGS=--remap-path-prefix=$PWD=. --remap-path-prefix=$HOME=~` so absolute
  paths do not leak into the binary.
- `LC_ALL=C` to fix any collation the pipeline performs.
- `CARGO_INCREMENTAL=0` so output does not depend on prior build state.
- No `wasm-opt` pass.

`scripts/check-web-determinism.sh` builds twice into `dist-a`/`dist-b` and
asserts an identical sorted file list and identical per-file bytes; a difference
fails the check.

## Output and embedding contract (§9)

- The build emits to **`crates/jeliya-ui/dist/`** — the one canonical output
  directory, deliberately **not** `ui/dist`, so the React and Dioxus outputs can
  never be confused and React has no path into the daemon.
- `dist/.dioxus-artifact` is a build-time marker (`renderer=dioxus-web` plus
  pinned tool versions). #183 later replaces it with a content-addressed sealed
  manifest and adds a runtime legacy-rejection.
- The daemon embeds `crates/jeliya-ui/dist` through its existing `embed-ui`
  feature (`crates/jeliyad/src/serve.rs`). `crates/jeliyad/build.rs` runs a
  **build-time guard** first: the `embed-ui` build fails closed if the embedded
  folder lacks the Dioxus marker, does not load a wasm module, or carries a
  React/Vite signature. The dev path `jeliyad --ui-dir crates/jeliya-ui/dist`
  serves the artifact without a daemon rebuild.

## Shared CSS and tokens, consumed canonically (§7)

There is one stylesheet source: `ui/src/styles.css`. The build injects it into
the artifact as `dist/styles.css`; a CI byte-equality guard keeps the served
copy identical to its single source so it cannot drift. Design tokens stay
sourced from `assets/design-tokens.json`, still validated by
`scripts/check-design-tokens.mjs` (which reads the unchanged `ui/src/styles.css`).
The Dioxus-side token gate is #177, recorded as a deferred gap.

## The wasm graph is Iroh-free and native-free (AC-2)

`scripts/check-jeliya-ui-wasm-graph.sh` (and `crates/jeliya-ui/tests/boundaries.rs`)
assert the `wasm32-unknown-unknown` `web` graph contains none of `iroh`,
`jeliya-core`, `jeliyad`, `jeliya-ffi`, `quinn`, `rustls`, `tokio`, `hickory`,
`wry`, `tao`, `openssl-sys`, or any WebSocket/native-TLS crate. Confirmed at the
lockfile level too: adding the `web` renderer introduced no `openssl-sys`,
`wry`, `tao`, or `dioxus-desktop` entry.

## Commands

### Development

```bash
# Build the browser artifact into crates/jeliya-ui/dist (pinned toolchain).
scripts/build-web.sh

# Iterate against a real daemon without rebuilding it.
cargo run -p jeliyad -- --ui-dir crates/jeliya-ui/dist

# The equivalent dx hot-reload loop is documented in crates/jeliya-ui/Dioxus.toml;
# the canonical, determinism-checked build is scripts/build-web.sh.
#   dx serve --features web
```

### Production

```bash
# 1. Emit the deterministic artifact.
scripts/build-web.sh
# 2. Embed it into the daemon (the build-time guard rejects React output).
cargo build --release -p jeliyad --features embed-ui
```

## Scope boundary and deferrals

- `jeliya-ui`'s `PlatformServices` is a **provisional seam pending #174**; when
  #174 lands, the local seam is replaced by a re-export.
- The release line (`.github/workflows/release.yml`) is **not** flipped by
  #176 — the [architecture record](dioxus-architecture.md) keeps the React
  `ui/dist` archive as the shipped artifact until #200, and shipping the
  mock-composed foundation shell on a tag would present a fake `Ready` that
  performs no daemon operation. The consequence is stated plainly: because
  the daemon's `embed-ui` build now embeds `crates/jeliya-ui/dist` behind the
  fail-closed `build.rs` guard, a release attempted from `main` **fails
  closed at that guard** (the release runner builds only the React archive) —
  it does not build, rather than shipping either non-working UI. That does
  not regress the line: since the protocol-v2 cutover the daemon is v2-only
  and refuses the v1 React UI with `426 protocol_unsupported` at the
  handshake, so release-from-main has been a non-functional product since
  then and stays one until the live browser transport (#168/#171), the
  sealed artifact (#183), and the release-line cutover (#200) land.
  `scripts/check-release.mjs`'s `embedded_ui` npm contract and
  `scripts/realnet-evidence.mjs` continue to record the React path
  unchanged. `v0.6.0`, cut from its own tag, is unaffected. `ui/` (React)
  stays intact and its per-client gates keep running until #200.
