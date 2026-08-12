# Switchyard (ADW) project pack

`config.json` configures [Switchyard](https://github.com/kortiene/switchyard)
runs that target this repository (`--project-root <this repo>`). Switchyard
deep-merges this file over its built-in defaults, so only deviations are
recorded here; everything omitted (providers, models, prompts, phases) inherits
the orchestrator's defaults. JSON records merge key-by-key; **arrays replace
wholesale** — which is why `gates.e2e.hints` is a complete list, not additions.

What this pack pins:

- **`project`** — identity used in run state and progress comments.
- **`branching.labelPrefixes`** — adds `enhancement → feat` and
  `github_actions → ci` to the default label→branch-prefix map (`bug → fix`,
  `documentation → docs`, … are inherited).
- **`gates.e2e.hints`** — this repo's domain vocabulary (rooms, invites,
  presence, transfers, pipes, byte-stream framing, conformance corpus, QR
  pairing, signing/trust) so protocol-touching changes trigger the e2e phase.
  Matching is whole-word over a lowercased signal, so singular and plural
  forms are listed separately.
- **`commands.defaultTestCommand`** — the fallback test gate when a run passes
  no `--test-cmd`, at CI's pinned toolchain (ci.yml pins Rust 1.96.0; a newer
  local default toolchain's clippy false-fails `-D warnings`). If toolchain
  1.96.0 is not installed the gate fails loudly — install it or pass an
  explicit `--test-cmd`. **Scope: the Rust workspace only.** Runs that touch
  `ui/`, `app/`, or the Node contract scripts must pass an explicit
  per-surface `--test-cmd`; the deliberately narrow default keeps ad-hoc runs
  cheap and deterministic in a cold linked worktree (the UI/Flutter suites are
  slow there and the Flutter widget tests are timing-flaky, which would burn
  the orchestrator's repair loops), while the required CI checks — which
  Switchyard watches and repairs via its ci-fix phase — remain the
  authoritative multi-surface gate before any merge.
- **`commands.defaultFinalizeGates`** — fmt and clippy (CI `rust-runtime`
  parity), then `scripts/check-docs.mjs`, because the orchestrator's document
  phase appends to `docs/` and this repo's docs profile (frontmatter contract,
  index reachability) is CI-enforced.

**Gate strings are argv-split, not shell-evaluated.** Switchyard tokenizes each
gate with a quote-aware splitter and `spawnSync`s it directly — shell operators
like `&&` are passed to the program as literal arguments and fail. Keep every
gate one command per string (the finalize list runs each entry separately). A
`--test-cmd` that genuinely needs a chain must be wrapped:
`--test-cmd 'bash -lc "cmd1 && cmd2"'`.

**Shell env-var prefix syntax (`KEY=val cmd`) is supported.** Switchyard's
`capture()` helper calls `stripEnvPrefix` before `spawnSync`, so leading
`KEY=value` tokens are extracted from the argv, `$VAR` references in their
values are expanded against the orchestrator's environment, and the remainder
is used as the actual command. For example,
`CARGO_TARGET_DIR=$HOME/.cache/foo cargo test` correctly spawns `cargo test`
with `CARGO_TARGET_DIR` set to the expanded path. Shell operators (`&&`, `|`,
`;`) are still not supported; chain commands with
`--test-cmd 'bash -lc "cmd1 && cmd2"'`.

The root `.gitignore` ignores `/agents/`: Switchyard writes per-run state to
`agents/<adw-id>/` inside the (work)tree, and its `--worktree` preflight
hard-fails if that path is not ignored.

Do not put secrets in this file; it is committed, and Switchyard treats a
project pack as executable configuration.
