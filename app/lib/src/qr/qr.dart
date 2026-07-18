/// A hand-vendored, pure-Dart QR encoder for room-invite codes (issue #103) —
/// no native plugin, no package. Deliberately narrow: it encodes exactly what an
/// invite needs and nothing more.
///
/// Fixed policy (matched 1:1 by the React port in `ui/src/lib/qr.ts`, and locked
/// by the shared fixtures in `ui/src/lib/conformance/qr-invite.fixtures.json`):
///   - Byte mode (UTF-8). Invite strings carry lowercase / `#` / `@` / `,`,
///     which alphanumeric mode cannot represent, so byte mode is always correct.
///   - Error-correction level M (~15%), the QR default.
///   - The smallest version 1..40 whose byte capacity at level M fits.
///   - All eight data masks evaluated; the lowest ISO/IEC 18004 penalty wins
///     (ties resolve to the lowest mask index).
///
/// Correctness is proven two ways in test/qr_test.dart: byte-for-byte equality
/// with the cross-client fixtures, and a self-contained round-trip decode
/// (un-mask -> reverse placement -> de-interleave -> Reed-Solomon syndrome check
/// -> parse) that recovers the input — i.e. the symbol actually scans.
library;

import 'dart:convert';

import 'package:flutter/foundation.dart' show visibleForTesting;

// _eccM[v-1] = EC block layout at level M for version v:
// each entry [numBlocks, totalPerBlock, dataPerBlock].
const List<List<List<int>>> _eccM = [
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

// _align[v-1] = alignment-pattern centre coordinates for version v.
const List<List<int>> _align = [
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
final List<int> _gfExp = _buildGfExp();
final List<int> _gfLog = _buildGfLog(_gfExp);

List<int> _buildGfExp() {
  final exp = List<int>.filled(512, 0);
  var x = 1;
  for (var i = 0; i < 255; i++) {
    exp[i] = x;
    x <<= 1;
    if (x & 0x100 != 0) x ^= 0x11d;
  }
  for (var i = 255; i < 512; i++) {
    exp[i] = exp[i - 255];
  }
  return exp;
}

List<int> _buildGfLog(List<int> exp) {
  final log = List<int>.filled(256, 0);
  for (var i = 0; i < 255; i++) {
    log[exp[i]] = i;
  }
  return log;
}

int _gfMul(int a, int b) {
  if (a == 0 || b == 0) return 0;
  return _gfExp[_gfLog[a] + _gfLog[b]];
}

/// Generator polynomial for [degree] EC codewords, high-degree-first and monic
/// (coefficient[0] == 1 is the x^degree term).
List<int> _rsGenerator(int degree) {
  var poly = <int>[1]; // low-degree-first during construction
  for (var i = 0; i < degree; i++) {
    final next = List<int>.filled(poly.length + 1, 0);
    for (var j = 0; j < poly.length; j++) {
      next[j] ^= _gfMul(poly[j], _gfExp[i]);
      next[j + 1] ^= poly[j];
    }
    poly = next;
  }
  return poly.reversed.toList();
}

List<int> _rsRemainder(List<int> data, int degree) {
  final gen = _rsGenerator(degree);
  final rem = List<int>.filled(degree, 0);
  for (final d in data) {
    final factor = d ^ rem[0];
    for (var i = 0; i < degree - 1; i++) {
      rem[i] = rem[i + 1];
    }
    rem[degree - 1] = 0;
    for (var i = 0; i < degree; i++) {
      rem[i] ^= _gfMul(gen[i + 1], factor);
    }
  }
  return rem;
}

// ---- capacity / version selection ------------------------------------------
int _dataCodewords(int version) {
  var total = 0;
  for (final g in _eccM[version - 1]) {
    total += g[0] * g[2];
  }
  return total;
}

/// Byte-mode character-count indicator width: 8 bits for v1-9, else 16.
int _charCountBits(int version) => version <= 9 ? 8 : 16;

/// The largest UTF-8 byte payload that fits at EC level M (version 40).
final int qrMaxBytes = (_dataCodewords(40) * 8 - 4 - 16) >> 3;

int? _chooseVersion(int nbytes) {
  for (var v = 1; v <= 40; v++) {
    final capBits = _dataCodewords(v) * 8;
    if (4 + _charCountBits(v) + nbytes * 8 <= capBits) return v;
  }
  return null;
}

// ---- bitstream / codewords -------------------------------------------------
List<int> _buildCodewords(List<int> bytes, int version) {
  final totalDcw = _dataCodewords(version);
  final bits = <int>[];
  void put(int value, int n) {
    for (var i = n - 1; i >= 0; i--) {
      bits.add((value >> i) & 1);
    }
  }

  put(0x4, 4); // byte mode indicator (0b0100)
  put(bytes.length, _charCountBits(version));
  for (final b in bytes) {
    put(b, 8);
  }
  final capBits = totalDcw * 8;
  final term = 4 < capBits - bits.length ? 4 : capBits - bits.length;
  put(0, term); // terminator
  if (bits.length % 8 != 0) put(0, 8 - (bits.length % 8)); // byte-align
  final cws = <int>[];
  for (var i = 0; i < bits.length; i += 8) {
    var v = 0;
    for (var j = 0; j < 8; j++) {
      v = (v << 1) | bits[i + j];
    }
    cws.add(v);
  }
  const pad = [0xec, 0x11];
  for (var i = 0; cws.length < totalDcw; i++) {
    cws.add(pad[i % 2]);
  }
  return cws;
}

List<int> _interleave(List<int> dataCw, int version) {
  final groups = _eccM[version - 1];
  final ecLen = groups[0][1] - groups[0][2];
  final blocks = <List<int>>[];
  final ecBlocks = <List<int>>[];
  var idx = 0;
  for (final g in groups) {
    final nb = g[0], nt = g[1], nd = g[2];
    for (var b = 0; b < nb; b++) {
      final blk = dataCw.sublist(idx, idx + nd);
      idx += nd;
      blocks.add(blk);
      ecBlocks.add(_rsRemainder(blk, nt - nd));
    }
  }
  final result = <int>[];
  final maxData = blocks.map((b) => b.length).reduce((a, b) => a > b ? a : b);
  for (var i = 0; i < maxData; i++) {
    for (final b in blocks) {
      if (i < b.length) result.add(b[i]);
    }
  }
  for (var i = 0; i < ecLen; i++) {
    for (final e in ecBlocks) {
      if (i < e.length) result.add(e[i]);
    }
  }
  return result;
}

// ---- matrix ----------------------------------------------------------------
int _matrixSize(int version) => version * 4 + 17;

List<List<int?>> _newGrid(int size) =>
    List.generate(size, (_) => List<int?>.filled(size, null));

void _placeFinder(List<List<int?>> m, int r, int c) {
  final size = m.length;
  for (var dr = -1; dr <= 7; dr++) {
    for (var dc = -1; dc <= 7; dc++) {
      final rr = r + dr, cc = c + dc;
      if (rr < 0 || rr >= size || cc < 0 || cc >= size) continue;
      if (dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6) {
        final on = dr == 0 ||
            dr == 6 ||
            dc == 0 ||
            dc == 6 ||
            (dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4);
        m[rr][cc] = on ? 1 : 0;
      } else {
        m[rr][cc] = 0; // separator
      }
    }
  }
}

void _reserveFormat(List<List<int?>> m) {
  final size = m.length;
  for (var i = 0; i < 9; i++) {
    if (m[8][i] == null) m[8][i] = 0;
    if (m[i][8] == null) m[i][8] = 0;
  }
  for (var i = 0; i < 8; i++) {
    if (m[8][size - 1 - i] == null) m[8][size - 1 - i] = 0;
    if (m[size - 1 - i][8] == null) m[size - 1 - i][8] = 0;
  }
}

void _reserveVersion(List<List<int?>> m) {
  final size = m.length;
  for (var i = 0; i < 6; i++) {
    for (var j = 0; j < 3; j++) {
      m[i][size - 11 + j] = 0;
      m[size - 11 + j][i] = 0;
    }
  }
}

void _placeFunctionPatterns(List<List<int?>> m, int version) {
  final size = m.length;
  _placeFinder(m, 0, 0);
  _placeFinder(m, 0, size - 7);
  _placeFinder(m, size - 7, 0);
  for (var i = 8; i < size - 8; i++) {
    final bit = i % 2 == 0 ? 1 : 0;
    if (m[6][i] == null) m[6][i] = bit;
    if (m[i][6] == null) m[i][6] = bit;
  }
  final centers = _align[version - 1];
  for (final r in centers) {
    for (final c in centers) {
      if ((r <= 8 && c <= 8) ||
          (r <= 8 && c >= size - 9) ||
          (r >= size - 9 && c <= 8)) {
        continue;
      }
      for (var dr = -2; dr <= 2; dr++) {
        for (var dc = -2; dc <= 2; dc++) {
          final on = dr.abs() == 2 || dc.abs() == 2 || (dr == 0 && dc == 0);
          m[r + dr][c + dc] = on ? 1 : 0;
        }
      }
    }
  }
  m[size - 8][8] = 1; // dark module
  _reserveFormat(m);
  if (version >= 7) _reserveVersion(m);
}

/// Walk the data region right-to-left in vertical two-module columns, zig-
/// zagging up then down, skipping the vertical timing column — the ISO codeword
/// placement order. `fixed[r][c] != null` marks a function module to skip.
void _placeData(
    List<List<int?>> m, List<List<int?>> fixed, List<int> codewords) {
  final size = m.length;
  final bits = <int>[];
  for (final cw in codewords) {
    for (var i = 7; i >= 0; i--) {
      bits.add((cw >> i) & 1);
    }
  }
  var idx = 0;
  var upward = true;
  for (var col = size - 1; col > 0; col -= 2) {
    if (col == 6) col -= 1; // skip timing column
    for (var i = 0; i < size; i++) {
      final row = upward ? size - 1 - i : i;
      for (final c in [col, col - 1]) {
        if (fixed[row][c] == null) {
          m[row][c] = idx < bits.length ? bits[idx++] : 0;
        }
      }
    }
    upward = !upward;
  }
}

// ---- masking ---------------------------------------------------------------
bool _maskCondition(int mask, int r, int c) {
  switch (mask) {
    case 0:
      return (r + c) % 2 == 0;
    case 1:
      return r % 2 == 0;
    case 2:
      return c % 3 == 0;
    case 3:
      return (r + c) % 3 == 0;
    case 4:
      return ((r ~/ 2) + (c ~/ 3)) % 2 == 0;
    case 5:
      return (r * c) % 2 + (r * c) % 3 == 0;
    case 6:
      return ((r * c) % 2 + (r * c) % 3) % 2 == 0;
    case 7:
      return ((r + c) % 2 + (r * c) % 3) % 2 == 0;
    default:
      throw ArgumentError('bad mask $mask');
  }
}

List<List<int>> _applyMask(
    List<List<int?>> base, List<List<int?>> fixed, int mask) {
  final size = base.length;
  final out = List.generate(size, (r) => List<int>.generate(size, (c) => base[r][c]!));
  for (var r = 0; r < size; r++) {
    for (var c = 0; c < size; c++) {
      if (fixed[r][c] == null && _maskCondition(mask, r, c)) out[r][c] ^= 1;
    }
  }
  return out;
}

/// ISO/IEC 18004 7.8.3 mask-penalty score (lower is better).
int _penalty(List<List<int>> m) {
  final size = m.length;
  var score = 0;
  void scanLine(int Function(int) get) {
    var run = 1;
    for (var i = 1; i < size; i++) {
      if (get(i) == get(i - 1)) {
        run++;
      } else {
        if (run >= 5) score += 3 + (run - 5);
        run = 1;
      }
    }
    if (run >= 5) score += 3 + (run - 5);
  }

  for (var r = 0; r < size; r++) {
    scanLine((i) => m[r][i]); // rule 1 rows
  }
  for (var c = 0; c < size; c++) {
    scanLine((i) => m[i][c]); // rule 1 cols
  }
  for (var r = 0; r < size - 1; r++) {
    for (var c = 0; c < size - 1; c++) {
      final v = m[r][c];
      if (v == m[r][c + 1] && v == m[r + 1][c] && v == m[r + 1][c + 1]) {
        score += 3; // rule 2
      }
    }
  }
  const pat1 = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
  const pat2 = [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1];
  bool matches(int Function(int) get, int i, List<int> pat) {
    for (var k = 0; k < 11; k++) {
      if (get(i + k) != pat[k]) return false;
    }
    return true;
  }

  for (var r = 0; r < size; r++) {
    for (var i = 0; i <= size - 11; i++) {
      if (matches((j) => m[r][j], i, pat1) ||
          matches((j) => m[r][j], i, pat2)) {
        score += 40; // rule 3 rows
      }
    }
  }
  for (var c = 0; c < size; c++) {
    for (var i = 0; i <= size - 11; i++) {
      if (matches((j) => m[j][c], i, pat1) ||
          matches((j) => m[j][c], i, pat2)) {
        score += 40; // rule 3 cols
      }
    }
  }
  var dark = 0;
  for (var r = 0; r < size; r++) {
    for (var c = 0; c < size; c++) {
      dark += m[r][c];
    }
  }
  final ratio = (dark * 100) ~/ (size * size);
  final prev = (ratio ~/ 5) * 5;
  final a = (prev - 50).abs() ~/ 5;
  final b = (prev + 5 - 50).abs() ~/ 5;
  score += (a < b ? a : b) * 10; // rule 4
  return score;
}

// ---- format & version information ------------------------------------------
/// 15-bit format string for EC level M and the given [mask] (bit 0 = LSB).
int _formatBits(int mask) {
  final data = (0x0 << 3) | mask; // EC level M indicator = 00
  var rem = data;
  for (var i = 0; i < 10; i++) {
    rem <<= 1;
    if (rem & (1 << 10) != 0) rem ^= 0x537; // 0b10100110111
  }
  return ((data << 10) | rem) ^ 0x5412; // 0b101010000010010
}

void _placeFormat(List<List<int>> m, int mask) {
  final size = m.length;
  final fmt = _formatBits(mask);
  var voffset = 0;
  var hoffset = 0;
  for (var i = 0; i < 8; i++) {
    final vbit = (fmt >> i) & 1;
    final hbit = (fmt >> (14 - i)) & 1;
    if (i == 6) {
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

/// 18-bit version information for v7-40 (bit 0 = LSB).
int _versionBits(int version) {
  var rem = version;
  for (var i = 0; i < 12; i++) {
    rem <<= 1;
    if (rem & (1 << 12) != 0) rem ^= 0x1f25; // 0b1111100100101
  }
  return (version << 12) | rem;
}

void _placeVersion(List<List<int>> m, int version) {
  if (version < 7) return;
  final size = m.length;
  final vb = _versionBits(version);
  for (var i = 0; i < 18; i++) {
    final bit = (vb >> i) & 1;
    final r = i ~/ 3;
    final c = i % 3;
    m[r][size - 11 + c] = bit;
    m[size - 11 + c][r] = bit;
  }
}

// ---- public API ------------------------------------------------------------
/// A QR matrix: [modules] is row-major, [size]×[size], `true` = a dark module.
class QrMatrix {
  const QrMatrix({
    required this.version,
    required this.mask,
    required this.size,
    required this.modules,
  });

  final int version;
  final int mask;
  final int size;
  final List<List<bool>> modules;
}

/// Encode [text] as a byte-mode, EC-level-M QR matrix, choosing the smallest
/// fitting version and the lowest-penalty mask. Returns `null` when the payload
/// exceeds the largest symbol (version 40) — the caller keeps Copy/Share as the
/// fallback rather than rendering a broken code.
QrMatrix? encodeQr(String text) {
  final bytes = utf8.encode(text);
  final version = _chooseVersion(bytes.length);
  if (version == null) return null;
  final codewords = _interleave(_buildCodewords(bytes, version), version);
  final size = _matrixSize(version);
  final base = _newGrid(size);
  _placeFunctionPatterns(base, version);
  final fixed = base.map((row) => List<int?>.from(row)).toList(); // function-module snapshot
  _placeData(base, fixed, codewords);

  List<List<int>>? best;
  var bestScore = 1 << 30;
  var bestMask = 0;
  for (var mask = 0; mask < 8; mask++) {
    final cand = _applyMask(base, fixed, mask);
    _placeVersion(cand, version);
    _placeFormat(cand, mask);
    final s = _penalty(cand);
    if (s < bestScore) {
      bestScore = s;
      best = cand;
      bestMask = mask;
    }
  }
  final modules = best!.map((row) => row.map((v) => v == 1).toList()).toList();
  return QrMatrix(version: version, mask: bestMask, size: size, modules: modules);
}

/// Internals exposed solely for test/qr_test.dart, which reverses the encode
/// pipeline (un-mask -> read placement -> de-interleave -> RS syndrome check ->
/// parse) to prove each matrix round-trips back to its input. Not public API.
@visibleForTesting
abstract final class QrInternals {
  static List<List<List<int>>> get eccM => _eccM;
  static List<int> get gfExp => _gfExp;
  static int gfMul(int a, int b) => _gfMul(a, b);
  static bool maskCondition(int mask, int r, int c) => _maskCondition(mask, r, c);
  static int charCountBits(int version) => _charCountBits(version);

  /// The function-module map for [version]: `true` where a module is a finder,
  /// separator, timing, alignment, dark, format-reserved, or version-reserved
  /// cell (i.e. not data), matching the encoder's placement skip logic.
  /// [_placeFunctionPatterns] already reserves the version block, so there is
  /// nothing more to reserve here.
  static List<List<bool>> functionMap(int version) {
    final m = _newGrid(_matrixSize(version));
    _placeFunctionPatterns(m, version);
    return m.map((row) => row.map((cell) => cell != null).toList()).toList();
  }
}
