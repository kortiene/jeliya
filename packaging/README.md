# Bantaba packaging & distribution (Phase 1)

These files distribute the `bantabad` daemon as prebuilt, per-platform binaries.
They are **templates**: every one contains a clearly-marked `OWNER/REPO`
placeholder and is **inert until Phase 0 is done** (a git remote exists and a
first tagged release has been published).

## Files

| File | What it is |
| --- | --- |
| `../.github/workflows/release.yml` | GitHub Actions release build. Triggers on a `v*` tag push; builds `bantabad` for five targets and attaches the archives (+ `.sha256` sidecars) to the GitHub Release. |
| `install.sh` | POSIX-sh one-liner installer for macOS + Linux (`curl \| sh`). Detects OS/arch, downloads the matching `.tar.gz`, installs `bantabad` to `/usr/local/bin` (or `~/.local/bin`). |
| `install.ps1` | Windows PowerShell equivalent. Downloads the `.zip`, expands to `%LOCALAPPDATA%\Programs\Bantaba`, adds it to the user PATH. |
| `bantaba.rb` | Homebrew formula template. Belongs in a tap (`OWNER/homebrew-bantaba`), not homebrew-core. |

## How they fit together

1. You push a tag like `v0.1.0`.
2. `release.yml` builds one archive per target and uploads them to the Release:
   - `bantabad-v0.1.0-aarch64-apple-darwin.tar.gz`
   - `bantabad-v0.1.0-x86_64-apple-darwin.tar.gz`
   - `bantabad-v0.1.0-x86_64-unknown-linux-musl.tar.gz`
   - `bantabad-v0.1.0-aarch64-unknown-linux-musl.tar.gz`
   - `bantabad-v0.1.0-x86_64-pc-windows-msvc.zip`
   - plus a `<asset>.sha256` next to each one.
3. End users install with `install.sh` / `install.ps1` (which resolve the
   latest tag, or a pinned `BANTABA_VERSION`), or with `brew install` once
   `bantaba.rb` is published in a tap with the real URLs + sha256s filled in.

## Mandatory build ordering (UI before cargo)

The release binary is built with the cargo feature `embed-ui`, which embeds
`ui/dist` into the binary via `rust-embed`. **`ui/dist` must exist before the
cargo build**, so every build path does, in order:

```sh
cd ui && npm ci && npm run build      # produces ui/dist  (do this FIRST)
cargo build --release -p bantabad --features embed-ui   # (or `cargo zigbuild` for musl)
```

`release.yml` already enforces this ordering (the "Build UI" step runs before
the cargo build in every job). If you build a release binary by hand, do the
same or the UI will be missing/stale.

Linux targets use `cargo zigbuild` against `*-unknown-linux-musl` to produce
static binaries and dodge glibc-version breakage (the tree has C deps — `ring`,
`libsqlite3-sys` — and a QUIC/UDP stack via `iroh`, so a C toolchain is
required; zig supplies it).

## Phase 0 prerequisites (do these before any of this works)

1. **Create the GitHub remote / repo** and decide the org. Nothing here is
   wired to a real org yet — that is deliberate.
2. **Confirm redistribution rights.** The `iroh-rooms` SDK is a *git*
   dependency (`github.com/kortiene/iroh-room`, pinned rev — not on crates.io)
   and is compiled into `bantabad`. Confirm its license permits redistributing
   the built binary before publishing artifacts. (The workflow only *fetches*
   from git at build time; it never runs `cargo publish`.)
3. **Push a first `v*` tag** so a Release (and its assets) exist for the
   installers/formula to point at.

## Placeholders a human must fill

| Placeholder | Where | Fill with |
| --- | --- | --- |
| `OWNER/REPO` | `install.sh`, `install.ps1`, `bantaba.rb` (homepage + urls) | the real GitHub slug, e.g. `your-org/bantaba` |
| `OWNER/homebrew-bantaba` | `bantaba.rb` comment | your tap repo |
| `version "0.1.0"` | `bantaba.rb` | the release number (no leading `v`) |
| `REPLACE_WITH_*_SHA256` (×4) | `bantaba.rb` | sha256 of each tarball — copy from the `<asset>.sha256` files the workflow uploads |

`release.yml` needs no slug edit — it always builds the repo it runs in.

## Signing / notarization = Phase 2 (deferred)

Artifacts are **unsigned**. A *browser* download of an unsigned binary trips
Gatekeeper (macOS) and SmartScreen (Windows). The `curl | sh` and Homebrew
install paths do **not** set the quarantine bit, so they install cleanly.
macOS notarization and Windows Authenticode signing are deferred to Phase 2 and
are intentionally out of scope for these files.
