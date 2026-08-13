/** Cross-client identity-palette parity (issue #177, spec §4-D3 / §5.5).
 *
 *  The deterministic identity-palette hash is the one token concept CSS cannot
 *  express, so it is ported to Rust in `crates/jeliya-ui/src/l10n/palette.rs`.
 *  Both ports answer to ONE shared fixture — `assets/identity-palette-fixture.json`
 *  — so an avatar colour cannot drift per device. This is the TS half of that
 *  pin: it asserts the React `colorForId`/`fileTint` (`ui/src/lib/format.ts`)
 *  produce exactly the fixture's colours; `palette.rs`'s Rust test asserts the
 *  same file. Change the fixture and BOTH ports must move in the same commit.
 *
 *  While React exists this mirror is live; when `ui/` retires (#200) the Rust
 *  test remains the sole guardian of the same fixture.
 */

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { colorForId, fileTint } from '../lib/format';

// vitest runs with `ui/` as the working directory; the fixture is at the repo
// root, one level up.
const FIXTURE = JSON.parse(
  readFileSync(resolve(process.cwd(), '../assets/identity-palette-fixture.json'), 'utf8'),
) as { avatars: Record<string, string>; fileTints: Record<string, string> };

describe('identity-palette fixture parity', () => {
  it('colorForId matches the shared fixture for every pinned id', () => {
    const entries = Object.entries(FIXTURE.avatars);
    expect(entries.length).toBeGreaterThan(0);
    for (const [id, expected] of entries) {
      expect(colorForId(id), `avatar colour for id ${JSON.stringify(id)}`).toBe(expected);
    }
  });

  it('fileTint matches the shared fixture for every pinned filename', () => {
    const entries = Object.entries(FIXTURE.fileTints);
    expect(entries.length).toBeGreaterThan(0);
    for (const [name, expected] of entries) {
      expect(fileTint(name), `file tint for ${JSON.stringify(name)}`).toBe(expected);
    }
  });
});
