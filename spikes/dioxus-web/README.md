# Dioxus web spike — results (issue #158)

**Disposable.** This crate is not a workspace member, is not built by CI, and is
not the shape the real client should take. It exists to answer one question:
*can a Dioxus/WASM slice render a real room against a real `jeliyad` from
embedded assets?* The answer is yes. Everything below is evidence for that
claim and nothing below is a commitment.

Ran 2026-07-28 against `jeliyad` 0.6.1 (protocol 1), Dioxus 0.7.9,
wasm-bindgen 0.2.126, rustc 1.97.1, aarch64 Linux, Chromium via Playwright
1.61.1.

## Acceptance criteria

| # | Criterion | Result |
|---|---|---|
| 1 | WASM feature graph contains no native core/Iroh dependency | **pass** — 124 crates resolve for `wasm32-unknown-unknown`; no `iroh`, `jeliya-core`, `jeliyad`, `jeliya-ffi`, `quinn`, `rustls`, or `hickory`. Enforced by `./check-wasm-graph.sh`, not by inspection |
| 2 | Development and `embed-ui` builds serve the Dioxus slice | **pass** — both. The dev path is `--ui-dir dist`; the embedded path runs a `--features embed-ui` daemon with **no** `--ui-dir` and serves the spike's own `index.html` and its 542,111-byte wasm from bytes compiled into the binary |
| 3 | Bootstrap, room list/open, send, and push receive pass against real `jeliyad` | **pass** — Playwright scenarios at desktop and both compact viewports, green on both serving paths, against a supervised daemon on a fresh temp data dir |
| 4 | CSS reuse and bundle/runtime measurements are recorded | **pass** — below |
| 5 | Results update the ADR without promoting spike code by assumption | done — see `docs/dioxus-architecture.md` |

## CSS reuse

`ui/src/styles.css` is copied into `dist` **byte-identical** (`cmp` clean, zero
edits), and the RSX emits the React client's own markup. All 25 classes the
slice uses resolve to rules in that file — `.app`, `.pane-rooms`, `.pane-room`,
`.sidebar`, `.room-item`, `.room-select`, `.room-info`, `.room-name-line`,
`.room-name`, `.center`, `.center-empty`, `.center-empty-title`, `.timeline`,
`.msg-row`, `.msg-col`, `.msg-bubble`, `.composer`, `.composer-bar`,
`.composer-send`, `.boot-screen`, `.boot-target`, `.error-note`, `.error-title`,
`.error-code`, `.mono` — plus the descendant rule `.composer-bar textarea`.

Assertions target computed style rather than the presence of a class, so an
unstyled render fails: `.msg-bubble` must have a non-transparent background, and
the composer must be a `TEXTAREA` computing `resize: none` (a browser-default
textarea is `resize: both`). Both were verified to fail against a deliberately
reverted build, so they are regression tests rather than decoration.

**Two corrections found in review**, both of which had made the reuse claim
broader than the evidence:

- The composer was an `input` carrying a `.composer-input` class **that has no
  rule in the stylesheet**. The editor is styled as `.composer-bar textarea`,
  which is why React renders a textarea. The composer was therefore browser
  default, not reused, and the original "23 classes resolve" count did not
  include the class the markup actually emitted.
- The root was always `class="app"`. Below 899.98px the stylesheet is a
  one-pane-at-a-time shell — `.app .sidebar` and `.app .center` are
  `display: none`, revealed only by a root pane state — so **every compact
  viewport rendered a blank screen**, including a phone system WebView. React
  sets `app pane-${pane}`; the slice now does too. `e2e/compact.spec.ts` asserts
  it at 390×844 and 320×568, and fails at both against the reverted build.

The lesson is about the test matrix, not the CSS: a desktop-only suite could not
see either defect. The repo's own `ui-e2e` runs four viewports for this reason.

This says the design system's **CSS** survives a renderer swap, on desktop and
compact. It does not say the design-token *gates* survive:
`scripts/check-design-tokens.mjs` reads `ui/src/styles.css`, and what enforces
the tokens after that file retires is undecided (#177).

## Bundle

No `wasm-opt`. The Dioxus CLI (`dx`) could not be installed here — it pulls
`openssl-sys`, which needs system OpenSSL headers this machine lacks — so the
build is `cargo build --release` plus `wasm-bindgen`, and these numbers are
therefore an **upper bound**. A `dx` build applies `wasm-opt`, which typically
takes 15–30% off the wasm. Do not quote these as budgets; #198 sets budgets.

| Artifact | Raw | gzip -9 |
|---|---|---|
| `jeliya_spike_dioxus_web_bg.wasm` | 542,111 B (529 KiB) | 211,557 B (207 KiB) |
| `jeliya_spike_dioxus_web.js` | 83,570 B | 12,352 B |
| `styles.css` (verbatim, unminified) | 95,524 B | 23,539 B |
| dioxus JS snippets (7 files) | 25,971 B | — |
| **dist total** | **747,884 B (730 KiB)** | — |

For scale, not as a verdict: the current React build ships
`assets/index-*.js` at 343,980 B and `assets/index-*.css` at 53,699 B, both
Vite-minified. The spike's CSS is unminified because it is a straight copy.

## Runtime

Loopback, debug daemon, Chromium. `appInteractive` covers wasm fetch and
instantiate plus seven daemon round trips (`/api/session`, socket open,
`daemon.status`, `identity.create`, `room.list`, `room.create`, `room.list`).

| Measure | Value |
|---|---|
| first contentful paint | 72 ms |
| app interactive | 263 ms |
| room open → send → live push rendered | 216 ms |
| wasm transfer | 542,153 B |
| JS heap after bootstrap | ~10 MB |

## Findings worth carrying forward

**The daemon serves embedded assets uncompressed.** A request with
`Accept-Encoding: gzip, br` gets `content-length: 542111` and no
`content-encoding`. On loopback that costs nothing. It matters to #183 and #113
if the same artifact is ever delivered over a network, and to #198's bundle
budget: gzip -9 alone takes this wasm from 529 KiB to 207 KiB, a 61% reduction
that is currently left on the table. Whether the daemon should negotiate
encoding, or ship pre-compressed bytes with the sealed manifest, is an open
question this spike did not answer.

**`dx` is not installable without system OpenSSL.** Pinning the toolchain is
#176's job; that issue should record the dependency, because it decides whether
CI images and contributor machines need `libssl-dev`.

**One socket dispatches strictly serially.** `crates/jeliyad/src/serve.rs`
awaits each `handle_frame` inside its `select!` arm, so while a slow call is in
flight that connection reads no further requests *and* forwards no pushes. The
spike is too fast to hit it, but it is a real constraint on #168's bounded
kernel and on #171.

**Nothing here is a protocol-v2 baseline.** The slice speaks v1 because that is
what exists; `src/proto.rs` says so at the top. #161 may retain, rename,
combine, or remove any of it, and these structs must not be lifted into
`jeliya-api`.

## Running it

```sh
cargo build -p jeliyad                 # from the repo root
./build.sh                             # wasm + wasm-bindgen + assets -> dist
./check-wasm-graph.sh                  # acceptance criterion 1
ln -sfn ../../ui/node_modules node_modules   # borrow the repo's Playwright
npx --no-install playwright test --config playwright.config.mjs
```

Playwright starts **one** daemon per invocation, so every spec in a run shares
one data dir and one room. The scenario therefore measures against the state it
finds — count before, count after — rather than assuming an empty timeline. An
earlier version asserted `toHaveCount(0)` before sending and failed the moment a
second spec ran first; that was the harness lying about isolation, not the slice
failing. Trace, screenshot, and video are retained on failure and that retention
is proven, not assumed — it captured the artifacts for exactly that failure.

For the embedded path: stage `dist` into `ui/dist` (a gitignored build
artifact — stash the React build first), `cargo build -p jeliyad --features
embed-ui`, then run with `SPIKE_EMBEDDED=1`, which passes `--no-ui-dir` so the
daemon has nothing to serve except what is compiled into it. Restore `ui/dist`
afterwards with `cd ui && npm run build`.

The fixture creates a fresh temp data dir per run and deletes it on exit, so the
spike leaves no durable state.
