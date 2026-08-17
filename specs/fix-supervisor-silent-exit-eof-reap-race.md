# Spec — De-flake `a_silent_nonzero_exit_surfaces_wedged_not_handshake`: bound the exit-status wait so a silent nonzero exit is always `Wedged` (#277)

Status: implemented · PR: #277 · Owner: jeliya-supervisor (#170) · Type: bug fix (test flakiness → real ordering bug in the no-announcement classifier)

## 1. Outcome

`crates/jeliya-supervisor/tests/eviction_refuse.rs::a_silent_nonzero_exit_surfaces_wedged_not_handshake`
intermittently fails the required `Rust + Dart + smoke + E2E + protocol conformance`
CI job on **unrelated** PRs (observed on the #274 run, job `94417284713`, commit
`f55cfcb`):

```
a silent non-zero exit must be Wedged (retryable), not Handshake;
got: Err(Handshake("stdout closed before the announcement"))
```

The test passes on most runs, so it is a timing race, not a deterministic
regression. This spec pins the **root cause** (a single nonblocking `try_wait()`
that races the child's reap on the stdout-EOF path), specifies a **bounded
exit-status wait** that removes the race, and replaces the flaky timing assertion
with a **deterministic red-before-green regression test** while keeping the
production-shape test that named the bug.

This is #170 supervisor code (`src/supervisor.rs`), unrelated to #177/#274.

## 2. Scope

**In scope**
- The `Err(AnnouncementError { .. })` arm of the `read_announcement` match in
  `Supervisor::start_or_adopt` (`crates/jeliya-supervisor/src/supervisor.rs`,
  currently lines ~354–427) — the "no announcement, classify by exit status"
  path.
- A new deterministic integration test (and stub) in
  `crates/jeliya-supervisor/tests/eviction_refuse.rs` that forces the racing
  ordering.

**Out of scope**
- `read_announcement` itself and the `AnnouncementError`/`stdout_closed` shape
  (unchanged — the flag is already correct; the bug is in how the caller waits
  for the exit status).
- The error taxonomy, the adopt/eviction/health paths, portfile handling, the
  Windows-deferred eviction lever (OQ-5), and every other test.
- Any behavior change for the non-EOF handshake faults (malformed JSON, read
  error, announcement-budget timeout) — those are preserved bit-for-bit.

## 3. Evidence and current behavior

### 3.1 The stub and the intended contract

The failing test drives the public API with a stub daemon that prints nothing and
exits non-zero:

```sh
#!/bin/sh
exit 1
```

Per the source spec (`specs/rust-desktop-jeliyad-supervisor.md`):

- §6.4: an `already_running` child "has already exited 0; drop our stdin, **await
  the child's exit** (non-zero → `Wedged`) …".
- §6.5: "**Exit 1 with no JSON line** (lock held, no healthy daemon, no progress
  in the daemon's ~15 s window) → `Wedged`; the caller retries briefly."

So the intended result for a silent nonzero exit is unambiguously `Wedged`
(retryable). The spec even says **await** the exit — a bounded wait, not a single
poll.

### 3.2 The exact code path (today)

In `read_announcement` (`supervisor.rs`), stdout EOF with no JSON line returns the
EOF-specific error and sets the flag:

```rust
Ok(None) => return Err(AnnouncementError::stdout_closed()), // stdout_closed: true,
                                                            // error = Handshake(
                                                            //   "stdout closed before the announcement")
```

Back in `start_or_adopt`, the caller drops stdin, captures the leader pgid, then
does a **single nonblocking** `try_wait()` (abridged):

```rust
let leader_pgid = guard.as_mut().id();
match guard.as_mut().try_wait() {
    Ok(Some(status)) if !status.success() => {
        if let Some(pgid) = leader_pgid { process::kill_reaped_process_group(pgid).await?; }
        if stdout_closed { return Err(SupervisorError::Wedged); } // intended path
        return Err(error);
    }
    Ok(Some(_)) => { /* zero exit */ ... return Err(error); }
    // Still running (Ok(None)) OR an errored wait:
    _ => return Err(abandon_child(guard.as_mut(), error).await), // <-- surfaces `error`
}
```

### 3.3 Root cause — the reap race

The stub's stdout closes **because the process exited** (the kernel closes fd 1 on
exit). Two things become observable at ~the same instant:

1. The async reader (`tokio::io::Lines`) sees EOF via pipe readiness and returns
   `Ok(None)` → `read_announcement` returns `stdout_closed`.
2. `waitpid(WNOHANG)` (behind `try_wait()`) can observe the reapable zombie.

These are ordered by the scheduler, not by us. On the common path, by the time the
caller reaches `try_wait()` the zombie is visible → `Ok(Some(exit 1))` →
`Wedged`. **Occasionally** the async reader delivers EOF a hair before the zombie
is visible, so `try_wait()` returns `Ok(None)` ("still running"). That falls into
the `_ =>` arm → `abandon_child(...)`, which force-kills the (already-dead) group
and **returns `error` = `Handshake("stdout closed before the announcement")`** —
the exact failure in the CI log.

`try_wait()` samples **once**; it does not await the reap. That single sample is
the race.

### 3.4 Blast radius beyond the named test

`a_reaped_leader_that_spawned_a_descendant_reaps_the_group` (same file) exercises
the identical `stdout_closed → nonzero exit → sweep group → Wedged` path with a
forking stub. It shares the same latent race and can flake the same way; the fix
hardens it too (no change to that test).

## 4. Fix — bound the exit-status wait on the EOF path

On `stdout_closed == true`, **await** the child's exit status bounded by
`teardown`, instead of sampling `try_wait()` once. Rationale that makes this
safe and near-instant:

- EOF means every write end of the stdout pipe is closed. For a single-process
  daemon the leader closed fd 1 by exiting; a descendant that still held an
  inherited fd 1 would keep the pipe **open** (no EOF). So `stdout_closed == true`
  ⟹ the leader has exited or is exiting **and** no live descendant holds stdout.
- `tokio::process::Child::wait()` is woken by the runtime's child reaper as soon
  as the zombie is reapable — it does **not** burn the budget; the timeout is only
  a ceiling for a pathological "closed fd 1 but kept running" binary.

The non-EOF faults keep the existing single nonblocking `try_wait()` (a specific
handshake fault must not be granted more child lifetime, and must never be masked
as `Wedged`).

### 4.1 Recommended replacement for the `Err(AnnouncementError { .. })` arm

```rust
Err(AnnouncementError { error, stdout_closed }) => {
    // No announcement. Drop our stdin first so a well-behaved daemon can exit on
    // EOF, then classify by the child's exit status.
    drop(stdin);
    // Capture the pgid (== leader pid) BEFORE any reaping wait: once the leader is
    // reaped, `child.id()` is None and the isolated group can no longer be reached
    // by pid to sweep a leaked descendant.
    let leader_pgid = guard.as_mut().id();

    if stdout_closed {
        // EOF: stdout closed with no line. For a single-process daemon the leader
        // has exited (or is exiting) and no descendant still holds fd 1 (an
        // inherited-and-open fd 1 would keep the pipe from reporting EOF), so the
        // exit status is imminent — AWAIT it, BOUNDED by `teardown`, instead of
        // sampling `try_wait` once. The single nonblocking poll RACES the reap:
        // the async reader can deliver EOF a hair before `waitpid(WNOHANG)` sees
        // the zombie, which mis-routed a silent nonzero exit into `abandon_child`
        // and surfaced `Handshake("stdout closed before the announcement")`
        // instead of the retryable `Wedged` (#277). `timeout_at` + `deadline_from`
        // SATURATES so a `Duration::MAX` teardown cannot overflow-panic the timer.
        match tokio::time::timeout_at(
            validate::deadline_from(self.timeouts.teardown),
            guard.as_mut().wait(),
        )
        .await
        {
            Ok(Ok(status)) => {
                // The reaped-leader arm must sweep the isolated group so a
                // descendant (fault #5 / the forking-stub test) cannot linger on
                // the data-dir lock; a verified sweep failure is propagated, never
                // masked by `Wedged`/the handshake error.
                if let Some(pgid) = leader_pgid {
                    process::kill_reaped_process_group(pgid).await?;
                }
                // A silent nonzero exit is the retryable held-lock / startup-
                // failure `Wedged` (spec §6.5); a clean (zero) exit keeps the
                // original handshake error.
                if !status.success() {
                    return Err(SupervisorError::Wedged);
                }
                return Err(error);
            }
            // The child closed stdout but did not exit within `teardown`
            // (overridden/pathological), or the wait itself errored: force-kill
            // the group and surface the handshake fault — NOT the simple retryable
            // held-lock case, so it is not masked as `Wedged`.
            _ => return Err(abandon_child(guard.as_mut(), error).await),
        }
    }

    // A NON-EOF fault (malformed JSON, a read error, or the announcement budget
    // elapsing) is a SPECIFIC handshake fault: the child may still be running and
    // must not be granted more time, so a single NONBLOCKING `try_wait` reaps a
    // child that already exited without extending a hung one. The error is
    // preserved regardless of the exit status — a persistent packaging/protocol
    // bug must NOT be masked as retryable `Wedged`.
    match guard.as_mut().try_wait() {
        Ok(Some(_)) => {
            if let Some(pgid) = leader_pgid {
                process::kill_reaped_process_group(pgid).await?;
            }
            return Err(error);
        }
        _ => return Err(abandon_child(guard.as_mut(), error).await),
    }
}
```

### 4.2 Behavior parity table (proves the only change is the race fix)

| Situation | Today | After fix |
|---|---|---|
| EOF + nonzero exit (zombie visible) | sweep + `Wedged` | sweep + `Wedged` |
| EOF + nonzero exit (**reap lags EOF**) | `Ok(None)` → abandon → **`Handshake`** (the bug) | bounded `wait` catches exit → sweep + **`Wedged`** |
| EOF + zero exit | sweep + `error` | sweep + `error` |
| EOF + genuinely still alive past budget | abandon + `error` | abandon + `error` (bounded by `teardown`) |
| Non-EOF (malformed/read err/timeout), any exit | reap + `error` | reap + `error` |
| Non-EOF, still running | abandon + `error` | abandon + `error` |

The group sweep (`kill_reaped_process_group`) and the `?`-propagated sweep failure
are preserved on every reaped arm.

### 4.3 Why `teardown` (and not `spawn`) for the budget

- `teardown` is semantically "await a child's exit"; the default is
  `DEFAULT_TEARDOWN` = 15 s and the tests use 200 ms — both are orders of
  magnitude larger than a reap that is essentially already done, so the ceiling
  is never the thing that resolves the wait.
- Using `spawn` would revive the "no 2× spawn budget" concern documented in the
  current comment (§6.5). That concern is about a **hung** child that emitted no
  line and ignores stdin-EOF — precisely the **non-EOF timeout** case, not this
  EOF case. Still, `teardown` keeps the "no second full spawn budget" property
  literally true, so it is the conservative choice.
- The `AlreadyRunning`-then-hangs path uses `spawn` for its bounded wait; that is
  a different path (a live-looking incumbent, not a silent exit) and is left
  unchanged. See Open Question OQ-1 if the maintainer prefers one budget knob for
  both.
- `Duration::MAX` safety: `timeout_at(validate::deadline_from(..), ..)` uses the
  crate's saturating deadline, so an absurd `teardown` cannot overflow-panic the
  timer (matching every other timed wait in the crate).

## 5. Test strategy

### 5.1 New deterministic regression test (red-before-green)

The current flaky test cannot *deterministically* prove the fix because it wins
the race most of the time. Add a stub that **forces** the racing ordering: close
stdout first, then stay alive briefly, then exit non-zero.

```rust
/// A child that CLOSES stdout and only THEN exits non-zero after a short delay
/// forces the EOF-before-reap ordering deterministically (unlike the timing race
/// that intermittently surfaced it, #277): the read loop observes stdout EOF while
/// the child is still alive. The pre-fix single nonblocking `try_wait` observes
/// `Ok(None)` and abandons → `Handshake`; the bounded exit-status wait observes the
/// eventual nonzero exit → `Wedged`.
fn write_close_stdout_then_delayed_exit_stub(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // `exec 1>&-` closes fd 1 so the supervisor's read sees EOF at once; the sleep
    // keeps the process ALIVE across that EOF (so a single poll loses) yet stays
    // well within the teardown budget the bounded wait uses. Fractional `sleep`
    // is already used by this file's forking stub (`sleep 0.02`), so it is a
    // proven-portable pattern on the CI Unix targets.
    std::fs::write(path, "#!/bin/sh\nexec 1>&-\nsleep 0.5\nexit 1\n")
        .expect("write delayed-exit stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

#[tokio::test]
async fn stdout_eof_before_a_nonzero_exit_still_surfaces_wedged() {
    // ... root/data temp setup identical to the sibling tests ...
    let stub = root.join("jeliyad-eof-then-exit");
    write_close_stdout_then_delayed_exit_stub(&stub);

    let config = SupervisorConfig {
        data_dir: Some(data),
        binary: Some(stub),
        // teardown MUST exceed the stub's post-EOF sleep so the bounded exit wait
        // catches the exit; spawn/health stay short so a lost race fails fast.
        timeouts: Timeouts { teardown: Duration::from_secs(3), ..short_timeouts() },
        ..SupervisorConfig::new(Generation::new(2, 2))
    };
    let sup = Supervisor::resolve(config).expect("resolve");
    let result = sup.start_or_adopt().await;
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        matches!(result, Err(SupervisorError::Wedged)),
        "stdout EOF observed before the reap of a nonzero exit must still be Wedged; got: {result:?}"
    );
}
```

Timing margins (both directions robust on loaded CI):
- **Red (pre-fix):** at EOF (~T0) the child is sleeping for 0.5 s ⟫ the sub-ms
  gap to `try_wait()`, so `Ok(None)` → `abandon_child` → `Handshake` is
  deterministic. (abandon force-kills the sleeping child — expected.)
- **Green (post-fix):** the bounded wait resolves at the exit (~T0 + 0.5 s), well
  under the 3 s `teardown` ceiling → `Wedged`.

Verify red-before-green by staging the test **before** the code change and running
it: it must fail with the `Handshake` message; after the §4 change it must pass.

### 5.2 Keep the existing tests

- Keep `a_silent_nonzero_exit_surfaces_wedged_not_handshake` (production-shape
  stub `exit 1`); it must now pass reliably. Optionally tighten its doc comment to
  note the deterministic sibling.
- `a_reaped_leader_that_spawned_a_descendant_reaps_the_group`,
  `a_hanging_already_running_child_times_out_as_wedged`,
  `a_malformed_announcement_does_not_leak_its_contents`, and the
  `stale_incompatible` / `fault14` refusal tests must stay green unchanged (§4.2
  shows their paths are untouched).

### 5.3 Anti-flake stress check (not committed)

Run the two silent-exit tests in a tight loop and require zero `Handshake`:

```
for i in $(seq 1 300); do \
  cargo test -p jeliya-supervisor --test eviction_refuse \
    stdout_eof_before_a_nonzero_exit_still_surfaces_wedged \
    a_silent_nonzero_exit_surfaces_wedged_not_handshake -- --exact --test-threads=1 \
    || { echo "FAIL on iter $i"; break; }; \
done
```

## 6. Acceptance criteria

1. On `stdout_closed == true`, `start_or_adopt` **awaits** the child's exit status
   bounded by `teardown` (via `timeout_at(deadline_from(teardown), wait())`); a
   silent **nonzero** exit yields `SupervisorError::Wedged` regardless of whether
   the async reader observed EOF before or after the reap.
2. The new `stdout_eof_before_a_nonzero_exit_still_surfaces_wedged` test fails with
   `Handshake("stdout closed before the announcement")` **before** the code change
   and passes with `Wedged` **after** it (documented red-before-green).
3. `a_silent_nonzero_exit_surfaces_wedged_not_handshake` and every other test in
   `crates/jeliya-supervisor` pass; `cargo test -p jeliya-supervisor` is green,
   including the 300× stress loop with zero `Handshake`.
4. Non-EOF handshake faults (malformed JSON, read error, announcement timeout) are
   unchanged: the redacted/specific `Handshake` is preserved and never mapped to
   `Wedged` (guarded by `a_malformed_announcement_does_not_leak_its_contents`).
5. The group sweep on the reaped-leader path still runs and still propagates a
   verified sweep failure (guarded by the forking-stub descendant test).
6. `cargo clippy -p jeliya-supervisor` is clean; `cargo fmt` applied;
   `#![forbid(unsafe_code)]`/workspace `unsafe_code` policy untouched; the test
   file stays `#![cfg(unix)]`; the `jeliya-ui` absence boundary test is unaffected.

## 7. Risks and mitigations

- **Fractional `sleep` portability.** Mitigated: the same file already uses
  `sleep 0.02`, so fractional `sleep` is a proven pattern on the CI Unix targets
  (Linux GNU + macOS BSD). If ever a concern, switch to integer `sleep 1` and
  `teardown: 4s`.
- **A pathological "closed stdout but still running" binary** now costs up to
  `teardown` before `abandon_child`. This case already went to abandon; the delta
  is bounded and only occurs for a misbehaving/overridden binary — acceptable, and
  strictly better than the wrong `Wedged`/`Handshake` non-determinism.
- **A near-zero `teardown` config** could, in theory, poll before the reap lands.
  Negligible: `wait()` is woken by the reaper (it does not sample once), the
  default is 15 s and tests use ≥200 ms, and even at `teardown == 0` the outcome is
  no worse than today's single `try_wait()`. Not floored to avoid a magic constant;
  see OQ-2.
- **CI green-margin under heavy load.** `teardown: 3s` vs a 0.5 s post-EOF sleep is
  a 6× margin; raise both together if a slow runner is ever observed.

## 8. Open questions

- **OQ-1 (budget knob).** Use `teardown` here (recommended) or reuse `spawn` for
  symmetry with the `AlreadyRunning`-hang path? Recommendation: `teardown` — it
  matches the spec wording ("await the child's exit") and preserves the "no second
  full spawn budget" property.
- **OQ-2 (teardown floor).** Should the EOF wait floor `teardown` at, say, 50 ms as
  belt-and-suspenders? Recommendation: no — awaited `wait()` makes it unnecessary
  and a floored constant invites review churn.
- **OQ-3 (test placement).** The deterministic test needs a real child (tokio
  `Child` is not mockable), so it lives in `tests/eviction_refuse.rs` beside the
  flaky one rather than as a `supervisor.rs` unit test. Confirm this is acceptable.
- **OQ-4 (Windows).** The whole arm is Unix-gated (`kill_reaped_process_group`, the
  `#!/bin/sh` stubs). Windows silent-exit classification remains deferred with the
  rest of Windows eviction (OQ-5 in the #170 spec). No change here.

## 9. Implementation steps (ordered)

1. **Red first.** Add `write_close_stdout_then_delayed_exit_stub` and
   `stdout_eof_before_a_nonzero_exit_still_surfaces_wedged` to
   `crates/jeliya-supervisor/tests/eviction_refuse.rs`. Run it; confirm it fails
   with `Handshake("stdout closed before the announcement")`.
2. **Fix.** Replace the `Err(AnnouncementError { .. })` arm in
   `Supervisor::start_or_adopt` (`src/supervisor.rs`) with the §4.1 structure:
   branch on `stdout_closed`; on EOF do `timeout_at(deadline_from(teardown),
   wait())` and classify (nonzero → sweep + `Wedged`, zero → sweep + `error`,
   timeout/err → `abandon_child`); keep the nonblocking `try_wait()` for the
   non-EOF branch. Remove the now-dead single-`try_wait` block.
3. **Green.** Re-run the new test (→ `Wedged`) and the flaky test in a loop
   (§5.3); confirm zero `Handshake`.
4. **Regression sweep.** `cargo test -p jeliya-supervisor`, `cargo clippy -p
   jeliya-supervisor -- -D warnings`, `cargo fmt --check`. Confirm the
   `boundaries.rs` and forking-descendant tests still pass.
5. **Docs (light).** No behavior contract change (§6.4/§6.5 already specify
   "await … non-zero → Wedged"). Optionally add a one-line note in the crate
   `README.md`/spec §6.5 clarifying that the exit-status wait is **bounded**, not a
   single poll, to prevent the regression from re-appearing.
