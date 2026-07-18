import { describe, expect, it } from 'vitest';
import { encodeQr, QR_MAX_BYTES, _internals, type QrMatrix } from './qr';
import fixtures from './conformance/qr-invite.fixtures.json';

const { ECC_M, GF_EXP, gfMul, maskCondition, charCountBits, buildFunctionMap } = _internals;

/** Render a matrix as the fixture row strings ('#' dark, '.' light). */
function toRows(qr: QrMatrix): string[] {
  return qr.modules.map((row) => row.map((on) => (on ? '#' : '.')).join(''));
}

/** Reverse the encode pipeline and recover the input bytes — the proof that the
 *  symbol is well-formed and actually scannable, independent of the fixtures.
 *  Reads the format info for the mask, un-masks, walks the same zig-zag to pull
 *  the interleaved codewords, de-interleaves, checks every Reed–Solomon block
 *  has a zero syndrome (error-free), then parses the byte-mode segment. */
function decode(qr: QrMatrix): string {
  const size = qr.size;
  const version = (size - 17) / 4;
  const m = qr.modules.map((row) => row.map((on) => (on ? 1 : 0)));

  // format info (top-left copy): bits 0..7 down column 8, bits 14..8 across row 8
  let fmt = 0;
  for (let i = 0; i < 8; i++) fmt |= m[i < 6 ? i : i + 1][8] << i;
  for (let i = 0; i < 7; i++) fmt |= m[8][i < 6 ? i : i + 1] << (14 - i);
  const unmasked = fmt ^ 0b101010000010010;
  const ec = (unmasked >> 13) & 0b11;
  const mask = (unmasked >> 10) & 0b111;
  expect(ec).toBe(0b00); // EC level M

  const fmap = buildFunctionMap(version);
  for (let r = 0; r < size; r++) {
    for (let c = 0; c < size; c++) {
      if (!fmap[r][c] && maskCondition(mask, r, c)) m[r][c] ^= 1;
    }
  }

  const bits: number[] = [];
  let upward = true;
  for (let col = size - 1; col > 0; col -= 2) {
    if (col === 6) col--;
    for (let i = 0; i < size; i++) {
      const row = upward ? size - 1 - i : i;
      for (const c of [col, col - 1]) if (!fmap[row][c]) bits.push(m[row][c]);
    }
    upward = !upward;
  }

  const groups = ECC_M[version - 1];
  let totalCw = 0;
  for (const [nb, nt] of groups) totalCw += nb * nt;
  const codewords: number[] = [];
  for (let i = 0; i < totalCw; i++) {
    let v = 0;
    for (let j = 0; j < 8; j++) v = (v << 1) | bits[i * 8 + j];
    codewords.push(v);
  }

  // de-interleave (mirror of interleave)
  const ecLen = groups[0][1] - groups[0][2];
  const blocks: number[][] = [];
  const ecBlocks: number[][] = [];
  for (const [nb, nt, nd] of groups) {
    for (let b = 0; b < nb; b++) {
      blocks.push(new Array<number>(nd).fill(0));
      ecBlocks.push(new Array<number>(nt - nd).fill(0));
    }
  }
  const maxData = Math.max(...blocks.map((b) => b.length));
  let idx = 0;
  for (let i = 0; i < maxData; i++) {
    for (const b of blocks) if (i < b.length) b[i] = codewords[idx++];
  }
  for (let i = 0; i < ecLen; i++) {
    for (const e of ecBlocks) if (i < e.length) e[i] = codewords[idx++];
  }

  // Reed–Solomon syndrome check: (data||ec) must be a valid codeword
  for (let bi = 0; bi < blocks.length; bi++) {
    const full = [...blocks[bi], ...ecBlocks[bi]];
    for (let k = 0; k < ecBlocks[bi].length; k++) {
      let syn = 0;
      for (const coef of full) syn = gfMul(syn, GF_EXP[k]) ^ coef;
      expect(syn).toBe(0);
    }
  }

  // parse byte-mode segment from the concatenated data codewords
  const dataBits: number[] = [];
  for (const b of blocks) for (const cw of b) for (let j = 7; j >= 0; j--) dataBits.push((cw >> j) & 1);
  let pos = 0;
  const take = (n: number) => {
    let v = 0;
    for (let i = 0; i < n; i++) v = (v << 1) | dataBits[pos++];
    return v;
  };
  expect(take(4)).toBe(0b0100); // byte mode
  const count = take(charCountBits(version));
  const out = new Uint8Array(count);
  for (let i = 0; i < count; i++) out[i] = take(8);
  return new TextDecoder().decode(out);
}

/** A finder pattern is a 7×7 ring: 1:1:3:1:1 concentric squares. */
function isFinderAt(qr: QrMatrix, top: number, left: number): boolean {
  for (let dr = 0; dr < 7; dr++) {
    for (let dc = 0; dc < 7; dc++) {
      const ring = dr === 0 || dr === 6 || dc === 0 || dc === 6;
      const core = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
      const expected = ring || core;
      if (qr.modules[top + dr][left + dc] !== expected) return false;
    }
  }
  return true;
}

describe('encodeQr — shared conformance fixtures (byte-for-byte parity with Flutter)', () => {
  for (const c of fixtures.cases) {
    it(`${c.name}: v${c.version}/mask${c.mask}/${c.size}²`, () => {
      const qr = encodeQr(c.input);
      expect(qr).not.toBeNull();
      expect(qr!.version).toBe(c.version);
      expect(qr!.mask).toBe(c.mask);
      expect(qr!.size).toBe(c.size);
      expect(toRows(qr!)).toEqual(c.rows);
    });
  }
});

describe('encodeQr — structural well-formedness', () => {
  for (const c of fixtures.cases) {
    it(`${c.name}: finders, timing, dimensions, dark module`, () => {
      const qr = encodeQr(c.input)!;
      expect(qr.size).toBe(qr.version * 4 + 17);
      expect(qr.modules.length).toBe(qr.size);
      expect(qr.modules.every((row) => row.length === qr.size)).toBe(true);
      // three finder patterns
      expect(isFinderAt(qr, 0, 0)).toBe(true);
      expect(isFinderAt(qr, 0, qr.size - 7)).toBe(true);
      expect(isFinderAt(qr, qr.size - 7, 0)).toBe(true);
      // timing patterns alternate between the finders
      for (let i = 8; i < qr.size - 8; i++) {
        expect(qr.modules[6][i]).toBe(i % 2 === 0);
        expect(qr.modules[i][6]).toBe(i % 2 === 0);
      }
      // fixed dark module
      expect(qr.modules[qr.size - 8][8]).toBe(true);
    });
  }
});

describe('encodeQr — round-trips back to the input (scannable)', () => {
  for (const c of fixtures.cases) {
    it(`${c.name} decodes to its input`, () => {
      expect(decode(encodeQr(c.input)!)).toBe(c.input);
    });
  }

  it('round-trips assorted invite-shaped strings not in the fixtures', () => {
    const samples = [
      'roomtkt1' + 'a'.repeat(300) + '#' + 'f'.repeat(64) + '@10.0.0.2:41337',
      'roomtkt1short',
      'x',
      'ünïcödé ☃ 🎫 invite',
      'a'.repeat(14), // exact v1 capacity
    ];
    for (const s of samples) {
      const qr = encodeQr(s);
      expect(qr).not.toBeNull();
      expect(decode(qr!)).toBe(s);
    }
  });
});

describe('encodeQr — graceful bounds', () => {
  it('encodes right up to the version-40 byte capacity', () => {
    const qr = encodeQr('a'.repeat(QR_MAX_BYTES));
    expect(qr).not.toBeNull();
    expect(qr!.version).toBe(40);
  });

  it('returns null when the payload exceeds every symbol (caller keeps Copy/Share)', () => {
    expect(encodeQr('a'.repeat(QR_MAX_BYTES + 1))).toBeNull();
  });
});
