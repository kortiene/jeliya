// Fleet attention projection — the shared classifier behind the Agent Fleet's
// "Needs Attention" section (docs/room-attention.md, decision 4: a CLOSED set
// of actionable states, folded from real evidence only). Mirrored 1:1 in
// dart/jeliya_protocol/lib/src/conventions/fleet.dart so React and Flutter
// group and rank agents identically.
//
// Every state here is provable from a `FleetAgent` the daemon already returns —
// its derived `liveness` and its latest signed `agent_status` label. Nothing is
// invented: there is no "offline-after-work" wire field, so it is derived
// (offline WITH a proven status history), and there is no run/task token on the
// wire, so none is claimed.

import type { Liveness } from './protocol';
import { labelTone } from './format';

/** Why an agent is in Needs Attention. A coarse, honest fold of real evidence:
 *  - `failed`  — the latest status tone is red (fail / error / block);
 *  - `review`  — the latest status tone is blue (awaiting review / reviewing /
 *                pending);
 *  - `stale`   — liveness is stale: a working-class claim whose peer is gone or
 *                whose freshness lapsed (fleet.rs THE RULE);
 *  - `offline` — offline-after-work: no live peer, but a proven status history
 *                (`latest != null`) — it did work and is no longer reachable.
 *  An agent that is merely offline and never posted a status is NOT attention:
 *  there is nothing to attend to. */
export type AttentionReason = 'failed' | 'review' | 'stale' | 'offline';

/** Severity order, most actionable first — the sort rank for the Needs
 *  Attention section. */
export const ATTENTION_ORDER: readonly AttentionReason[] = ['failed', 'review', 'stale', 'offline'];

/** The attention state of one agent, or null when it needs none. Evaluated in
 *  severity order, first match wins: a red/blue latest status is more
 *  actionable than a liveness state, and an outright failure outranks a review
 *  request. (A stale agent's latest label is always the working-class label, so
 *  red/blue and stale do not actually overlap; the order is defined anyway for
 *  robustness against unknown future labels.) */
export function attentionReason(liveness: Liveness, latestLabel: string | null): AttentionReason | null {
  if (latestLabel != null) {
    const tone = labelTone(latestLabel);
    if (tone === 'red') return 'failed';
    if (tone === 'blue') return 'review';
  }
  if (liveness === 'stale') return 'stale';
  if (liveness === 'offline' && latestLabel != null) return 'offline';
  return null;
}

/** Whether an agent belongs in Needs Attention (docs/room-attention.md,
 *  decision 4). */
export function needsAttention(liveness: Liveness, latestLabel: string | null): boolean {
  return attentionReason(liveness, latestLabel) !== null;
}

/** Sort rank for the Needs Attention section: lower is more urgent. Agents that
 *  need no attention rank last. */
export function attentionRank(liveness: Liveness, latestLabel: string | null): number {
  const reason = attentionReason(liveness, latestLabel);
  return reason === null ? ATTENTION_ORDER.length : ATTENTION_ORDER.indexOf(reason);
}

/** A stale or offline agent's last posted label is a claim its liveness no
 *  longer supports. A surface must therefore never render that label as a bare
 *  present-tense status — a Stale pill beside a "Working" chip is exactly the
 *  contradiction #69 removes. True when the latest status must be qualified
 *  (past-tense / "unverified"), never shown as a live state. */
export function statusUnverified(liveness: Liveness): boolean {
  return liveness === 'stale' || liveness === 'offline';
}

/** A history point is a real numeric-progress datum only when it carries a
 *  finite `progress`. A label-only point has NO magnitude, so the numeric
 *  progress chart must skip it rather than invent a y-value for it
 *  (docs/room-attention.md, decision 6 — no fabricated intermediate state). */
export function hasNumericProgress(progress: number | null | undefined): progress is number {
  return typeof progress === 'number' && Number.isFinite(progress);
}
