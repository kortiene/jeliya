/** A self-contained QR encoder for room-invite codes (issue #103) — no runtime
 *  dependency, no CDN. Deliberately narrow: it encodes exactly what an invite
 *  needs and nothing more.
 *
 *  Fixed policy (matched 1:1 by the Flutter port in
 *  `app/lib/src/qr/qr.dart`, and locked by the shared fixtures in
 *  `conformance/qr-invite.fixtures.json`):
 *    - Byte mode (UTF-8). Invite strings carry lowercase / `#` / `@` / `,`,
 *      which alphanumeric mode cannot represent, so byte mode is always correct.
 *    - Error-correction level M (~15%), the QR default.
 *    - The smallest version 1..40 whose byte capacity at level M fits.
 *    - All eight data masks evaluated; the lowest ISO/IEC 18004 penalty wins
 *      (ties resolve to the lowest mask index).
 *
 *  Correctness is proven two ways in qr.test.ts: byte-for-byte equality with the
 *  cross-client fixtures, and a self-contained round-trip decode (un-mask →
 *  reverse placement → de-interleave → Reed–Solomon syndrome check → parse) that
 *  recovers the input — i.e. the symbol actually scans. */

// ECC_M[v-1] = error-correction block layout at EC level M for version v:
// each entry [numBlocks, totalCodewordsPerBlock, dataCodewordsPerBlock].
const ECC_M: readonly (readonly [number, number, number])[][] = [
  [[1, 26, 16]], // v1
  [[1, 44, 28]], // v2
  [[1, 70, 44]], // v3
  [[2, 50, 32]], // v4
  [[2, 67, 43]], // v5
  [[4, 43, 27]], // v6
  [[4, 49, 31]], // v7
  [[2, 60, 38], [2, 61, 39]], // v8
  [[3, 58, 36], [2, 59, 37]], // v9
  [[4, 69, 43], [1, 70, 44]], // v10
  [[1, 80, 50], [4, 81, 51]], // v11
  [[6, 58, 36], [2, 59, 37]], // v12
  [[8, 59, 37], [1, 60, 38]], // v13
  [[4, 64, 40], [5, 65, 41]], // v14
  [[5, 65, 41], [5, 66, 42]], // v15
  [[7, 73, 45], [3, 74, 46]], // v16
  [[10, 74, 46], [1, 75, 47]], // v17
  [[9, 69, 43], [4, 70, 44]], // v18
  [[3, 70, 44], [11, 71, 45]], // v19
  [[3, 67, 41], [13, 68, 42]], // v20
  [[17, 68, 42]], // v21
  [[17, 74, 46]], // v22
  [[4, 75, 47], [14, 76, 48]], // v23
  [[6, 73, 45], [14, 74, 46]], // v24
  [[8, 75, 47], [13, 76, 48]], // v25
  [[19, 74, 46], [4, 75, 47]], // v26
  [[22, 73, 45], [3, 74, 46]], // v27
  [[3, 73, 45], [23, 74, 46]], // v28
  [[21, 73, 45], [7, 74, 46]], // v29
  [[19, 75, 47], [10, 76, 48]], // v30
  [[2, 74, 46], [29, 75, 47]], // v31
  [[10, 74, 46], [23, 75, 47]], // v32
  [[14, 74, 46], [21, 75, 47]], // v33
  [[14, 74, 46], [23, 75, 47]], // v34
  [[12, 75, 47], [26, 76, 48]], // v35
  [[6, 75, 47], [34, 76, 48]], // v36
  [[29, 74, 46], [14, 75, 47]], // v37
  [[13, 74, 46], [32, 75, 47]], // v38
  [[40, 75, 47], [7, 76, 48]], // v39
  [[18, 75, 47], [31, 76, 48]], // v40
];

// ALIGN[v-1] = alignment-pattern centre coordinates for version v (empty for v1).
const ALIGN: readonly (readonly number[])[] = [
  [], // v1
  [6, 18], // v2
  [6, 22], // v3
  [6, 26], // v4
  [6, 30], // v5
  [6, 34], // v6
  [6, 22, 38], // v7
  [6, 24, 42], // v8
  [6, 26, 46], // v9
  [6, 28, 50], // v10
  [6, 30, 54], // v11
  [6, 32, 58], // v12
  [6, 34, 62], // v13
  [6, 26, 46, 66], // v14
  [6, 26, 48, 70], // v15
  [6, 26, 50, 74], // v16
  [6, 30, 54, 78], // v17
  [6, 30, 56, 82], // v18
  [6, 30, 58, 86], // v19
  [6, 34, 62, 90], // v20
  [6, 28, 50, 72, 94], // v21
  [6, 26, 50, 74, 98], // v22
  [6, 30, 54, 78, 102], // v23
  [6, 28, 54, 80, 106], // v24
  [6, 32, 58, 84, 110], // v25
  [6, 30, 58, 86, 114], // v26
  [6, 34, 62, 90, 118], // v27
  [6, 26, 50, 74, 98, 122], // v28
  [6, 30, 54, 78, 102, 126], // v29
  [6, 26, 52, 78, 104, 130], // v30
  [6, 30, 56, 82, 108, 134], // v31
  [6, 34, 60, 86, 112, 138], // v32
  [6, 30, 58, 86, 114, 142], // v33
  [6, 34, 62, 90, 118, 146], // v34
  [6, 30, 54, 78, 102, 126, 150], // v35
  [6, 24, 50, 76, 102, 128, 154], // v36
  [6, 28, 54, 80, 106, 132, 158], // v37
  [6, 32, 58, 84, 110, 136, 162], // v38
  [6, 26, 54, 82, 110, 138, 166], // v39
  [6, 30, 58, 86, 114, 142, 170], // v40
];

// ---- Galois field GF(256), primitive polynomial 0x11d ----------------------
const GF_EXP = new Uint8Array(512);
const GF_LOG = new Uint8Array(256);
(() => {
  let x = 1;
  for (let i = 0; i < 255; i++) {
    GF_EXP[i] = x;
    GF_LOG[x] = i;
    x <<= 1;
    if (x & 0x100) x ^= 0x11d;
  }
  for (let i = 255; i < 512; i++) GF_EXP[i] = GF_EXP[i - 255];
})();

function gfMul(a: number, b: number): number {
  if (a === 0 || b === 0) return 0;
  return GF_EXP[GF_LOG[a] + GF_LOG[b]];
}

/** Generator polynomial for `degree` EC codewords, high-degree-first and monic
 *  (coefficient[0] === 1 is the x^degree term). */
function rsGenerator(degree: number): number[] {
  let poly = [1]; // low-degree-first during construction
  for (let i = 0; i < degree; i++) {
    const next = new Array<number>(poly.length + 1).fill(0);
    for (let j = 0; j < poly.length; j++) {
      next[j] ^= gfMul(poly[j], GF_EXP[i]);
      next[j + 1] ^= poly[j];
    }
    poly = next;
  }
  return poly.reverse();
}

function rsRemainder(data: readonly number[], degree: number): number[] {
  const gen = rsGenerator(degree);
  const rem = new Array<number>(degree).fill(0);
  for (const d of data) {
    const factor = d ^ rem[0];
    rem.shift();
    rem.push(0);
    for (let i = 0; i < degree; i++) rem[i] ^= gfMul(gen[i + 1], factor);
  }
  return rem;
}

// ---- capacity / version selection ------------------------------------------
function dataCodewords(version: number): number {
  let total = 0;
  for (const [nb, , nd] of ECC_M[version - 1]) total += nb * nd;
  return total;
}

/** Byte-mode character-count indicator width: 8 bits for v1-9, else 16. */
function charCountBits(version: number): number {
  return version <= 9 ? 8 : 16;
}

/** The largest UTF-8 byte payload that fits at EC level M (version 40). */
export const QR_MAX_BYTES = (dataCodewords(40) * 8 - 4 - 16) >> 3;

function chooseVersion(nbytes: number): number | null {
  for (let v = 1; v <= 40; v++) {
    const capBits = dataCodewords(v) * 8;
    if (4 + charCountBits(v) + nbytes * 8 <= capBits) return v;
  }
  return null;
}

// ---- bitstream / codewords -------------------------------------------------
function buildCodewords(bytes: Uint8Array, version: number): number[] {
  const totalDcw = dataCodewords(version);
  const bits: number[] = [];
  const put = (value: number, n: number) => {
    for (let i = n - 1; i >= 0; i--) bits.push((value >> i) & 1);
  };
  put(0b0100, 4); // byte mode indicator
  put(bytes.length, charCountBits(version));
  for (const b of bytes) put(b, 8);
  const capBits = totalDcw * 8;
  put(0, Math.min(4, capBits - bits.length)); // terminator
  if (bits.length % 8) put(0, 8 - (bits.length % 8)); // byte-align
  const cws: number[] = [];
  for (let i = 0; i < bits.length; i += 8) {
    let v = 0;
    for (let j = 0; j < 8; j++) v = (v << 1) | bits[i + j];
    cws.push(v);
  }
  const pad = [0xec, 0x11];
  for (let i = 0; cws.length < totalDcw; i++) cws.push(pad[i % 2]);
  return cws;
}

function interleave(dataCw: readonly number[], version: number): number[] {
  const groups = ECC_M[version - 1];
  const ecLen = groups[0][1] - groups[0][2];
  const blocks: number[][] = [];
  const ecBlocks: number[][] = [];
  let idx = 0;
  for (const [nb, nt, nd] of groups) {
    for (let b = 0; b < nb; b++) {
      const blk = dataCw.slice(idx, idx + nd);
      idx += nd;
      blocks.push(blk);
      ecBlocks.push(rsRemainder(blk, nt - nd));
    }
  }
  const result: number[] = [];
  const maxData = Math.max(...blocks.map((b) => b.length));
  for (let i = 0; i < maxData; i++) {
    for (const b of blocks) if (i < b.length) result.push(b[i]);
  }
  for (let i = 0; i < ecLen; i++) {
    for (const e of ecBlocks) if (i < e.length) result.push(e[i]);
  }
  return result;
}

// ---- matrix ----------------------------------------------------------------
type Cell = 0 | 1 | null; // null = unset
type Grid = Cell[][];

function matrixSize(version: number): number {
  return version * 4 + 17;
}

function newGrid(size: number): Grid {
  return Array.from({ length: size }, () => new Array<Cell>(size).fill(null));
}

function placeFinder(m: Grid, r: number, c: number): void {
  const size = m.length;
  for (let dr = -1; dr <= 7; dr++) {
    for (let dc = -1; dc <= 7; dc++) {
      const rr = r + dr;
      const cc = c + dc;
      if (rr < 0 || rr >= size || cc < 0 || cc >= size) continue;
      if (dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6) {
        const on = dr === 0 || dr === 6 || dc === 0 || dc === 6 || (dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4);
        m[rr][cc] = on ? 1 : 0;
      } else {
        m[rr][cc] = 0; // separator
      }
    }
  }
}

function reserveFormat(m: Grid): void {
  const size = m.length;
  for (let i = 0; i < 9; i++) {
    if (m[8][i] === null) m[8][i] = 0;
    if (m[i][8] === null) m[i][8] = 0;
  }
  for (let i = 0; i < 8; i++) {
    if (m[8][size - 1 - i] === null) m[8][size - 1 - i] = 0;
    if (m[size - 1 - i][8] === null) m[size - 1 - i][8] = 0;
  }
}

function reserveVersion(m: Grid): void {
  const size = m.length;
  for (let i = 0; i < 6; i++) {
    for (let j = 0; j < 3; j++) {
      m[i][size - 11 + j] = 0;
      m[size - 11 + j][i] = 0;
    }
  }
}

function placeFunctionPatterns(m: Grid, version: number): void {
  const size = m.length;
  placeFinder(m, 0, 0);
  placeFinder(m, 0, size - 7);
  placeFinder(m, size - 7, 0);
  for (let i = 8; i < size - 8; i++) {
    const bit: Cell = i % 2 === 0 ? 1 : 0;
    if (m[6][i] === null) m[6][i] = bit;
    if (m[i][6] === null) m[i][6] = bit;
  }
  const centers = ALIGN[version - 1];
  for (const r of centers) {
    for (const c of centers) {
      if ((r <= 8 && c <= 8) || (r <= 8 && c >= size - 9) || (r >= size - 9 && c <= 8)) continue;
      for (let dr = -2; dr <= 2; dr++) {
        for (let dc = -2; dc <= 2; dc++) {
          const on = Math.abs(dr) === 2 || Math.abs(dc) === 2 || (dr === 0 && dc === 0);
          m[r + dr][c + dc] = on ? 1 : 0;
        }
      }
    }
  }
  m[size - 8][8] = 1; // dark module
  reserveFormat(m);
  if (version >= 7) reserveVersion(m);
}

/** Walk the data region right-to-left in vertical two-module columns, zig-
 *  zagging up then down, skipping the vertical timing column — the ISO codeword
 *  placement order. `fixed[r][c] !== null` marks a function module to skip. */
function placeData(m: Grid, fixed: Grid, codewords: readonly number[]): void {
  const size = m.length;
  const bits: number[] = [];
  for (const cw of codewords) for (let i = 7; i >= 0; i--) bits.push((cw >> i) & 1);
  let idx = 0;
  let upward = true;
  for (let col = size - 1; col > 0; col -= 2) {
    if (col === 6) col--; // skip timing column
    for (let i = 0; i < size; i++) {
      const row = upward ? size - 1 - i : i;
      for (const c of [col, col - 1]) {
        if (fixed[row][c] === null) {
          m[row][c] = idx < bits.length ? ((bits[idx++] as Cell)) : 0;
        }
      }
    }
    upward = !upward;
  }
}

// ---- masking ---------------------------------------------------------------
function maskCondition(mask: number, r: number, c: number): boolean {
  switch (mask) {
    case 0: return (r + c) % 2 === 0;
    case 1: return r % 2 === 0;
    case 2: return c % 3 === 0;
    case 3: return (r + c) % 3 === 0;
    case 4: return (Math.floor(r / 2) + Math.floor(c / 3)) % 2 === 0;
    case 5: return ((r * c) % 2) + ((r * c) % 3) === 0;
    case 6: return (((r * c) % 2) + ((r * c) % 3)) % 2 === 0;
    case 7: return (((r + c) % 2) + ((r * c) % 3)) % 2 === 0;
    default: throw new Error(`bad mask ${mask}`);
  }
}

function applyMask(base: Grid, fixed: Grid, mask: number): number[][] {
  const size = base.length;
  const out = base.map((row) => row.map((v) => v as number));
  for (let r = 0; r < size; r++) {
    for (let c = 0; c < size; c++) {
      if (fixed[r][c] === null && maskCondition(mask, r, c)) out[r][c] ^= 1;
    }
  }
  return out;
}

/** ISO/IEC 18004 7.8.3 mask-penalty score (lower is better). */
function penalty(m: number[][]): number {
  const size = m.length;
  let score = 0;
  const scanLine = (get: (i: number) => number) => {
    let run = 1;
    for (let i = 1; i < size; i++) {
      if (get(i) === get(i - 1)) {
        run++;
      } else {
        if (run >= 5) score += 3 + (run - 5);
        run = 1;
      }
    }
    if (run >= 5) score += 3 + (run - 5);
  };
  for (let r = 0; r < size; r++) scanLine((i) => m[r][i]); // rule 1 rows
  for (let c = 0; c < size; c++) scanLine((i) => m[i][c]); // rule 1 cols
  for (let r = 0; r < size - 1; r++) {
    for (let c = 0; c < size - 1; c++) {
      const v = m[r][c];
      if (v === m[r][c + 1] && v === m[r + 1][c] && v === m[r + 1][c + 1]) score += 3; // rule 2
    }
  }
  const pat1 = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
  const pat2 = [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1];
  const matches = (get: (i: number) => number, i: number, pat: number[]) => {
    for (let k = 0; k < 11; k++) if (get(i + k) !== pat[k]) return false;
    return true;
  };
  for (let r = 0; r < size; r++) {
    for (let i = 0; i <= size - 11; i++) {
      if (matches((j) => m[r][j], i, pat1) || matches((j) => m[r][j], i, pat2)) score += 40; // rule 3 rows
    }
  }
  for (let c = 0; c < size; c++) {
    for (let i = 0; i <= size - 11; i++) {
      if (matches((j) => m[j][c], i, pat1) || matches((j) => m[j][c], i, pat2)) score += 40; // rule 3 cols
    }
  }
  let dark = 0;
  for (let r = 0; r < size; r++) for (let c = 0; c < size; c++) dark += m[r][c];
  const ratio = Math.floor((dark * 100) / (size * size));
  const prev = Math.floor(ratio / 5) * 5;
  score += Math.min(Math.abs(prev - 50) / 5, Math.abs(prev + 5 - 50) / 5) * 10; // rule 4
  return score;
}

// ---- format & version information ------------------------------------------
/** 15-bit format string for EC level M and the given mask (bit 0 = LSB). */
function formatBits(mask: number): number {
  const data = (0b00 << 3) | mask; // EC level M indicator = 00
  let rem = data;
  for (let i = 0; i < 10; i++) {
    rem <<= 1;
    if (rem & (1 << 10)) rem ^= 0b10100110111;
  }
  return ((data << 10) | rem) ^ 0b101010000010010;
}

function placeFormat(m: number[][], mask: number): void {
  const size = m.length;
  const fmt = formatBits(mask);
  let voffset = 0;
  let hoffset = 0;
  for (let i = 0; i < 8; i++) {
    const vbit = (fmt >> i) & 1;
    const hbit = (fmt >> (14 - i)) & 1;
    if (i === 6) {
      voffset += 1;
      hoffset = 1;
    }
    m[i + voffset][8] = vbit; // vertical, top-left
    m[8][i + hoffset] = hbit; // horizontal, top-left
    m[8][size - 1 - i] = vbit; // horizontal, top-right
    m[size - 1 - i][8] = hbit; // vertical, bottom-left
  }
  m[size - 8][8] = 1; // dark module
}

/** 18-bit version information for v7-40 (bit 0 = LSB). */
function versionBits(version: number): number {
  let rem = version;
  for (let i = 0; i < 12; i++) {
    rem <<= 1;
    if (rem & (1 << 12)) rem ^= 0b1111100100101;
  }
  return (version << 12) | rem;
}

function placeVersion(m: number[][], version: number): void {
  if (version < 7) return;
  const size = m.length;
  const vb = versionBits(version);
  for (let i = 0; i < 18; i++) {
    const bit = (vb >> i) & 1;
    const r = Math.floor(i / 3);
    const c = i % 3;
    m[r][size - 11 + c] = bit;
    m[size - 11 + c][r] = bit;
  }
}

// ---- public API ------------------------------------------------------------
export interface QrMatrix {
  version: number;
  mask: number;
  size: number;
  /** row-major, size×size; `true` = dark module. */
  modules: boolean[][];
}

/** Encode `text` as a byte-mode, EC-level-M QR matrix, choosing the smallest
 *  fitting version and the lowest-penalty mask. Returns `null` when the payload
 *  exceeds the largest symbol (version 40) — the caller keeps Copy/Share as the
 *  fallback rather than rendering a broken code. */
export function encodeQr(text: string): QrMatrix | null {
  const bytes = new TextEncoder().encode(text);
  const version = chooseVersion(bytes.length);
  if (version === null) return null;
  const codewords = interleave(buildCodewords(bytes, version), version);
  const size = matrixSize(version);
  const base = newGrid(size);
  placeFunctionPatterns(base, version);
  const fixed: Grid = base.map((row) => row.slice()); // snapshot: non-null = function module
  placeData(base, fixed, codewords);

  let best: number[][] | null = null;
  let bestScore = Infinity;
  let bestMask = 0;
  for (let mask = 0; mask < 8; mask++) {
    const cand = applyMask(base, fixed, mask);
    placeVersion(cand, version);
    placeFormat(cand, mask);
    const s = penalty(cand);
    if (s < bestScore) {
      bestScore = s;
      best = cand;
      bestMask = mask;
    }
  }
  const modules = best!.map((row) => row.map((v) => v === 1));
  return { version, mask: bestMask, size, modules };
}

/** Internals exposed solely for qr.test.ts, which reverses the encode pipeline
 *  (un-mask → read placement → de-interleave → RS syndrome check → parse) to
 *  prove each matrix round-trips back to its input. Not part of the public API. */
export const _internals = {
  ECC_M,
  GF_EXP,
  gfMul,
  maskCondition,
  charCountBits,
  /** The function-module map for `version`: `true` where a module is a finder,
   *  separator, timing, alignment, dark, format- or version-reserved cell (i.e.
   *  not data) — the encoder's placement skip logic. placeFunctionPatterns
   *  already reserves the version block, so there is nothing more to reserve. */
  buildFunctionMap(version: number): boolean[][] {
    const m = newGrid(matrixSize(version));
    placeFunctionPatterns(m, version);
    return m.map((row) => row.map((cell) => cell !== null));
  },
};
