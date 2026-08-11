# `jeliya-ui/assets` — canonical shared-asset consumption (§7)

The shared stylesheet is consumed **canonically, referenced not copied**. There
is exactly one source of truth for the design-system CSS:

- **`ui/src/styles.css`** — the single stylesheet. It is unchanged by #176 and
  is still validated against `assets/design-tokens.json` by
  `scripts/check-design-tokens.mjs`, which continues to read `ui/src/styles.css`
  (the Dioxus-side token gate is #177, recorded as a deferred gap).

No divergent duplicate of that file is committed here. Instead:

- `scripts/build-web.sh` injects `ui/src/styles.css` into the build output as
  `dist/styles.css`, and a CI byte-equality guard asserts the served copy is
  identical to its single source, so a copy cannot silently drift (the same
  "no divergence" property the token fixture enforces today).
- `crates/jeliya-ui/index.html` references it as `/styles.css` (root-relative,
  so the daemon's SPA fallback can serve the document at nested routes).

Static assets the shell actually references (favicon, `og.png`,
`site.webmanifest`, …) are pulled into the deterministic hashed output only when
the shell uses them; unreferenced assets are not embedded.

One generated file lives here and is **not** committed (gitignored):
`build.rs` refreshes `assets/styles.css` from the single source on every build
so the `dx serve --features web` dev loop (whose `[web.resource]` declares
`/styles.css`) serves a styled shell — `dx` never runs `build-web.sh`, so
without the copy the dev server would 404 the stylesheet. Being regenerated
from `ui/src/styles.css` each build, it cannot drift.

This directory otherwise holds `jeliya-ui`-specific static assets (none yet).
The design tokens stay sourced from the repo-root `assets/design-tokens.json`.
