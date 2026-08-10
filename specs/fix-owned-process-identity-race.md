# Fix: owned-process identity reads race a dying process and fail the run

- **Issue:** #206 `[Bug][QA]: Owned-process identity reads race a dying process and fail the run`
- **Priority / labels:** p1 · bug · security · javascript
- **Owning module:** `scripts/e2e-process-ownership.mjs` (Linux `/proc` identity read)
- **Test surface:** `scripts/e2e-port-safety.test.mjs`
- **Consumers affected:** `scripts/package-linux.mjs`, `scripts/agent-e2e.mjs`, `scripts/fleet-e2e.mjs`
- **Platform:** Linux only (the macOS/`ps` branch already returns `null` on exit status 1)
- **Type:** current-stack maintenance; no dependencies; independently fixable

---

## 1. Problem statement

`readProcessIdentity(pid)` builds a stable identity string for a run-owned process so that
teardown can refuse to signal a **recycled** PID. On Linux it does so by reading
`/proc/<pid>/stat` and `/proc/<pid>/cmdline` **non-atomically**
(`scripts/e2e-process-ownership.mjs:4-27`). Between those two reads the process can exit.

A process that exits after the `stat` read but before the `cmdline` read leaves a
still-readable `/proc/<pid>` directory whose **`cmdline` is empty** (a zombie/exiting task has
had its `mm` torn down). The current code then:

1. `command` becomes `""` (lines 16-20), which is falsy;
2. line 21 throws `incomplete proc identity`;
3. that error carries no `.code`, so the `ENOENT`/`ESRCH` guard at line 24 does **not** catch it;
4. line 25 rethrows `could not inspect Linux process <pid>`, and the caller dies.

The zombie guard at line 13 (`if (fieldsFromState[0] === "Z") return null;`) only helps when the
process was **already** `Z` at the instant `stat` was read. A process that reaches `Z` *after*
that read has already passed the guard. The reaped sub-case (cmdline read itself throws
`ENOENT`) already returns `null` via the catch at line 24; **the only broken sub-case is a
successful-but-empty `cmdline` read** on a `/proc` entry that still exists.

### Observed failure (from the issue)

PR #205, run `30322833605`, job `90162074343` (`Linux Flutter app + bundled sidecar`):

```
package-linux: runtime gate sidecar healthy (pid 4867, port 39163)
package-linux: Flutter session rendered and ready (phase noIdentity, authenticated protocol 1)
package-linux: runtime gate failed:
1. could not inspect Linux process 4867
```

The gate called pid 4867 healthy and failed on it **21 ms later** while it was shutting down.
A re-run of the identical commit passed. This is a pure timing race, not a logic error in the
caller.

### Confirmed call sites that hit the racy read

- `scripts/package-linux.mjs:413` — the cleanup/runtime gate calls `readProcessIdentity(candidate.pid)`
  directly; a throw fails the gate (this is the reported crash).
- `scripts/package-linux.mjs:196,214` — `ownedProcessIsAlive` / `signalOwnedProcess` call
  `readIdentity(record.pid)` on a process that teardown is actively killing.
- `scripts/e2e-process-ownership.mjs:62` — `signalOwnedProcessGroup` reads the identity of a
  process that is, by definition, being killed; `agent-e2e.mjs:143` and `fleet-e2e.mjs:153,606`
  reach it during teardown.

---

## 2. Desired outcome

Make a **vanished** process indistinguishable from the `ENOENT` case the reader already handles:
a process that exits between the `stat` and `cmdline` reads must yield `null` (absence), exactly
like the existing `ENOENT`/`ESRCH` contract and the comment at lines 10-12 that tells callers to
probe the process group. Genuine inspection failures (`EACCES`, malformed `stat`, an empty
`cmdline` on a *still-live* process of the *same* identity) must stay loud and distinguishable
from absence. The recycled-PID guard in `signalOwnedProcessGroup` must keep refusing to signal a
process whose identity changed.

---

## 3. Design

### 3.1 Chosen approach — re-read `stat` to confirm the empty `cmdline` means "gone"

When `cmdline` comes back **empty**, re-read `/proc/<pid>/stat` and re-apply the same absence
logic the function already trusts at the top:

- **Re-read `stat` throws `ENOENT`/`ESRCH`** → the process was reaped between reads → `null`
  (this already falls through to the existing catch at line 24; no special-casing needed).
- **Re-read `stat` state is `Z`, `X`, or `x`** → the process became a zombie or reached the
  kernel's dead states after the first read → `null` (same reasoning as the top-of-function dead
  guard, applied to the current instant; `X`/`x` sit on the same exit path as `Z` and were added
  after review — recognizing only `Z` reproduced the original intermittent throw for a task that
  advanced to `X` inside the window).
- **Re-read `stat` start-time differs from the first read** → the PID was **recycled** to a new
  task mid-read. Absence would be unsafe here: `signalOwnedProcessGroup` responds to absence by
  probing and then signalling `-pid`, and nothing prevents the new occupant from being a group
  leader with `pgid == pid` (a fresh leader mid-`exec` even exposes the same empty `cmdline`), so
  an unrelated group could be killed. Instead the reader **surfaces the new occupant's identity**
  (`linux:<bootId>:<newStartTime>:` — the command segment may still be empty mid-`exec`), which
  can never equal a recorded identity (records are only created from live processes with a
  non-empty command), so the caller's recycled-leader guard refuses to signal. Fail closed, not
  silent.
- **Re-read `stat` is still the same live, non-dead process (same start-time) yet `cmdline` is
  still empty** → a genuine inspection failure → **throw** `incomplete proc identity` (stays loud
  via the existing catch rethrow).

This is the issue's "read `stat` and `cmdline` and re-read `stat` to confirm the process neither
exited nor changed identity between them" direction. It is preferred over a bare
`process.kill(pid, 0)` existence probe (the issue's first-listed alternative) because **`kill(pid, 0)`
returns success for a zombie** — a zombie is still a task in the table — so an existence probe
alone cannot catch the exact reported window (non-`Z` at the `stat` read, `Z` at the `cmdline`
read). Re-reading `stat` detects the `Z` transition directly and coherently reuses the line-13
guard's own logic.

Scope discipline: the re-check is added **only** on the successful-but-empty `command` branch. A
`cmdline` read that *throws* is untouched — `ENOENT` still becomes `null`, and `EACCES` (or any
other code) still rethrows loudly through the existing catch. This is what keeps genuine failures
loud while converting only the vanished case to absence.

### 3.2 Testability seam

`linuxProcessIdentity` currently reads the real filesystem, so the exit-during-read window cannot
be reproduced deterministically with a real process (you would have to catch a task in the
sub-millisecond transition between non-`Z` and empty-`cmdline`). Introduce a small **injectable
reader seam** with production defaults, and export `linuxProcessIdentity` so the unit test can
drive it with fakes on any platform:

```js
export function linuxProcessIdentity(
  pid,
  {
    readStat = (p) => readFileSync(`/proc/${p}/stat`, "utf8"),
    readCmdline = (p) => readFileSync(`/proc/${p}/cmdline`),
    readBootId = () => readFileSync("/proc/sys/kernel/random/boot_id", "utf8"),
  } = {},
) { ... }
```

`readProcessIdentity(pid, deps = {})` forwards `deps` to `linuxProcessIdentity(pid, deps)`. The
`deps` parameter is optional and additive: every existing call site
(`readProcessIdentity(pid)` at `package-linux.mjs:413`, and the injected-`readIdentity` callers in
`recordOwnedProcess` / `signalOwnedProcessGroup`) is unaffected. Defaults preserve current
production behavior byte-for-byte, including reading `cmdline` as a `Buffer` before `.toString`.

> Alternative if a new export is undesirable: keep `linuxProcessIdentity` private and reach the
> branch through `readProcessIdentity(pid, deps)` guarded by `skip: process.platform !== "linux"`
> (matching the existing zombie test at `e2e-port-safety.test.mjs:170`). The exported-function
> route is recommended because it makes the new tests deterministic on any runner and needs no
> real processes.

### 3.3 Target implementation shape (illustrative)

```js
function parseProcState(stat) {
  const commandEnd = stat.lastIndexOf(") ");
  if (commandEnd < 0) throw new Error("malformed proc stat");
  return stat.slice(commandEnd + 2).trim().split(/\s+/);
}

export function linuxProcessIdentity(
  pid,
  {
    readStat = (p) => readFileSync(`/proc/${p}/stat`, "utf8"),
    readCmdline = (p) => readFileSync(`/proc/${p}/cmdline`),
    readBootId = () => readFileSync("/proc/sys/kernel/random/boot_id", "utf8"),
  } = {},
) {
  try {
    const fieldsFromState = parseProcState(readStat(pid));
    // A zombie has already exited, so its PID can no longer be recycled until
    // the parent reaps it. Treat the leader as absent and let the caller probe
    // the still-existing process group for any surviving children.
    if (fieldsFromState[0] === "Z") return null;
    const startTime = fieldsFromState[19]; // proc(5) field 22, with state at index 0.
    const bootId = readBootId().trim();
    const command = readCmdline(pid)
      .toString("utf8")
      .split("\0")
      .filter(Boolean)
      .join(" ");
    if (!startTime || !bootId) throw new Error("incomplete proc identity");
    if (!command) {
      // The stat read passed the zombie guard, but the cmdline came back empty.
      // A process we own always has a cmdline while it is alive, so an empty one
      // means the leader vanished in the ~20 ms between the two reads: it was
      // reaped (ENOENT on re-read -> caught below as absence), became a zombie
      // (state Z), or its PID was recycled (start time changed). In each case the
      // recorded leader is gone: report absence and let the caller probe the
      // still-existing process group. A live process that keeps the SAME identity
      // yet exposes no cmdline is a genuine inspection failure and stays loud.
      const recheck = parseProcState(readStat(pid));
      if (recheck[0] === "Z" || recheck[19] !== startTime) return null;
      throw new Error("incomplete proc identity");
    }
    return `linux:${bootId}:${startTime}:${command}`;
  } catch (error) {
    if (error?.code === "ENOENT" || error?.code === "ESRCH") return null;
    throw new Error(`could not inspect Linux process ${pid}`);
  }
}
```

Notes:
- Splitting `if (!startTime || !bootId || !command)` (old line 21) into two checks keeps
  `startTime`/`bootId` anomalies loud (they should never be empty for any task with a readable
  `stat`) while routing only the empty-`command` case through the re-check.
- A malformed re-read `stat` throws `malformed proc stat`, which is not `ENOENT`/`ESRCH`, so it
  rethrows loudly — a genuine failure, as intended.
- The change touches only Linux; the `ps` branch (`e2e-process-ownership.mjs:32-42`) is untouched.

### 3.4 Why this preserves the recycled-PID guard

`signalOwnedProcessGroup` (`e2e-process-ownership.mjs:62-65`) refuses to signal when
`readIdentity(record.pid)` returns a **non-null identity that differs** from the recorded one. A
recycled PID that is fully alive has a **non-empty** `cmdline`, so it returns its own valid,
different identity string through the normal path (no re-check, no `null`) → the guard fires. The
fix converts to `null` only the empty-`cmdline`/vanished case, never a live process's real
(different) identity. This is exactly the "must not turn a *changed* identity into `null`"
constraint from the issue's security note.

---

## 4. Implementation steps

1. **`scripts/e2e-process-ownership.mjs`**
   1. Extract a `parseProcState(stat)` helper (the `lastIndexOf(") ")` + `malformed proc stat`
      parse) so it can be reused for the re-read.
   2. Add the `{ readStat, readCmdline, readBootId }` injectable-reader seam to
      `linuxProcessIdentity`, with production defaults equal to today's `readFileSync` calls, and
      `export` the function.
   3. Split the combined `!startTime || !bootId || !command` throw: validate `startTime`/`bootId`
      first, then handle the empty-`command` branch with a `readStat` re-read that returns `null`
      on `Z` or changed start-time and throws otherwise.
   4. Thread an optional `deps = {}` param through `readProcessIdentity(pid, deps)` to
      `linuxProcessIdentity(pid, deps)`. Leave the `ps` branch and the top-level `pid` validation
      unchanged.
2. **`scripts/e2e-port-safety.test.mjs`** — add the unit tests in §5 (no production behavior
   depends on them; they import the newly exported `linuxProcessIdentity`).
3. **No other files change.** All consumers call `readProcessIdentity(pid)` with a single
   argument and are unaffected by the added optional parameter.

---

## 5. Test plan

### 5.1 New deterministic unit tests (in `scripts/e2e-port-safety.test.mjs`)

Drive `linuxProcessIdentity(pid, fakes)` directly. Build helper stat strings, e.g.
`` `123 (jeliyad) R 1 123 123 0 -1 0 0 0 0 0 0 0 0 0 0 0 0 <T> ...` `` where field index 19 after
the `) ` split is the start-time. A queued `readStat` returns the first-read value, then the
re-read value.

- **AC-1 — exit-during-read, became zombie → `null`, not a throw.** First `readStat` returns a
  live `R` state with start-time `111`; `readCmdline` returns `Buffer.from("")`; re-read `readStat`
  returns a `Z` state with start-time `111`. Assert `=== null`.
- **AC-1 — exit-during-read, reaped → `null`.** First `readStat` live; `readCmdline` empty; re-read
  `readStat` throws an `ENOENT`-coded error. Assert `=== null`.
- **AC-1 — recycled mid-read → `null`.** First `readStat` live start-time `111`; `readCmdline`
  empty; re-read `readStat` live but start-time `222`. Assert `=== null` (the original leader is
  gone; the signal side then safely probes the old group).
- **AC-2 — live process, genuinely empty `cmdline` → throws.** Both `readStat` calls return the
  same live, non-`Z`, same-start-time state; `readCmdline` empty. Assert
  `throws(/could not inspect Linux process/)`.
- **AC-2 — unreadable `cmdline` (`EACCES`) → throws.** `readCmdline` throws an `EACCES`-coded
  error (the re-check branch is never entered because the read *threw*). Assert
  `throws(/could not inspect Linux process/)`, proving genuine failures stay loud.
- **Regression — happy path unchanged.** Live `R` state + non-empty `cmdline` + a `bootId` fake →
  returns `` `linux:<bootId>:<startTime>:<command>` ``.
- **Regression — already-`Z` at first read → `null`.** Confirms the line-13 guard is intact.

### 5.2 Existing tests that must remain green

- `e2e-port-safety.test.mjs:90-122` (identity-match signalling, orphan reap) and `:124-142`
  (non-silent failures) — unchanged; verifies **AC-3** (recycled PID still refused, `/recycled
  process-group leader/`).
- `e2e-port-safety.test.mjs:170-199` (real zombie leader → `readProcessIdentity(...) === null`) —
  unchanged; the first `stat` read is already `Z`, so it hits the line-13 guard.

### 5.3 Manual / live verification (from the issue)

Run each consumer repeatedly against a real daemon and confirm the teardown never emits
`could not inspect Linux process <pid>`:

```
node --test scripts/e2e-port-safety.test.mjs           # new + existing unit tests
node scripts/agent-e2e.mjs                             # x10
node scripts/fleet-e2e.mjs --trials 5                  # x5
xvfb-run -a node scripts/package-linux.mjs             # x10
```

`agent-e2e` / `fleet-e2e` require a built `target/debug/jeliyad`; `package-linux` requires a built
Flutter Linux bundle. If those artifacts are unavailable in this phase, state so and hand the
maintainer the exact commands above; the deterministic unit tests in §5.1 still fully prove the
read-side fix (**AC-1, AC-2, AC-3, AC-4**).

---

## 6. Acceptance criteria → coverage map

| # | Criterion | Covered by |
|---|-----------|------------|
| 1 | Process exiting between `stat` and `cmdline` reads yields `null`, not a throw | §3.1 re-read logic; §5.1 AC-1 tests (zombie / reaped / recycled) |
| 2 | Live process with unreadable or genuinely empty `cmdline` still raises | §3.1 scope discipline; §5.1 AC-2 tests (`EACCES`, same-identity empty) |
| 3 | Recycled PID still refused by `signalOwnedProcessGroup` | §3.4; §5.2 unchanged guard tests |
| 4 | `scripts/e2e-port-safety.test.mjs` covers the exit-during-read window | §5.1 new tests |
| 5 | `agent-e2e`, `fleet-e2e`, `package-linux` teardown pass repeatedly | §5.3 live runs |

---

## 7. Risks and mitigations

- **Masking a real recycle as absence.** Mitigated by only converting the empty-`cmdline`/vanished
  case to `null`; a live recycled PID has a non-empty `cmdline` and returns its own differing
  identity, so `signalOwnedProcessGroup`'s guard still fires (§3.4, AC-3).
- **A second `stat` read is itself racy.** Accepted and bounded: the re-read only *narrows* the
  window and resolves each outcome to a safe verdict (`ENOENT`→`null`, `Z`→`null`, changed
  start-time→`null`, same-live→loud throw). It never signals; the caller still probes the process
  group with signal `0` before killing, so no recycled process is ever signalled.
- **Silently swallowing genuine failures.** Guarded: only a *successful, empty* `cmdline` read on a
  *vanished* process becomes `null`; thrown reads (`EACCES`) and malformed `stat` stay loud
  (AC-2). This directly honors the "do not make a truly uninspectable process silently succeed"
  non-goal.
- **New export widening the module surface.** Low risk; `linuxProcessIdentity` is a script-local
  helper with no external consumers. If undesirable, use the §3.2 skip-guarded alternative.
- **Regression from splitting the `startTime`/`bootId`/`command` check.** Covered by the §5.1
  happy-path and already-`Z` regression tests.

## 8. Non-goals (per issue)

- No retry/sleep-until-settled loop; the fix is a single re-read, not polling.
- No weakening of the recycled-PID guard in `signalOwnedProcessGroup`.
- No making a truly uninspectable (same-identity, empty-`cmdline`) live process silently succeed.
- No change to the macOS/`ps` branch (already returns `null` on exit status 1).

---

## 9. Assumptions and open questions

- **Assumption:** a run-owned daemon (`jeliyad`) always exposes a non-empty `/proc/<pid>/cmdline`
  while alive, so an empty `cmdline` on a non-`Z` process is either the exit race or a genuine
  anomaly worth raising — not a normal state. (True for ordinary user-space processes; kernel
  threads have empty `cmdline`, but this helper is only ever pointed at spawned daemons.)
- **Open question — CI wiring.** `scripts/e2e-port-safety.test.mjs` is **not** currently invoked by
  `.github/workflows/ci.yml` (grep finds no reference), so the new deterministic guard runs only
  locally / on demand. The reported failure surfaced in the `linux-flutter` job, which runs
  `node --test scripts/package-linux.test.mjs` but neither the e2e scripts nor the port-safety
  test. The issue's "clean-slate cutover" note says `agent-e2e`/`fleet-e2e` run in the **required**
  `rust-runtime` job, but the current `ci.yml` gates the v1-coupled live suites off
  (`#202`/`#203`), so that claim appears aspirational relative to `main` today. **Recommended
  follow-up (out of scope for this fix):** add `node --test scripts/e2e-port-safety.test.mjs` to
  the `linux-flutter` step alongside `package-linux.test.mjs` so the regression is guarded
  deterministically in CI. Flag for maintainer decision; not required by the issue's acceptance
  criteria.
- **Open question — start-time width.** The re-read compares `fieldsFromState[19]` (proc(5) field
  22) as a string; this matches the existing extraction exactly, so no normalization is needed. If
  a future change trims/normalizes start-time, keep both reads using the identical parse.
