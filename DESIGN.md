---
name: Jeliya
description: Private P2P workspace shell for humans and AI agents — dark teal-black ground, one emerald voice
colors:
  ground: "#070d10"
  chrome: "#0a1116"
  card: "#0e161b"
  card-nested: "#111b21"
  input-well: "#0c1419"
  border-quiet: "#16232a"
  border-strong: "#21343c"
  emerald: "#2fd6a4"
  emerald-deep: "#1fb4a8"
  ink: "#dcebe6"
  ink-dim: "#8aa39d"
  ink-mute: "#7a938c"
  amber: "#f5b453"
  red: "#f26d6d"
  blue: "#6aa8f7"
  scrim: "#030709b8"
  shadow: "#00000080"
typography:
  display:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, 'Helvetica Neue', sans-serif"
    fontSize: "30px"
    fontWeight: 700
    lineHeight: 1.2
  headline:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, 'Helvetica Neue', sans-serif"
    fontSize: "19px"
    fontWeight: 700
    lineHeight: 1.3
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, 'Helvetica Neue', sans-serif"
    fontSize: "13.5px"
    fontWeight: 600
    lineHeight: 1.4
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, 'Helvetica Neue', sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, 'Helvetica Neue', sans-serif"
    fontSize: "11px"
    fontWeight: 700
    lineHeight: 1.2
    letterSpacing: "0.12em"
  mono:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
    fontSize: "0.92em"
rounded:
  tail: "4px"
  tight: "7px"
  sm: "8px"
  control: "9px"
  nav: "10px"
  tile: "11px"
  card-sm: "12px"
  card: "13px"
  stat: "14px"
  card-lg: "15px"
  surface: "16px"
  pill: "999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "18px"
  xl: "24px"
components:
  button:
    backgroundColor: "{colors.card}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-primary:
    backgroundColor: "rgba(47, 214, 164, 0.12)"
    textColor: "{colors.emerald}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-danger:
    backgroundColor: "{colors.card}"
    textColor: "{colors.red}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  input:
    backgroundColor: "{colors.input-well}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 11px"
  chip:
    backgroundColor: "transparent"
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.pill}"
    padding: "2px 8px"
  card:
    backgroundColor: "{colors.card}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "12px 14px"
  modal:
    backgroundColor: "{colors.chrome}"
    textColor: "{colors.ink}"
    rounded: "{rounded.surface}"
    padding: "16px 18px"
---

# Design System: Jeliya

## 1. Overview

**Creative North Star: "The Meeting Tree"**

*Jeliya* is the Manding word for the art of the jeli — the keeper of the
community's true record, who speaks where the village gathers under the
meeting tree — and the interface holds that posture: a quiet, dark room where
a team's work — humans and AI agents together — gathers, stays legible, and
is never quietly rewritten.
The ground is a near-black teal (#070d10), surfaces step up through four
close-set tonal layers, and a single emerald voice (#2fd6a4) marks what is
actively alive: the primary action, the current selection, a healthy peer, a
running agent. Everything else stays calm so the few signals read instantly.

This is a **product** surface: design serves the task. The vocabulary is
earned familiarity — standard buttons, tabs with a 2px underline, pill
chips, dense bordered cards — executed with terminal-adjacent confidence:
mono identifiers, the flat meeting-tree brand mark, hexagonal identity
avatars, truthful status language. The system explicitly rejects generic
SaaS dashboard slop, fake-state UI, the crypto/web3 glow aesthetic, and the
Slack/Discord chat-app skin (see PRODUCT.md anti-references).

**Key Characteristics:**
- Dark teal-black tonal layering; depth by surface steps and 1px borders, not shadows
- One emerald accent reserved for actions, selection, and live/healthy state
- Truthful status hues: amber = degraded, red = failed, blue = waiting — always dot + label, never color alone
- System sans at a fixed px scale; mono strictly for machine truth (ids, addresses, error codes)
- The meeting-tree mark as the brand motif; hexagonal clip avatars as the
  identity signature for people and agents
- Motion is minimal and state-bound; WCAG 2.1 AA is the design target, with
  targeted checks rather than complete automated enforcement or certification

## 2. Colors

A four-step teal-black tonal ramp under one emerald accent and three
truthful status hues.

### Primary
- **Emerald** (#2fd6a4): the one voice. Primary-action text, active nav
  glyph, selected-tab underline, focus ring, live dots, verified checks, the
  P2P badge. Used as tint fills at 12% alpha (`--accent-dim`) and borders at
  40% alpha (`--accent-line`) — never as a solid button fill.
- **Deep Emerald** (#1fb4a8): gradient partner in the progress fill only.

### Secondary
- **Amber** (#f5b453): degraded-but-working — reconnecting, relay path,
  stale agents. Tints via `--amber-dim` (12%), borders via `--amber-line` (40%).
- **Red** (#f26d6d): failed/disconnected — errors, hash mismatch, danger
  buttons. Tints via `--red-dim` (10%), borders via `--red-line` (40%).
- **Blue** (#6aa8f7): waiting/informational — connecting peers, awaiting
  review. Tints via `--blue-dim` (10%), borders via `--blue-line` (40%).

### Neutral
- **Ground** (#070d10): the app body and fleet page.
- **Chrome** (#0a1116): the second neutral layer — sidebar, panels, headers,
  composer strip, modal surface (the product register's sidebar/toolbar layer).
- **Card** (#0e161b): message bubbles, event tiles, agent/file/pipe cards,
  default buttons.
- **Nested surface** (#111b21): the one level allowed above card — file/pipe
  tiles, count badges, input tracks.
- **Input well** (#0c1419): all text inputs, the composer bar.
- **Quiet border** (#16232a) / **Strong border** (#21343c): dividers vs.
  interactive edges.
- **Ink** (#dcebe6) / **Dim ink** (#8aa39d) / **Mute ink** (#7a938c): primary
  text, secondary text, and small info-bearing text. Mute ink is
  contrast-audited in source: ≥4.5:1 on every surface it sits on.

### Named Rules
**The One Emerald Voice Rule.** The accent appears on at most ~10% of any
screen: primary actions, current selection, live/healthy state. Emerald as
decoration is prohibited.

**The Truthful Hue Rule.** A status color may only render a state the signed
event log proves. Status is never conveyed by color alone — always a dot (or
glyph) plus a text label.

## 3. Typography

**Body Font:** system sans stack (-apple-system, BlinkMacSystemFont,
'Segoe UI', Inter, Roboto, 'Helvetica Neue')
**Label/Mono Font:** ui-monospace, SFMono-Regular, Menlo, Consolas

**Character:** One quiet sans carries everything at a fixed px scale —
product-register typography with no display font and no fluid clamps.
Hierarchy is built from size and weight only; mono is a semantic marker, not
a style.

### Hierarchy
- **Display** (700, 26–30px): boot and onboarding headings only.
- **Headline** (700, 19–22px): room title, fleet title, brand name.
- **Title** (600–700, 13.5–16px): names, tabs, modal headings, card titles.
- **Body** (400, 14px/1.5): messages, forms, general UI. Message prose caps
  at 72ch.
- **Label** (700, 11px, 0.06–0.12em tracking, uppercase): the micro-label
  system — section headers (YOUR ROOMS), identity labels, the AGENT chip
  (9.5px). Always in mute ink, which clears AA for its size.
- **Mono** (0.92em of context): identity ids, peer addresses, pipe targets,
  error codes, commands.

### Named Rules
**The Quiet Mono Rule.** Mono marks machine truth only — ids, addresses,
error codes, commands, progress numbers. Mono headings or decorative mono
are prohibited.

## 4. Elevation

Flat by doctrine: depth is tonal layering (ground → chrome → card → nested
surface) plus 1px borders. There are no resting shadows anywhere in the
system. Exactly one component elevates — the modal
(`box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5)` over a 72% scrim with a 3px
functional blur). The 6px color glows on status dots are signal, not depth.

### Shadow Vocabulary
- **Modal lift** (`box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5)`): the modal
  dialog only.
- **Status glow** (`box-shadow: 0 0 6px <hue at 70%>`): live/error/info dots
  and the live pill's dot. Never on containers.

### Named Rules
**The Border-Not-Shadow Rule.** Surfaces separate by tonal step and 1px
border. If a new component seems to need a shadow, it's either a modal or
it's wrong.

## 5. Components

Interactive states are a contract: default, hover, focus-visible (global 2px
emerald ring, offset 2), active (pressed tonal step), and disabled (0.55
opacity) exist on every control; loading renders as skeletons or in-button
spinners with text.

### Buttons
- **Shape:** gently rounded (9px; 8px for small, 7px for icon buttons)
- **Default:** card surface, strong border, ink text; hover shifts border to
  emerald 40%; pressed steps to the nested surface
- **Primary:** emerald tint fill (12%) + emerald 40% border + emerald text —
  never solid emerald; hover deepens to 20%, pressed 28%
- **Ghost:** transparent until hover (border + full ink)
- **Danger:** red text and red 40% border; hover deepens its own hue
  (red tint fill) — a danger control never glows emerald
- **Icon buttons:** 26px square, glyph wrapped `aria-hidden`, named by
  `aria-label`

### Chips (status pills)
- **Style:** 999px pills, 11px text, 1px border; base chip in dim ink
- **Tone system:** hue text + 40% border + ~10% tint background per truthful
  hue (green = live/ok, blue = waiting, red = failed, amber = degraded);
  mapped from event labels, paired with a dot or text — never color alone

### Cards / Containers
- **Corner Style:** 13px (cards), 15px (fleet cards), 16px (modal/onboarding)
- **Background:** card surface on ground/chrome; nested surface for the one
  inner tile level
- **Shadow Strategy:** none — borders and tonal steps (see Elevation)
- **Border:** 1px quiet border; state borders swap to hue lines (live =
  emerald, warn = amber)
- **Internal Padding:** 12–15px
- **Receding:** off/closed items mute their ground and dim graphic marks
  only — never blanket opacity over text (AA floor, documented in source)

### Inputs / Fields
- **Style:** input well background, strong border, 9px radius, 8px 11px
  padding; placeholders in mute ink (AA-clearing)
- **Focus:** border to emerald 40% on focus; unmistakable 2px solid emerald
  outline on keyboard focus (:focus-visible)
- **Disabled:** 0.55 opacity; selects draw their own chevron (appearance:
  none is always paired with a replacement affordance)

### Navigation
- **Sidebar:** chrome layer; items are full-width rounded (10px) rows in dim
  ink; hover = card surface + ink; active = emerald tint + emerald line +
  emerald glyph
- **Panel tabs:** bare buttons with a 2px transparent underline; the emerald
  underline is the single active-tab affordance; count badges as nested pills
- **Mobile tab bar:** 58px-minimum bottom bar — 58px is the base height, not
  a cap: the bar grows with the OS accessibility font scale rather than clamp
  or clip its labels — five glyph+label tabs, active = emerald text,
  `aria-current="page"`

### Identity Marks (signature)
The brand mark is the flat single-accent meeting tree (`TreeMark` — see
"Brand mark & wordmark" below); it carries no gradient. Clip-path hexagonal
avatars tinted by a 6-color identity hash at ~15% alpha remain the identity
signature for people and agents — flat clips, never glowing, never gradient.
Nothing else in the system is hexagonal; the progress fill is the only
sanctioned accent gradient (faint tonal tint washes on card surfaces are a
separate, quieter device).

## 6. Do's and Don'ts

### Do:
- **Do** keep the emerald voice under ~10% of any screen — actions,
  selection, live state only.
- **Do** pair every status color with a dot/glyph and a text label; render
  only states the event log proves (typed failures like `unavailable` /
  `unauthorized` / `hash_mismatch` get designed presentations).
- **Do** hold the AA floor: ≥4.5:1 for body and small info-bearing text on
  its actual surface — recede via muted grounds and dimmed marks, never
  blanket opacity over text.
- **Do** ship every interactive state: default, hover, focus-visible, active,
  disabled — plus honest loading (static skeletons that mirror the real
  anatomy) and empty states that teach the feature.
- **Do** use the token system (`--*-dim` / `--*-line` companions, the z-index
  scale) instead of raw rgba/z literals.
- **Do** honor `prefers-reduced-motion` for any animation you add; keep
  transitions in the 150–250ms band, easing out.

### Don't:
- **Don't** produce "generic SaaS dashboard slop" (PRODUCT.md): cream/light
  defaults, purple-blue gradients, identical KPI card grids, hero-metric
  tiles.
- **Don't** build "fake-state UI" (PRODUCT.md): optimistic delivered checks,
  spinners implying progress that isn't happening, silent partial fetches.
- **Don't** drift toward the "crypto/web3 aesthetic" (PRODUCT.md):
  glow-everything, gradient text, neon hexagons — the TreeMark brand mark is
  a flat single-accent stroke, the progress fill is the only accent gradient,
  and dots are the only glow.
- **Don't** skin it like a chat app (PRODUCT.md): agents, files, and pipes
  are first-class panes, not bolted-on integrations.
- **Don't** nest bordered cards. One card level; inner tiles sit on the
  nested surface inside flat rows.
- **Don't** use side-stripe borders (>1px colored border-left/right as an
  accent), decorative glassmorphism (the modal scrim's 3px blur is the only
  backdrop-filter), or display/mono fonts in UI labels.
- **Don't** let unbroken tokens (64-hex ids, URLs, paths) overflow: every
  surface that renders user or agent text carries truncation or
  overflow-wrap.

## Brand mark & wordmark

- **Mark — the meeting tree.** Canopy arc + trunk + three gathered peer-dots
  (the community gathered under the village tree where the jeli speaks —
  the team, humans and agents, keeping one true record), drawn flat in the single
  accent `#2fd6a4`: round caps, no gradients, no glow, no hexagons. One
  source of truth in the app — `TreeMark` in `ui/src/components/ui.tsx` —
  with static twins in `ui/public/favicon.svg` (16px variant: canopy +
  trunk + one dot) and the icon set in `ui/public/`.
- **Wordmark.** "Jeliya" in the display stack, weight 700, letter-spacing
  0.01em, color ink (`--text`) — **never emerald**: the mark carries the
  accent, and a green wordmark would spend signal on decoration. In the app,
  always via `Wordmark` (`ui/src/components/ui.tsx`); size comes from the
  surrounding context class.
- **Lockups.** Canonical static lockups live in `assets/banner.svg` (README)
  and `assets/og.png` (social preview); reuse them rather than re-deriving
  spacing per surface.
- **Asset a11y receipts.** All text in static assets sits on `#070d10`:
  ink 15.9:1, text-dim 7.3:1, accent 10.5:1 (AA floor is 4.5:1). Every
  README image carries descriptive alt text. Keep both bars for any new
  asset.
- **Mockups are layout reference only** (`mockups/README.md`): their
  cube/hexagon logo and multi-hue glows predate this identity and are
  anti-references for chrome.
