# Dioxus desktop spike — results (issue #159)

**Disposable.** This crate is not a workspace member, is not built by CI, and is
not the shape the real desktop shell should take. It exists to answer one
question: *can a Dioxus desktop shell supervise or adopt a real `jeliyad`, talk
to it from native Rust, render through the system WebView, and tear down
without orphaning or murdering a daemon?* The answer is **yes, with four
recorded caveats**. Everything below is evidence for that claim and nothing
below is a commitment.

Ran 2026-07-28 against `jeliyad` 0.6.1 (protocol 1), Dioxus 0.7.9,
wry/tao via `dioxus-desktop` 0.7.9, rustc 1.97.1, aarch64 Ubuntu 24.04,
webkit2gtk-4.1 2.52.3, X11 (`DISPLAY=:1`).

## Acceptance criteria

| # | Criterion | Result |
|---|---|---|
| 1 | A packaged shell resolves a bundled sidecar and performs the ready/portfile/health agreement | **pass** — `./build.sh` produces a bundle whose shell finds `jeliyad` at `$exeDir/jeliyad` with **no** `JELIYAD_BIN` set, then renders live daemon facts |
| 2 | The shell connects over native Rust WebSocket auth, not WebView JavaScript | **pass** — `Authorization: Bearer` from the 0600 portfile; `/api/session` is never called; the token is provably absent from the rendered DOM |
| 3 | Bootstrap status renders through the system WebView | **pass** — measured, not assumed: `getBoundingClientRect` 868×24, computed weight 700, themed background `rgb(7,13,16)` |
| 4 | Owned versus adopted shutdown is proven | **pass** — four headless tests plus an end-to-end run where the shell adopts a harness-owned daemon and leaves it alive |
| 5 | Security defaults fail closed | **partial** — navigation is closed; downloads and new-window are **not closeable** in this version. See "Negative results" |

## What is proven, and how

`./evidence.sh` drives the **real built binary**, not a test harness linking the
supervisor. It starts a daemon *itself* so it knows the auth token, which is
what makes the token-absence assertion possible — and as a side effect forces
the shell down the adopted path, where getting it wrong means killing a daemon
that belongs to someone else.

```
PASS  the heading has non-zero laid-out geometry
PASS  the stylesheet applied (weight 700, not a browser default)
PASS  the dark theme applied (body background is not transparent/white)
PASS  the shell reports the ADOPTED daemon
PASS  it rendered the harness daemon's pid
PASS  the auth token is absent from the entire rendered DOM
PASS  no 64-hex string at all appears in the DOM
PASS  the adopted daemon is still alive
PASS  its portfile is intact
```

`cargo test --test supervision` covers what no screenshot can:

| Test | Asserts |
|---|---|
| `a_spawned_daemon_is_owned_and_its_portfile_agrees_with_its_announcement` | ownership, schema/protocol, a 64-hex token, and that the displayable `ws://` URL never embeds it |
| `an_adopted_daemon_outlives_the_shell_that_adopted_it` | the guest adopts the incumbent, its shutdown is a no-op, the daemon and portfile survive — and the real owner's shutdown does end it |
| `the_portfile_is_not_readable_by_other_users` | mode `0600`, because the portfile carries the token |
| `killing_the_shell_takes_the_owned_daemon_with_it` | dropping the shell's end of stdin ends an owned daemon |

### Deliberate regressions

Assertions that cannot fail are decoration. Both of these were run:

- **adopted shutdown kills the daemon** (treat a guest like an owner) →
  `an_adopted_daemon_outlives_the_shell_that_adopted_it` **FAILED**, and only
  that test. The other three stayed green.
- **the token is rendered into the DOM** (appended to the displayed endpoint as
  `?token=…`) → both token assertions in `evidence.sh` **FAILED**, and only
  those. Rendering and lifecycle assertions stayed green.

## Four things that would have made this spike lie

Each of these produced a green-looking result that meant nothing.

**1. `kill_on_drop(true)` masked the parent-death mechanism.** With it set,
`killing_the_shell_takes_the_owned_daemon_with_it` passed because `Drop` sent
SIGKILL — not because closing stdin worked. Worse, it does not help in the case
it appears to cover: a SIGKILLed or hard-panicking shell runs **no destructors**,
so `kill_on_drop` never fires. The only thing standing between a dead shell and
an orphaned daemon is the OS closing its fds, which EOFs the child's stdin.
`kill_on_drop` is now explicitly `false` so the tests measure the real guarantee.

**2. Spawning with `Stdio::null()` is instant death.** `--supervised` installs
its stdin watcher *after* printing `ready`, so a supervisor reads a perfectly
valid ready line and then connects to a corpse ~70 ms later. The `ChildStdin`
must be held for the app's whole life. `Stdio::inherit()` from a `.desktop`
launcher usually has the same effect.

**3. `tokio::process::Child::wait()` drops stdin before waiting**
(`tokio-1.53.1/src/process/mod.rs:1379-1382`). Calling it to poll liveness on an
owned `--supervised` daemon therefore *kills* the daemon. Use `try_wait()`.

**4. The `ready` line's first key is `data_dir`, not `event`.** `serde_json`'s
`json!` serializes through a `BTreeMap`, so keys come out alphabetically. Every
docstring and example shows `{"event":"ready",…}`; a supervisor that prefix-matches
that string hangs until its own timeout. Parse the object.

## Negative results

These are the caveats on criterion 5, and they are upstream gaps, not omissions.

**Navigation is closed; downloads and new-window are not.**
`Config::with_navigation_handler(|_| false)` closes navigation — and Dioxus's
default **fails open**, permitting navigation to any origin, so this must be set
explicitly. But `dioxus-desktop` 0.7.9 exposes no hook for wry's download or
new-window policy, so `docs/dioxus-architecture.md` Decision 7's requirement
that those fail closed **cannot be satisfied in this version**. #187 should know
before it commits to a Linux WebView policy.

**The WebView inspector can only be disabled in release, and not by excluding
the dependency.** The obvious approach —
`dioxus-desktop = { default-features = false }` — does not compile in release:

```
error[E0599]: no method named `open_devtools` found for reference `&WebView`
  --> dioxus-desktop-0.7.9/src/app.rs:145
error[E0599]: no method named `close_devtools` found for reference `&WebView`
  --> dioxus-desktop-0.7.9/src/app.rs:147
```

`dioxus-desktop` calls both unconditionally, while wry only exposes them under
`debug_assertions` **or** its own `devtools` feature. It builds in debug, which
is how the trap hides. So the feature stays on and the inspector is off in
release only, by wry's `devtools: false` under `not(debug_assertions)`. **A debug
build of any Dioxus desktop app has a live inspector.**

**The shipped binary links OpenSSL.** `dioxus-desktop` declares `tungstenite`
with `features = ["native-tls"]` **non-optionally** for every non-Android target
(its `Cargo.toml:249-251`); Android gets `rustls`. No feature selection avoids
it. Confirmed on the release bundle:

```
libssl.so.3 => /lib/aarch64-linux-gnu/libssl.so.3
libcrypto.so.3 => /lib/aarch64-linux-gnu/libcrypto.so.3
```

So a packaged desktop artifact carries OpenSSL's CVE surface, for a TLS stack
this shell never uses — it dials loopback only. That is an M4 packaging and
supply-chain fact (#193/#194, #198). Measuring this needs care: a binary whose
`main` does not actually reference Dioxus links none of it, and `ldd` looks
clean.

**The WebView renders nothing on this host without an env var.** webkit2gtk's
DMABUF renderer fails against this driver by rendering a healthy **blank page** —
window opens, process fine, no content. `main` sets
`WEBKIT_DISABLE_DMABUF_RENDERER=1` before `launch`. Dioxus sets it only when
`XDG_SESSION_TYPE == "wayland"`, while forcing `GDK_BACKEND=x11`
unconditionally — so an X11 session gets the broken path. This is why criterion
3 is measured geometry rather than "the window appeared".

## Prerequisites

Every one of these was installed on the build host on 2026-07-28; **none was
present beforehand**, so this build reproduces nowhere without them:

```sh
sudo apt-get install -y --no-install-recommends \
  libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libsoup-3.0-dev \
  libgtk-3-dev libxdo-dev librsvg2-dev libssl-dev
```

Versions used: webkit2gtk/javascriptcoregtk 2.52.3, soup 3.4.4, gtk 3.24.41,
xdo 1:3.20160805.1, openssl 3.0.13. The repository's existing Linux CI job
installs `libgtk-3-dev` but **not** webkit2gtk, so CI could not build this today.

## Running it

This crate is excluded from the root workspace, so the working directory
matters — the daemon build is root-scoped and everything else is not:

```sh
# from the repository root
cargo build -p jeliyad

cd spikes/dioxus-desktop
cargo test --test supervision   # headless ownership + teardown proof
./check-native-graph.sh         # no TLS backend under the transport
./evidence.sh                   # end-to-end against the built binary
./build.sh release              # the packaged layout
```

`evidence.sh` needs a display; it honours `DISPLAY` and defaults to `:1`.

## What this does not prove

- Anything about macOS or Windows. One host platform, as #159 scopes.
- Any release packaging: no desktop entry, icons, `.deb`, CMake install, or
  signing. That is M4 (#193/#194).
- Protocol generation mismatch handling beyond a hard stop on `daemon.status`.
- That the daemon token is unreachable from a *hostile* page. It is absent from
  the DOM this shell renders; a page that could call `/api/session` is a
  different question, and CSP cannot help — Dioxus's own renderer transport is a
  loopback WebSocket on a runtime-assigned port, so `connect-src ws://127.0.0.1:*`
  is unavoidable and port-blind.
- Any resource, startup, or memory budget. Those are #198.
