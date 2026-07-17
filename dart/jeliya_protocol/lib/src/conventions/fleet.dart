/// Fleet attention projection — the Dart mirror of ui/src/lib/fleet.ts, the
/// shared classifier behind the Agent Fleet's "Needs Attention" section
/// (docs/room-attention.md, decision 4: a CLOSED set of actionable agent
/// states, folded from real evidence only). Kept 1:1 with the reference so
/// React and Flutter group and rank agents identically.
///
/// Every state is provable from a `FleetAgent` the daemon already returns — its
/// derived `liveness` and its latest signed `agent_status` label. Nothing is
/// invented: "offline-after-work" is derived (offline WITH a proven status
/// history), and there is no run/task token on the wire, so none is claimed.
library;

import '../models.dart' show LivenessValues;
import 'format.dart';

/// Why an agent is in Needs Attention. A coarse, honest fold of real evidence
/// (see the reference doc in fleet.ts for the per-value rules).
enum AttentionReason { failed, review, stale, offline }

/// Severity order, most actionable first — the sort rank for the section.
const List<AttentionReason> attentionOrder = [
  AttentionReason.failed,
  AttentionReason.review,
  AttentionReason.stale,
  AttentionReason.offline,
];

/// The attention state of one agent, or null when it needs none. Evaluated in
/// severity order, first match wins: a red/blue latest status outranks a
/// liveness state, and a failure outranks a review request.
AttentionReason? attentionReason(String liveness, String? latestLabel) {
  if (latestLabel != null) {
    final tone = labelTone(latestLabel);
    if (tone == LabelTone.red) return AttentionReason.failed;
    if (tone == LabelTone.blue) return AttentionReason.review;
  }
  if (liveness == LivenessValues.stale) return AttentionReason.stale;
  if (liveness == LivenessValues.offline && latestLabel != null) {
    return AttentionReason.offline;
  }
  return null;
}

/// Whether an agent belongs in Needs Attention.
bool needsAttention(String liveness, String? latestLabel) =>
    attentionReason(liveness, latestLabel) != null;

/// Sort rank for the Needs Attention section: lower is more urgent; agents that
/// need no attention rank last.
int attentionRank(String liveness, String? latestLabel) {
  final reason = attentionReason(liveness, latestLabel);
  return reason == null ? attentionOrder.length : attentionOrder.indexOf(reason);
}

/// A stale or offline agent's last posted label is a claim its liveness no
/// longer supports, so a surface must never render it as a bare present-tense
/// status (a Stale pill beside a "Working" chip is the contradiction #69
/// removes). True when the latest status must be qualified as unverified/past.
bool statusUnverified(String liveness) =>
    liveness == LivenessValues.stale || liveness == LivenessValues.offline;

/// A history point is real numeric progress only when it carries a finite
/// value; a label-only point has no magnitude, so the numeric chart must skip
/// it rather than invent a y-value (docs/room-attention.md, decision 6).
bool hasNumericProgress(num? progress) => progress != null && progress.isFinite;
