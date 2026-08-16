---
type: "Guide"
title: "Jeliya diagnostics and logging"
description: "Enable, locate, follow, rotate, and safely share jeliyad diagnostic logs on the released daemon and packaged desktop."
tags: ["diagnostics", "logging", "operations", "daemon", "desktop"]
timestamp: "2026-08-16T00:00:00Z"
status: "canonical"
implementation_status: "partial"
verification_status: "unverified"
release_status: "partial"
audience: ["operators", "contributors", "maintainers"]
---

# Jeliya diagnostics and logging

This guide explains how to enable, locate, follow, rotate, and safely share the
`jeliyad` daemon's diagnostic logs. It covers the released daemon on macOS,
Linux, and Windows, and the daemon as it runs under a supervising desktop
process. Everything here describes the current Dioxus/`jeliyad` stack; legacy
client launchers are out of scope (see [Clean-slate scope](#clean-slate-scope)).

Two very different things are called "logs" in Jeliya. This guide is about the
first; read [Diagnostic logs versus signed room event logs](#diagnostic-logs-versus-signed-room-event-logs)
before you share anything.

- **Diagnostic (process) logs** — the `tracing` output described here. Meant for
  troubleshooting, and safe to share **after redaction**.
- **Signed room event logs** — the product's cryptographically signed room
  state. Operational truth, **never** a troubleshooting artifact, and **never**
  something to attach to an issue.

## Status and scope of this guide

- The daemon's stderr and rolling-file logging is implemented and ships in the
  released daemon (`v0.6.0`).
- The packaged desktop application is not built in this repository yet, so its
  operating-system-specific way of setting environment variables and collecting
  logs is owned by issue #189 and marked pending below. The daemon-level truth
  in this guide holds regardless of how the daemon is launched.
- The in-app "export diagnostics" affordance is owned by issue #180 and does not
  exist yet; it is referenced as the intended push-button collection path.
- The normative redaction contract and release-security evidence are owned by
  issue #196; the safe-sharing checklist here defers to it.
- The commands below are derived from the daemon source and have **not** yet been
  exercised against a release candidate on every claimed operating system, so the
  page's `verification_status` is `unverified`. Treat the macOS and Windows paths
  as the documented intent until per-operating-system evidence is recorded.

## What jeliyad logs, and where

On startup the daemon installs two log sinks that receive the **same** filtered
event stream:

- **Standard error (stderr)** — human-readable lines for whoever launched the
  process. Ephemeral: it is only whatever the launching terminal or service
  captured.
- **A daily-rolling file** under the data directory, at `logs/` with base name
  `jeliyad.log`. This is the durable record.

The default level is `info`. The file sink is plain text with no ANSI colour;
both sinks suppress the event **target** column, so a line shows its level and
message but not the module it came from. Filtering still matches on the target
(see below), so targeted filters work even though the target is not printed.

The daemon also prints its data directory to standard output on startup (a
`ready` JSON line and a human-readable `data dir: …` line), so you can always see
which directory a running daemon is using.

## Diagnostic logs versus signed room event logs

This distinction is the most important safety rule on this page.

- **Diagnostic/process logs** are the `tracing` output: the dated files under the
  data directory's `logs/` folder, plus stderr. These are what "share your logs"
  means. They are safe to share **after** the redaction checklist below.
- **Signed room event logs** are the product's cryptographically signed room
  state, stored in the room store under the data directory (for example the room
  database and blob files). They are the product's operational source of truth,
  **not** a process log. They can contain message content and identities. Never
  treat them as diagnostic logs, and **never attach them to an issue**.

When someone asks you for "logs" to debug a problem, they mean the diagnostic
files, never the room store.

## Setting the log level and filters

The filter is chosen once, from the first source that is set:

1. `JELIYAD_LOG` (Jeliya-specific; wins if present),
2. otherwise `RUST_LOG`,
3. otherwise the built-in default `info`.

So `JELIYAD_LOG` overrides `RUST_LOG`, and `RUST_LOG` is honoured only when
`JELIYAD_LOG` is unset. Both variables use
[`tracing_subscriber::EnvFilter`](https://docs.rs/tracing-subscriber/0.3/tracing_subscriber/filter/struct.EnvFilter.html)
syntax: a comma-separated list of directives, each `target[=level]`. A bare level
(for example `debug`) sets the default for everything; a `target=level` directive
raises or lowers one module. Module paths use `::`.

The daemon process emits events under two targets today: `jeliyad` (the daemon
binary) and `jeliya_core` (the protocol engine). At the default `info` level,
dependency crates such as `iroh`, `hyper`, `tokio`, and `tungstenite` are
effectively silent. A bare global level like `debug` raises **all** of them
and can be very verbose; prefer the scoped form
`info,jeliyad=debug,jeliya_core=debug` to stay focused on Jeliya code. Use
a global `debug` only when you need to diagnose a problem in a dependency.

A parse-invalid directive in `JELIYAD_LOG` (or the value `JELIYAD_LOG=`)
causes the whole variable to be silently ignored; the daemon then falls through
to `RUST_LOG` and finally to `info`. If the level you set appears to have no
effect, check for a typo in the directive string first.

Useful, safe examples:

```sh
# Whole daemon at debug — all crates including dependencies; can be verbose.
JELIYAD_LOG=debug

# Daemon and engine at debug, dependencies left at info.
JELIYAD_LOG=info,jeliyad=debug,jeliya_core=debug

# One module only.
JELIYAD_LOG=info,jeliya_core::engine=debug

# Quiet baseline, daemon only.
JELIYAD_LOG=warn,jeliyad=debug
```

Setting the variable per shell:

```sh
# macOS / Linux (bash, zsh): one-shot for a single run.
JELIYAD_LOG=debug jeliyad

# Or persist it for the shell session.
export JELIYAD_LOG=info,jeliyad=debug,jeliya_core=debug
jeliyad
```

```powershell
# Windows PowerShell.
$env:JELIYAD_LOG = 'debug'
jeliyad
```

**`trace` is a footgun.** The `trace` level can surface high-cardinality and
potentially sensitive values. Do not enable it for logs you intend to share, and
never paste a `trace` run into an issue without careful redaction.

## Where the log files live

Logs live in a `logs/` subdirectory of the daemon's data directory. The default
data directory is a per-user platform location, so a launch from any working
directory always lands in the same place:

| Operating system | Default data directory | Log directory |
|---|---|---|
| macOS | `~/Library/Application Support/Jeliya` | `~/Library/Application Support/Jeliya/logs/` |
| Linux | `$XDG_DATA_HOME/Jeliya` (by default `~/.local/share/Jeliya`) | `~/.local/share/Jeliya/logs/` |
| Windows | `%APPDATA%\Jeliya` (`C:\Users\<user>\AppData\Roaming\Jeliya`) | `%APPDATA%\Jeliya\logs\` |

If no platform data directory can be discovered, the daemon falls back to
`./.jeliya-data` relative to its working directory, with logs at
`./.jeliya-data/logs/`.

You can move the whole root with `--data-dir DIR`; logs then live at `DIR/logs/`.
The daemon canonicalizes the path (resolving symlinks and relative spelling), so
the directory it reports on startup can differ textually from what you typed —
trust the printed path.

## Rotation, retention, and what survives restart

- **Rotation is daily.** With the base name `jeliyad.log`, the appender writes
  **dated** files named `jeliyad.log.YYYY-MM-DD` (for example
  `jeliyad.log.2026-08-16`). The date boundary is **UTC**.
- **There is no plain `jeliyad.log` file and no "current" symlink.** The newest
  dated file is the active one. Any tool or instruction that expects a bare
  `jeliyad.log` is wrong for this daemon.
- **Nothing is pruned.** Every day's file is kept; old dated files are never
  deleted automatically. Logs grow without bound across days, so cleanup of old
  files is the operator's responsibility.
- **Prior files survive restarts and crashes.** Restarting the daemon appends to
  the current day's file (or opens the next day's file after a rollover); older
  dated files are untouched.
- The file writer is non-blocking, so on a hard crash a few of the most recent
  buffered records can be lost. A clean shutdown flushes the buffer before exit.

## Following the active log

Follow the newest dated file:

```sh
# macOS / Linux — the current UTC day explicitly.
tail -f "$HOME/.local/share/Jeliya/logs/jeliyad.log.$(date -u +%F)"

# Or follow every dated file, so you always see the newest without computing
# the date (handy around the UTC midnight rollover).
tail -f "$HOME/.local/share/Jeliya"/logs/jeliyad.log.*
```

```powershell
# Windows PowerShell — pick the most recently written dated file and follow it.
$log = Get-ChildItem "$env:APPDATA\Jeliya\logs\jeliyad.log.*" |
  Sort-Object LastWriteTime | Select-Object -Last 1
if ($log) { Get-Content -Wait -Tail 50 $log.FullName }
else { Write-Host "No log files found — has jeliyad been run?" }
```

Because the daily boundary is UTC, a follow command that computes a **local**
date can point at the wrong file for a few hours around midnight; prefer the
glob/newest-file forms to sidestep it. Adjust the path if you launched the daemon
with `--data-dir`.

## Foreground versus supervised and packaged launches

### Foreground (you launched it)

When you run the daemon yourself, stderr goes to your terminal and the dated file
is written in parallel:

```sh
JELIYAD_LOG=debug jeliyad
# Optionally capture stderr to a file too:
JELIYAD_LOG=debug jeliyad 2> jeliyad.stderr.log
```

### Supervised or packaged (the app owns the daemon)

When the desktop application owns the daemon, it launches it in **supervised**
mode (`jeliyad --supervised --data-dir … --port 0`) and **drains and discards the
daemon's stderr**. There is no console to read.

> In a supervised or packaged launch, the dated file under the data directory's
> `logs/` folder is the **only** place the daemon's logs appear. This is the
> single most important operator fact on this page.

To raise the level for a supervised daemon, set the environment variable in the
environment of the process that launches the supervisor — the packaged app — and
then restart it. The daemon reads its filter from its own process environment at
startup, so the variable must already be present when the launcher starts. The
exact per-operating-system mechanism (a macOS launchd plist, a Linux
`.desktop`/systemd user environment, or the Windows application environment) is
owned by issue #189 and will be documented and verified against the packaged
build when it exists.

The in-app diagnostics export (issue #180) is the intended push-button way to
collect logs from a packaged launch. It does not exist yet; until it lands, read
the dated file directly.

## Runtime filter changes are unsupported

The filter is read **exactly once at daemon startup**. There is no runtime
reload, no signal, and no admin endpoint to change verbosity while the daemon is
running. To change the level you set the environment variable and **restart** the
daemon. Adding a runtime log-level flag is an explicit non-goal of this work.

## Before you share logs: redaction and safe collection

Diagnostic logs are safe to share only after redaction. The normative rules are
owned by issue #196; this checklist defers to it and names the values you must
never paste into an issue, a chat, or any external service:

- **The daemon authentication / bearer token.** By design it lives in the 0600
  `daemon.json` portfile, not in logs — but never paste `daemon.json` either. The
  supervisor holds this token behind a redaction shield that prints `<redacted>`,
  so a stray debug print cannot spill it; do not defeat that by pasting the
  portfile.
- **Single-use connect tickets** — the `?ct=` values minted by
  `POST /api/session`.
- **Invite tickets and room join material.**
- **Message bodies and any room content.**
- **Full private filesystem paths** that reveal a username or home directory.
  Scrub any absolute path down to a placeholder. Note that the daemon prints its
  data-directory path on startup and can mention paths in warnings, so this value
  routinely appears even at `info`.
- **Any secret or key material.**

Also:

- Prefer `info` or `debug` for logs you intend to share. Avoid `trace` unless
  asked, and redact it heavily if you must share it.
- Share the smallest useful slice — the lines around the failure, not an entire
  multi-day file.
- When the diagnostics export (issue #180) ships, prefer it: it is the intended
  redaction-aware collection path.

## How this fits release and security evidence

- Daemon supervision behaviour — including the drain-and-discard of a supervised
  daemon's stderr — is owned by issue #170.
- Packaged-desktop environment and log-collection evidence is owned by issue
  #189; treat the packaged specifics above as pending until that evidence exists.
- The in-app diagnostics export is owned by issue #180.
- Redaction rules and release-security evidence are owned by issue #196; see also
  the [security and threat model](security-threat-model.md) and
  [signing and notarization](signing-notarization.md) records, and the
  [platform matrix](platform-matrix.md) for per-operating-system release status.

## Clean-slate scope

This guide documents only the current Dioxus/`jeliyad` stack. Legacy client
launchers (the React web UI and the Flutter desktop and Android apps) are out of
scope and are being retired under issues #200–#203; when their owners remove
them, no launch instructions for them belong here. If you are looking for how a
legacy client surfaced logs, that path is not supported by this guide.
