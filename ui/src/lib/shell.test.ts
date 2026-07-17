import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { COMPACT_MAX, WIDE_MIN, shellFor } from './shell';

const CSS = readFileSync(fileURLToPath(new URL('../styles.css', import.meta.url)), 'utf8');

/** Every `@media` condition in the stylesheet, as `(max-width: 899.98px)`. */
function mediaConditions(): string[] {
  return [...CSS.matchAll(/@media\s+([^{]+)\{/g)].map((m) => m[1].trim());
}

describe('shellFor', () => {
  it('classifies the widths the responsive contract names', () => {
    // docs/room-workbench.md, decision 3 — and the exact matrix issue #62 asks
    // for coverage at.
    expect(shellFor(360)).toBe('compact');
    expect(shellFor(899)).toBe('compact');
    expect(shellFor(900)).toBe('medium');
    expect(shellFor(920)).toBe('medium');
    expect(shellFor(1279)).toBe('medium');
    expect(shellFor(1280)).toBe('wide');
    expect(shellFor(1440)).toBe('wide');
  });

  it('leaves no width unclassified across the boundaries', () => {
    // The fractional band between the compact and medium queries is where a
    // zoomed or scaled viewport lands; it must belong to exactly one shell.
    for (const width of [899.5, 899.98, 899.99, 900, 1279.98, 1279.99]) {
      expect(['compact', 'medium', 'wide']).toContain(shellFor(width));
    }
    expect(shellFor(899.98)).toBe('compact');
    expect(shellFor(899.99)).toBe('medium');
  });
});

describe('the stylesheet agrees with this module', () => {
  // The breakpoints exist twice — here and in styles.css — because a media
  // query cannot read a var(). That duplication gets no compiler help, so it
  // gets this instead.

  it('uses COMPACT_MAX as the only compact boundary', () => {
    const maxWidths = mediaConditions()
      .flatMap((c) => [...c.matchAll(/max-width:\s*([\d.]+)px/g)].map((m) => Number(m[1])))
      // 480 is a component-internal reflow, not a shell boundary.
      .filter((px) => px > 480);
    expect(maxWidths.length).toBeGreaterThan(0);
    for (const px of maxWidths) {
      expect([COMPACT_MAX, WIDE_MIN - 0.02]).toContain(px);
    }
  });

  it('uses WIDE_MIN and the compact boundary as the only min-widths', () => {
    const minWidths = mediaConditions().flatMap((c) =>
      [...c.matchAll(/min-width:\s*([\d.]+)px/g)].map((m) => Number(m[1])),
    );
    expect(minWidths.length).toBeGreaterThan(0);
    for (const px of minWidths) {
      // Medium starts where compact stops, and wide starts at WIDE_MIN.
      expect([Math.ceil(COMPACT_MAX), WIDE_MIN]).toContain(px);
    }
  });

  it('declares the compact shell and the wide inspector column', () => {
    expect(CSS).toContain(`@media (max-width: ${COMPACT_MAX}px)`);
    expect(CSS).toContain(`@media (min-width: ${WIDE_MIN}px)`);
  });

  it('keeps the connection banner out of the overlay stacking order', () => {
    // Decision 3: connection status reserves layout space. A banner that is
    // positioned and z-indexed is one that covers Back and list content.
    const banner = /\.conn-banner\s*\{([^}]*)\}/.exec(CSS);
    expect(banner).not.toBeNull();
    expect(banner?.[1]).not.toMatch(/position:\s*(absolute|fixed)/);
    expect(banner?.[1]).not.toMatch(/z-index/);
  });
});
