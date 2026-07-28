---
type: "Runbook"
title: "Accessibility release checklist"
description: "The screen-reader and keyboard behaviours automated checks cannot prove, verified by hand before a release."
tags: ["accessibility", "release", "qa", "manual"]
timestamp: "2026-07-27T22:58:56Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "unreleased"
audience: ["contributors", "reviewers", "maintainers"]
---

# Accessibility release checklist

CI enforces what a machine can decide: no critical or serious axe violations
across every destination and viewport, no clipped layout at 100/200/320% text
in English and French, a focus indicator that exists, target sizes that clear
their floor, and a token palette both clients agree on.

None of that proves the app is usable with a screen reader. Automated checks
read the accessibility tree; they cannot hear the order things are announced
in, notice that a live region interrupts mid-sentence, or tell that focus
technically moved somewhere useless. This checklist covers exactly that gap —
the residue, not a re-run of the gates.

Work through it on one desktop screen reader and one mobile screen reader
before a release. Record the pass in the release notes with the reader and
version used; a checklist nobody can tell was run is not evidence.

## What CI already covers — do not re-verify by hand

**Amended 2026-07-27 (issue #157).** Every suite listed below belongs to the
retiring React and Flutter stack. The
[Dioxus clean-slate architecture](dioxus-architecture.md) record decides that
both clients are replaced by one Rust client stack; nothing in that record is
built and no suite named here has been removed, so this table stays valid only
while that stack still ships. When a surface moves to the new client, its
automated coverage must be re-established before the matching manual row may
be skipped again — until then that row is verified by hand. No enforcement may
lapse before its replacement is qualified, and what replaces the retired
cross-client design-token, localization, and accessibility gates is not yet
decided. New accessibility results are described as enforced evidence, not
certification.

| Enforced by | Covers |
|---|---|
| `ui/e2e/a11y-matrix.spec.ts` | No critical/serious axe violations, all destinations x 4 viewports |
| `ui/e2e/a11y.spec.ts` | One `main` and one `h1` per destination, landmark names, skip links, target floors |
| `app/test/a11y_semantics_test.dart` | Button/link roles, Enter and Space activation, focus-ring presence |
| `app/test/a11y_text_scale_test.dart` | 100/200/320% text in English and French at 360x640 |
| `app/test/a11y_matrix_test.dart` | Widths 360/899/900/1280, tab traversal, safe-area insets |
| `scripts/check-design-tokens.mjs`, `app/test/design_tokens_test.dart` | Token parity and the contrast floors |

## Screen reader

- [ ] **Reading order matches visual order** on a room's Activity. Follow the
      timeline top to bottom with the virtual cursor and confirm nothing is
      announced out of sequence, especially around the day dividers and folded
      agent runs.
- [ ] **A new message is announced once.** Send from a second device with the
      timeline focused. Confirm one announcement, not one per intervening
      re-render. This is the failure mode a `liveRegion` over a rebuilding list
      produces, and no automated check can hear it.
- [ ] **A connection transition is announced once.** Stop the daemon, wait for
      the banner, restart it. The record allows exactly one live region for
      this (`docs/room-workbench.md`, decision 3) — confirm you hear one
      transition, not a repeat per retry attempt.
- [ ] **Landmark navigation is useful.** Jump by landmark on each destination
      and confirm the names distinguish the panes ("Room rail", "Files
      inspector") rather than announcing two unnamed complementary regions.
- [ ] **The status vocabulary reads as intended.** On Agent Fleet, confirm each
      agent's liveness and last-posted status are announced as two separate
      facts. A "Stale" agent whose last posted label was "Working" must not
      sound like it is working now.
- [ ] **Error copy is the friendly message, not the raw code.** Trigger a fetch
      failure and confirm the announcement leads with the designed sentence;
      the daemon's own code and hint belong in the collapsed technical
      disclosure.

## Keyboard

- [ ] **Skip links work as the first two tab stops** and land focus, not just
      scroll. Tab once from a fresh load, confirm the link becomes visible,
      activate it, and confirm the NEXT Tab continues from the destination.
- [ ] **The focus ring is visible everywhere it lands**, including over the
      inspector drawer at a medium width and against the tinted primary and
      danger buttons.
- [ ] **No focus trap outside a dialog.** Tab all the way around each
      destination and back. Inside a dialog, confirm the trap holds and Escape
      releases it to the control that opened it.
- [ ] **Destructive actions never take initial focus.** Open Leave room and
      press Enter immediately: it must abandon, not confirm.
- [ ] **The room tab strip behaves as a tablist** — arrows move between tools,
      Home and End jump to the ends, and one Tab enters and one leaves.
- [ ] **Nothing is reachable but invisible.** Watch for a focus ring that
      disappears behind the drawer, the composer, the jump-to-latest pill, or
      the bottom tab bar.

## Platform behaviours

- [ ] **OS text size at maximum** on a phone: every primary and Cancel action
      still reachable, by scrolling if necessary, in both English and French.
- [ ] **Reduced motion honoured** at the OS level: the jump-to-latest scroll
      lands instantly rather than animating.
- [ ] **The desktop app is operable with the keyboard alone** from launch —
      including onboarding, which a pointer-only assumption tends to miss
      because it is seen once.

## Known gaps

- **Amended 2026-07-27 (issue #157).** Issue #74 landed on 2026-07-18: the
  web client ships `ui/src/l10n/en.ts` and `fr.ts` under
  `scripts/check-ui-i18n.mjs`, so the bilingual checks apply to both clients
  today, not to the Flutter app alone. Both catalogs and both localization
  gates retire with the clients they test. The
  [Dioxus clean-slate architecture](dioxus-architecture.md) record decides
  that one Rust client stack replaces both, so the bilingual checks must be
  re-homed on the replacement surface before either client's row may be
  dropped; neither that surface nor its localization gate exists yet.
- `ui-e2e` is not currently in the repository's required status checks, so the
  accessibility gate runs on every pull request but does not yet BLOCK a merge.
  Adding it is a branch-protection change, outside any pull request's diff.
