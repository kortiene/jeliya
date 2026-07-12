---
type: "Glossary"
title: "French localization — glossary & scoping decisions"
description: "Canonical French terminology and localization decisions for Jeliya product surfaces."
tags: ["french", "i18n", "localization", "terminology"]
timestamp: "2026-07-11T21:27:07Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "unreleased"
audience: ["contributors", "reviewers", "translators"]
---

# French localization — glossary & scoping decisions

**Status: agreed before any translation landed.** This file gated the first
French release: translators and reviewers enforce it so terms don't drift
across surfaces. Jeliya targets francophone West Africa first (Mali,
Senegal, Guinea, Côte d'Ivoire); Bambara (bm) is the community aspiration
unlocked now that the French catalog has landed.

This is a contract for translators and reviewers — it is not a live,
end-user-facing glossary. It did its gating work: PR #12 (commit `fdb4c97`,
merged 2026-07-09) shipped the full French catalog —
`app/lib/src/l10n/arb/app_fr.arb` translates all 444/444 keys of
`app_en.arb` (gen-l10n's untranslated-messages report is empty) — plus the
French macOS menu (`app/macos/Runner/fr.lproj`); mechanics in
`docs/i18n.md`. Still open: the user-facing discoverability pointer (a
`README.fr.md`, or a "Voir aussi" link from the main README) does not exist
yet — until one lands, francophone users have no way to find this page.

## Tier 1 — communal vocabulary: translate

Everyday nouns rendered as prose. First-pass equivalents (translators may
refine, but consistently):

| English | French |
|---|---|
| room | salon |
| member(s) | membre(s) |
| files | fichiers |
| invite / invitation | inviter / invitation |
| share | partager |
| join | rejoindre |
| create | créer |
| settings | réglages |
| Your Rooms | Vos salons |
| ticket | ticket |
| agent | agent |

## Tier 2 — protocol truth tokens: never translate

These are grep-able wire identifiers rendered in mono/code style. Translate
the sentence and the hint around them; keep the token verbatim:

- `direct` / `relay` connection badges — rendered exactly as reported by the
  daemon (honesty rule).
- Error codes `unavailable`, `unauthorized`, `hash_mismatch`.
- `daemon`, `jeliyad`, endpoint and identity ids.
- `pipe` — the stated audience (technical operators) knows the Unix term;
  « tuyau » may appear in explanatory prose, never as the token.

## Tier 3 — the brand: a told story

*Jeliya* is the Manding word for the art of the jeli (in French, the
**djéli** or griot) — the hereditary keeper of the community's true record.
In the target market the jeli is universally understood, and the concept
maps directly onto what the product does: a tamper-evident log nobody can
quietly rewrite. The name is an asset, not a problem; it just has to be
told. The French onboarding carries one quiet line of dim prose under the
wordmark (no badge, no animation):

> Jeliya — l'art du djéli, gardien de la mémoire vraie.

## Recorded scoping decisions

1. **Daemon/CLI output stays English.** Operators and agents grep logs and
   search error text; translated diagnostics are a support liability. Daemon
   errors already reach the UI as structured `{code, message, hint}`, so the
   UI can translate the message around a frozen code token without the
   daemon ever localizing.
2. **RTL is out of scope for French/Bambara.** Both use LTR Latin
   orthography — add no speculative RTL layout work. (N'Ko, if it ships
   later, is RTL and gets its own groundwork phase.)
3. **Status labels are an English-token contract.** `labelTone()` in
   `dart/jeliya_protocol/lib/src/conventions/format.dart` (normative in
   `docs/PROTOCOL.md`) derives chip/dot tone from known English tokens;
   labels it can't read (any language) render neutral — green is earned,
   never a fallback. The long-term fix is a typed severity field on the
   agent-status protocol event so tone keys off protocol truth, not prose.
4. **Text locale ≠ formatting locale.** Bambara users will run fr-locale
   systems; locale plumbing must let UI strings (bm) and date/number
   formatting (fr) diverge from day one. (The seam today is
   `app/lib/src/format.dart` — every display formatter lives there.)
5. **Rollout scope.** Superseded 2026-07-08: the desktop app's permanent
   three-column layout shows RightPanel/Settings/Fleet beside the timeline,
   so a partial (release-1) translation would ship a mixed-language window.
   French ships **full-catalog** in one release, at desktop launch
   (`app/lib/src/l10n/arb/app_en.arb` is the complete inventory — one key
   per user-visible string, each with a translator `@description`).
   `docs/agent-guide.md` stays English (an API contract, not an onboarding
   surface); a `README.fr.md` quickstart follows the app release.
6. **Bambara feasibility notes.** Standard orthography needs ɛ ɔ ɲ ŋ — the
   sans stack covers them on mainstream platforms; smoke-test the mono stack
   before shipping bm. CLDR bm has a single plural category, which an
   ICU-based catalog handles with no extra work.
7. **Typographie française (settled 2026-07-09, before the first string —
   the docs/i18n.md step-4 precondition).**
   - **Espaces insécables** : espace fine insécable U+202F before `;` `!`
     `?` and inside guillemets (« texte ») ; espace insécable U+00A0 before
     `:`. Never a breaking space before high punctuation.
   - **Apostrophe typographique** U+2019 (l’identité), **ellipse** U+2026
     (…) — both already the EN catalog's norm.
   - **Guillemets « »** (with the U+202F inner spaces) wherever EN uses
     curly quotes “ ”.
   - **Sentence case** (« Créer un salon », never Title Case), accents kept
     on capitals (À propos, États). Traditional orthography (no 1990
     rectifications).
   - **Vouvoiement**, calm and concrete — the app's honest register
     (green-is-earned) carries into French; no exclamatory marketing tone.
   - **Octets** for byte units: o, Ko, Mo, Go (decision 4's accepted
     deviation: unit WORDS follow the text locale). Percent renders
     « 42 % » (U+202F before %).
   - **Tier 3 placement**: the onboarding tagline slot (the one dim line
     under the wordmark) carries the brand story in French —
     « Jeliya — l’art du djéli, gardien de la mémoire vraie. »
