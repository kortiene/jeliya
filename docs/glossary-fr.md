# French localization — glossary & scoping decisions

**Status: agreed before any translation lands.** This file gates the first
French release: translators and reviewers enforce it so terms don't drift
across surfaces. Jeliya targets francophone West Africa first (Mali,
Senegal, Guinea, Côte d'Ivoire); Bambara (bm) is a community aspiration
unlocked after French ships.

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
2. **RTL is out of scope.** French and Bambara (standard Latin orthography)
   are both LTR. Do not add speculative `[dir=rtl]` CSS.
3. **Status labels are an English-token contract.** `labelTone()` in
   `ui/src/lib/format.ts` derives chip/dot tone from known English tokens;
   labels it can't read (any language) render neutral — green is earned,
   never a fallback. The long-term fix is a typed severity field on the
   agent-status protocol event so tone keys off protocol truth, not prose.
4. **Text locale ≠ formatting locale.** Bambara users will run fr-locale
   systems; locale plumbing must let UI strings (bm) and date/number
   formatting (fr) diverge from day one.
5. **Rollout order.** Release 1: Onboarding + room shell (Sidebar,
   RoomHeader, Composer, Timeline, shared widgets) + `format.ts` locale
   plumbing — a francophone user can create, join, and work in a room
   entirely in French. Release 2: RightPanel, InviteModal, FleetDashboard,
   plus a `README.fr.md` quickstart. `docs/agent-guide.md` stays English
   (an API contract, not an onboarding surface).
6. **Bambara feasibility notes.** Standard orthography needs ɛ ɔ ɲ ŋ — the
   sans stack covers them on mainstream platforms; smoke-test the mono stack
   before shipping bm. CLDR bm has a single plural category, which an
   ICU-based catalog handles with no extra work.
