#!/usr/bin/env node
/**
 * Design-system conformance gate (issue #75).
 *
 * `assets/design-tokens.json` is the shared fixture both clients answer to.
 * This checks the React half — that `ui/src/styles.css` declares the palette
 * the fixture pins, and that the rules DESIGN.md states as absolutes are
 * actually absolute in the stylesheet.
 *
 * It exists because the drift it now blocks was invisible in review: a
 * side-stripe border, a resting shadow and an accent gradient each looked
 * local and reasonable at the call site, while together they contradicted four
 * Named Rules — and the Flutter client had grown matching TOKENS for them, so
 * both clients were confidently wrong in the same direction.
 *
 * The Flutter half is `app/test/design_tokens_test.dart`, reading the same
 * file.
 *
 * Usage: node scripts/check-design-tokens.mjs
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const TOKENS = join(ROOT, 'assets', 'design-tokens.json');
const STYLES = join(ROOT, 'ui', 'src', 'styles.css');

/** Map a fixture colour name to the CSS custom property that carries it. */
const CSS_VAR = {
  ground: '--bg',
  chrome: '--bg-raise',
  card: '--bg-card',
  'card-nested': '--bg-card-2',
  'input-well': '--bg-input',
  'bubble-remote': '--bg-bubble-remote',
  'border-quiet': '--border',
  'border-strong': '--border-strong',
  'border-interactive': '--border-interactive',
  accent: '--accent',
  'accent-deep': '--accent-2',
  ink: '--text',
  'ink-dim': '--text-dim',
  'ink-mute': '--text-mute',
  amber: '--amber',
  red: '--red',
  blue: '--blue',
};

const findings = [];
const add = (rule, detail) => findings.push({ rule, detail });

const tokens = JSON.parse(readFileSync(TOKENS, 'utf8'));
const css = readFileSync(STYLES, 'utf8');

// -- 1. every pinned colour is declared, with the pinned value ----------------

const rootMatch = css.match(/^:root\s*\{([\s\S]*?)^\}/m);
if (!rootMatch) {
  add('root-missing', 'styles.css has no :root block to read tokens from');
} else {
  const root = rootMatch[1];
  const declared = new Map();
  for (const [, name, value] of root.matchAll(/^\s*(--[a-z0-9-]+)\s*:\s*([^;]+);/gim)) {
    declared.set(name, value.trim().toLowerCase());
  }

  for (const [token, value] of Object.entries(tokens.color)) {
    if (token.startsWith('$')) continue;
    const varName = CSS_VAR[token];
    if (!varName) continue; // scrim/shadow are composed, checked below
    if (!declared.has(varName)) {
      add('token-missing', `${varName} (${token}) is not declared in :root`);
    } else if (declared.get(varName) !== value.toLowerCase()) {
      add(
        'token-drift',
        `${varName} is ${declared.get(varName)}, fixture pins ${value} — one of them is wrong, say which in the PR`,
      );
    }
  }

  // A referenced-but-undeclared variable silently disables whatever rule uses
  // it. `--accent-strong` sat dead in a hover state for exactly this reason.
  const used = new Set([...css.matchAll(/var\((--[a-z0-9-]+)/gi)].map((m) => m[1]));
  for (const name of used) {
    if (!declared.has(name) && !name.startsWith('--safe-') && name !== '--vh-full') {
      add('token-undeclared', `var(${name}) is used but never declared — the rule using it does nothing`);
    }
  }
}

// -- 2. no resting shadows outside the documented vocabulary ------------------

const ALLOWED_SHADOWS = [
  tokens.elevation['modal-lift'],
  tokens.elevation['drawer-lift'],
];
for (const [, decl] of css.matchAll(/box-shadow\s*:\s*([^;]+);/gi)) {
  const value = decl.trim();
  if (value === 'none') continue;
  // Status glows are the second sanctioned entry: `0 0 6px <hue>`.
  if (/^0\s+0\s+6px\s/.test(value)) continue;
  if (ALLOWED_SHADOWS.some((s) => value.replace(/\s+/g, ' ') === s.replace(/\s+/g, ' '))) continue;
  add(
    'shadow-vocabulary',
    `box-shadow: ${value} is outside DESIGN.md's shadow vocabulary ` +
      `(${tokens.elevation.allowed.join(', ')}). Flat by doctrine: if a component seems to need a shadow, ` +
      `it is either a modal or it is wrong.`,
  );
}

// -- 3. no side-stripe borders ------------------------------------------------

for (const [, side, width] of css.matchAll(/border-(left|right)\s*:\s*(\d+)px\s+solid\s+(?!transparent)/gi)) {
  if (Number(width) > 1) {
    add(
      'side-stripe',
      `border-${side}: ${width}px is a side stripe — DESIGN.md forbids >1px coloured left/right borders as an accent`,
    );
  }
}

// -- 4. gradients: one sanctioned accent fill, washes stay faint and one-hue --

for (const [, decl] of css.matchAll(/linear-gradient\(([^;]+?)\)\s*(?:,|;)/gi)) {
  const value = decl.trim();
  const isProgressFill = /90deg,\s*var\(--accent-2\),\s*var\(--accent\)/.test(value);
  if (isProgressFill) continue;

  const alphas = [...value.matchAll(/rgba\([^)]*?,\s*([0-9.]+)\s*\)/g)].map((m) => Number(m[1]));
  const overCeiling = alphas.filter((a) => a > tokens.gradient['wash-max-alpha']);
  if (overCeiling.length > 0) {
    add(
      'gradient-strength',
      `linear-gradient(${value}) uses alpha ${overCeiling.join(', ')} — a tint wash caps at ` +
        `${tokens.gradient['wash-max-alpha']}; the progress fill is the only sanctioned accent gradient`,
    );
  }

  // Count distinct hues by their rgb triplet.
  const hues = new Set([...value.matchAll(/rgba\((\d+,\s*\d+,\s*\d+)/g)].map((m) => m[1].replace(/\s+/g, '')));
  if (hues.size > tokens.gradient['wash-max-hues']) {
    add(
      'gradient-hues',
      `linear-gradient(${value}) blends ${hues.size} hues — a wash stays single-hue; ` +
        `multi-hue washes are the crypto/web3 anti-reference PRODUCT.md names`,
    );
  }
}

// -- report -------------------------------------------------------------------

if (findings.length === 0) {
  console.log('design-tokens: OK — React tokens match the shared fixture and the shadow/stripe/gradient rules hold.');
  process.exit(0);
}

console.error(`design-tokens: ${findings.length} finding(s)\n`);
for (const { rule, detail } of findings) {
  console.error(`  [${rule}] ${detail}`);
}
console.error(
  `\nThe shared fixture is assets/design-tokens.json and the normative prose is DESIGN.md.\n` +
    `Where an implementation and the design system disagree, one of them is a bug — say which in the pull request.`,
);
process.exit(1);
