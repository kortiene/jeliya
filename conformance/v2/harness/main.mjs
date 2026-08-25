#!/usr/bin/env node
// Replay the protocol-v2 conformance corpus against a live `jeliyad`.
//
// Usage:
//   node conformance/v2/harness/main.mjs <path-to-jeliyad> [options]
//
// Options:
//   --filter <substr>     run only cases whose name contains the substring
//   --file <name>         run only one corpus file (e.g. rooms, handshake)
//   --case <name>         run only the named case (repeatable)
//   --verbose             per-step logging
//   --jobs <n>            cases to run concurrently (default 1; daemons use
//                         OS-chosen ports, so parallelism is safe but noisy)
//
// Exit status: 0 when every non-blocked case passed and every blocked case
// failed as expected; 1 otherwise; 2 for a selector usage error. Every
// selector error is reported BEFORE any daemon starts — a typo must not cost
// a process, and must never silently select nothing and exit 0.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { Outcome, Runner } from './runner.mjs';
import { caseNeedsRealNetwork } from './topology.mjs';
import { runRealNetworkCanary } from './canary.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const CORPUS_DIR = join(HERE, '..');
const FILES = [
  'handshake.json',
  'subject-daemon.json',
  'rooms.json',
  'invites.json',
  'timeline-streams.json',
  'files.json',
  'pipes.json',
];

export function parseArgs(argv) {
  const args = { binary: null, filter: null, file: null, caseNames: [], verbose: false, jobs: 1 };
  const rest = [...argv];
  while (rest.length) {
    const a = rest.shift();
    if (a === '--filter') args.filter = rest.shift();
    else if (a === '--file') args.file = rest.shift();
    else if (a === '--case') args.caseNames.push(rest.shift());
    else if (a === '--verbose') args.verbose = true;
    else if (a === '--jobs') args.jobs = Number(rest.shift()) || 1;
    else if (!args.binary) args.binary = a;
  }
  return args;
}

export function loadCases(fileFilter) {
  const cases = [];
  for (const f of FILES) {
    if (fileFilter && !f.startsWith(fileFilter)) continue;
    const raw = JSON.parse(readFileSync(join(CORPUS_DIR, f), 'utf8'));
    const list = raw.cases || raw;
    for (const c of list) cases.push({ ...c, _file: f });
  }
  return cases;
}

/**
 * Apply the selector options to the loaded corpus, enforcing selector
 * hygiene BEFORE the caller constructs a Runner (and therefore before any
 * daemon spawns). Returns the selected case list, or a usage error
 * `{ code, message }` the caller must report and exit with.
 */
export function selectCases(cases, args) {
  if (args.caseNames.length > 0 && args.filter !== null) {
    return {
      error: {
        code: 2,
        message:
          '--case and --filter are mutually exclusive: --case names exact cases, ' +
          '--filter matches substrings — combine them and neither selector means what it says',
      },
    };
  }
  const duplicates = args.caseNames.filter((n, i) => args.caseNames.indexOf(n) !== i);
  if (duplicates.length > 0) {
    return {
      error: {
        code: 2,
        message: `duplicate --case selector(s): ${[...new Set(duplicates)].join(', ')}`,
      },
    };
  }
  if (args.file !== null && FILES.filter((f) => f.startsWith(args.file)).length === 0) {
    return {
      error: {
        code: 2,
        message: `--file ${args.file} matches no corpus file (available: ${FILES.join(', ')})`,
      },
    };
  }
  if (args.caseNames.length > 0) {
    const wanted = new Set(args.caseNames);
    const selected = cases.filter((c) => wanted.has(c.name));
    const unknown = args.caseNames.filter((n) => !cases.some((c) => c.name === n));
    if (unknown.length > 0) {
      return {
        error: {
          code: 2,
          message:
            `--case names no corpus case: ${unknown.join(', ')} ` +
            `(${cases.length} cases in scope — a typo must fail, not silently select nothing)`,
        },
      };
    }
    return { cases: selected };
  }
  if (args.filter !== null) {
    const selected = cases.filter((c) => c.name.includes(args.filter));
    if (selected.length === 0) {
      return {
        error: {
          code: 2,
          message: `--filter ${args.filter} matches none of the ${cases.length} corpus cases`,
        },
      };
    }
    return { cases: selected };
  }
  return { cases };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.binary) {
    console.error('usage: node main.mjs <path-to-jeliyad> [--filter s] [--file f] [--case n] [--verbose] [--jobs n]');
    process.exit(2);
  }
  // Selector hygiene first: every guard below exits before the Runner (and
  // any daemon) is constructed.
  const selection = selectCases(loadCases(args.file), args);
  if (selection.error) {
    console.error(`selector error: ${selection.error.message}`);
    process.exit(selection.error.code);
  }
  const cases = selection.cases;

  // RealNetwork serialization (175c0b): a case whose daemons must reach each
  // other (`link:up`, or a member join — member:b/member:c/room:left) runs
  // discovery-bound daemons whose convergence the witness polls; running
  // several at once multiplies relay dial load and port churn for zero
  // throughput gain, so jobs>1 over such a selection is an explicit usage
  // error BEFORE any daemon spawns. Loopback-only selections keep
  // parallelism.
  const realNetworkCases = cases.filter((c) => caseNeedsRealNetwork(c.requires));
  if (realNetworkCases.length > 0 && args.jobs > 1) {
    console.error(
      `--jobs ${args.jobs} is not available for RealNetwork cases (link:up / member:b / ` +
        `member:c / room:left stage cross-daemon discovery): ${realNetworkCases.map((c) => c.name).join(', ')} ` +
        `— rerun with --jobs 1`,
    );
    process.exit(2);
  }

  // The run-level RealNetwork canary: when a selected case uses link:up, prove
  // the RealNetwork path itself works BEFORE any case runs — a bounded,
  // disposable, two-daemon round (subjects, room, mint/redeem, joiner
  // timeline, total cleanup). A canary failure is a harness setup error that
  // aborts the run; it is never a fallback to loopback and never a substitute
  // for the per-case link witness.
  if (realNetworkCases.some((c) => (c.requires || []).includes('link:up'))) {
    try {
      const canary = await runRealNetworkCanary(args.binary, {
        onLog: (line) => console.error(line),
      });
      console.error(
        `RealNetwork canary OK (redeem ${canary.redeemMs}ms, joiner timeline ${canary.convergeMs}ms)`,
      );
    } catch (err) {
      console.error(`RealNetwork canary FAILED — the run aborts before any case: ${err.message}`);
      process.exit(1);
    }
  }

  const runner = new Runner(args.binary, { verbose: args.verbose });
  const results = [];
  let failed = 0;
  let idx = 0;

  const runOne = async (c) => {
    const r = await runner.runCase(c);
    const i = ++idx;
    const tag =
      r.outcome === Outcome.PASS ? 'PASS'
      : r.outcome === Outcome.BLOCKED_FAIL ? 'BLOCKED(fail expected)'
      : r.outcome === Outcome.BLOCKED_PASS ? 'BLOCKED(!! PASSED !!)'
      : r.outcome === Outcome.ERROR ? 'ERROR'
      : 'FAIL';
    const line = `[${i}/${cases.length}] ${tag}  ${r.name}`;
    console.log(r.reason && r.outcome !== Outcome.PASS ? `${line}\n      ${r.reason}` : line);
    return r;
  };

  if (args.jobs <= 1) {
    for (const c of cases) results.push(await runOne(c));
  } else {
    const queue = [...cases];
    const workers = Array.from({ length: args.jobs }, async () => {
      while (queue.length) {
        const c = queue.shift();
        if (c) results.push(await runOne(c));
      }
    });
    await Promise.all(workers);
  }

  const summary = { pass: 0, fail: 0, error: 0, blockedFail: 0, blockedPass: 0 };
  for (const r of results) {
    if (r.outcome === Outcome.PASS) summary.pass++;
    else if (r.outcome === Outcome.FAIL) { summary.fail++; failed++; }
    else if (r.outcome === Outcome.ERROR) { summary.error++; failed++; }
    else if (r.outcome === Outcome.BLOCKED_FAIL) summary.blockedFail++;
    else if (r.outcome === Outcome.BLOCKED_PASS) { summary.blockedPass++; failed++; }
  }
  console.log(
    `\n${results.length} cases: ${summary.pass} passed, ${summary.fail} failed, ` +
      `${summary.error} errors, ${summary.blockedFail} blocked(failed as expected), ` +
      `${summary.blockedPass} blocked(passed unexpectedly)`,
  );
  process.exit(failed === 0 ? 0 : 1);
}

// Run only when invoked as a script; the test suite imports the helpers.
const isDirect = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isDirect) {
  main().catch((err) => {
    console.error('harness crashed:', err);
    process.exit(2);
  });
}
