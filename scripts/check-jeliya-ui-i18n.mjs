#!/usr/bin/env node
// Dioxus (jeliya-ui) EN/FR catalog gate entrypoint (#177, spec §6.1).
//
// One implementation (scripts/lib/jeliya-ui-catalog.mjs) exposed as three
// independently-required CI contexts via `--only=<group>`:
//
//   node scripts/check-jeliya-ui-i18n.mjs --only=catalog      # ui-catalog
//   node scripts/check-jeliya-ui-i18n.mjs --only=literals     # ui-literal-copy
//   node scripts/check-jeliya-ui-i18n.mjs --only=typography   # ui-french-typography
//   node scripts/check-jeliya-ui-i18n.mjs                     # all three (local)
//
// rustc already enforces key/placeholder parity between l10n::En and l10n::Fr;
// this gate is defence in depth for what types cannot see. Exit 1 on findings.

import { checkJeliyaUiI18n } from './lib/jeliya-ui-catalog.mjs';

const SELECTOR_GROUPS = ['catalog', 'typography', 'literals'];

function parseOnly(argv) {
  const requested = [];
  for (const arg of argv) {
    const match = /^--only=(.+)$/.exec(arg);
    if (match) requested.push(...match[1].split(',').map((s) => s.trim()).filter(Boolean));
  }
  if (requested.length === 0) return SELECTOR_GROUPS;
  const unknown = requested.filter((g) => !SELECTOR_GROUPS.includes(g));
  if (unknown.length > 0) {
    console.error(`check-jeliya-ui-i18n: unknown --only group(s): ${unknown.join(', ')}`);
    console.error(`valid groups: ${SELECTOR_GROUPS.join(', ')}`);
    process.exit(2);
  }
  return requested;
}

function main() {
  const only = parseOnly(process.argv.slice(2));
  const findings = checkJeliyaUiI18n({ only });
  if (findings.length > 0) {
    console.error(`check-jeliya-ui-i18n [${only.join(',')}]: ${findings.length} finding(s)\n`);
    for (const f of findings) console.error(`  ${f.file}:${f.line}  [${f.code}] ${f.message}`);
    console.error('\nCopy belongs in crates/jeliya-ui/src/l10n/en.rs (source of truth) and');
    console.error('fr.rs (compile-enforced parity); components read it through use_strings().');
    console.error('French typography is docs/glossary-fr.md decision 7 — U+202F before ; ! ? %,');
    console.error('U+00A0 before :, U+2019, U+2026, guillemets. A French value that is right in');
    console.error('English anyway goes in IDENTICAL_ALLOWLIST with the reason. One-off');
    console.error('exceptions: `// i18n-exempt: <reason>` on the line or the line above.');
    process.exitCode = 1;
    return;
  }
  console.log(`check-jeliya-ui-i18n [${only.join(',')}]: OK`);
}

main();
