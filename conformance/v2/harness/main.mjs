#!/usr/bin/env node
// Replay the protocol-v2 conformance corpus against a live `jeliyad`.
//
// Usage:
//   node conformance/v2/harness/main.mjs <path-to-jeliyad> [options]
//
// Options:
//   --filter <substr>     run only cases whose name contains the substring
//   --file <name>         run only one corpus file (e.g. rooms, handshake)
//   --case <name>         run only the named case
//   --verbose             per-step logging
//   --jobs <n>            cases to run concurrently (default 1; daemons use
//                         OS-chosen ports, so parallelism is safe but noisy)
//
// Exit status: 0 when every non-blocked case passed and every blocked case
// failed as expected; 1 otherwise.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Outcome, Runner } from './runner.mjs';

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

function parseArgs(argv) {
  const args = { binary: null, filter: null, file: null, caseName: null, verbose: false, jobs: 1 };
  const rest = [...argv];
  while (rest.length) {
    const a = rest.shift();
    if (a === '--filter') args.filter = rest.shift();
    else if (a === '--file') args.file = rest.shift();
    else if (a === '--case') args.caseName = rest.shift();
    else if (a === '--verbose') args.verbose = true;
    else if (a === '--jobs') args.jobs = Number(rest.shift()) || 1;
    else if (!args.binary) args.binary = a;
  }
  return args;
}

function loadCases(fileFilter) {
  const cases = [];
  for (const f of FILES) {
    if (fileFilter && !f.startsWith(fileFilter)) continue;
    const raw = JSON.parse(readFileSync(join(CORPUS_DIR, f), 'utf8'));
    const list = raw.cases || raw;
    for (const c of list) cases.push({ ...c, _file: f });
  }
  return cases;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.binary) {
    console.error('usage: node main.mjs <path-to-jeliyad> [--filter s] [--file f] [--case n] [--verbose] [--jobs n]');
    process.exit(2);
  }
  let cases = loadCases(args.file);
  if (args.caseName) cases = cases.filter((c) => c.name === args.caseName);
  else if (args.filter) cases = cases.filter((c) => c.name.includes(args.filter));

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

main().catch((err) => {
  console.error('harness crashed:', err);
  process.exit(2);
});
