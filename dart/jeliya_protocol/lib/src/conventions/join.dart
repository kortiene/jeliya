/// `room.join` with the reference retry ladder, ported 1:1 from
/// ui/src/lib/join.ts: 5 attempts, 1.5/2/3/4s delays, retrying ONLY
/// `peer_unreachable` (any other error — and the final attempt — rethrows
/// immediately).
library;

import '../methods.dart';
import '../models.dart';
import '../protocol.dart';

/// The two join-progress phases (join.ts `JoinPhase`).
abstract final class JoinPhases {
  static const String connecting = 'connecting';
  static const String retrying = 'retrying';
  static const List<String> all = [connecting, retrying];
}

/// One progress callback payload (join.ts `JoinProgress`).
class JoinProgress {
  const JoinProgress({
    required this.phase,
    required this.attempt,
    required this.maxAttempts,
    required this.message,
    this.lastError,
  });

  /// One of [JoinPhases.all].
  final String phase;
  final int attempt;
  final int maxAttempts;

  /// Reference UI copy, emitted verbatim so both clients narrate joins the
  /// same way.
  final String message;

  /// The shaped `peer_unreachable` error that triggered a retry; null on
  /// [JoinPhases.connecting].
  final RequestError? lastError;
}

const int _joinAttempts = 5;
const List<Duration> _retryDelays = [
  Duration(milliseconds: 1500),
  Duration(milliseconds: 2000),
  Duration(milliseconds: 3000),
  Duration(milliseconds: 4000),
];

/// Join a room, retrying only [ErrorCodes.peerUnreachable] (the inviter's
/// first path did not answer) up to 5 attempts with 1.5/2/3/4s delays.
/// Returns the joined `room_id`; any other failure — or the fifth
/// `peer_unreachable` — rethrows the original error. [peers] are optional
/// `<endpoint_id>@<ip:port>` dial hints. [sleep] is a test seam and defaults
/// to a real [Future.delayed].
Future<String> joinRoomWithRetry(
  Client client, {
  required String ticket,
  List<String>? peers,
  void Function(JoinProgress progress)? onProgress,
  Future<void> Function(Duration delay)? sleep,
}) async {
  final wait = sleep ?? (delay) => Future<void>.delayed(delay);

  for (var attempt = 1; attempt <= _joinAttempts; attempt++) {
    onProgress?.call(JoinProgress(
      phase: JoinPhases.connecting,
      attempt: attempt,
      maxAttempts: _joinAttempts,
      message: attempt == 1
          ? 'Finding the inviter and syncing the room invite...'
          : 'Retrying join ($attempt/$_joinAttempts)...',
    ));

    try {
      return await client.roomJoin(ticket, peers: peers);
    } catch (e) {
      final err = errorShape(e);
      if (err.code != ErrorCodes.peerUnreachable || attempt == _joinAttempts) {
        rethrow;
      }
      final delay = _retryDelays[
          attempt - 1 < _retryDelays.length ? attempt - 1 : _retryDelays.length - 1];
      onProgress?.call(JoinProgress(
        phase: JoinPhases.retrying,
        attempt: attempt,
        maxAttempts: _joinAttempts,
        message: 'The first path did not answer. '
            'Retrying in ${(delay.inMilliseconds / 1000).round()}s...',
        lastError: err,
      ));
      await wait(delay);
    }
  }

  // Unreachable: the fifth attempt either returned or rethrew above (the
  // reference's trailing `throw lastError` is the same dead end).
  throw StateError('joinRoomWithRetry: exhausted attempts without a rethrow');
}
