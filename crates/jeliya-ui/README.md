# `jeliya-ui` — shared Dioxus UI crate (#176)

The production-shaped, system-WebView-targeted Dioxus 0.7 component library and
application root for the Jeliya clean-slate stack. It consumes `jeliya-api`
(typed view models), `jeliya-client::ClientHandle` (the lifecycle-aware seam),
and an injected `PlatformServices` boundary, and selects target composition
only at the crate root. See `docs/dioxus-architecture.md` (Decisions 1/3/5) and
`docs/dioxus-web-build.md` (the reproducible-build contract).

This is the M3 *web foundation*: it produces the one reproducible artifact the
daemon embeds and the desktop/Android system-WebView shells (M4/M5) later render
unchanged. It is **not** feature-complete UI and does **not** build the native
desktop shell. It renders against the deterministic **mock** — it opens no
socket; the real browser transport (`WsWeb`) is #168.

## Feature graph

| feature | pulls | built where |
|---|---|---|
| `default` (none) | no Dioxus, no OpenSSL | MSRV `--workspace`, clippy, tests |
| `ui` | `dioxus/minimal` + `jeliya-client/mock` | shared surface (native + wasm32) |
| `web` | `ui` + `dioxus/web` | **wasm32 only**, the web job |
| `native` | `ui` | layering seam only; the renderer (`dioxus-desktop`, OpenSSL-bearing) is wired in by M4 (#186–#189) |

## Development

```bash
# Build the browser artifact into ./dist (pinned toolchain; no dx, no npm).
scripts/build-web.sh

# Iterate against a real daemon without rebuilding it: the daemon serves the
# Dioxus dist/ from its own loopback origin (the #158 spike's dev loop).
cargo run -p jeliyad -- --ui-dir crates/jeliya-ui/dist

# The equivalent dx hot-reload loop (documented in Dioxus.toml; the canonical,
# determinism-checked build is scripts/build-web.sh — see Open Question O-4):
#   dx serve --features web
```

## Production

```bash
# 1. Emit the deterministic artifact (byte-identical across two clean builds).
scripts/build-web.sh

# 2. Embed it into the daemon. The embed folder is crates/jeliya-ui/dist (NOT
#    ui/dist); a build-time guard rejects React/Vite output (§9).
cargo build --release -p jeliyad --features embed-ui
```

## What CI asserts

- `scripts/check-jeliya-ui-wasm-graph.sh` — the wasm32 `web` graph excludes
  Iroh/native crates (AC-2). Also asserted in `tests/boundaries.rs`.
- `scripts/build-web.sh` + `scripts/check-web-determinism.sh` — the artifact is
  reproducible and byte-identical across two clean builds (AC-1/AC-3).
- `scripts/check-dx-pin.sh` + `scripts/check-web-build-toolchain.sh` — CI cannot
  fetch an unpinned `dx`, and the canonical build depends on no
  React/Vite/Flutter tooling and never reads `ui/dist` (AC-5).
- Shared CSS/tokens are consumed canonically from the single source (AC-4).
- A headless, offline render smoke (`e2e/render.spec.ts`) asserts the shell
  renders with the shared design system driving computed style.
