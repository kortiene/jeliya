/// Envelope-level conformance replay, ported 1:1 from
/// `ui/src/lib/conformance/harness.ts`. Replays the shared corpus against any
/// [Client] and asserts on normalized frames, so the Dart client is held to the
/// exact same golden vectors as the reference TypeScript client and the daemon.
library;

import 'dart:async';

import 'protocol.dart';

final _hex64 = RegExp(r'^[0-9a-f]{64}$', caseSensitive: false);
final _hex32 = RegExp(r'^[0-9a-f]{32}$', caseSensitive: false);
final _roomId = RegExp(r'^blake3:[0-9a-f]{64}$', caseSensitive: false);
final _ticket = RegExp(r'^roomtkt1[a-z2-7]+$', caseSensitive: false);
final _fileId = RegExp(r'^file_[0-9a-f]{32}$', caseSensitive: false);
final _addr = RegExp(r'^[0-9a-f]{64}@(\d|\[)', caseSensitive: false);

/// Replace dynamic scalars with stable type tags so two conformant oracles
/// compare equal despite different ids/timestamps. See harness.ts `normalize`.
dynamic normalize(dynamic value, [String? key]) {
  if (value is List) return value.map((v) => normalize(v)).toList();
  if (value is Map) {
    final out = <String, dynamic>{};
    value.forEach((k, v) => out[k as String] = normalize(v, k));
    return out;
  }
  if (value is num) {
    if (key == 'ts' || key == 'last_seen_ts' || key == 'fetched_at_ms' || key == 'started_at_ms') return '<ts>';
    if (key == 'pid' || key == 'port') return '<number>';
    if (key == 'size' || key == 'member_count' || key == 'providers' || key == 'bytes' || key == 'local_bytes') return '<number>';
    return value;
  }
  if (value is String) {
    if (key == 'version') return '<version>';
    if (key == 'data_dir' || key == 'path' || key == 'local_path' || key == 'save_dir') return '<path>';
    if (key == 'device_id' || key == 'identity_id' || key == 'sender_id' || key == 'endpoint_id') {
      return _hex64.hasMatch(value) ? '<hex64>' : value;
    }
    if (_roomId.hasMatch(value)) return '<room_id>';
    if (_ticket.hasMatch(value)) return '<ticket>';
    if (_fileId.hasMatch(value)) return '<file_id>';
    if (_addr.hasMatch(value)) return '<addr>';
    if (_hex64.hasMatch(value)) return '<hex64>';
    if (_hex32.hasMatch(value)) return '<hex32>';
    return value;
  }
  return value;
}

class Diff {
  Diff(this.path, this.expected, this.actual);
  final String path;
  final dynamic expected;
  final dynamic actual;
  @override
  String toString() => 'at $path: expected $expected, got $actual';
}

/// Deep-compare the normalized [actual] against [template]. When [subset] is
/// false the actual must have no extra keys either.
List<Diff> diffAgainst(dynamic actual, dynamic template, {required bool subset, String path = r'$'}) =>
    _walk(normalize(actual), template, subset, path);

List<Diff> _walk(dynamic actual, dynamic template, bool subset, String path) {
  if (template is Map) {
    if (actual is! Map) return [Diff(path, template, actual)];
    final diffs = <Diff>[];
    template.forEach((k, v) {
      // A required key that is ABSENT from actual is a diff even when the
      // template value is null — mirroring the TS harness where a missing key
      // is `undefined`, and `undefined !== null`. Routing a missing key through
      // the scalar compare would let a `key: null` template pass on a frame
      // that omits the key entirely (a real fidelity gap).
      if (actual.containsKey(k)) {
        diffs.addAll(_walk(actual[k], v, subset, '$path.$k'));
      } else {
        diffs.add(Diff('$path.$k', v, '<absent>'));
      }
    });
    if (!subset) {
      actual.forEach((k, v) {
        if (!template.containsKey(k)) diffs.add(Diff('$path.$k', null, v));
      });
    }
    return diffs;
  }
  if (template is List) {
    if (actual is! List) return [Diff(path, template, actual)];
    final diffs = <Diff>[];
    for (var i = 0; i < template.length; i++) {
      diffs.addAll(_walk(i < actual.length ? actual[i] : null, template[i], subset, '$path[$i]'));
    }
    return diffs;
  }
  return _scalarEq(actual, template) ? const [] : [Diff(path, template, actual)];
}

bool _scalarEq(dynamic a, dynamic b) {
  if (a is num && b is num) return a == b;
  return a == b;
}

class StepResult {
  StepResult(this.step, this.method, this.ok, this.detail);
  final int step;
  final String method;
  final bool ok;
  final String? detail;
}

Map<String, dynamic> _resolveParams(Map<String, dynamic>? params, Map<String, dynamic> bag) {
  final out = <String, dynamic>{};
  params?.forEach((k, v) {
    out[k] = (v is String && v.startsWith(r'$')) ? bag[v.substring(1)] : v;
  });
  return out;
}

dynamic _pick(dynamic obj, String dotted) {
  dynamic acc = obj;
  for (final k in dotted.split('.')) {
    acc = (acc is Map) ? acc[k] : null;
  }
  return acc;
}

/// Replay one scenario (a decoded corpus entry) against [client].
Future<List<StepResult>> replayScenario(Client client, Map<String, dynamic> scenario,
    {int pushWaitMs = 2000}) async {
  final bag = <String, dynamic>{};
  final pushes = <Push>[];
  final waiters = <void Function()>[];
  final sub = client.pushes.listen((p) {
    pushes.add(p);
    final ws = List.of(waiters);
    waiters.clear();
    for (final w in ws) {
      w();
    }
  });
  final results = <StepResult>[];
  try {
    final steps = (scenario['steps'] as List).cast<Map<String, dynamic>>();
    for (var i = 0; i < steps.length; i++) {
      final step = steps[i];
      final method = step['call'] as String;
      final params = _resolveParams((step['params'] as Map?)?.cast<String, dynamic>(), bag);
      try {
        final result = await client.call(method, params);
        if (step.containsKey('expectError')) {
          results.add(StepResult(i, method, false, 'expected error ${step['expectError']}, got success'));
          continue;
        }
        (step['save'] as Map?)?.forEach((k, path) => bag[k as String] = _pick(result, path as String));
        final detail = _checkResult(result, step);
        if (detail != null) {
          results.add(StepResult(i, method, false, detail));
          continue;
        }
        if (step['expectPush'] != null) {
          final ok = await _waitForPush(
              (step['expectPush'] as Map).cast<String, dynamic>(), pushes, waiters, pushWaitMs);
          if (!ok) {
            results.add(StepResult(i, method, false, 'no ${(step['expectPush'] as Map)['push']} push matched within ${pushWaitMs}ms'));
            continue;
          }
        }
        results.add(StepResult(i, method, true, null));
      } on RequestError catch (e) {
        if (step.containsKey('expectError')) {
          final ok = e.code == step['expectError'];
          results.add(StepResult(i, method, ok, ok ? null : 'expected error ${step['expectError']}, got ${e.code}'));
        } else {
          results.add(StepResult(i, method, false, 'unexpected error: ${e.code}'));
        }
      }
    }
  } finally {
    await sub.cancel();
  }
  return results;
}

String? _checkResult(dynamic result, Map<String, dynamic> step) {
  final hasExact = step.containsKey('expect');
  final template = hasExact ? step['expect'] : step['expectSubset'];
  if (template == null && !hasExact) return null;
  final diffs = diffAgainst(result, template, subset: !hasExact);
  return diffs.isEmpty ? null : diffs.first.toString();
}

Future<bool> _waitForPush(Map<String, dynamic> want, List<Push> pushes, List<void Function()> waiters, int timeoutMs) {
  bool matches() => pushes.any((p) =>
      p.name == want['push'] && diffAgainst(p.data, want['match'], subset: true).isEmpty);
  if (matches()) return Future.value(true);
  final completer = Completer<bool>();
  final timer = Timer(Duration(milliseconds: timeoutMs), () {
    if (!completer.isCompleted) completer.complete(matches());
  });
  void check() {
    if (matches()) {
      if (!completer.isCompleted) {
        timer.cancel();
        completer.complete(true);
      }
    } else {
      waiters.add(check);
    }
  }
  waiters.add(check);
  return completer.future;
}
