# Spec: Document jeliyad log levels, locations, rotation, and safe collection (issue #42)

Status: proposal / planning spec. This document tells an engineer or agent
exactly what operator-facing documentation to author, where, and with what
verified facts. It does **not** change production code and does **not** add a
new runtime log flag (issue non-goal). The deliverable is one new page in the
`docs/` wiki plus its index wiring.

## 1. Outcome

Publish an accurate user/operator guide for enabling, locating, filtering,
following, and safely sharing `jeliyad` diagnostic logs across the released
daemon and the packaged desktop surface. The guide must be true against the
current Dioxus/`jeliyad` stack and pass the documentation gate
(`node scripts/check-docs.mjs`).

## 2. Owning surfaces and sources of truth (verified against code)

Everything the guide states about behavior must trace to one of these. Cite
them in the PR body when the doc is authored; re-verify each before publication
because line numbers drift.

| Fact | Source of truth (this tree) |
|---|---|
| Filter precedence `JELIYAD_LOG` → `RUST_LOG` → `info`; `EnvFilter` syntax | `crates/jeliyad/src/lifecycle.rs` `init_tracing` (`EnvFilter::try_from_env("JELIYAD_LOG").or_else(try_from_default_env).unwrap_or(EnvFilter::new("info"))`) |
| Two sinks: stderr + daily-rolling file under `<data-dir>/logs/`, base `jeliyad.log` | `crates/jeliyad/src/lifecycle.rs` `init_tracing` (`tracing_appender::rolling::daily(&logs_dir, "jeliyad.log")`, `logs_dir = data_dir.join("logs")`) |
| File layer is plain text (`with_ansi(false)`); both layers set `with_target(false)` | `crates/jeliyad/src/lifecycle.rs` `init_tracing` |
| Filter is read once at startup; there is **no** `reload` handle | `crates/jeliyad/src/lifecycle.rs` (no `tracing_subscriber::reload` anywhere) |
| Log worker is non-blocking; the guard must be dropped to flush at exit | `crates/jeliyad/src/lifecycle.rs` (`tracing_appender::non_blocking`) + `crates/jeliyad/src/main.rs` (`drop(log_guard)` before `std::process::exit`) |
| Default data-dir per OS; `--data-dir` override; canonicalized | `crates/jeliyad/src/main.rs` `default_data_dir` (`dirs::data_dir().map(|d| d.join("Jeliya")).unwrap_or("./.jeliya-data")`) and `Args.data_dir` |
| Supervised launch discards stderr; rolling file is the only durable copy | `crates/jeliya-supervisor/src/supervisor.rs` (spawns `jeliyad --supervised --data-dir … --port 0`, `stderr(Stdio::piped())`, then a background task drains stderr in raw chunks and discards the bytes) |
| Secret shield used for the daemon auth token | `crates/jeliya-supervisor/src/redact.rs` `Redacted<T>` (Debug/Display print `<redacted>`; mirrors `jeliya_client::kernel::diag::Redacted`) |
| Released daemon targets (claimed OSes) | `packaging/README.md` (five archives: `aarch64`/`x86_64-apple-darwin`, `x86_64`/`aarch64-unknown-linux-musl`, `x86_64-pc-windows-msvc`) |
| Install locations per OS | `packaging/install.sh` (`/usr/local/bin` or `~/.local/bin`), `packaging/install.ps1` (`%LOCALAPPDATA%\Programs\Jeliya`, added to user PATH) |
| Signed room event log is the product's operational truth, not a process log | `docs/PROFILE.md` Non-goals ("Jeliya's signed room event log remains the product's source of operational truth") |
| Docs contract (frontmatter, single H1, reachability, no raw HTML) | `docs/PROFILE.md`, enforced by `scripts/check-docs.mjs` |

### 2.1 Downstream owners referenced by the issue

The guide must **reference** these but must not invent their behavior. Where a
surface is not yet built in this tree, say so plainly and use the least
favorable truthful status.

- **#170 — daemon supervision.** Implemented as `crates/jeliya-supervisor`.
  This is the authority for how an owned/adopted daemon is launched and how its
  stderr is handled (drained and discarded). Cite it for supervised collection.
- **#189 — packaged desktop evidence.** Owns the per-OS packaged-launch
  environment and log-collection evidence. At authoring time the packaged
  desktop app is **not** built in this tree (see `docs/platform-matrix.md`).
  Document the daemon-level truth and mark packaged-desktop specifics as owned
  by #189, to be verified against the packaged build when it exists.
- **#180 — diagnostics UI integration / diagnostics export.** Owns the in-app
  "export diagnostics" affordance. Reference it as the intended integration
  point; do not describe an export bundle format that does not yet exist.
- **#196 — redaction and release/security evidence.** Owns the authoritative
  redaction rules and the release-security evidence. The guide's safe-sharing
  section must align with #196 and defer the normative redaction contract to it.

## 3. Ground-truth behavior the guide must state (and must not contradict)

### 3.1 Levels and filtering
- Precedence: `JELIYAD_LOG` wins; else `RUST_LOG`; else the default `info`.
- Syntax is `tracing_subscriber::EnvFilter` (directives are
  `target[=level]`, comma-separated; a bare level sets the default, e.g.
  `debug`; module paths use `::`).
- The same filter applies to **both** the stderr sink and the file sink.
- Event **targets are suppressed in the printed output** (`with_target(false)`),
  but `EnvFilter` still matches on target — so targeted directives work even
  though the target column is not shown.
- Emitting targets inside the daemon process today are `jeliyad` (from
  `main.rs`, `lifecycle.rs`) and `jeliya_core` (from `engine.rs`,
  `protocol_upload.rs`); dependency crates (for example `iroh`, `hyper`,
  `tokio`, `tungstenite`) emit only when their targets are enabled. The author
  MUST re-derive this list from the code at authoring time (grep `tracing::`
  macros in the daemon's dependency graph) rather than copying it blindly.
- Useful, safe examples to include (verify each runs):
  - Whole-daemon debug, still quiet-ish: `JELIYAD_LOG=debug`
  - Daemon + core at debug, deps quiet: `JELIYAD_LOG=info,jeliyad=debug,jeliya_core=debug`
  - One module: `JELIYAD_LOG=info,jeliya_core::engine=debug`
  - Quiet baseline, daemon only: `JELIYAD_LOG=warn,jeliyad=debug`
- **`trace` is a footgun**: state that `trace` can surface high-cardinality,
  potentially sensitive values and must not be pasted into an issue unredacted.

### 3.2 Locations and rotation
- Logs live at `<data-dir>/logs/`. Default `<data-dir>` per OS:
  - macOS: `~/Library/Application Support/Jeliya`
  - Linux: `$XDG_DATA_HOME/Jeliya`, i.e. `~/.local/share/Jeliya` by default
  - Windows: `%APPDATA%\Jeliya`, i.e. `C:\Users\<user>\AppData\Roaming\Jeliya`
  - Fallback when no platform dir is discoverable: `./.jeliya-data` (relative to
    the working directory)
- `--data-dir DIR` moves the whole root; logs then live at `DIR/logs/`. The
  daemon canonicalizes the path, so the printed path may differ from the spelled
  one.
- Rotation is **daily**. With base name `jeliyad.log`, `tracing-appender` writes
  **dated** files named `jeliyad.log.YYYY-MM-DD`. State explicitly that there is
  **no** plain `jeliyad.log` file and **no** "current" symlink — the newest
  dated file is the active one.
- The daily boundary is a wall-clock **date** as `tracing-appender` computes it.
  The author MUST verify empirically whether that date is UTC or local before
  claiming a timezone (0.2.5 rolls on UTC date; confirm on the RC).
- **No pruning**: the daily appender keeps every dated file; nothing deletes old
  days. State that operators are responsible for cleanup and that logs grow
  unbounded across days.

### 3.3 What survives restart
- Restarting appends to the same day's file (or opens the next day's file at the
  rollover). Prior dated files are untouched and survive restarts and crashes.
- stderr output is ephemeral (it is whatever the launching terminal/service
  captured); the dated files are the durable record.
- On a hard crash a few buffered records can be lost because the file writer is
  non-blocking; a clean shutdown drops the guard and flushes.

### 3.4 Following the active file
- Provide copy-paste follow commands per OS, targeting the newest dated file:
  - macOS/Linux: `tail -f "<data-dir>/logs/jeliyad.log.$(date -u +%F)"` (note the
    UTC-vs-local caveat; also show a glob form
    `tail -f "<data-dir>"/logs/jeliyad.log.*` for "whatever is newest").
  - Windows PowerShell: `Get-Content -Wait -Tail 50 (Get-ChildItem "$env:APPDATA\Jeliya\logs\jeliyad.log.*" | Sort-Object LastWriteTime | Select-Object -Last 1)`
- All angle-bracket placeholders such as `<data-dir>` and `<user>` MUST be inside
  inline code or a fenced block (see §6 authoring hazard).

### 3.5 Foreground vs supervised/packaged collection
- **Foreground / manual run**: `JELIYAD_LOG=debug jeliyad …` prints to stderr
  (redirect with `2> jeliyad.stderr.log` if desired) and also writes the dated
  file. Show setting the env var per OS shell (`VAR=… cmd`, `export`, and
  PowerShell `$env:JELIYAD_LOG='debug'`).
- **Supervised / packaged**: when the desktop app owns the daemon through
  `jeliya-supervisor`, the child is spawned `--supervised` and its **stderr is
  drained and discarded**. There is no console to read. The **dated file in
  `<data-dir>/logs/` is the only place logs appear** — this is the single most
  important operator fact and must be stated prominently.
  - To raise the level for a supervised daemon, the environment must be set for
    the process that launches the supervisor (the packaged app), then restart.
    The exact per-OS mechanism (launchd plist, `.desktop`/systemd user env,
    Windows app env) is **owned by #189**; document the daemon-level requirement
    (env must be present in the launcher's environment before start) and defer
    the packaged specifics to #189, to be filled in and verified against the
    packaged build.
- Cross-link the in-app diagnostics export (#180) as the intended
  push-button collection path once it exists.

### 3.6 Runtime filter changes are unsupported
- State clearly: the filter is read exactly once at daemon startup; there is no
  runtime reload, no signal, and no admin endpoint to change verbosity live.
  Changing the level requires setting the env var and **restarting** the daemon.
- Note the non-goal: this issue does not add a runtime log-level flag.

### 3.7 Diagnostic logs vs signed room event logs (must be clearly distinguished)
- **Diagnostic/process logs** = the `tracing` output described here
  (`<data-dir>/logs/jeliyad.log.*` and stderr). Safe-to-share after redaction;
  intended for troubleshooting.
- **Signed room event logs** = the product's cryptographically signed room
  state (in the room store under `<data-dir>`, e.g. `rooms.db`/blobs), which is
  the product's operational source of truth per `docs/PROFILE.md`. These are
  **not** diagnostic logs, are **not** what "share your logs" means, and must
  **never** be attached to an issue (they can contain message content and
  identities). Make this boundary explicit and unambiguous.

## 4. Safe-sharing and redaction guidance (name the prohibited values)

The guide must include an explicit "before you paste logs into an issue"
checklist. Align with #196 (owns the normative rules) and state that the guide
defers to it. At minimum, name these as **never paste**:

- The daemon auth token / bearer token (held redacted as `Redacted<T>` in the
  supervisor; it lives in the `daemon.json` portfile, **not** in logs by design —
  but never paste `daemon.json`).
- Single-use connect tickets (the `?ct=` values minted by `POST /api/session`).
- Invite tickets / room join material.
- Message bodies and any room content.
- Full private filesystem paths that reveal a username or home directory
  (redact `<data-dir>` and any absolute path down to a placeholder).
- Any secret/key material.

Also state:
- Prefer `info`/`debug` for shared logs; avoid `trace` unless asked, and redact.
- The daemon already prints its data-dir path on startup (stdout and info logs);
  tell operators to scrub that path before sharing.
- Point to the diagnostics export (#180) as the preferred, redaction-aware
  collection path when available.

## 5. Target document and index wiring

- **Path**: `docs/diagnostics-logging.md` (kebab-case; author may choose
  `daemon-logging.md` if preferred, but keep title/H1 in sync).
- **Type**: `Guide` (task-oriented operator instructions).
- **Frontmatter** (exactly the ten required fields; use the least favorable
  truthful status axes because packaged-desktop specifics are unbuilt and the
  commands are unverified until §7 runs):

```yaml
---
type: "Guide"
title: "Jeliya diagnostics and logging"
description: "Enable, locate, follow, rotate, and safely share jeliyad diagnostic logs on the released daemon and packaged desktop."
tags: ["diagnostics", "logging", "operations", "daemon", "desktop"]
timestamp: "<authoring-UTC-instant>"
status: "canonical"
implementation_status: "partial"
verification_status: "unverified"
release_status: "partial"
audience: ["operators", "contributors", "maintainers"]
---
```

  Rationale for the axes (adjust only with evidence):
  - `implementation_status: partial` — daemon file/stderr logging is implemented;
    packaged-desktop diagnostics and in-app export are not in this tree.
  - `verification_status: unverified` until §7 evidence is recorded against a
    release candidate; upgrade to `partial`/`verified` with a Status Report or an
    evidence link when the per-OS runs are done.
  - `release_status: partial` — the daemon (and its file logging) ships in
    `v0.6.0`; packaged-desktop diagnostics do not.
- **Title/H1**: the single `#` heading must read exactly `Jeliya diagnostics and
  logging` (must equal `title`).
- **Index wiring** (`docs/index.md`): add one bullet under
  **Operations and release evidence** (or a new **Diagnostics** subsection if the
  author prefers) linking `diagnostics-logging.md` with a one-line description.
  Reachability from `docs/index.md` is required or CI fails.
- **Inbound cross-links to add** so the page is well-connected: from
  `docs/security-threat-model.md` (redaction/evidence) and optionally
  `docs/known-gaps-roadmap.md`. Keep links file-relative, no leading slash.

## 6. Authoring hazards (docs gate) — read before writing

`scripts/check-docs.mjs` will reject the page for any of these; the doc is
angle-bracket-heavy, so the first one is the likely failure:

1. **Raw-HTML from angle-bracket placeholders.** The gate masks fenced code,
   indented code, and inline backtick spans first, then flags anything matching
   `<tag …>` as `raw-html`. `<data-dir>`, `<date>`, `<user>`, `<port>` in plain
   prose **will** trip it. Every such placeholder MUST be inside inline code
   (`` `<data-dir>` ``) or a fenced block. Prefer showing paths in code spans or
   code fences throughout.
2. **Single H1.** Exactly one `#` heading (the title); start sections at `##`.
3. **Frontmatter subset.** Double-quoted strings, flow arrays, exactly the ten
   fields, no unknown keys, valid UTC `timestamp`.
4. **Links.** File-relative only; no leading slash; fragments must resolve; any
   external link is a credential-free `https://` URL.
5. **No secrets in examples.** Use obvious placeholders, never a real token,
   ticket, or absolute home path.
6. Run the gate locally: `node scripts/check-docs.mjs`.

## 7. Verification plan (required before publication)

Acceptance requires that documented commands are exercised against a **release
candidate** daemon and, where claimed, a packaged desktop build, on each claimed
OS (macOS, Linux, Windows). Record results as evidence (a Status Report page or
an entry under `docs/evidence/…`, or at minimum a PR-body evidence block) and
only then consider raising `verification_status`.

For each claimed OS:
1. Start the daemon with no env → confirm default level is `info` and the dated
   file appears at the documented default `<data-dir>/logs/jeliyad.log.<date>`.
2. Start with `JELIYAD_LOG=debug` → confirm debug lines appear in both stderr and
   the dated file; confirm `RUST_LOG` is honored only when `JELIYAD_LOG` is unset,
   and that `JELIYAD_LOG` overrides `RUST_LOG`.
3. Exercise each targeted-filter example and confirm it filters as documented and
   that targets match despite the suppressed target column.
4. Confirm the exact rotation filename and the UTC-vs-local date boundary; confirm
   there is no plain `jeliyad.log` and no symlink; confirm old dated files survive
   a restart and that nothing is auto-pruned.
5. Confirm the follow command works and selects the newest dated file.
6. `--data-dir DIR` → confirm logs move to `DIR/logs/` and the canonicalized path
   is what the daemon reports.
7. Supervised path: confirm a `--supervised` daemon's stderr is not visible to the
   parent and that the dated file still captures records (this is already asserted
   by supervisor tests; cite them and also verify end-to-end).
8. Redaction: grep a `debug` run's dated file for the auth token and connect
   tickets and confirm they are absent; confirm the data-dir path is the only
   private value present and is called out for scrubbing.

If a claimed OS or the packaged build cannot be exercised, do **not** claim it:
state exactly what was not verified and who owns it (#189 for packaged desktop),
and keep the page's status axes truthful.

## 8. Clean-slate cutover handling

- Document **only** the current Dioxus/`jeliyad` stack. Do not add Flutter/React
  launch instructions.
- If, at authoring time, legacy launch paths are still shipped (per
  `docs/platform-matrix.md` the React/Flutter clients still exist in-tree),
  keep the guide daemon- and Dioxus-focused and add a one-line note that legacy
  client launchers are out of scope and will be removed when their owners
  (#200/#201/#202/#203) retire them. Prefer no legacy content over stale content.

## 9. Acceptance criteria (maps issue ACs + repo gate)

- [ ] `JELIYAD_LOG`, `RUST_LOG`, default `info`, and at least three useful
  targeted-filter examples are documented and verified.
- [ ] Default and `--data-dir` log locations, the dated rotation filename, the
  no-plain-file/no-symlink fact, no-pruning, and restart survival are correct on
  macOS, Linux, and Windows.
- [ ] Foreground and supervised/packaged collection instructions are present;
  the supervised "stderr is discarded → read the dated file" fact is stated
  prominently; packaged-desktop env/collection specifics defer to #189.
- [ ] Diagnostic logs and signed room event logs are clearly and unambiguously
  distinguished, with an explicit "never attach signed room logs" statement.
- [ ] Safe-sharing/redaction guidance names the prohibited values (auth/bearer
  token, `?ct=` connect tickets, invite tickets, message bodies/room content,
  full private paths, key material) and defers the normative contract to #196.
- [ ] "Runtime filter changes are unsupported; restart required" is stated.
- [ ] Cross-links to #170 (supervision), #180 (diagnostics export), #189
  (packaged evidence), #196 (redaction/security) are present and truthful.
- [ ] `node scripts/check-docs.mjs` passes; the page is reachable from
  `docs/index.md`; frontmatter has exactly the ten fields with truthful status
  axes.
- [ ] Commands were verified against a release candidate on each claimed OS (or
  the unverified surfaces are explicitly named with their owner).

## 10. Risks and open questions

- **Timezone of the daily boundary.** `tracing-appender` 0.2.5 rolls on a UTC
  date; the follow command using local `date +%F` can point at the wrong file
  around midnight. Verify and document the actual timezone; prefer the glob-based
  "newest file" follow command to sidestep it.
- **Packaged desktop is unbuilt here.** Anything OS-specific about how a packaged
  app sets env or surfaces logs is owned by #189 and must be marked pending, not
  invented. Keep `implementation_status`/`release_status` truthful.
- **Angle-bracket density vs the raw-HTML rule.** The most likely gate failure;
  enforce code-span/fence discipline for every placeholder.
- **Target list drift.** The emitting-target set is derived from current code;
  re-derive at authoring time and avoid promising deps that emit nothing at the
  default level.
- **Data-dir path leakage.** The daemon prints its data-dir on startup and in
  logs; the redaction section must call this out even though it is not a
  "secret" per se.
- **Open question:** should the guide live under a new `## Diagnostics` group in
  `docs/index.md` or under existing `## Operations and release evidence`?
  Recommendation: reuse the existing group unless #180 lands companion pages.
- **Open question:** does #196 want the redaction checklist to live in this Guide
  or to be referenced from a #196-owned page? Default to a short checklist here
  that explicitly defers the normative list to #196.
