---
type: "Reference"
title: "Cross-client design tokens"
description: "Mapping from every Jeliya design-token concept to its React custom property and its Flutter getter, with the shared fixture and the two gates that enforce it."
tags: ["design", "design-system", "tokens", "accessibility", "cross-client"]
timestamp: "2026-07-18T12:00:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "partial"
release_status: "unreleased"
audience: ["client-authors", "contributors", "designers", "maintainers"]
---

# Cross-client design tokens

Jeliya renders the same design system twice: once as CSS custom properties in
the React client and once as Dart getters in the Flutter client. Nothing
mechanical connected the two, so they drifted — and drifted *together*, which
is worse than drifting apart. The message bubble grew an accent gradient in
CSS and a matching pair of gradient tokens in Dart, so each client looked
correct against the other while both contradicted the design record.

This page is the mapping that keeps them honest. [The design
system](../DESIGN.md) stays normative for *why* a token exists; this page says
*where* it lives on each side.

## Source of truth and gates

[`assets/design-tokens.json`](../assets/design-tokens.json) is the shared
fixture: the machine-checkable half of the design system. Colours, alpha
companions, radii, the contrast floors, the shadow vocabulary, and the
gradient ceiling are pinned there as values, not prose.

Two gates read that one file:

- [`scripts/check-design-tokens.mjs`](../scripts/check-design-tokens.mjs)
  checks the React half against
  [`ui/src/styles.css`](../ui/src/styles.css). It asserts that every pinned
  colour is declared with the pinned value, that no `var()` is referenced
  without being declared, and that the absolute rules — the shadow
  vocabulary, no side stripes, one sanctioned accent gradient — are actually
  absolute in the stylesheet. Run `node scripts/check-design-tokens.mjs` from
  the repository root.
- `app/test/design_tokens_test.dart` checks the Flutter half against
  `app/lib/src/theme.dart`, reading the same fixture.

Where an implementation and the design record disagree, one of them is a bug.
Say which in the pull request; do not edit the fixture to match whichever side
you happened to open first.

## Colour mapping

Fixture keys are the names in `assets/design-tokens.json`. The Flutter column
names getters on `JeliyaTokens`, read with `JeliyaTokens.of(context)`.

### Surfaces

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| App ground | `ground` | `--bg` | `bg` |
| Chrome (sidebar, panels, headers, modal) | `chrome` | `--bg-raise` | `bgRaise` |
| Card | `card` | `--bg-card` | `bgCard` |
| Nested surface | `card-nested` | `--bg-card-2` | `bgCard2` |
| Input well | `input-well` | `--bg-input` | `bgInput` |
| Remote message bubble | `bubble-remote` | `--bg-bubble-remote` | `bubbleRemoteBg` |

### Borders

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| Quiet hairline / divider | `border-quiet` | `--border` | `border` |
| Strong border, and selected-state edges | `border-strong` | `--border-strong` | `borderStrong` |
| Control-identifying boundary, 3:1 (WCAG 1.4.11) | `border-interactive` | `--border-interactive` | `borderInteractive` |

### Accent

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| Emerald | `accent` | `--accent` | `accent` |
| Deep emerald (progress fill partner) | `accent-deep` | `--accent-2` | `accent2` |
| Tint fill, 12% | `alpha.dim.accent` | `--accent-dim` | `accentDim` |
| Border line, 40% | `alpha.line.accent` | `--accent-line` | `accentLine` |
| Tinted-button hover, 20% | `alpha.accent-hover` | `--accent-hover` | *(React only)* |
| Tinted-button pressed, 28% | `alpha.accent-active` | `--accent-active` | *(React only)* |
| Brightened emerald for a hovered link | not pinned | `--accent-strong` | *(React only)* |

### Ink

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| Primary ink | `ink` | `--text` | `text` |
| Secondary ink | `ink-dim` | `--text-dim` | `textDim` |
| Small info-bearing ink, AA-audited | `ink-mute` | `--text-mute` | `textMute` |

### Status hues

Every hue carries exactly two companions: a tint and a 40% line. Any third
alpha is drift — the audit found five distinct alphas where two are specified.

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| Degraded | `amber` | `--amber` | `amber` |
| Degraded tint, 12% / line, 40% | `alpha.dim.amber`, `alpha.line.amber` | `--amber-dim`, `--amber-line` | `amberDim`, `amberLine` |
| Failed | `red` | `--red` | `red` |
| Failed tint, 10% / line, 40% | `alpha.dim.red`, `alpha.line.red` | `--red-dim`, `--red-line` | `redDim`, `redLine` |
| Waiting | `blue` | `--blue` | `blue` |
| Waiting tint, 10% / line, 40% | `alpha.dim.blue`, `alpha.line.blue` | `--blue-dim`, `--blue-line` | `blueDim`, `blueLine` |

Status is never colour alone. The hue is half of a token pair whose other half
is a dot and a text label; see [room attention](room-attention.md) for the
vocabulary and the evidence rule behind each label.

### Scrim and elevation

| Concept | Fixture key | React | Flutter |
|---|---|---|---|
| Modal scrim, 72% | `scrim` | composed in the `.modal-backdrop` rule | `modalBarrier` |
| Modal lift | `elevation.modal-lift` | `box-shadow` on the modal | Material dialog elevation |
| Drawer lift, medium shell only | `elevation.drawer-lift` | `box-shadow` on `.right-panel` | inspector drawer decoration |
| Status glow, `0 0 6px` | `elevation.status-glow` | `box-shadow` on status dots | dot decoration |

The fixture's `elevation.allowed` list is exhaustive by design: the React gate
treats any other `box-shadow` as drift.

## Concepts that exist on only one side

Neither list is drift. Each entry is a concept one platform needs and the
other gets from its framework, or composes at the call site.

React only:

- `--accent-strong` — the brightened emerald for a hovered fetched-path link.
  It was referenced before it was declared, so the hover state silently did
  nothing; it is now declared, and the gate fails on any repeat.
- `--z-fleet`, `--z-drawer`, `--z-tabbar`, `--z-modal` — the stacking scale.
  Flutter has no analogue: paint order comes from the widget tree.
- `--vh-full`, `--safe-top`, `--safe-bottom`, `--safe-left`, `--safe-right`,
  `--tabbar-h` — viewport and safe-area mechanics. Flutter reads the
  equivalents from `MediaQuery`.
- `--accent-hover` and `--accent-active` — the two sanctioned tinted-button
  states. Both alphas are pinned in the fixture, so a Flutter equivalent can
  be added without a new decision; Flutter currently composes those states
  from `accentDim` and `accentLine` at the call site instead.

Flutter only:

- `ownTileBorder` — the accent edge on an own event tile.
- `agentCardBg`, `agentCardBorder`, `bannerReconnectBg`,
  `bannerReconnectBorder`, `bannerDisconnectBg`, `errorNoteBg`,
  `errorNoteBorder` — component tints that React composes directly in its
  stylesheet rules rather than naming.
- `toneColor`, `toneBg`, `toneBorder` — the `LabelTone` to hue mapping.
  React applies the same mapping through per-tone chip classes.

Shared concept, different module: the deterministic identity palette and the
file-type tint are `colorForId`, `avatarBg`, `tileBg`, and `fileTint` on
`JeliyaTokens`, and the same-named functions in `ui/src/lib/format.ts`. Both
carry a violet that has no token on either side, because it appears only
inside those two functions. The hash must stay identical across clients or the
same person gets a different avatar colour per device.

## Radii

Radii are pinned as numbers in the fixture and mirrored in Flutter's
`JeliyaRadii`. React has no radius custom properties — the values appear as
literals in the rule that uses them — so the React column here is the value,
not a variable name.

| Fixture key | Value | Flutter | Used for |
|---|---|---|---|
| `tail` | 4 | `bubbleSharp` | The bubble's sharp tail corner |
| `tight` | 7 | `iconBtn` | Icon buttons, skeletons, pipe address chips |
| `sm` | 8 | `btnSm` | Small buttons |
| `control` | 9 | `btn` | Buttons, inputs, 34px square tiles |
| `nav` | 10 | `nav` | Nav items, stat inner cells |
| `tile` | 11 | `row` | Room rows, file and pipe tiles, agent-work cards |
| `card-sm` | 12 | `card` | Profile and settings cards |
| `card` | 13 | `composer` | Composer bar, agent and pipe cards, panel forms |
| `stat` | 14 | `bubble` | Member and file rows, bubble corners, stat tiles |
| `card-lg` | 15 | `hero` | Heroes, fleet cards |
| `surface` | 16 | `modal` | Modal, onboarding card |
| `pill` | 999 | `pill` | All pills, chips, and badges |

The two sides name the same twelve values after different things: the fixture
names the *step*, Flutter names its *first use*. That is survivable because
the numbers are pinned and asserted. Do not rename either set to match the
other without moving every call site in the same change.

## Spacing is not pinned, and that is deliberate

This is a real, recorded disagreement rather than an oversight.

[The design system](../DESIGN.md) names five spacing steps: 4, 8, 12, 18, 24.
Flutter's `JeliyaSpacing` carries ten: 2, 4, 6, 8, 10, 12, 14, 16, 18, 24,
plus three semantic aliases (`panel` = 14, `section` = 18, `page` = 24). The
five design steps are a subset of the ten, so nothing conflicts numerically —
but the intermediates are load-bearing in the Flutter tree, where 14px panel
padding and 6px inline gaps are used throughout.

Pinning either scale in the shared fixture would encode the disagreement
instead of resolving it: pinning five would make the Flutter client
permanently non-conformant against padding it actually ships, and pinning ten
would silently promote intermediate values into the design record without
anyone deciding they belong there. So spacing stays out of the fixture and is
documented here.

The consequence is that spacing is the one part of the token system with no
gate behind it. Treat the five design steps as the default and reach for an
intermediate only when a real layout needs it — and if a *sixth* step turns
out to be genuinely systematic rather than local, promote it in the design
record first.

## Stale reference: `phase3-design.json`

`app/lib/src/theme.dart` cites `phase3-design.json` in its library comment and
in three section headers, and two widget files cite it for layout and motion
rules. That file does not exist anywhere in this repository. It was the
contract used when the Flutter client was first ported from the web client,
and it did not survive into the tree.

Those citations are stale and point at nothing a reader can open. The shared
fixture supersedes them: `assets/design-tokens.json` for values, and
[the design system](../DESIGN.md) for the prose those values serve. Treat any
surviving `phase3-design.json` mention as a comment to correct when you next
touch the surrounding code, not as a document to go looking for.
