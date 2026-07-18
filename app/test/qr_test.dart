/// QR encoder parity + well-formedness (issue #103): the Dart side of the
/// hand-vendored QR encoder, replaying the SAME fixtures as ui/src/lib/qr.test.ts
/// from ui/src/lib/conformance/qr-invite.fixtures.json — so React and Flutter
/// produce byte-identical matrices. Also decodes each matrix back to its input
/// (un-mask -> reverse placement -> de-interleave -> Reed-Solomon syndrome check
/// -> parse), the proof that the code is well-formed and actually scans.
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:jeliya_app/src/qr/qr.dart';

Directory _repoRoot() {
  var dir = Directory.current;
  for (var i = 0; i < 8; i++) {
    if (File('${dir.path}/ui/src/lib/conformance/qr-invite.fixtures.json')
        .existsSync()) {
      return dir;
    }
    final parent = dir.parent;
    if (parent.path == dir.path) break;
    dir = parent;
  }
  throw StateError('could not locate repo root from ${Directory.current.path}');
}

List<String> _toRows(QrMatrix qr) =>
    [for (final row in qr.modules) row.map((on) => on ? '#' : '.').join()];

/// Reverse the encode pipeline and recover the input — the scannability proof.
String _decode(QrMatrix qr) {
  final size = qr.size;
  final version = (size - 17) ~/ 4;
  final m = [for (final row in qr.modules) [for (final on in row) on ? 1 : 0]];

  // format info (top-left copy): bits 0..7 down column 8, bits 14..8 across row 8
  var fmt = 0;
  for (var i = 0; i < 8; i++) {
    fmt |= m[i < 6 ? i : i + 1][8] << i;
  }
  for (var i = 0; i < 7; i++) {
    fmt |= m[8][i < 6 ? i : i + 1] << (14 - i);
  }
  final unmasked = fmt ^ 0x5412;
  final ec = (unmasked >> 13) & 0x3;
  final mask = (unmasked >> 10) & 0x7;
  expect(ec, 0x0, reason: 'EC level M');

  final fmap = QrInternals.functionMap(version);
  for (var r = 0; r < size; r++) {
    for (var c = 0; c < size; c++) {
      if (!fmap[r][c] && QrInternals.maskCondition(mask, r, c)) m[r][c] ^= 1;
    }
  }

  final bits = <int>[];
  var upward = true;
  for (var col = size - 1; col > 0; col -= 2) {
    if (col == 6) col -= 1;
    for (var i = 0; i < size; i++) {
      final row = upward ? size - 1 - i : i;
      for (final c in [col, col - 1]) {
        if (!fmap[row][c]) bits.add(m[row][c]);
      }
    }
    upward = !upward;
  }

  final groups = QrInternals.eccM[version - 1];
  var totalCw = 0;
  for (final g in groups) {
    totalCw += g[0] * g[1];
  }
  final codewords = <int>[];
  for (var i = 0; i < totalCw; i++) {
    var v = 0;
    for (var j = 0; j < 8; j++) {
      v = (v << 1) | bits[i * 8 + j];
    }
    codewords.add(v);
  }

  // de-interleave (mirror of the encoder's interleave)
  final ecLen = groups[0][1] - groups[0][2];
  final blocks = <List<int>>[];
  final ecBlocks = <List<int>>[];
  for (final g in groups) {
    for (var b = 0; b < g[0]; b++) {
      blocks.add(List<int>.filled(g[2], 0));
      ecBlocks.add(List<int>.filled(g[1] - g[2], 0));
    }
  }
  final maxData = blocks.map((b) => b.length).reduce((a, b) => a > b ? a : b);
  var idx = 0;
  for (var i = 0; i < maxData; i++) {
    for (final b in blocks) {
      if (i < b.length) b[i] = codewords[idx++];
    }
  }
  for (var i = 0; i < ecLen; i++) {
    for (final e in ecBlocks) {
      if (i < e.length) e[i] = codewords[idx++];
    }
  }

  // Reed-Solomon syndrome check: (data||ec) must be a valid codeword
  for (var bi = 0; bi < blocks.length; bi++) {
    final full = [...blocks[bi], ...ecBlocks[bi]];
    for (var k = 0; k < ecBlocks[bi].length; k++) {
      var syn = 0;
      for (final coef in full) {
        syn = QrInternals.gfMul(syn, QrInternals.gfExp[k]) ^ coef;
      }
      expect(syn, 0, reason: 'nonzero RS syndrome — codeword not error-free');
    }
  }

  final dataBits = <int>[];
  for (final b in blocks) {
    for (final cw in b) {
      for (var j = 7; j >= 0; j--) {
        dataBits.add((cw >> j) & 1);
      }
    }
  }
  var pos = 0;
  int take(int n) {
    var v = 0;
    for (var i = 0; i < n; i++) {
      v = (v << 1) | dataBits[pos++];
    }
    return v;
  }

  expect(take(4), 0x4, reason: 'byte mode indicator');
  final count = take(QrInternals.charCountBits(version));
  final out = <int>[for (var i = 0; i < count; i++) take(8)];
  return utf8.decode(out);
}

/// A finder pattern is a 7×7 ring: 1:1:3:1:1 concentric squares.
bool _isFinderAt(QrMatrix qr, int top, int left) {
  for (var dr = 0; dr < 7; dr++) {
    for (var dc = 0; dc < 7; dc++) {
      final ring = dr == 0 || dr == 6 || dc == 0 || dc == 6;
      final core = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
      if (qr.modules[top + dr][left + dc] != (ring || core)) return false;
    }
  }
  return true;
}

void main() {
  final root = _repoRoot();
  final fixtures = jsonDecode(
    File('${root.path}/ui/src/lib/conformance/qr-invite.fixtures.json')
        .readAsStringSync(),
  ) as Map<String, dynamic>;
  final cases = (fixtures['cases'] as List).cast<Map<String, dynamic>>();

  group('encodeQr — shared conformance fixtures (byte-for-byte parity with React)', () {
    for (final c in cases) {
      test('${c['name']}: v${c['version']}/mask${c['mask']}/${c['size']}²', () {
        final qr = encodeQr(c['input'] as String);
        expect(qr, isNotNull);
        expect(qr!.version, c['version']);
        expect(qr.mask, c['mask']);
        expect(qr.size, c['size']);
        expect(_toRows(qr), (c['rows'] as List).cast<String>());
      });
    }
  });

  group('encodeQr — structural well-formedness', () {
    for (final c in cases) {
      test('${c['name']}: finders, timing, dimensions, dark module', () {
        final qr = encodeQr(c['input'] as String)!;
        expect(qr.size, qr.version * 4 + 17);
        expect(qr.modules.length, qr.size);
        expect(qr.modules.every((row) => row.length == qr.size), isTrue);
        expect(_isFinderAt(qr, 0, 0), isTrue);
        expect(_isFinderAt(qr, 0, qr.size - 7), isTrue);
        expect(_isFinderAt(qr, qr.size - 7, 0), isTrue);
        for (var i = 8; i < qr.size - 8; i++) {
          expect(qr.modules[6][i], i % 2 == 0);
          expect(qr.modules[i][6], i % 2 == 0);
        }
        expect(qr.modules[qr.size - 8][8], isTrue);
      });
    }
  });

  group('encodeQr — round-trips back to the input (scannable)', () {
    for (final c in cases) {
      test('${c['name']} decodes to its input', () {
        expect(_decode(encodeQr(c['input'] as String)!), c['input']);
      });
    }

    test('round-trips assorted invite-shaped strings not in the fixtures', () {
      final samples = [
        'roomtkt1${'a' * 300}#${'f' * 64}@10.0.0.2:41337',
        'roomtkt1short',
        'x',
        'ünïcödé ☃ 🎫 invite',
        'a' * 14, // exact v1 capacity
      ];
      for (final s in samples) {
        final qr = encodeQr(s);
        expect(qr, isNotNull);
        expect(_decode(qr!), s);
      }
    });
  });

  group('encodeQr — graceful bounds', () {
    test('encodes right up to the version-40 byte capacity', () {
      final qr = encodeQr('a' * qrMaxBytes);
      expect(qr, isNotNull);
      expect(qr!.version, 40);
    });

    test('returns null when the payload exceeds every symbol (caller keeps Copy/Share)', () {
      expect(encodeQr('a' * (qrMaxBytes + 1)), isNull);
    });
  });
}
