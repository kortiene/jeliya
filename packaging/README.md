# Jeliya packaging & distribution (Phase 1)

These files distribute the `jeliyad` daemon as prebuilt, per-platform binaries.
The installer scripts and archive URLs are wired to `kortiene/jeliya`; they
become usable after the first `v*` GitHub Release has published assets.
`jeliya.rb` remains a per-release Homebrew formula template until the release
sha256 values are copied in.

## Files

| File | What it is |
| --- | --- |
| `../.github/workflows/release.yml` | GitHub Actions release build. Triggers on a `v*` tag push; builds `jeliyad` for five targets and attaches the archives (+ `.sha256` sidecars) to the GitHub Release. |
| `install.sh` | POSIX-sh one-liner installer for macOS + Linux (`curl \| sh`). Detects OS/arch, downloads the matching `.tar.gz`, installs `jeliyad` to `/usr/local/bin` (or `~/.local/bin`). |
| `install.ps1` | Windows PowerShell equivalent. Downloads the `.zip`, expands to `%LOCALAPPDATA%\Programs\Jeliya`, adds it to the user PATH. |
| `jeliya.rb` | Homebrew formula template. Belongs in a tap (`kortiene/homebrew-jeliya`), not homebrew-core. |

## How they fit together

1. You push a tag like `v0.1.0`.
2. `release.yml` builds one archive per target and uploads them to the Release:
   - `jeliyad-v0.1.0-aarch64-apple-darwin.tar.gz`
   - `jeliyad-v0.1.0-x86_64-apple-darwin.tar.gz`
   - `jeliyad-v0.1.0-x86_64-unknown-linux-musl.tar.gz`
   - `jeliyad-v0.1.0-aarch64-unknown-linux-musl.tar.gz`
   - `jeliyad-v0.1.0-x86_64-pc-windows-msvc.zip`
   - plus a `<asset>.sha256` next to each one.
3. End users install with `install.sh` / `install.ps1` (which resolve the
   latest tag, or a pinned `JELIYA_VERSION`), or with `brew install` once
   `jeliya.rb` is published in a tap with the real URLs + sha256s filled in.

## Mandatory build ordering (UI before cargo)

The release binary is built with the cargo feature `embed-ui`, which embeds
`ui/dist` into the binary via `rust-embed`. **`ui/dist` must exist before the
cargo build**, so every build path does, in order:

```sh
cd ui && npm ci && npm run build      # produces ui/dist  (do this FIRST)
cargo build --release -p jeliyad --features embed-ui   # (or `cargo zigbuild` for musl)
```

`release.yml` already enforces this ordering (the "Build UI" step runs before
the cargo build in every job). If you build a release binary by hand, do the
same or the UI will be missing/stale.

Linux targets use `cargo zigbuild` against `*-unknown-linux-musl` to produce
static binaries and dodge glibc-version breakage (the tree has C deps — `ring`,
`libsqlite3-sys` — and a QUIC/UDP stack via `iroh`, so a C toolchain is
required; zig supplies it).

## Release status (and the 2026-07-05 rename)

The project was renamed **Bantaba → Jeliya** on 2026-07-05 (`docs/naming.md`).
Everything in this directory targets the new name, so the published history
needs one bridging release:

1. **GitHub repo:** the remote is still `git@github.com:kortiene/bantaba.git`
   until the repo is renamed to `jeliya` on GitHub (GitHub redirects old URLs
   afterwards). Rename the repo **before** pushing the next tag.
2. **Existing releases:** `v0.1.0` and `v0.2.0` were published under the old
   name with `bantabad-<tag>-<target>` archives containing a `bantabad`
   binary. The current formula and install scripts look for
   `jeliyad-<tag>-<target>` — they **cannot install those old releases**.
3. **Next release (restores installs):** after the repo rename, push a new
   tag (e.g. `v0.3.0`); `release.yml` already builds `-p jeliyad` and
   packages `jeliyad-<tag>-<target>` archives. Then copy the new version and
   all four sha256 values into `jeliya.rb`.
4. **Homebrew tap:** rename the tap repo to `homebrew-jeliya` (formula file
   `Formula/jeliya.rb`) and push the updated formula.
5. **Redistribution rights:** confirmed for publishing built binaries that
   include the pinned `iroh-rooms` git dependency (unchanged by the rename).

## Per-release follow-up

For the next release, update `version` in `jeliya.rb`, push the new `v*` tag,
then replace the formula sha256 values from the new release sidecars.

`release.yml` needs no slug edit — it always builds the repo it runs in.

## Signing / notarization = Phase 2 (deferred)

Artifacts are **unsigned**. A *browser* download of an unsigned binary trips
Gatekeeper (macOS) and SmartScreen (Windows). The `curl | sh` and Homebrew
install paths do **not** set the quarantine bit, so they install cleanly.
macOS notarization and Windows Authenticode signing are deferred to Phase 2 and
are intentionally out of scope for these files.
