---
type: "Research"
title: "Production deployment architecture review"
description: "Adversarially verified three-track review of the production deployment proposal, recording every surviving finding, every refuted finding, and every claim checked and found accurate."
tags: ["architecture", "deployment", "review", "verification", "security", "evidence"]
timestamp: "2026-07-19T00:08:29Z"
status: "canonical"
implementation_status: "not-applicable"
verification_status: "partial"
release_status: "not-applicable"
audience: ["contributors", "maintainers", "security-reviewers", "release-engineers", "product"]
---

# Production deployment architecture review

This page is the evidence record for the technical review of
[Production deployment architecture](production-deployment.md). It records every finding
that survived adversarial verification, every finding that was refuted and dropped, and
every claim that was checked and found accurate.

The review informs the architecture decision. It does not enact it. The proposal remains
a proposal until an accepted decision record supersedes it, and this page does not change
any status on that page.

## Scope and method

The review ran as a fan-out of 12 independent investigators over three tracks, followed by
a three-lens adversarial verification pass over every finding above low severity. The three
lenses were accuracy of the cited evidence, correctness of the finding's classification, and
materiality to the decision the proposal asks for. A finding was dropped when two or more of
its three verifiers voted to refute it.

| Track | Question |
|---|---|
| `T1-Repo` | Do the proposal's repository-grounded claims hold against the actual tree? |
| `T2-External` | Do its platform, pricing, and upstream claims hold against live sources? |
| `T3-Judgment` | Does the architecture decision, security design, gate set, and estimate hold? |

| Outcome | Count |
|---|---:|
| Claims checked and found accurate | 138 |
| Findings that survived verification | 78 |
| Findings refuted and dropped | 47 |
| Low-severity findings passed through unverified | 21 |
| High severity | 12 |
| Medium severity | 34 |
| Low severity | 32 |

Totals were produced by 387 agents over roughly 58 minutes with no agent errors. The
underlying run is not reproducible from this repository; this page is the record of it.

## Verdict

The proposal's frontmatter is honest. Its `status: "proposal"` and
`verification_status: "partial"` do not overclaim, and several of its self-limiting
statements proved to be stricter than they needed to be. The factual core is unusually well
grounded: its commit deltas, test counts, dependency pin, and its two accusations against
other pages in this wiki all reproduce exactly.

The recommendation is to accept the page as the basis for an architecture decision record,
subject to six amendments. The findings below are the evidence for those amendments.

One finding changes what the first phase does. Upstream issues 121 and 119, which the
proposal treats as unresolved risks requiring Jeliya-side mitigation, were both closed
upstream two days before the proposal was written. The corresponding work is a dependency
repin and requalification, not new mitigation.

## How to read this ledger

Findings are grouped by verified severity, then by kind. The five kinds are kept separate
throughout because they call for different responses:

| Kind | Meaning |
|---|---|
| `WRONG` | The proposal states something the evidence contradicts. |
| `INTERNALLY-INCONSISTENT` | Two parts of the proposal cannot both be true. |
| `UNVERIFIABLE` | The claim could not be checked; it is neither upheld nor refuted. |
| `DISAGREE` | The claim is accurate; the reviewer disputes the judgment built on it. |
| `MISSING` | The proposal omits a surface, threat, failure mode, or cost. |

A verdict of `CONFIRMED` means all three verifiers upheld the finding. `PLAUSIBLE` means one
verifier voted to refute and was outvoted; the dissent is reproduced under those findings so
a reader can weigh it. Claims, evidence, and proposed fixes are reproduced verbatim from the
verification run and are not edited for tone or length.

## High-severity findings

12 findings.

### F1. Wrong at lines 390-391

- Kind: `WRONG` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: "Fix upstream issue #121 or suspend normal room fanout while an unproven provisional connection exists." — prescribed as work Jeliya must do (invite/join hardening list).

Evidence: Issue #121 is CLOSED. `gh issue view 121 --repo kortiene/iroh-room --json state,stateReason,closedAt,title` → {"state":"CLOSED","stateReason":"COMPLETED","closedAt":"2026-07-16T14:05:35Z","title":"[NET] Outbound fan-out and handshake are not gated for unproven provisional peers (#112 residual)"}. Closed by merge commit 58aca4ba93c35584810d15c160c2936851121666 (issue timeline `referenced` event, 2026-07-16T14:05:35Z) = PR #125 "fix(net): defer engine handshake for unproven provisional peers (#121)", merged 2026-07-16T14:05:34Z, touching crates/iroh-rooms-net/src/node.rs, src/transport.rs, tests/join_e2e.rs. `gh api repos/kortiene/iroh-room/compare/71fbb500...58aca4ba` → {"status":"ahead","ahead_by":8}: the fix is 8 commits AFTER the pinned rev. The doc's own timestamp is 2026-07-18T20:29:18Z (line 6), two days after the fix landed.

Proposed fix: Replace with: "Repin `iroh-rooms` past upstream commit 58aca4ba (PR #125, 2026-07-16), which defers `engine.on_connect` for a provisional peer until its capability proof verifies, and re-run join/isolation qualification at the new rev. The pinned rev 71fbb500 (tag v0.1.0-rc.3) predates the fix, so an application-level fanout suspension is only needed if the repin is deferred."

### F2. Missing at lines 424-439 (bullet at 431-433); citation at 1078

- Kind: `MISSING` | Severity: `high` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `browser-platform-facts`

Claim under review: "Request `navigator.storage.persist()`... Persistent storage reduces automatic eviction but does not prevent user deletion." The browser-peer storage section never mentions Safari/WebKit's seven-day eviction of script-writable storage for origins without user interaction.

Evidence: The omission is on the exact MDN page the doc cites at line 1078. https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria states verbatim: "Safari proactively evicts data when cross-site tracking prevention is turned on. If an origin has no user interaction, such as click or tap, in the last seven days of browser use, its data created from script will be deleted. Cookies set by server are exempt from this eviction." Confirmed at the primary source, https://webkit.org/blog/10218/full-third-party-cookie-blocking-and-more/ : "deleting all of a website's script-writable storage after seven days of Safari use without user interaction on the site", affecting "Indexed DB, LocalStorage, Media keys, SessionStorage, Service Worker registrations and cache" — i.e. every storage tier the doc names at lines 426-430. Same source gives the mitigation the doc also fails to state: "Web applications added to the home screen are not part of Safari and thus have their own counter of days of use. Their days of use will match actual use of the web application which resets the timer. We do not expect the first-party in such a web application to have its website data deleted."

Proposed fix: Add to the browser-peer bullets: "On WebKit (all Safari, and every browser on iOS), script-writable storage — IndexedDB, Cache Storage, and service-worker registrations — is deleted after seven days of browser use without user interaction with the origin. Browser-peer mode on iOS is therefore only durable for home-screen-installed PWAs, which run on their own use counter. Non-installed Safari tabs must be treated as companion mode only." Also promote this into open question 9 (line 1070), which currently says only "PWA storage behavior across real Safari/iOS" without naming the known policy.

### F3. Missing at lines 830-859, 864-996

- Kind: `MISSING` | Severity: `high` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: The roadmap and change map omit localization of all new UI.

Evidence:

````text
`grep -n -i -E 'i18n|localiz|translat|french' docs/production-deployment.md` returns ZERO relevant hits. The repository ships EN/FR parity as a CI-enforced gate: `.github/workflows/ci.yml:81-82` runs `node --test scripts/check-ui-i18n.test.mjs` and `node scripts/check-ui-i18n.mjs`, whose rules include `No French value byte-identical to its English counterpart` and `French typography (docs/glossary-fr.md, decision 7). U+202F before ; ! ? %, U+00A0 before :, U+2019 for the apostrophe, U+2026 for the ellipsis, guillemets rather than double quotes` (scripts/check-ui-i18n.mjs header, rules 3-4). `ui/src/l10n/` contains en.ts and fr.ts. Every string in the new pairing, invite, recovery, quota, and offline surfaces therefore needs a French counterpart with correct typography — real translation work, not a mechanical step. SECOND, SHARPER PROBLEM: the literal-scan that catches untranslated copy is scoped to `const LITERAL_SCAN_ROOTS = Object.freeze(['ui/src/App.tsx', 'ui/src/components'])` (scripts/check-ui-i18n.mjs:67). The proposal places user-facing UI at `ui/src/pairing/` (line 840), `ui/src/invites/` (line 841), and `ui/src/storage/` (line 839) — all OUTSIDE that scan root. Copy written there would ship untranslated with a green CI.
````

Proposed fix: Add to the change map a row for `scripts/check-ui-i18n.mjs`: "extend LITERAL_SCAN_ROOTS to cover ui/src/pairing, ui/src/invites, ui/src/storage, and ui/src/runtime." Add a Phase 2 deliverable "EN/FR catalog entries for all pairing, invite, recovery, and quota copy" and budget 1-2 person-weeks including translation review. Note that the copy-policing gates at lines 974 and 995-996 must then be evaluated against BOTH catalogs.

### F4. Missing at lines 692-693; PR/promotion gates 643-684

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Version skew between the auto-updating web origin and the manually-updated native companion is left to a one-line kill switch with no upgrade UX and no estimate of stranded users.

Evidence: Line 692-693 is the entire treatment: "The companion can enforce a minimum-safe control-protocol version, but the web origin cannot rewrite native state or silently elevate scopes." `grep -c -i` over the doc = 0 for: "version skew", skew, "upgrade prompt", strand, "not running", "companion is not". The allowed telemetry list at 717-723 includes "frontend build and runtime version" but NOT companion version, so the fraction of stranded users would not even be measurable. The repo has a normative precedent the doc never cites: docs/PROTOCOL.md:121-126 defines an adopt-vs-respawn rule that only works for a same-host sidecar the client can restart, and docs/known-gaps-roadmap.md:71-73 records the real-world consequence — "mixed pre/post-repin fleets cannot complete joins, so joiners and admins must upgrade together."

Proposed fix: Add a "Companion update and version skew" subsection specifying: the companion's auto-update channel (or explicit decision that it has none), the in-browser upgrade prompt shown when the origin is newer than the paired companion, the grace window before minimum-safe enforcement hard-fails, and a companion-version bucket in the allowed metrics at 717-723 so the stranded fraction can be measured before enforcement is turned on.

### F5. Missing at lines 785-796, esp. 795

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The abuse-controls list (lines 785-796) contains "user block/report tools" as its only user-safety item and defines no recipient, triage owner, or legal reporting duty.

Evidence: Line 795 is the only report-related bullet: "user block/report tools with explicit content disclosure when reporting". `grep -n -i report` on the doc returns exactly 4 hits (379, 381, 745, 795) — the other three are "error reporting", "crash reports", "CSP reports". `grep -c -i` = 0 for: moderation, moderator, "trust and safety", appeal, spam, "abuse@", contact, CSAM, "law enforcement". Critically, the doc's own architecture states the takedown limit but never draws the legal conclusion: line 285 "Signatures prevent forgery; they do not prevent an authorized peer from copying content" and line 362 "It cannot recall material already received."

Proposed fix: Add an "Abuse reporting and legal obligations" subsection under 785-796 specifying: where a report goes (a published abuse contact), who triages it and within what time, the mandatory CSAM reporting posture for a messaging product, a DMCA designated agent, an EU DSA notice-and-action path if the EU relay is used, and — explicitly — a statement that the P2P architecture makes content takedown impossible so that the legal posture is designed around that, not surprised by it.

### F6. Missing at lines 837-841 (new frontend areas), 1000-1015 (vertical slice), 643-658 (PR gates)

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The entire frontend plan — service worker, pairing UI, recovery UI, eviction UI, storage warnings — never mentions accessibility or internationalization once, in a repo that just shipped EN/FR parity and an enforced a11y CI matrix.

Evidence: `grep -c -i` over the doc = 0 for every one of: accessib, a11y, WCAG, "screen reader", keyboard, axe, contrast, aria, i18n, l10n, localization, translat, locale, French, RTL, "text scale", "reduced motion". The single "language" hit and single "focus" hit are unrelated. Meanwhile the doc adds five new frontend areas at lines 837-841 (`ui/src/sw.ts`, `ui/src/runtime/`, `ui/src/storage/`, `ui/src/pairing/`, `ui/src/invites/`) plus recovery UI, re-pair UI, SAS confirmation and quota warnings in the slice (lines 1004, 1005, 1013, 683). The repo's existing investment: `grep -cE "^\s+[a-zA-Z][a-zA-Z0-9]*(\??):" ui/src/l10n/catalog.ts` = 504 catalog members; docs/i18n.md:149-150 records 444/444 ARB messages with "untranslated_messages.json empty"; docs/accessibility-checklist.md:35-40 lists six enforced CI gates. The doc's own PR gate list at 643-658 enumerates twelve NEW gates (Wasm, CSP, Trusted Types, storage eviction, fuzzing, SBOM…) and includes neither an a11y nor an i18n gate.

Proposed fix: Add "every new user-visible surface enters `ui/src/l10n/catalog.ts` in both `en.ts` and `fr.ts`" and "every new destination enters `ui/e2e/a11y-matrix.spec.ts`" to the PR gate list at 643-658; add a11y/i18n sign-off to the Phase 2 gate (922-930), which is where the recovery and re-pair UIs land; link docs/accessibility-checklist.md and docs/i18n.md from the document.

### F7. Missing at lines whole document; relay in Europe at 535-536; retention at 739-748

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The document plans a public messaging product at app.jeliya.ai with an EU relay (lines 535-536) and 72-hour log retention (line 743), but contains no legal or regulatory framing of any kind.

Evidence:

````text
`grep -c -i` over docs/production-deployment.md returns 0 for each of: GDPR, "privacy policy", "terms of service", DMCA, "data subject", consent, cookie, legal, jurisdiction, lawful, subpoena, "law enforcement", "Digital Services", "Online Safety", "export control", CSAM, minor. The 6 hits for "controller" are all "browser controller"/"web controller" (lines 281, 295, 917, 924, 928, 1065) and the single "processor" hit is "social-preview processors" (line 383) — no data-protection sense anywhere. Repo-wide: `grep -rn -il "privacy policy|terms of service|gdpr|dmca|data protection" --include=*.md --include=*.ts --include=*.tsx --include=*.dart .` returns ZERO files. Doc line 535-536: "Start with two dedicated managed relays, one in North America and one in Europe."
````

Proposed fix: Add a "Legal and compliance" section and a Phase 0 go/no-go gate item: publish a privacy policy and terms of service at app.jeliya.ai before the origin serves anything; name the legal entity behind app.jeliya.ai; record the lawful basis for the 72h security-log retention at line 743; state the data-residency consequence of the EU relay.

### F8. Missing at lines 1017-1026, 336

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: The first slice "explicitly excludes: ... files; pipes; agents" and "file access, pipes ... require separate approval."

Evidence:

````text
Excluded-from-the-slice is not the same as absent-from-the-binary, and one of the excluded methods has an unconfined filesystem sink. `file.share` is confined — crates/jeliya-core/src/supervisor.rs:291-300 (`assert_shareable_path`) rejects anything outside the data dir. But `file.fetch`'s destination is NOT: crates/jeliya-core/src/supervisor.rs:1981 reads `let dir = save_dir.map_or_else(|| self.data_dir.join(DOWNLOADS_DIR), PathBuf::from);` followed by `std::fs::create_dir_all(&dir)` — an arbitrary caller-supplied absolute path, with no confinement check of any kind. `sanitize_name` (supervisor.rs:3152-3161) guards only the FILENAME against traversal, not the directory. So a single bug in a runtime scope allowlist turns `file.fetch` into: create any directory the companion process can create, and write a new file with attacker-chosen bytes (a blob the attacker shared into a room) and a mostly attacker-chosen name into it — e.g. `~/.config/autostart/x.desktop` or `~/Library/LaunchAgents/`, i.e. local code execution as the user. The companion holds the identity keys, so this is the highest-value process in the design.
````

Proposed fix: Two changes. (1) In the slice-exclusion section, state that excluded surfaces are removed at COMPILE TIME from the companion binary via cargo features (see the next finding), not gated at runtime. (2) Independently, add to the repository change map (line 844) that `supervisor.rs:1981` must confine `save_dir` the way `assert_shareable_path` confines share paths, before any non-loopback transport reaches the engine.

### F9. Missing at lines 243, 417, 840

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "paired control key" held in the browser; "Treat cache or pairing-key eviction as a re-pair and resync event"; `ui/src/pairing/` holds "Browser control keys".

Evidence: The doc mandates non-extractability for the browser IDENTITY key — line 317-318: "prefer a nonextractable WebCrypto Ed25519 key ... Otherwise wrap the seed with a nonextractable WebCrypto key" — and then says nothing at all about the extractability of the browser CONTROL key, which is the key that actually authorizes the companion to sign on the user's behalf in slice 1 (the identity key does not even exist in slice 1; line 1019 excludes "a browser-owned room identity"). If the control key is extractable, a CDN compromise exfiltrates it once, and the attacker then drives the companion from their own client, off-origin, indefinitely — surviving the 15-minute frontend rollback (line 688), the CSP, and the service-worker update. The doc's mitigation list at line 708 does include "browser-control-key revocation", but that requires a human to notice and act; nothing bounds the window, because line 337's "expire" carries no duration.

Proposed fix: Add to the pairing section: "The browser control key is generated as a nonextractable WebCrypto CryptoKeyPair and persisted as a CryptoKey handle in IndexedDB. The private key MUST NOT be exportable, so an origin compromise yields use of the key only while the attacker controls the origin, not a portable credential." Add a maximum control-key lifetime (e.g. 30 days) to line 337 and a Phase 2 gate that `exportKey`/`wrapKey` on the control private key throws.

Dissent from the verification panel:

````text
REFUTE (high): The finding's literal core is TRUE but its classification (MISSING) and severity (critical) do not survive the materiality lens.

VERIFIED TRUE: `grep -n -i "extractab" docs/production-deployment.md` returns only lines 316, 318, 1053, 1085 — all in the identity/signing-key context. Lines 322-337 (Browser-to-companion pairing) are indeed silent on control-key extractability, and line 337's "expire" carries no numeric duration (line 386's "30-minute expiry for live pairing" is scoped to invite tickets, not control keys). Line 1019 does exclude "a browser-owned room identity" from slice 1, so the investigator is right that the control key is the only browser key in slice 1.

WHY IT COLLAPSES ANYWAY:

1. The doc explicitly flags this exact threat in its own unknowns section. Line 1064-1065, highest-risk unknown #4: "Browser-origin/CDN compromise and the maximum authority granted to a web controller." That is a near-verbatim restatement of the finding. Per the lens instruction, a finding the doc already flags is much weaker.

2. The doc already states the residual the finding claims is unaddressed. Line 319-320: "Treat browser key protection as at-rest defense. A malicious same-origin script can still invoke a usable key and may observe active memory." The doc's own model says nonextractability does NOT prevent an origin compromise from using the key. The proposed fix therefore buys portability/persistence resistance only — real, but a narrower delta than "critical" implies.

3. The lifecycle is gated, not omitted. Line 910 (Phase 1 go/no-go): "independent security review approves the wire formats and key lifecycle." Line 909: "replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed." Line 924: "1,000 automated pairing/revocation cycles accept no unauthorized controller." Line 928: "a malicious controller cannot invoke files, pipes, agents, or identity reset."

4. It is routed to an ADR by name. Line 1049 ADR #2 "Companion control protocol and pairing transcript"; line 1053 ADR #6 "Browser signing strategy: nonextractable WebCrypto signer or wrapped Wasm seed." Key-material handling is exactly the decision class the doc defers to ADRs.

5. Blast radius is bounded by design. Line 334: "Default scopes cover selected-room reads and idempotent chat sends only." Line 336 requires separate approval for invite creation, file access, pipes, identity operations, agents. Line 295 forbids the companion from accepting an unpaired controller. A stolen slice-1 control key yields scoped chat read/send against selected rooms, not identity takeover — the investigator's "drives the companion indefinitely" overstates authority.

6. The origin-compromise boundary is an accepted, stated assumption, not an oversight. Line 275-277 (TB1): "A compromised origin, CDN account, or frontend dependency controls the browser session. CSP reduces injection risk but cannot make a deliberately malicious first-party build trustworthy." Line 1043-1044: "The product accepts that a hosted first-party origin can observe the content it renders and actions within its granted scope."

7. Revocation is present throughout: line 337, line 681 (production smoke test "companion pairing and control-key revocation"), line 708 (incident response "browser-control-key revocation"), line 1013 (slice-1 deliverable "control-key revocation").

WHAT SURVIVES: a genuine, cheap, literal drafting asymmetry — the doc mandates nonextractability for the key that does not exist in slice 1 (identity, 316-318) and says nothing for the key that does (control, 322-337), and gives no max control-key lifetime at 337 while it does give numerics for invites at 386. Worth one sentence in the review as a specificity/hardening note against the pairing section, at LOW. It does not change the approve/reject decision the doc asks for, so it is not critical and should not be classified MISSING in the sense of an unconsidered gap. Not dropped entirely because the fix is concrete, checkable, and correctly placed.

Repo check: `ls ui/src/` returns App.tsx, components, l10n, lib, main.tsx, styles.css, vite-env.d.ts — `ui/src/pairing/` does not exist, confirming line 840 is purely proposed future work with no implementation to audit. This further reduces materiality: there is no shipped extractable key here, only an unwritten spec line.
````

### F10. Missing at lines 334-336

- Kind: `MISSING` | Severity: `high` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "Default scopes cover selected-room reads and idempotent chat sends only. Invite creation, file access, pipes, identity operations, and agents require separate approval."

Evidence: Invite REDEMPTION (`room.join`) appears in neither the default-scope list nor the separate-approval list, yet it is explicitly IN the first production slice (line 1006 "identity-bound fragment invitations", line 1007 "join and text chat") and is the entire point of the /join route (line 371). This is the confused deputy: the browser names a ticket, and the companion redeems it with the ROOT identity's authority. In the code, `room.join` is deliberately exempt from the room-access preflight — crates/jeliya-core/src/engine.rs:45-46: "`room.join` is intentionally absent: its authorization object is the key-bound ticket, and the caller is not a room member until redemption succeeds" — so there is no existing guard to inherit. Identity binding (line 393) does NOT close this: it stops ticket theft, not attacker-CHOSEN rooms. An attacker who controls app.jeliya.ai already knows the victim's identity_id (it is returned by `daemon.status` and `room.members`), and the doc's own onboarding flow at lines 395-397 has invitees publish "a public identity request". So the attacker mints a valid identity-bound ticket into a room they own and feeds it to the paired companion. Consequences: a signed `member.joined` authored by the victim's device key; the victim's endpoint dialed by attacker peers (IP disclosure, which the doc treats as sensitive at line 546); and per lines 165-166/390 an open join window leaks live fanout to unproven dialers (upstream #121).

Proposed fix: Add `room.join`/invite redemption to line 335's separate-approval list, and require that the companion's approval prompt display the room name, the room id, and the identity of the inviter resolved BY THE COMPANION from the ticket — never text supplied by the browser. Add a Phase 2 gate: "a malicious controller cannot cause the identity to join a room the user did not approve on the companion's own UI."

### F11. Missing at lines 688, 690, 699, 947, 606-613

- Kind: `MISSING` | Severity: `high` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: TB1 mitigations: "The CDN deployment pointer returns to immutable N-1 within 15 minutes" plus "Component metadata has an independent signed kill switch" and the runbook for "malicious frontend or CDN credential compromise".

Evidence: The 15-minute rollback SLO bounds when clean bytes are AVAILABLE, not when hostile code stops RUNNING, and the doc conflates them. A malicious service worker survives the pointer flip: it keeps serving its own cached shell until the browser fetches a byte-different worker script, and the update check is bounded only by the normative 24-hour staleness cap (registration is stale once 86400s have passed since the last check, after which the fetch bypasses the HTTP cache; Chrome 68+ bypasses the HTTP cache for the top-level worker script on every update check — https://developer.chrome.com/blog/fresher-sw). An installed PWA that is not navigated may not re-check for a full day. The kill switch at line 690 is scoped to component metadata only; there is none for the web shell. Critically, the one header that actually forces unregistration is absent from the header list at lines 606-613: per MDN, `Clear-Site-Data: "storage"` executes `ServiceWorkerRegistration.unregister` for each registration on the origin (https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Clear-Site-Data).

Proposed fix: Split the objective at line 947 into "CDN pointer rollback: 15 minutes" and "hostile-client eviction: bounded by the service-worker update check, worst case ~24h plus a navigation". Add `Clear-Site-Data: "cache", "storage", "executionContexts"` to the incident runbook at line 699 as the shell kill switch (served from the rolled-back origin on the SW script path and index.html), and require the service worker to self-check a signed version/kill-switch document on activate. Add a production smoke test (line 673) for the eviction path, not just for N/N-1 update.

### F12. Missing at lines 163-166

- Kind: `MISSING` | Severity: `high` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: "Upstream issue #121 leaves live fanout visible to an unproven provisional dialer during an open join window. Upstream issue #119 leaves some store holes incompletely healable. The former must be fixed or prevented before production; the latter needs repair or a fail-loud integrity response."

Evidence: The technical characterization is accurate for the pinned revision, but the doc omits that both defects are already fixed upstream and that the fixes postdate the pin. `gh api repos/kortiene/iroh-room/tags` → v0.1.0-rc.3 = 71fbb5007bef4ce83631c94762ec68c2beef3d79 (i.e. the pin IS the newest tag); `gh api .../compare/71fbb500...main` → {"status":"ahead","ahead_by":26,"behind_by":0}. Both fix PRs (#125 files: crates/iroh-rooms-net/src/node.rs, transport.rs; #132 files: crates/iroh-rooms-core/src/store/mod.rs, sync/engine.rs, sync/config.rs) change library code, so the fixes cannot be obtained without a repin.

Proposed fix: Append to this bullet: "Both are fixed on upstream `main` after the pinned rev — #121 at 58aca4ba (PR #125) and #119 at a5d98b70 (PR #132), both 2026-07-16 — but neither fix is in any tagged release (newest tag v0.1.0-rc.3 IS the pinned rev 71fbb500). The required action is a Phase 0 repin plus requalification, not new Jeliya-side mitigation." Add an explicit repin line item to the Phase 0 deliverables and go/no-go gate.

## Medium-severity findings

34 findings.

### F13. Wrong at lines 96-97

- Kind: `WRONG` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim under review: "The workspace pins the reviewed upstream revision and forbids unsafe Rust in workspace code."

Evidence: `unsafe_code = "forbid"` is declared once, at Cargo.toml:17-18 under `[workspace.lints.rust]`. A workspace lint table has NO effect on a crate that does not opt in via `[lints] workspace = true`. Two of three workspace members opt in — crates/jeliya-core/Cargo.toml:46-47 (`[lints]` / `workspace = true`) and crates/jeliyad/Cargo.toml:53-54 — but the third does NOT. crates/jeliya-ffi/Cargo.toml:28-30 says verbatim: "NOTE: this crate intentionally does NOT set `[lints] workspace = true`, so it does not inherit the workspace's `unsafe_code = \"forbid\"` — the C ABI boundary genuinely needs `unsafe`. jeliya-core itself stays unsafe-forbidden." `grep -n "lints" crates/jeliya-ffi/Cargo.toml` returns only that comment line — there is no `[lints]` table. jeliya-ffi IS a workspace member (Cargo.toml:2 `members = ["crates/jeliya-core", "crates/jeliyad", "crates/jeliya-ffi"]`) and it uses unsafe heavily: `grep -rc unsafe crates/jeliya-ffi/src/*.rs` → dart_api.rs:7, lib.rs:18, host.rs:0 (25 occurrences), e.g. crates/jeliya-ffi/src/lib.rs:88 `pub unsafe extern "C" fn jeliya_ffi_init_dart_api(...)`, lib.rs:219 `unsafe { drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len))) };`.

Proposed fix: Replace with: "The workspace pins the reviewed upstream revision. `unsafe_code = \"forbid\"` is declared workspace-wide (`Cargo.toml:17-18`) and inherited by `jeliya-core` and `jeliyad`; `jeliya-ffi` deliberately opts out because the C ABI boundary requires `unsafe` (`crates/jeliya-ffi/Cargo.toml:28-30`). The unsafe surface is therefore confined to the FFI shim, not eliminated from the workspace." This matters to the deployment argument: the doc's own change map (line 849) proposes a new `crates/jeliya-web/` `wasm-bindgen` crate, which is the second plausible unsafe-opt-out — the ADR list should state that new crates must opt in.

### F14. Wrong at lines 1067

- Kind: `WRONG` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: "Highest-risk unknowns … 6. Resolution of upstream issues #121 and #119."

Evidence: Both issues are resolved. #121: CLOSED/COMPLETED 2026-07-16T14:05:35Z via 58aca4ba (PR #125). #119: `gh issue view 119` → {"state":"CLOSED","stateReason":"COMPLETED","closedAt":"2026-07-16T19:30:18Z"}; timeline `referenced` commit a5d98b70d717f35d3ce60953a88e12e646f2e871 = PR #132 "fix(sync): retry failed store inserts and surface store degradation (#119)", merged 2026-07-16T19:30:17Z. `compare/71fbb500...a5d98b70` → {"status":"ahead","ahead_by":24}. `gh pr list --repo kortiene/iroh-room --state open` returns zero open PRs; `gh issue list --state open` returns only #100/#101/#102/#103, all `area/community` process issues.

Proposed fix: Replace item 6 with: "Requalification of Jeliya against an upstream rev that carries the #121/#119/#126 fixes — the fixes exist on upstream `main` but in no tagged release, so the repin target is an untagged commit."

Dissent from the verification panel:

````text
REFUTE (high): The investigator's underlying facts check out: I independently confirmed via `gh issue view` that #121 is CLOSED/COMPLETED (2026-07-16T14:05:35Z) and #119 is CLOSED/COMPLETED (2026-07-16T19:30:18Z). But the finding fails the materiality lens, and its "WRONG"/high classification does not survive.

DECISIVE FACT THE INVESTIGATOR UNDER-WEIGHTED: the pinned rev does not carry either fix. `gh api compare/71fbb500...58aca4ba` (#121 fix) returns {"status":"ahead","ahead_by":8,"behind_by":0} and `compare/71fbb500...a5d98b70` (#119 fix) returns {"status":"ahead","ahead_by":24,"behind_by":0}. `gh api .../tags` shows v0.1.0-rc.3 = 71fbb5007bef... is the newest tag, i.e. the pin IS the latest release. Cargo.toml:15 pins that rev and Cargo.lock:2015 resolves to `git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef...` with no [patch]/[replace] override. The binary Jeliya builds today contains neither fix.

Therefore nothing the doc asks the reader to decide changes. Line 163-166 ("#121 leaves live fanout visible to an unproven provisional dialer ... must be fixed or prevented before production") remains true of the shipped artifact. Line 390 ("Fix upstream issue #121 or suspend normal room fanout") remains the required mitigation verbatim. Line 887's go/no-go gate ("production work does not continue with upstream issue #121 exploitable and unmitigated") remains correctly binding, because at the pinned rev it IS still exploitable. A high-severity finding on a deployment proposal should change the plan, the gate, or the risk posture; this changes none of them.

The "WRONG" classification also overstates. The doc's substantive claim -- that this exposure is an unresolved risk to the production candidate -- holds. Only the word "Resolution" is stale, now naming a repin/requalification task rather than an upstream-fix wait. That is two-day-old drift (doc assessed at 4d4621c9; issues closed 2026-07-16; today 2026-07-18), i.e. STALE/IMPRECISE, not WRONG.

Strongest internal tell: the investigator's own proposed fix keeps the item, at the same list position, at the same risk weight, merely reworded. A finding whose remedy preserves the risk item intact is a wording correction. Additionally, the requalification concern the rewrite introduces is already adjacent to the doc's own item 10, "Exact-revision qualification of the final production candidate" (line 1071), within the same unknowns section (1058-1071) -- partial self-awareness that further weakens the finding.

Not dropped entirely: the rewrite adds one genuinely useful refinement -- the fixes exist only on an untagged commit, and this project pins to tagged rc commits with certified evidence, so requalification is non-trivial. That arguably makes the risk harder rather than softer. Worth a one-line wording edit at low severity, not a high-severity error claim.
````

### F15. Internally inconsistent at lines 46-48, 214, 869, 890, 912, 932

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Effort is stated in "engineer-weeks" but computed as calendar weeks, making the headline estimate inconsistent with the team size by a factor of about 2.5.

Evidence: Line 46-48: "For a small team of two to three engineers, the companion-backed production slice is estimated at **11 to 17 engineering weeks**." Line 214 (comparison table, "First safe production" row): "Approximately 11 to 17 engineer-weeks". The phase headings are elapsed durations: line 869 "Phase 0: freeze the claim boundary, 1 to 2 weeks", line 890 "Phase 1: ... 3 to 5 weeks", line 912 "Phase 2: ... 5 to 7 weeks", line 932 "Phase 3: ... 2 to 3 weeks". These sum to exactly 11 to 17 — so the same number is being used as both elapsed schedule AND engineer-weeks. For a 2-3 person team those differ by 2-3x: 11-17 calendar weeks of a 2.5-person team is 27-43 engineer-weeks.

Proposed fix: Pick one unit and label it consistently. If the phases are calendar weeks (which the phase headings imply), change lines 47 and 214 to "11 to 17 calendar weeks (approximately 27 to 43 engineer-weeks at the assumed staffing)" — the engineer-week figure is the one that drives cost.

### F16. Disagreement with a judgment at lines 830-859, 869-952

- Kind: `DISAGREE` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phases 0-3 ("11 to 17 weeks", lines 869/890/912/932) deliver the repository change map at lines 830-859.

Evidence: MY INDEPENDENT ESTIMATE — judgment, not a verified fact. Scope confirmed from the change map: 8 new crates (rows at lines 846-853: jeliya-protocol, jeliya-runtime, jeliya-platform-native, jeliya-web, jeliya-control, jeliya-companion, jeliya-components, jeliya-server-peer), against a current workspace of exactly 3 (`ls crates/` -> jeliya-core, jeliyad, jeliya-ffi) — a 3.7x increase in crate count. Plus 5 new UI paths (lines 837-841: sw.ts, runtime/, storage/, pairing/, invites/) and 3 reworked existing ones (834-836); 3 new workflows (854-856) against a current 2 (`ls .github/workflows/` -> ci.yml, release.yml); infra/, docs/adr/, docs/runbooks/; and `Split host-independent protocol/runtime behavior from native persistence and network assumptions` in jeliya-core (line 842). Of the 8 new crates, 5 land in Phases 0-3. Bottom-up in person-weeks: Phase 0 3-6 (doc reconciliation, hybrid ADR, threat-model rewrite for 3 new boundaries, and a browser-to-native Iroh proof that requires the wasm-bindgen wrapper of line 511 plus relay auth that does not exist yet); Phase 1 17-29 (keystore abstraction across Keychain/DPAPI-CNG/Secret Service/Keystore with a versioned KDF fallback 4-6; recovery bundle 2-3; idempotency 1-2; cursors 1-2; invite expiry+cancel 1-2; the jeliya-control pairing protocol — a new Noise-XX-equivalent transcript with SAS, scoped RPC, replay protection, revocation — 5-8; version negotiation 1-2; store-hole repair 2-4; gate harnesses 2-3); Phase 2 19-34 (see the separate Phase 2 finding); Phase 3 10-21 (DNS/TLS/CDN/CSP 1-2; service worker with N/N-1 plus an *encrypted* companion-view cache 2-4; two relays plus an endpoint-bound proof-of-possession relay-auth service, which line 1062 itself lists as highest-risk unknown #2, 2-4; promotion, the 10-scenario smoke suite of lines 675-684, and rollback 2-4; metrics plus 8 exercised runbooks 2-4; infra/ OpenTofu across 3 environments 1-3). Total 49-90 person-weeks. Available under the most charitable (calendar) reading of 11-17 weeks at the line 197-198 staffing of ~2.5 FTE is 27.5-42.5 person-weeks. That is roughly 2x optimistic; under the person-week reading of "engineer-weeks" it is 4-5x optimistic. Also unbudgeted in the same window: the ~14 new CI gate categories at lines 645-658 (Chromium/Firefox/WebKit suites, real-companion integration through a dedicated test relay, protocol fuzzing, SBOM, bundle budgets, service-worker N/N-1, storage quota/eviction/corruption/migration), which are themselves multi-week infrastructure.

Proposed fix: Rebaseline Phases 0-3 to 20-36 calendar weeks at 2.5 FTE, or cut scope. The cheapest credible cut: defer the jeliya-core/jeliya-protocol/jeliya-runtime/jeliya-platform-native split out of Phases 0-3 entirely (it is a prerequisite for jeliya-web in Phase 4, not for the companion slice) and let the companion ship on the existing core, which removes the single largest refactor from the critical path. Publish the bottom-up decomposition alongside the phase totals so the estimate is auditable.

Dissent from the verification panel:

````text
REFUTE (high): The finding does not hold up at high severity. Four independent problems, three of them checkable facts rather than counter-judgment.

**1. The doc is already self-aware — explicitly, in the text the finding never quotes.** Lines 47-48: "These are planning estimates, not release commitments." Line 45 scopes them: "For a small team of two to three engineers." The materiality lens asks specifically whether the doc pre-flags the issue; here it does so in the summary, not buried in the unknowns list. Attacking an estimate that the document itself disclaims as non-committal, and calling that "high", is the exact shape this lens exists to downgrade. (I checked lines 1032-1071 too: unknowns #1 upstream-interface acceptance, #2 relay-auth PoP, and #7 signing/notarization timing each flag schedule-bearing risk, though none says "the phase durations may be wrong" outright — so the self-awareness is partial but real.)

**2. The finding's stated premise is factually wrong.** The claim under review is "Phases 0-3 deliver the repository change map at lines 830-859." The change map spans Phases 0-5, not 0-3, and says so on its own face. Line 852: `crates/jeliya-components/` — "**Later** signed package, WIT policy, quota, and native component host". Line 853: `crates/jeliya-server-peer/` — "**Later** explicitly invited availability or hosted-agent peer" (those are Phase 5, lines 977-985). Line 849 `crates/jeliya-web/` is verbatim Phase 4's deliverable ("Wasm signing and Iroh endpoint wrapper", lines 958-959). The change map section (line 830) also sits *before* the roadmap section (line 864) with no phase attribution anywhere. Charging the whole map against the 11-17 week window is a scoping error, not a discovery.

**3. The finding contradicts itself on the number that drives its arithmetic.** It asserts "Of the 8 new crates, 5 land in Phases 0-3" to build the 3.7x-crate-growth framing — then its own proposed fix concedes the jeliya-core/protocol/runtime/platform-native split "is a prerequisite for jeliya-web in Phase 4, not for the companion slice." I verified the concession is the correct reading: `awk 'NR>=871 && NR<=940' docs/production-deployment.md | grep -iE "jeliya-protocol|jeliya-runtime|platform-native|split|crate"` returns **no matches** — the Phase 0-3 deliverable lists (871-879, 892-900, 914-919, 934-940) never mention the split. Only 2 of 8 new crates (jeliya-control, jeliya-companion) are actually named as Phase 0-3 work. The single largest line item in the bottom-up (the core refactor) is charged to a window the doc never put it in.

**4. Decision-irrelevance.** The doc asks for one decision, lines 216-219: "Adopt the hybrid model and use the companion-backed shell as the first production slice." The 11-17 figure's job is comparative — line 214 ranks four options (16-24 / 11-17 / at-least-24 / 11-17). A uniform optimism bias inflates all four columns and leaves the ranking, and therefore the decision, unchanged; the finding offers no argument that the companion option is *differentially* underestimated. The roadmap is also structurally gate-driven, not calendar-driven (lines 866-867: "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase"), so slip is absorbed by gates rather than by shipping unready.

**What legitimately survives, at low severity.** Two checkable residues, neither matching the finding as written:
(a) A genuine unit conflation. Line 46 says "**11 to 17 engineering weeks**" and line 214 "Approximately 11 to 17 **engineer-weeks**", while the Phase 0-3 *durations* (lines 869/890/912/932: 1-2 + 3-5 + 5-7 + 2-3) sum to exactly 11-17 **calendar** weeks. Both can only be true at 1 FTE, which contradicts the 2-3 engineers of line 45 and the ~2.5 FTE of lines 197-198. That is arithmetic, not judgment — but it is an *ambiguity* finding, not the "2-4x optimistic" finding under review.
(b) The ~14 CI gate categories at lines 645-658 (Chromium/Firefox/WebKit, real-companion integration via dedicated test relay, protocol fuzzing, SBOM, bundle budgets, SW N/N-1, storage quota/eviction/corruption/migration) are not attributed to any phase's deliverable list, though the change map does acknowledge the work exists (line 854, `.github/workflows/web-ci.yml`). Fair as a MISSING-attribution note.

Sub-claims I checked and found **accurate**: workspace is exactly jeliya-core/jeliyad/jeliya-ffi (`ls crates/`); workflows are exactly ci.yml/release.yml (`ls .github/workflows/`); the ~2.5 FTE reading of lines 197-198 is a fair paraphrase; the 8 new-crate rows at 846-853 and 5 new UI paths at 837-841 are quoted correctly; relay-auth is indeed highest-risk unknown #2 (line 1062).

Recommended disposition: keep only as a low-severity note that the phase estimates publish no bottom-up decomposition and mix engineer-weeks with calendar weeks (lines 46/214 vs 869-932). Drop the "2x-4x optimistic / rebaseline to 20-36 weeks" counter-estimate — it is unfalsifiable, uncalibrated against any cited baseline, rests on a mis-scoped premise, and is aimed at a number the document explicitly declines to commit to.
````

### F17. Disagreement with a judgment at lines 912-930

- Kind: `DISAGREE` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 2, "5 to 7 weeks" (line 912), delivers `jeliya-companion` and PWA companion transport, a scoped chat-only browser controller, "signed macOS and Windows packages and a verified Linux package", and "recovery and re-pair user interfaces" (lines 916-919).

Evidence: MY INDEPENDENT ESTIMATE — judgment. Bottom-up: jeliya-companion from scratch, a new signed native service with an Iroh control ALPN, no public listener, and service lifecycle/autostart on three OSes (4-7 pw); PWA companion transport plus the scoped controller (3-5 pw); the pairing UI — QR display and scan, SAS confirmation, scope grant, expiry display, revocation list, re-pair (3-5 pw); recovery UI (2-3 pw); macOS Developer ID plus notarization, hardened runtime, stapling, and CI wiring (1.5-3 pw); Windows Authenticode via Azure Trusted Signing plus an installer (2-4 pw); a verified Linux package with repository, checksums, and provenance (1.5-3 pw); gate harnesses — the 1,000-cycle pairing/revocation rig of line 924, NAT-separated two-user e2e infrastructure of lines 925-926, forced-relay runs, a 48-hour soak rig, installer tamper tests (2-4 pw). Total 19-34 person-weeks against 12.5-17.5 available (5-7 weeks at 2.5 FTE) — roughly 1.5-2x over, before any external dependency. Three aggravating factors the phase does not account for: (1) line 929's `48-hour soak loses no committed event` consumes 2 calendar days per attempt and any failure costs a fix-plus-rerun cycle; (2) three package formats across three OSes is three separate CI signing pipelines, not one; (3) Azure's own FAQ states Artifact Signing (formerly Trusted Signing) `doesn't issue Extended Validation (EV) certificates` and that `SmartScreen reputation builds up automatically. The prompt stops appearing once the file hash has sufficient download history` (https://learn.microsoft.com/en-us/azure/artifact-signing/faq) — so first-run Windows installs will trip SmartScreen for an unknown period after launch. Line 1068 lists `Native signing, notarization, SmartScreen, and Linux distribution timing` as a risk but no gate or estimate absorbs it.

Proposed fix: Split Phase 2 into 2a (companion + transport + controller + pairing UI, 4-6 weeks) and 2b (three signed package pipelines + recovery/re-pair UI + soak, 3-5 weeks), total 7-11 weeks. Move all signing-account procurement to Phase 0 as a gated item. Add a SmartScreen expectation to the launch communications plan rather than to the engineering gate.

### F18. Missing at lines 864-996 (roadmap), vs 526-528, 861-862, 1060-1061

- Kind: `MISSING` | Severity: `medium` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim under review: Implicit: the Phase 4 browser peer (954, 10-14 weeks) is schedulable, and "A browser peer remains the intended zero-install capability" (221-222).

Evidence: Lines 526-528 require "Portable traits are introduced upstream or in an audited short-lived patch for event store, blob store, sync transport, clock, and task scheduling"; line 861-862 says these "should preferably land upstream. A long-lived private fork is a security and maintenance liability"; line 1060-1061 lists as highest-risk unknown #1 "Whether Iroh Rooms will accept and maintain the portable browser store, transport, and blob interfaces upstream." Grep over the entire roadmap (lines 864-1000) for upstream/portable/trait/interface returns only three hits: the #121 gate (888), "recovery and re-pair user interfaces" (919), and "the exact upstream/browser-adapter revision receives security qualification" (975). No phase deliverable and no gate owns securing the upstream traits. Cargo.lock confirms why this is decisive: `rusqlite` 0.37.0 is a dependency of `iroh-rooms-core` 0.1.0-rc.3 (Cargo.lock, iroh-rooms-core dependency block), i.e. the native store assumption lives in the pinned third-party crate, not in Jeliya code.

Proposed fix: Add a Phase 0 deliverable: "Open the portable store/blob/transport trait RFC upstream and obtain maintainer commitment, OR record an explicit fork-and-maintain decision with its recurring cost." Add a Phase 0 gate: "Phase 4 is not scheduled until the upstream path or the fork decision is written down." Until then Phase 4's 10-14 weeks is not an estimate, it is a wish.

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as framed (MISSING / critical); a small residual survives as a low-severity judgment disagreement.

WHAT I CONFIRMED (investigator's facts are sound):
1. Roadmap grep verified. `grep -n -iE "upstream|portable|trait|interface" docs/production-deployment.md | awk -F: '$1>=864 && $1<=1000'` returns exactly three lines: 887 ("production work does not continue with upstream issue #121 exploitable and unmitigated"), 919 ("recovery and re-pair user interfaces" — an incidental substring hit), 975 ("the exact upstream/browser-adapter revision receives security qualification"). No phase deliverable and no gate owns securing the portable traits. TRUE.
2. Cargo.lock:2023-2034 — `iroh-rooms-core` 0.1.0-rc.3, source `git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef4ce83631c94762ec68c2beef3d79`, dependencies include `rusqlite` (Cargo.lock:2032). The native-store assumption does live in the pinned third-party crate. TRUE.
3. I also checked something the investigator did not: no ADR in the list at doc lines 1046-1056 covers portable-trait upstreaming. Phase 0's ADR deliverable (line 875) is "accept or reject the hybrid architecture." So the item genuinely has no owner anywhere. TRUE.

WHY IT FAILS THE MATERIALITY LENS:

A. The doc is explicitly self-aware — this is dispositive. Lines 1060-1061 list as **highest-risk unknown #1**: "Whether Iroh Rooms will accept and maintain the portable browser store, transport, and blob interfaces upstream." That is the finding's own thesis, stated by the doc, ranked first among ten unknowns. The investigator cites this passage as *supporting evidence for a MISSING classification*, which inverts the argument: a document that names something as its top open risk has not omitted it. Per the review rules this must be separated — it is not MISSING, it is at most a DISAGREEMENT about where the item belongs (unknowns list vs. scheduled deliverable).

B. The doc does not actually assume upstream acceptance. Line 527 is a disjunction: "Portable traits are introduced upstream **or in an audited short-lived patch**." Line 861-862 says traits "**should preferably** land upstream. A long-lived private fork is a security and maintenance liability" — a stated preference plus an explicit naming of the alternative's cost. Line 975's gate reads "the exact **upstream/browser-adapter** revision," contemplating an adapter revision distinct from clean upstream. The finding's proposed fix ("OR record an explicit fork-and-maintain decision with its recurring cost") is largely already present at 527 and 861-862.

C. Phase 4 is not load-bearing for the decision being requested. The decision (lines 216-219) is "Adopt the hybrid model and use the companion-backed shell as the first production slice." I verified the arithmetic: Phases 0-3 = (1-2)+(3-5)+(5-7)+(2-3) = 11 to 17 weeks, which is exactly the hybrid column's "First safe production ... 11 to 17 weeks" at line 214. That table entry then says only "browser mode follows" — it quotes no Phase 4 number. Line 952 marks Phase 3 as "the first production launch gate," and line 1019 excludes "a browser-owned room identity" from the first slice. Line 866-867 already states "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase," so Phase 4 is by construction downstream of the launch gate. The 10-14 week figure is therefore a planning horizon beyond the approval boundary, not an input to the go/no-go the doc is asking for. "Phase 4's 10-14 weeks is not an estimate, it is a wish" is rhetorically strong but immaterial to the decision.

RESIDUAL KERNEL (why low, not drop): there is a genuine small gap between "listed as risk #1" and "someone opens the RFC by a date." Given the estimate depends on it and no ADR covers it, adding the upstream-trait RFC as a Phase 0 or Phase 1 deliverable, or as a Phase 4 *entry* gate, is a real and cheap improvement. That is worth one line in a review. It is not critical, and it should be labeled a judgment disagreement about sequencing/ownership, not an omission.
````

### F19. Missing at lines 374-375, 377-379, 400-402

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `browser-platform-facts`

Claim under review: "URI fragments are processed by the browser and are not included in the HTTP request or Referer header", followed by controls whose first item is a bootstrap that "reads the fragment into memory and calls `history.replaceState()` before React startup". The residual-risk sentence names only "Browser extensions, screenshots, copied links, and OS clipboard managers".

Evidence: The transport claim is correct (see confirmed claims), but the doc treats first-party replaceState as sufficient, and it is not — the fragment is committed to browser-managed state before any page script runs. RFC 9700 (OAuth 2.0 Security Best Current Practice) §4.3.2 states verbatim that with a fragment-delivered credential "a URL like client.example/redirection_endpoint#access_token=abcdef may also end up in the browser history as a result of a redirect from a provider's authorization endpoint", and §2.1.2 concludes clients "SHOULD NOT use the implicit grant (response type `token`)" because such responses "are vulnerable to access token leakage and access token replay". The doc's ticket-in-fragment scheme is structurally the implicit grant, and the doc cites no source acknowledging the history-retention vector. Unverified-but-plausible vectors in the same class, which I flag rather than assert: profile-level history sync (Chrome Sync / Firefox Sync / iCloud), session restore, bookmark sync, omnibox search-suggestion transmission when a user pastes the link into the address bar, and URL-reputation lookups such as Chrome Enhanced Safe Browsing (Google's own support documentation does not state whether fragments are stripped — see notes).

Proposed fix: Reframe lines 374-379: state that fragment-only delivery removes the *network* exposure but not the *client-side* exposure, and that replaceState replaces only the current session-history entry — it cannot retract a URL already committed to profile history, session-restore state, or account sync. Then make the ticket's security independent of URL secrecy rather than dependent on it: the doc already has the right primitives elsewhere (single-use, 30-minute expiry at line 386, identity binding at line 393, `invite.cancel` at line 388). Add that the residual list at 400-402 must also name browser history and history/bookmark sync to vendor accounts, and prefer the two-step identity-bound flow at 396-397 as the default rather than the fallback.

Dissent from the verification panel:

````text
REFUTE (high): The finding's technical kernel is true but trivially small; its stated rationale is wrong on three load-bearing points, and its severity is inflated by roughly two levels.

WHAT SURVIVES (and it is thin). I confirmed the doc never mentions browser history: `grep -n -i -E "browser history|chrome://|session restore|omnibox|safe browsing|profile history" docs/production-deployment.md` returns no matches (exit 1). The only adjacent assertion, line 653-654, covers "invite fragments never enter HTTP requests, logs, or crash evidence" — not client-side history. So "browser history is absent from the residual list at 400-402" is factually correct, and `replaceState()` genuinely cannot retract a URL already committed to profile history. That is one missing list item.

WHY THE FINDING AS FILED DOES NOT HOLD:

(1) "The doc treats first-party replaceState as sufficient" is FALSE — this is the finding's central characterization and it misreads the text. `replaceState` at 377-379 is the FIRST of EIGHT required controls in a single bulleted list running 377-391. The same list contains single-use + 30-minute expiry (386-387), `invite.cancel` and immediate window closure (388-389), no-third-party-scripts + `Referrer-Policy: no-referrer` (382-383), structural redaction (384-385), and a requirement to fence provisional connections (390-391). The doc nowhere presents replaceState as standalone.

(2) The RFC 9700 analogy is INVERTED. I fetched https://www.rfc-editor.org/rfc/rfc9700.html and confirmed the investigator's quotes are substantially accurate — §4.3.2: "In the case of implicit grant, a URL like `client.example/redirection_endpoint#access_token=abcdef` may also end up in the browser history as a result of a redirect from a provider's authorization endpoint"; §2.1.2: "Clients SHOULD NOT use the implicit grant (response type `token`)... unless access token injection in the authorization response is prevented and the aforementioned token leakage vectors are mitigated." But §2.1.2's stated REASON is that "no standardized method for sender-constraining exists to bind access tokens to a specific client." The Jeliya ticket is sender-constrained, which is exactly the carve-out. Verified in the implementation: `crates/jeliya-core/src/supervisor.rs:1345-1353` rejects a ticket whose `invitee_key` differs from the local identity, and — more importantly, since a client-side check is patchable — the peer-side authority does the same. `supervisor.rs:3044-3047` maps `RejectReason::BadCapability` to "this ticket's secret or identity does not match the invite", and `supervisor.rs:751` states "The on-log gate_join stays the convergent membership authority regardless." Redemption requires authoring a signed `member.joined` (supervisor.rs:1324-1326), i.e. proof-of-possession of the invitee key. A ticket recovered from browser history or a synced Chrome profile is NOT redeemable by the recoverer. So the claim "The doc's ticket-in-fragment scheme is structurally the implicit grant" is wrong on the precise axis RFC 9700 objects to.

(3) The proposed fix is largely ALREADY IN THE DOC. Its second half — "prefer the two-step identity-bound flow at 396-397 as the default rather than the fallback" — misreads the text: line 393 says "Current tickets are bound to a known invitee identity. Preserve that property"; line 394 says "New-user onboarding is therefore a two-step flow" (already the default, not a fallback); lines 399-400 forbid holder-bearer from implicitly replacing identity binding; and line 1026 puts "generic holder-bearer invitations" explicitly OUT OF SCOPE. The fix's first half ("make the ticket's security independent of URL secrecy") concedes in its own text that "the doc already has the right primitives elsewhere."

SELF-AWARENESS. The one genuinely bearer-ish exposure is `capability_secret` inside `BootstrapProof` (supervisor.rs:1396-1400), which buys a provisional connection to pull the membership sub-DAG. The doc flags exactly this at lines 390-391 ("Fix upstream issue #121 or suspend normal room fanout while an unproven provisional connection exists") and again as high-risk unknown #6 at line 1067 ("Resolution of upstream issues #121 and #119").

MATERIALITY. The doc's residual list at 400-402 already names "screenshots, copied links, and OS clipboard managers" — the identical category of client-side post-delivery disclosure. Browser history is a missing MEMBER of a category the doc already opened, not a missing category. Nothing here changes any of the 8 ADRs (1046-1056), any phase gate, or any provider/architecture decision the doc asks for. It changes one clause of prose. Additionally, the investigator's own severity driver — Chrome Sync, session restore, bookmark sync, omnibox transmission, Enhanced Safe Browsing — is self-labelled unverified, and unverified speculation cannot carry a "high".

RECOMMENDATION: file at most a low-severity editorial note — add "browser history and history/bookmark sync to vendor accounts" to the residual list at 400-402, and note at 377-379 that replaceState addresses the current session-history entry only. Strip the implicit-grant framing and the "make two-step the default" recommendation entirely; both are wrong and would damage reviewer trust in the rest of the review.
````

### F20. Missing at lines 62-63

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim under review: The doc quantifies the gap between the qualified commit and the assessed HEAD ("14 commits and 142 changed files") but never quantifies the gap between the assessed HEAD and `main`.

Evidence: The doc states only the backward delta. Measured forward deltas: `git rev-list --count 4d4621c9..HEAD` -> 16; `git rev-list --count 77501d7..HEAD` -> 13; `git rev-list --count 55024a4..HEAD` -> 26 and `git diff --shortstat 55024a4 HEAD` -> "231 files changed, 36973 insertions(+), 5313 deletions(-)" versus `git diff --shortstat 55024a4 4d4621c9` -> "142 files changed, 12331 insertions(+), 2679 deletions(-)". The unassessed forward gap (89 additional changed files, ~24.6k additional insertions) is larger than the gap the doc does disclose, and it includes the entire a11y/i18n milestone (88d76a3, ab9c29f, 676a0e0, 6a882d3, 5695e61, 7248fb0) that directly touches the React UI the doc classifies at lines 112 and 834-841.

Proposed fix: Add a bullet after line 63: "`main` is a further 13 commits past the merged assessment tree; measured from the qualified commit `55024a4…`, current `main` is 26 commits and 231 changed files ahead versus the 14 commits and 142 files assessed here. The React localization, accessibility, and design-conformance work in that gap is unassessed by this page."

### F21. Missing at lines 597

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: `require-trusted-types-for 'script';` (line 597) is the entire Trusted Types configuration — there is no `trusted-types` directive naming an allowlist of policies.

Evidence: Doc line 597 is the last directive in the block (583-598); no `trusted-types` appears anywhere in the file (grep for `trusted type` over docs/production-deployment.md returns only lines 597 and 655). MDN, Content-Security-Policy/require-trusted-types-for: "Together with the `trusted-types` directive, which guards creation of Trusted Type policies, this allows authors to define rules guarding writing values to the DOM" — and its canonical example header is `require-trusted-types-for 'script'; trusted-types foo;`. MDN, Content-Security-Policy/trusted-types: "specify an allowlist of Trusted Type policy names that a website can create using `trustedTypes.createPolicy()`. This prevents website code from creating unexpected policies"; `'none'`: "Disallows creating any Trusted Type policy." MDN, TrustedTypePolicyFactory/createPolicy: TypeError is "Thrown if policy names are restricted by the Content Security Policy `trusted-types` directive and this name is not on the allowlist" — i.e. with no directive, policy creation is unrestricted. MDN default-src page confirms `trusted-types` does NOT fall back to default-src, so `default-src 'none'` (584) does not cover it.

Proposed fix:

````text
Add `trusted-types 'none';` to the block today. Verified safe: `grep -rn "dangerouslySetInnerHTML|innerHTML|.outerHTML" ui/src/` returns zero hits, and `grep -rn "trustedTypes|TrustedHTML|createPolicy" node_modules/react-dom/cjs/*.js` returns zero hits — React 18.3.1 (ui/package.json:16) neither creates nor needs a policy here. Relax to a named allowlist (e.g. `trusted-types jeliya-wasm;`) only when the Wasm peer or a sanitizer actually needs one, and never add a policy literally named `default` without review.
````

Dissent from the verification panel:

````text
REFUTE (high): The underlying observation is TRUE but the finding as filed (MISSING, high) does not survive the materiality lens, and its proposed fix is actively harmful to the doc's own architecture.

WHAT I CONFIRMED (accurate):
- Doc lines 583-598 are the "Baseline Content Security Policy" block; line 597 `require-trusted-types-for 'script';` is the last directive. `grep -n -i "trusted" docs/production-deployment.md` returns only lines 203, 269, 562, 597, 655 — no `trusted-types` directive anywhere. The investigator's core observation holds.
- MDN Content-Security-Policy/trusted-types (fetched): "used to specify an allowlist of Trusted Type policy names that a website can create using `trustedTypes.createPolicy()`... prevents website code from creating unexpected policies, making it easier to audit trusted type code". `'none'`: "Disallows creating any Trusted Type policy." Confirmed.

WHY THE SEVERITY IS INFLATED:
1. The primary protection is already present and enforcing. MDN require-trusted-types-for (fetched): `'script'` "Disallows using strings with DOM XSS injection sink functions, and requires matching types created by Trusted Type policies" — sinks "only accept non-spoofable, typed values... and reject strings." Line 597 alone blocks every DOM XSS sink. The missing `trusted-types` directive adds policy-creation auditability (defense-in-depth against a script gadget registering a permissive `default` policy), which only matters after `script-src 'self'` has already failed. That is a hardening refinement, not a hole.
2. Nothing is deployed. `grep -rn "Content-Security-Policy\|require-trusted-types" --include=*.ts --include=*.tsx --include=*.html --include=*.json --include=*.toml --include=*.yml .` (excluding node_modules) returns ZERO hits. There is no CSP in the repo at all. The delta between doc-as-written and doc-as-fixed is one directive in a block explicitly labeled "Baseline" (line 583).
3. The doc IS partially self-aware. Line 655 makes "CSP and Trusted Types tests" a mandatory per-PR gate. Not in the unknowns section (1032-1071), but the doc explicitly routes Trusted Types configuration to dedicated CI work rather than treating line 597 as final.

WHY THE PROPOSED FIX IS WRONG (this is the strongest refutation):
The fix says to add `trusted-types 'none';` and calls it "Verified safe" on the basis of grepping today's `ui/src/` for innerHTML. That methodology is invalid here, and the conclusion is wrong. MDN Trusted_Types_API (fetched) lists TrustedScriptURL sinks including "`ServiceWorkerContainer.register()`", "`url` argument to `Worker()` constructor", and "`WorkerGlobalScope.importScripts()`". The doc mandates a service worker it does not yet have — line 406: "The existing UI has an install manifest but no service worker or browser room runtime"; line 837 plans `ui/src/sw.ts` "Versioned service worker and N/N-1 cache lifecycle"; line 621 sets its cache policy; line 651 requires "service-worker install, update, offline, and N/N-1 compatibility tests". Under `require-trusted-types-for 'script'` plus `trusted-types 'none'` (which "Disallows creating any Trusted Type policy"), `navigator.serviceWorker.register()` could never be given a trusted value and the doc's planned SW architecture would not register. Same exposure for the Wasm peer's workers. Grepping the current UI cannot validate a CSP for an app the same doc says must add a service worker, worker-based Wasm runtime, and storage layers that do not exist yet.

VERDICT: downgrade to low. Worth one line in the review because the observation is correct and reviewers do paste baseline CSP blocks verbatim into edge configs — but it must be filed as "name a policy allowlist when the SW/Wasm work lands" (e.g. `trusted-types jeliya-sw;`), NOT `trusted-types 'none'`. Filed as high with a fix that breaks the doc's own service worker, it would mislead the decision it claims to protect.

Minor citation drift, non-load-bearing: investigator cited ui/package.json:16 for React 18.3.1; actual lines are 17 (`"react": "^18.3.1"`) and 18 (`react-dom`). I did not verify the react-dom node_modules grep since the fix it supports is being rejected on other grounds.
````

### F22. Missing at lines 858-859, 875, 1046-1056, 697-706

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: ADR and runbook authoring is scoped in the change map (lines 858-859) but budgeted in only one phase deliverable (line 875).

Evidence: Line 1046 heading `Decisions that require an ADR` is followed by 8 numbered decisions (lines 1048-1056). Line 697 `Create and exercise runbooks for:` is followed by 8 bullets (lines 699-706). That is 16 documents. Only ONE is budgeted: line 875 `accept or reject the hybrid architecture through an ADR` (Phase 0), which covers ADR #1 partially. Line 940 mentions `incident runbooks` in the Phase 3 deliverable list but no gate requires them to exist, and line 697's verb is `Create and exercise` — exercising 8 runbooks is itself multi-day work. Each document also carries a fixed authoring overhead in this repo: `scripts/check-docs.mjs` requires ten front-matter fields (`requiredFields` at lines 22-33: type, title, description, tags, timestamp, status, implementation_status, verification_status, release_status, audience) with constrained enums, and rejects any document not reachable from the index (lines 968-969, `document-orphan`, `document is not reachable from docs/index.md`).

Proposed fix: Add an explicit line item per phase: "ADRs #2-#4 authored and accepted (Phase 1), #5-#6 (Phase 2), #7-#8 (Phase 4)", and "8 runbooks authored AND exercised" as a Phase 3 gate bullet rather than a deliverable. Budget 1.5-3 person-weeks total across phases.

Dissent from the verification panel:

````text
REFUTE (high): The finding is technically anchored but fails the materiality lens and contains two counting errors.

VERIFIED ACCURATE: lines 858-859 scope docs/adr/ and docs/runbooks/; lines 1048-1056 are 8 ADR decisions; lines 699-706 are 8 runbook bullets; line 875 is the only per-ADR phase deliverable (exhaustive grep for "adr|runbook" returns only 695, 697, 858, 859, 875, 940, 1046). scripts/check-docs.mjs:21-32 does require the ten listed front-matter fields, and lines 968-969 do emit 'document-orphan' / 'document is not reachable from docs/index.md'. docs/adr and docs/runbooks do not exist.

WHY IT FAILS:
(1) "Only ONE is budgeted" is wrong. Line 940 budgets "privacy-safe metrics and incident runbooks" as a Phase 3 deliverable. The investigator concedes this then dismisses it, silently converting a wrong count into a narrower gate complaint.
(2) The count is inflated by 2 out-of-scope items. ADR #7 (components, line 1055) maps to line 1023 "third-party components" and ADR #5 (server peers, line 1052) maps to line 1024 "optional server peers" — both inside the doc's explicit exclusion list at 1017-1026. They need no phase budget in this slice.
(3) 6 of 8 ADRs attach to already-budgeted decision work: #1->line 879, #2->line 898, #3->line 894, #4->line 960, #6->line 959, #8->lines 1036-1037. The finding counts documents where the doc budgets decisions; authoring an ADR for a decision already budgeted is marginal, not additive.
(4) SELF-AWARENESS (the lens's explicit test): lines 1046-1056 sit inside "## Assumptions, unresolved decisions, and high-risk unknowns" at line 1032. The doc enumerates all 8 ADRs by name in its own unresolved-decisions section and all 8 runbook categories at 695-706. This is a granularity gap in phase deliverable lists, not a blind spot.
(5) MAGNITUDE: the investigator's own estimate of 1.5-3 person-weeks sits against a 21-31 week roadmap (1-2 + 3-5 + 5-7 + 2-3 + 10-14) with a self-declared 10-week band; Phase 4 alone spans 4 weeks. It cannot move the accept/reject decision the doc asks for at line 875.
(6) Part of "exercise" is already gated: line 859 scopes runbooks for "Deployment, rollback, relay failure, key rotation, and incident procedures", and Phase 3 gates 947 (N-to-N-1 rollback within 15 minutes) and 948 (regional relay failover within 2 minutes) exercise two of those. Gates in this doc are selective, not deliverable checklists.

RESIDUAL KERNEL: line 697's verb is "Create and exercise" and no Phase 3 gate bullet (942-950) requires runbooks to exist, while Phase 3 is only 2-3 weeks for DNS/TLS/CDN/CSP, service worker, two relays plus relay-auth, promotion/smoke/rollback, metrics, and runbooks. That is real but belongs merged into a Phase 3 schedule-realism finding, not standing alone. Downgrade to low; do not report at medium as framed.
````

### F23. Missing at lines 864-996, 713-748, 346

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: The roadmap omits legal and privacy-policy work.

Evidence:

````text
`grep -n -i -E 'privacy policy|terms|legal' docs/production-deployment.md` returns no hits for a policy document (only line 546's `sensitive metadata` and similar substantive uses). Yet the proposal creates every trigger for one: a first-party hosted origin the document concedes can observe user content (line 1043-1044 `The product accepts that a hosted first-party origin can observe the content it renders`); telemetry collection (lines 715-748); a stated retention period (line 743 `Retain raw security access logs for no more than 72 hours initially`); third-party subprocessors (Cloudflare, Iroh managed relays, lines 553-558); optional cloud storage of user material (line 346 `Optional cloud storage holds only the opaque encrypted envelope`); and a public-facing product at a stable domain. A published privacy policy, terms of service, and subprocessor list are launch blockers in most jurisdictions, and the 72-hour retention figure is a policy commitment that currently exists only inside an engineering document.
````

Proposed fix: Add a Phase 3 deliverable and gate bullet: "privacy policy, terms of service, and subprocessor list published at app.jeliya.ai, reviewed by counsel, and consistent with the retention and telemetry rules in [Privacy-safe observability]." Treat the 72-hour retention and the opt-in telemetry default as policy commitments that the document and the published policy must agree on.

### F24. Missing at lines 864-996, 830-859

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: The roadmap (lines 864-996) and change map (lines 830-859) omit accessibility work entirely.

Evidence:

````text
`grep -n -i -E 'accessib|a11y|screen reader' docs/production-deployment.md` returns ZERO hits. The repository enforces accessibility as a release requirement: `docs/accessibility-checklist.md` states `CI enforces what a machine can decide: no critical or serious axe violations across every destination and viewport, no clipped layout at 100/200/320% text in English and French, a focus indicator that exists, target sizes that clear their floor`, and lists the enforcing suites — `ui/e2e/a11y-matrix.spec.ts` (`No critical/serious axe violations, all destinations x 4 viewports`) and `ui/e2e/a11y.spec.ts` (`One main and one h1 per destination, landmark names, skip links, target floors`). `ui/e2e/a11y.spec.ts:28` sets `const A11Y_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']`. `ui/e2e/a11y-matrix.spec.ts:41` enumerates DESTINATIONS (onboarding, rooms, room workbench activity, People, Agents & Runs, Files, Pipes) and its comment insists on `Every room destination, not a sample of two`. The proposal adds at least five new user-facing surfaces — join/invite (line 841), pairing with QR and SAS confirmation (line 840), recovery and re-pair (line 919), storage-quota and eviction warnings (line 839), and offline/degraded states (line 934) — every one of which must be added to the DESTINATIONS matrix and cleared across 4 viewports and 3 text scales, and the manual screen-reader checklist must be re-run before release. None of that is in any deliverable or gate. The SAS pairing confirmation is a particularly hard case: a short authentication string compared under time pressure has real screen-reader and cognitive-load requirements.
````

Proposed fix: Add to Phase 2 deliverables: "extend ui/e2e/a11y-matrix.spec.ts DESTINATIONS with join, pairing, SAS confirmation, recovery, re-pair, and quota/offline states; pass the a11y matrix and the manual checklist in docs/accessibility-checklist.md." Add to the Phase 3 gate: "no critical or serious axe violation on any new destination at any supported viewport." Budget 1.5-3 person-weeks across Phases 2-3.

### F25. Missing at lines 910

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 1 go/no-go gate: "independent security review approves the wire formats and key lifecycle" (line 910).

Evidence: ASPIRATIONAL. The clause defines none of: who counts as "independent" (a maintainer not on the feature? a contracted firm?), what standard or checklist "approves" is measured against, what artifacts are reviewed (spec? implementation? both?), what severity of finding blocks the gate, or what evidence records the approval. Compare line 950, which for the same class of activity at least states a threshold: `an external penetration review has no unresolved critical or high finding`. Line 198 mentions `an independent security review` only as a staffing assumption, with no scope, no budget line, and no schedule allocation. As written this gate can be signed off with a one-line message and no evidence.

Proposed fix: Restate as: "a reviewer with no authorship of the pairing or key-lifecycle code signs a written review of [named spec documents] against a published checklist; every critical and high finding is fixed and re-reviewed, every medium is fixed or has an accepted-risk ADR; the signed review and the reviewed commit SHA land in docs/evidence/." Name the reviewer source (internal role or external firm) and the checklist in Phase 0.

Dissent from the verification panel:

````text
REFUTE (high): The textual observation is ACCURATE, but the finding as filed (MISSING / high) does not survive the materiality lens. I verified every element rather than taking the investigator's word.

WHAT IS CONFIRMED
- Line 910 reads exactly: `- independent security review approves the wire formats and key lifecycle.` It does not define independence, checklist, blocking severity, or evidence artifact. Accurate.
- Line 198 reads `engineer at least part-time, and an independent security review.` — a staffing/resource assumption only, no scope or schedule. Accurate.
- Line 950 reads `- an external penetration review has no unresolved critical or high finding.` — does state a blocking threshold. The asymmetry the investigator points to is real.
- The doc's own self-awareness sections (planning assumptions 1034-1044, ADRs 1046-1056, unknowns 1058-1071) do NOT list review scope/standard. So the "doc already flags it" mitigation does not apply. Investigator is right here.

WHY IT STILL FAILS ON MATERIALITY

1. Line 910 is not an outlier — it is the doc's uniform register, so the severity is miscalibrated by singling it out. Printed side by side:
  910: `independent security review approves the wire formats and key lifecycle.`
  944: `external TLS/header/CSP assessment passes;`
  975: `the exact upstream/browser-adapter revision receives security qualification.`
  989: `sandbox escape and confused-deputy review passes;`
All four name an assessor with no checklist, no blocking severity, no evidence artifact. Line 950 is the single exception, not the norm. A high-severity MISSING against 910 while three identical siblings go unmentioned is a calibration error. The defensible version is one consolidated low-severity note on review-gate specificity covering 910/944/975/989.

2. It does not move the decision the doc asks for. The ask is at lines 216-219: "Adopt the hybrid model and use the companion-backed shell as the first production slice," plus the phase/estimate plan. All four columns of the comparison table (200-214) would carry the same security-review obligation, so tightening 910's wording changes no column and no estimate. Nothing about model selection turns on it.

3. The blast radius is bounded by the doc's own structure, contradicting "can be signed off with a one-line message and no evidence":
  - Line 952 states `This is the first production launch gate.` — that is PHASE 3, not Phase 1. Nothing reaches users on the strength of line 910, and the actual launch gate (950) is the one that carries a hard critical/high threshold. The doc reserved its rigor for the gate where it matters.
  - Phase 1's gate is not one bullet. Lines 904-909 are six concretely testable criteria including `10,000 injected lost-response retries produce no duplicate message` and `replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed`. Line 910 is a qualitative capstone on a mostly objective gate, not the sole barrier.
  - The doc has an established evidence convention a reader extends by default: line 885 `direct and forced-relay evidence is signed and bound to that SHA`, lines 105-107 signed evidence at exact revisions pointing to docs/verification-evidence.md. I confirmed both exist in-repo: `docs/verification-evidence.md` (25928 bytes) and `docs/evidence/` containing `v0.5.0` and `v0.6.0`. The "no evidence" scenario is not the ambient default here.

4. The proposed fix is an altitude mismatch. It is a ~60-word replacement in a document where every other gate bullet runs under 15 words; adopting it verbatim makes 910 the longest bullet in the doc by roughly 4x. The legitimate kernel is a ~10-word edit mirroring 950 (add a blocking-severity threshold).

VERDICT: refuted as filed. Not a drop — the 910-vs-950 asymmetry is real, the doc demonstrably knows how to state a threshold, and a one-clause fix exists. But it is a low-severity editorial consistency note that should be merged with lines 944/975/989, not a standalone high-severity MISSING.
````

### F26. Missing at lines 949

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 3 go/no-go gate: "load tests stay inside resource and cost ceilings" (line 949).

Evidence:

````text
`grep -n -i "ceiling" docs/production-deployment.md` returns exactly one line: `949:- load tests stay inside resource and cost ceilings;`. No ceiling is defined anywhere in the document. The cost section (lines 804-811) gives *starting estimates*, not ceilings, and is explicitly open-ended: line 811 `| Initial fixed total | Approximately $400 to $600 plus relay bandwidth |`, with line 815 warning `Browser peers are always relayed, so file traffic can dominate cost.` No resource targets exist either — no concurrent-peer count, message rate, relay connection cap, Worker CPU/request budget, or per-user bandwidth cap appears anywhere. `grep -n -i budget` returns only line 647 (`bundle-size budgets`), which is unrelated. The gate is therefore unfalsifiable: any load-test result satisfies an undefined ceiling.
````

Proposed fix: Before Phase 3 starts, define in the cost section: (a) a load profile (e.g. 200 concurrent paired sessions, 500 rooms, 5 msg/s aggregate, p95 event size); (b) hard ceilings — relay egress GiB/month, relay-auth Worker requests/day and CPU-ms/request, and a monthly all-in spend cap; (c) the alert threshold as a fraction of each ceiling. Then restate line 949 as "load tests at the profile in [Initial monthly cost model] stay within the published egress, request, and spend ceilings".

### F27. Missing at lines 1036, 1025, 1056

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The existing Flutter applications are never reconciled with the companion architecture — they appear in the document only as CI job names.

Evidence:

````text
`grep -n -i flutter` on the doc returns exactly two lines, 113 and 645, both listing "Dart/Flutter" as a CI gate name. `grep -c -i mobile` = 2 (lines 1025, 1056). Line 1036: "The first supported production matrix is desktop-focused". Line 1025 excludes "mobile background-availability claims". Line 1056 defers "Supported browser, desktop OS, and mobile matrix" to an ADR. But docs/platform-matrix.md:54-58 records three shipped-or-building Flutter surfaces (macOS app + DMG pipeline, Linux GTK app, Android app with in-process Rust engine) plus "iOS app | no scaffold or engine build", and PRODUCT.md:13-15 states as product definition: "Primary context is a desktop working session...; mobile is for checking in on rooms and agent runs on the go." The doc never says whether the Flutter apps continue, are deprecated, become companions, or are superseded by the PWA.
````

Proposed fix: Add a "Relationship to the existing Flutter applications" subsection stating, for macOS/Linux/Android: does each become the signed companion, remain an independent full peer, or get deprecated — and which phase owns that decision. Link docs/platform-matrix.md, which the doc cites once (line 156) but never reconciles.

### F28. Missing at lines 1063 vs 964-975

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Room-history compatibility for multi-device is named as an unknown but no phase gate tests a v2 device joining a v1-created room.

Evidence: Line 1063, unknown #3: "Multi-device compatibility with existing room membership history." The Phase 4 go/no-go gate at 964-975 tests browser/native signature and fold compatibility (line 969: "browser and native peers produce byte-compatible signatures and membership folds") and revocation (line 972), but no gate item exercises a v2 client against a room whose genesis and membership events were authored under v1 by the shipped v0.5.0 preview. `grep -c -i` for "existing room" in the doc = 1, and it is line 1063 itself.

Proposed fix: Add a Phase 4 gate item: a device authorized under protocol v2 joins and correctly folds a room created and populated by a v0.5.0 v1 client, using a preserved fixture room rather than a freshly created one.

Dissent from the verification panel:

````text
REFUTE (high): The finding's literal assertion is ACCURATE, but it fails the materiality lens decisively and should not appear in the review.

WHAT I CONFIRMED (so the reader knows coverage, not silence):
- Doc is 1087 lines total (`wc -l` = 1087), so line 1063 is real and near the end.
- Phase 4 gate at 964-975 verbatim contains exactly 8 bullets: browser matrix (966-967), "browser and native peers produce byte-compatible signatures and membership folds" (968-969), forced-relay browser/native (970), storage-clearing recovery (971), offline convergence (972), "a revoked device cannot author an accepted future event" (973), product copy (974), upstream-revision qualification (975). None names a room created BEFORE the multi-device upgrade. Literal claim: TRUE.
- Investigator's `grep -c -i "existing room"` = 1 — I reproduced it exactly (output: 1), and line 1063 is indeed the only hit.

WHY IT STILL FAILS MATERIALITY:

1. Triple self-awareness — the lens's explicit disqualifier. The doc flags this in THREE places, not one. (a) Line 1063, "Highest-risk unknowns" #3: "Multi-device compatibility with existing room membership history." (b) Line 1051, "Decisions that require an ADR" #4: "Multi-device and revocation event semantics." (c) The design section already walks the exact scenario at 356-360: "An existing authorized device pairs with the new device and asks the profile root to authorize its public device key" ... "The new device synchronizes signed history from current room peers or an explicitly invited availability peer." The finding's own primary evidence IS the doc telling the reader this is unresolved.

2. Sequencing makes the "missing" gate correct, not defective. ADR #4 (line 1051) has not decided multi-device/revocation event semantics. You cannot pre-write a conformance acceptance criterion for semantics an ADR has yet to fix. Naming an unresolved unknown and NOT pre-writing its test is normal planning structure. The finding effectively criticizes the doc for having an unknowns section.

3. "No gate item" != "untested" — standing requirements cover it outside the phase gate. Line 531: "One conformance corpus runs across native, browser, FFI, and fixture clients." Line 656 (per-PR CI): "protocol conformance, fuzzing, and malformed-frame tests." Line 652: "storage quota, eviction, corruption, and migration tests." Line 689 (rollback): "Runtime data migrations support N and N-1 or provide a forward-compatible read-only fallback." Line 846: `crates/jeliya-protocol/` holds "conformance fixtures." Gate bullets in this doc are summaries layered on these standing gates.

4. The evidence method is unsound. `grep -c "existing room" = 1` is used to imply conceptual absence, but the concept is present at 356-360 in different words. Phrase-frequency is not concept-coverage, and the investigator did not check for the concept under other phrasings.

5. The proposed fix rests on a partly fabricated premise. It asks to test against "a room created and populated by a v0.5.0 v1 client" using "a preserved fixture room." But there is no evidenced field population of v1 rooms to preserve: `git log --tags` shows the repo's first tag v0.1.0 is dated 2026-07-05 and v0.5.0 is 2026-07-14 (project is ~2 weeks old; HEAD is `v0.6.0-25-g7248fb0`); docs/index.md:33 calls this a "technical preview"; and the doc itself states at line 952 that the Phase 3 gate "is the first production launch gate." The genuine (forward-looking) concern is rooms created between the Phase 2/3 launch and Phase 4 — which line 689's N/N-1 migration requirement already speaks to. Note also that "v1" in the doc (line 371, `#v1.<base64url(...)>`) is the invitation-envelope version, not a protocol generation; the investigator's "v1 client / v2 device" framing conflates two different version namespaces.

6. Altitude mismatch. Every gate in the doc is 5-8 summary bullets. Phase 1's gate (902-910) omits keystore-migration tests; Phase 3's (944-950) omits service-worker update-poisoning. Demanding one more specific bullet in Phase 4 holds one gate to a standard no gate in the doc meets.

DECISION IMPACT: none. The doc asks for approval of a phased roadmap beginning at Phase 0. Phase 4 is 10-14 weeks of work that starts only after the first production launch, and its scope is explicitly excluded from the vertical slice (line 1019: "a browser-owned room identity"). Nothing here changes approve/reject or funding.

RESIDUAL: there is a thin, legitimate suggestion — once ADR #4 lands, convert unknown #3 into a Phase 4 gate bullet. That is a nit whose marginal information to a reader who already read lines 1051 and 1063 is approximately zero. Asserted severity "medium" is inflated by roughly two levels. Drop.

Classification note: this was filed as MISSING; the accurate classification would be DISAGREE-with-judgment (a suggestion to promote a self-declared unknown into a gate), since the doc neither omits nor misstates the issue.

Relevant absolute paths: /home/sekou/AGI/jeliya/docs/production-deployment.md, /home/sekou/AGI/jeliya/docs/index.md, /home/sekou/AGI/jeliya/Cargo.toml
````

### F29. Missing at lines 109-110, 768-770, 418-420

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Peer-offline — the most common failure in a P2P messaging product — receives no user-facing treatment, despite the protocol already reserving a field for it.

Evidence:

````text
Line 109-110: "There is deliberately no central inbox or guaranteed offline delivery." Line 768-770: "There is intentionally no service-availability claim when every room peer is offline." So for a two-person room the modal experience is that nothing arrives. The doc's queued-send design at 418-420 ("Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection") never connects to the extension point the protocol already reserved for exactly this: docs/PROTOCOL.md:292-297 — "A `TimelineEvent` MAY later carry an optional `\"delivery\": \"live\" | \"queued\" | \"resent\"` (absent => `\"live\"`)... This is why honesty rule 1 forbids a *delivered confirmation*, not a *pending/queued* state — the latter stays addable." `grep -c -i` in the doc for "last seen", presence-as-UX = 0.
````

Proposed fix: Specify the offline-peer user experience: what a queued message looks like in the timeline, whether the reserved `delivery` field from PROTOCOL.md:292-297 is adopted in protocol v2, and what copy explains to a user that their message will arrive only when the recipient comes online.

Dissent from the verification panel:

````text
REFUTE (high): The finding's central assertion — "peer-offline receives no user-facing treatment" — is factually wrong about the product, and its key negative evidence has its sign inverted.

VERIFIED ACCURATE: doc quotes are faithful (109-110, 419-420); the availability passage is at 767-769, not the asserted 768-770 (minor). PROTOCOL.md:292-297 is quoted verbatim and correctly. The doc genuinely never mentions the `delivery` field, never cites room-workbench.md, and `grep -c -i "last seen"` = 0. Peer-offline UX is genuinely absent from lines 1032-1071.

REFUTED (core claim): Peer-offline has a shipped, specified, localized user-facing treatment. docs/room-workbench.md:245 (Decision 4, "the status vocabulary") defines the peer-reachability vocabulary: `PeerStatus.state (connected|connecting|offline)` renders as Direct/Relay/Connected/Connecting/Offline, aggregate "No peers connected". Implemented at ui/src/components/RoomHeader.tsx:18 (`if (state === 'offline') return s.roomHeaderPeerStateOffline;`) and :60 (`roomHeaderNoPeersConnected`); strings at ui/src/l10n/en.ts:457,468 and fr.ts:503,514 (localization parity). RoomHeader.tsx:37-43 documents a FIXED REGRESSION on precisely this scenario: "'Alone in this room' used to render here whenever zero connections were observed — including in a five-member room whose peers are merely offline. Absence of an observed connection is not evidence of solitude." The doc lists "peer status" as working today (line 99) and includes "truthful direct/relay status" in the vertical slice (1011). It does not re-specify status chips because they exist and are documented in a dedicated design doc.

EVIDENCE SIGN-INVERSION: the investigator offers `grep "last seen" = 0` as proof of a gap. docs/PROTOCOL.md:594, honesty rule 4, binds the UI: "never fabricate progress or heartbeats, never extrapolate `last_seen`." Zero hits is compliance with a normative project rule, not omission — the fix as proposed would violate the honesty rules.

MATERIALITY (my lens): the decision at stake (lines 16-43) is the capability-aware hybrid — static PWA + signed companion, browser Wasm peer behind later gates, dedicated relays, optional server peers. No timeline-chip or delivery-marker design changes that decision. PROTOCOL.md:288-291 frames the reservation as headroom "named so adding it stays a non-breaking minor per the forward-compat rules"; a change explicitly engineered to be addable later without a protocol break cannot be a high-severity gap in a phased proposal. The finding's own strongest evidence undercuts its severity.

SELF-AWARENESS: present inline rather than in the unknowns block — "deliberately no central inbox" (109) and "intentionally no service-availability claim" (767), plus go/no-go gate 974 "product copy makes no durable background-availability claim," which directly governs offline-related copy.

SCOPE CONFLATION: doc lines 419-422 concern the browser↔companion link dropping, not a remote peer being offline; 421-422 does specify user-facing unavailability marking for that link. The finding treats the two as one.

RESIDUAL: a real but minor editorial improvement survives — the doc could cross-reference PROTOCOL.md:292-297 and room-workbench.md:245 where it discusses queued sends (419-420) and "idempotent sending" (1009). That is a low-severity cross-reference nit, not a MISSING/high architectural gap. Downgrade to low and reframe rather than drop entirely.
````

### F30. Missing at lines 327-338, 340-349

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: SAS pairing strings and recovery phrases are specified without any accessibility, localization, or non-visual-alternative design.

Evidence: Line 327-328: "The companion shows a QR or custom-protocol link containing an ephemeral public key, endpoint, and nonce". Line 331-332: "Both sides display a short authentication string and require user confirmation." Line 341-342: "Generate a random 256-bit recovery key, optionally represented as a recovery phrase." None of these says how a SAS is announced to a screen reader (character disambiguation), whether the recovery wordlist is localized or stays English, or what the non-visual alternative to QR is. The repo already binds this work: docs/accessibility-checklist.md:93-95 requires "The desktop app is operable with the keyboard alone from launch — including onboarding, which a pointer-only assumption tends to miss because it is seen once" — and pairing IS the new onboarding. docs/i18n.md rule 3 and the glossary Tier 2 never-translate list govern whether a token like a recovery phrase is copy or data. `grep -c -i` for "screen reader", aria, keyboard in the doc = 0.

Proposed fix: Specify in the pairing section: a manual-entry code path as the QR alternative, the SAS presentation and its screen-reader announcement, and an explicit decision (with reason, per docs/i18n.md rule 1) on whether the recovery phrase wordlist is translated or is a never-translate token.

### F31. Missing at lines 353-364, 899, 846

- Kind: `MISSING` | Severity: `medium` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Protocol v2 (line 353) is introduced with no migration path for existing preview users and no reference to the repo's normative wire-compatibility policy.

Evidence: Line 353: "Protocol v2 needs root-signed `device.authorized` and `device.revoked` events". `grep -c -i` over the doc = 0 for: "backward compat", "backward-compat", "wire compat", "migration path", "existing user", "preview user", "v1 peer", "v1 room", min_protocol, "major bump". docs/PROTOCOL.md:217-252 IS a normative policy the doc never cites, and its rule 3 (lines 240-242) is decisive: "A higher `protocol` is only assumed backward-compatible across the same major. A major bump may remove or reshape fields and requires an explicit client update." By the project's own policy, "protocol v2" is a breaking major bump requiring every existing v0.5.0 installation to update — a fact never stated, scheduled, or costed. The doc links PROTOCOL.md once (line 410) and only for browser-persistence context.

Proposed fix: Add a "Protocol v1 to v2 migration" subsection: state explicitly that v2 is a major bump under PROTOCOL.md rule 3, define the support window for v1 preview installs, and add a Phase 1 gate item that a v1-created room remains readable and joinable by a v2 client.

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as framed (MISSING / critical). It survives only as a low-severity documentation-completeness nit.

WHAT I CONFIRMED (the finding's true parts):
- docs/production-deployment.md:353 does read "Protocol v2 needs root-signed `device.authorized` and `device.revoked` events and multiple active device bindings per identity." Verified verbatim.
- docs/PROTOCOL.md:217-252 is a normative "Protocol version & forward compatibility" section, and rule 3 (240-242) reads exactly as quoted. Verified verbatim.
- The doc never cites that section. Its only PROTOCOL.md link is line 410, in browser-persistence context. Confirmed.
- crates/jeliya-core/src/engine.rs:34 — `pub const PROTOCOL_VERSION: u32 = 1;` — so the live wire major is indeed 1.

WHY IT FAILS THE MATERIALITY LENS:

1. The doc DOES schedule the migration machinery, and the investigator cited the line proving it without crediting it. Line 899 is a Phase 1 deliverable: "protocol version and capability negotiation." That is precisely the mechanism PROTOCOL.md:244-252 reserves for exactly this purpose ("`daemon.status` / `ready` line / portfile MAY gain `min_protocol` … `/ws` MAY accept a `?protocol=<n>` connect param, or the server MAY send a first `hello` frame carrying `{ protocol, min_protocol }`"). "No migration path" is not sustainable when negotiation is a named Phase 1 deliverable.

2. Sequencing is already correct. `device.authorized`/`device.revoked` are not Phase 1 — line 960 puts "root-signed device authorization and revocation" in Phase 4 (line 954, "Phase 4: browser peer and multi-device identity"), i.e. AFTER negotiation exists in Phase 1. Phase 4's gate (lines 968-969) requires "browser and native peers produce byte-compatible signatures and membership folds" — a cross-implementation conformance gate over exactly the surface at issue.

3. The doc IS self-aware, which the lens flags as decisive. In the very section the lens names (1032-1071):
   - Line 1051, ADR #4: "Multi-device and revocation event semantics." — the semantics of these two events are explicitly deferred to a required ADR.
   - Line 1063, highest-risk unknown #3: "Multi-device compatibility with existing room membership history." — this IS the finding's subject, already listed as a top-10 risk.
   The finding's proposed fix is a refinement of two items the doc already raises by name, not a missing surface.

4. The finding's decisive legal argument is overstated. Rule 3 (240-242) says what a major bump PERMITS and IMPLIES; it does not establish that adding two event kinds IS a major bump. Rules 1-2 (232-239) cut the other way and are explicitly designed for this case: "Clients MUST ignore unknown top-level keys… New optional fields are added without a major bump" and "Clients MUST ignore `TimelineEvent` `kind` values they do not recognize… This is what lets a lower-`protocol` peer coexist with a higher one in the same P2P room." So the repo's own policy makes new kinds presumptively NON-breaking. "By the project's own policy… a breaking major bump" does not follow from the text quoted.

5. The grep evidence is methodologically weak — over-specific bigrams that miss the doc's actual vocabulary. My own grep over the same file: "compatibility" at lines 205, 317, 647, 651, 968, 1063; "migration(s)" at 134, 434, 465, 652, 689. Line 689: "Runtime data migrations support N and N-1 or provide a forward-compatible read-only fallback." Line 692: "The companion can enforce a minimum-safe control-protocol version." A zero-count on "backward compat"/"migration path" does not mean the concept is absent.

6. Population at risk does not support "critical." The doc's own premise (118-128) is that nothing today is deployable — existing daemon artifacts are "subject to their technical preview and unsigned-package limitations" (124-125), and line 764-765: objectives "are not guarantees inherited from the current preview." `git tag` stops at v0.6.0 (0.x throughout). README.md:378 already warns the pinned SDK's experimental tier "can change on any release." The at-risk cohort is preview users on unsigned local-loopback builds, not a production install base. A v1 support window changes no decision the doc asks for: not provider selection, not phase ordering, not the cost model, not any go/no-go gate.

RESIDUAL TRUTH WORTH ONE LINE: line 353 says "Protocol v2" without ever stating whether that is a major bump under docs/PROTOCOL.md:217-252, and no gate anywhere asserts a v1-created room stays readable/joinable by a v2 client. That is a real but small gap, and its natural home is ADR #4 (line 1051), which already owns "Multi-device and revocation event semantics." Correct framing is "cite the normative forward-compat policy at line 353 and fold the v1 support window into ADR #4" — not "no migration path exists."
````

### F32. Missing at lines 554-563, 738-743

- Kind: `MISSING` | Severity: `medium` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Cloudflare (lines 554-557) and Iroh managed relays (line 558) are selected as infrastructure, and the doc acknowledges at line 738-740 that they "necessarily observe source IPs" — but no controller/processor determination, DPA, or DPIA is planned.

Evidence: Doc line 739-740: "CDN and relay providers necessarily observe source IPs. Treat access logs as sensitive." Doc line 546-547 additionally classifies "source IPs, endpoint routing, timing, and traffic volumes as sensitive metadata". Yet `grep -c -i` for "controller" in the data-protection sense = 0, "processor" = 0 (only line 383 "social-preview processors"), "data subject" = 0, "retention" = 0, "consent" = 0. No Art. 28 processor agreement, records of processing, DPIA, or transfer mechanism is named anywhere in lines 549-568 or 713-748.

Proposed fix: In the infrastructure section (549-568) add a subsection recording, for each provider: what personal data it processes (IP, timing, volume), the controller/processor role, the signed DPA, and the international-transfer mechanism. Add a DPIA to the Phase 0 deliverables at lines 872-879 — a public messaging service with an EU relay processing IP addresses is a standard DPIA trigger.

### F33. Missing at lines 628-637, 699-711

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Signing-key ceremony and custody are named as requirements but never specified, and the existing evidence-signing key is omitted entirely.

Evidence: Line 634: "Keep component root trust offline or in an HSM. Use delegated online signing keys with short validity." Line 636-637: "Keep Apple and Windows signing material in platform-approved secret/HSM services and fail closed when signing is unavailable." That is the whole treatment. `grep -c -i` over the doc = 0 for "key ceremony"; "custody" appears 4 times (lines 202, 1050, 1055, 1066) and never as a procedure. No M-of-N, no witnesses, no rotation schedule, no revocation drill, no what-if-lost. Three key classes exist and only two are mentioned: the third, the evidence-signing Ed25519 key, gets `grep -c "evidence key"` = 0 and `"evidence-signing"` = 0, despite docs/known-gaps-roadmap.md:37 recording "private-key custody is out of band" and line 102 listing it as an open item: "operate signing, notarization, and evidence keys with documented custody, rotation, and incident response". The deployment doc never links known-gaps-roadmap.md at all.

Proposed fix: Add a key-custody subsection covering all three key classes (component trust root, native code-signing, evidence-signing), each with generation ceremony, custodians, M-of-N or escrow, rotation cadence, and a rehearsed compromise drill. Link docs/known-gaps-roadmap.md, whose NEXT list at line 102 already owns this.

Dissent from the verification panel:

````text
REFUTE (high): The investigator's greps replicate, but the finding fails the materiality lens on both legs because its evidence base omits the parts of the doc that already allocate this work.

VERIFIED ACCURATE (I re-ran everything):
- `grep -n -i "ceremony" docs/production-deployment.md` → exit 1, no match. Correct.
- `grep -n -i "custody"` → exactly 4 hits: lines 202, 1050, 1055, 1066. Correct.
- `grep -n -i "evidence-signing\|evidence key"` → exit 1, no match. Correct.
- `grep -n "known-gaps" docs/production-deployment.md` → exit 1. The doc genuinely never links known-gaps-roadmap.md. Correct.
- docs/known-gaps-roadmap.md:37 does read "private-key custody is out of band". Correct. (The NEXT item is at line 100, not 102 — trivial off-by-two.)
- Lines 628-637 and 699-711 read as quoted.
- The evidence-signing key is real and operational: docs/capability-status.md:64 "the evidence key is provisioned"; docs/verification-evidence.md:23 "a detached Ed25519 signature verified against the pinned release-evidence key."
- docs/signing-notarization.md (linked from doc line 158) does NOT cover custody — one HSM mention at line 115, nothing on ceremony/rotation/escrow. So native code-signing custody is indeed unspecified repo-wide.

WHY IT STILL FAILS — the doc is far more self-aware than the finding admits. The investigator quotes only 634-637 and 699-711 and never engages four other passages that structurally own this work:
1. Line 859 (repository change map, a deliverables table): "`docs/runbooks/` | Deployment, rollback, relay failure, **key rotation**, and incident procedures". `ls docs/runbooks/` → "No such file or directory", i.e. this is explicitly a to-be-created deliverable. The finding's "no rotation schedule" is answered by a named artifact.
2. Line 697: "Create and **exercise** runbooks for:" followed by "native signing-key compromise" (701) and "component publisher compromise" (702). The finding calls this "the whole treatment" while ignoring that it is a deliverable list with a rehearsal verb — which is precisely the "rehearsed compromise drill" the proposed fix asks for.
3. Line 1055, in the doc's own unresolved-decisions section: "Component package metadata, **trust-root custody**, and WIT world" is listed as requiring an ADR. Line 1050 does the same for recovery-bundle custody; 1066 for user custody; 1068 lists native signing among highest-risk unknowns.
4. Line 879, Phase 0 deliverable: "confirm DNS, CDN, relay, and **signing ownership**."

REFUTED OUTRIGHT — "the existing evidence-signing key is omitted entirely." The doc's Phase 0 go/no-go gate at line 885 requires "direct and forced-relay **evidence is signed** and bound to that SHA," and line 873 requires reconciling "status, threat, evidence, and platform documentation." The evidence-signing operation is a blocking gate in the doc. The key is not *named* and its *custody* is unspecified — but "omitted entirely" is false.

ALTITUDE/DECISION MATERIALITY. The doc is an architecture proposal with a dependency-ordered roadmap that routes procedures to ADRs and runbooks by design. An M-of-N ceremony with witnesses and escrow is a runbook artifact, not a proposal artifact; the fix asks the proposal to do the job it has already assigned to `docs/runbooks/`. Sequencing is also correct: docs/signing-notarization.md:107 states "Not implemented — nothing in `release.yml` signs `jeliyad.exe` today, and none of the secrets below exist." You cannot write a custody ceremony for keys not yet procured, which is exactly why the doc makes procurement a planning assumption (line 1038) and a Phase 0 confirmation (879). Nothing here changes the go/no-go the doc asks for.

SURVIVING RESIDUE (low). Two cheap, concrete items: (a) the evidence-signing key should be named in the runbook/ADR list since it exists today and known-gaps-roadmap.md:100 already owns it ("operate signing, notarization, and evidence keys with documented custody, rotation, and incident response"); (b) the doc should link known-gaps-roadmap.md, which it verifiably never does. That is a documentation-hygiene note worth one line in a review — not a high-severity MISSING finding. Correct classification is DEFERRED-BY-DESIGN, not MISSING.
````

### F34. Missing at lines 665, 647 vs 800-811

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Staging and test relays are required by the CI/CD plan but the cost model prices exactly two relays, understating infrastructure by roughly $389-583/month.

Evidence:

````text
Line 665: "Staging smoke and compatibility suites run against dedicated staging relays" (plural, dedicated). Line 647 adds a further one: "real companion integration through a dedicated test relay, not only a mock". Line 572-573 also creates a separate `staging.app.jeliya.ai` origin, and line 578-579 requires separate "development, staging, and production origins, relay projects, trust roots, credentials". The cost table prices only line 807: "Two managed Iroh relays | Approximately $389 before bandwidth/SLA", derived at line 800-801 from "$0.27 per hour" — arithmetic I confirmed: 0.27 x 24 x 30 = $194.40 each, x2 = $388.80. Two staging relays plus one test relay at the same rate add ~$583/month, roughly doubling the stated "Approximately $400 to $600" total at line 811.
````

Proposed fix: Add staging and test relay rows to the cost table at 804-811, or state the mitigation (e.g. staging relays run only during CI windows) with the resulting hourly math, so the total at line 811 matches the environments the CI/CD section at 660-666 actually requires.

### F35. Missing at lines 697-711, 549-568

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: No security disclosure plan exists for the hosted origin; the document never references the repo's SECURITY.md and plans no security.txt or advisory process.

Evidence: `grep -c` over the doc = 0 for: SECURITY.md, security.txt, CVE, "coordinated disclosure", "responsible disclosure". "advisory" appears once (line 705, "dependency advisories") in the runbook list. SECURITY.md exists at the repo root and docs/security-threat-model.md:225 points to it ("see `SECURITY.md` for private reporting"), so the plan is dropping an existing artifact rather than having none. SECURITY.md's scope notes (lines 32-54) cover the agent runner, the loopback daemon bind, and unsigned release binaries — they say nothing about the two new attack surfaces this plan creates: the companion control ALPN and `relay-auth.jeliya.ai`. The incident runbook list at 699-706 covers internal response but no external reporter path.

Proposed fix: Add to Phase 3 deliverables (934-940): publish `/.well-known/security.txt` (RFC 9116) at app.jeliya.ai, and update SECURITY.md's scope notes to cover the companion control protocol and the relay-auth Worker. Define whether hosted-origin vulnerabilities get GHSA/CVE identifiers, since a silently-patched hosted origin leaves users no way to know they were exposed.

### F36. Missing at lines 830-859 (change map); jeliya-ffi appears only at 80-81

- Kind: `MISSING` | Severity: `medium` (asserted `critical`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: `jeliya-ffi` — one of exactly three workspace crates and the entire mobile in-process transport — is absent from the repository change map, while the plan proposes restructuring the crate it depends on.

Evidence: `grep -n "jeliya-ffi"` on the doc returns only lines 80-81: "A full `cargo test --locked --workspace` could not build `jeliya-ffi` because the local environment lacked Dart SDK headers" — i.e. it appears once, as a local build failure, never as a surface. The change map (lines 832-859) lists `crates/jeliya-core`, `crates/jeliyad`, and eight NEW crates, but not `jeliya-ffi`. Yet line 842 proposes for jeliya-core: "Split host-independent protocol/runtime behavior from native persistence and network assumptions", and `crates/jeliya-ffi/Cargo.toml:16` reads `jeliya-core = { path = "../jeliya-core" }`. Cargo.toml:2 confirms members are exactly `["crates/jeliya-core", "crates/jeliyad", "crates/jeliya-ffi"]`. The separate `dart/jeliya_protocol` package (docs/platform-matrix.md:60) is likewise unmentioned.

Proposed fix: Add rows to the change map (832-859) for `crates/jeliya-ffi` and `dart/jeliya_protocol` stating what happens to them under the jeliya-core split — whether the FFI shim retargets `jeliya-runtime` + `jeliya-platform-native`, and who owns that migration and in which phase.

### F37. Missing at lines 837-841, 643-658

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The enforced React i18n literal scanner does not cover any of the directories this plan creates, so new user-visible copy would ship untranslated with CI green.

Evidence: `scripts/check-ui-i18n.mjs:67`: `const LITERAL_SCAN_ROOTS = Object.freeze(['ui/src/App.tsx', 'ui/src/components']);` — with the comment at :65-66 "Files rule 5 scans. Everything else in `ui/src` is either not copy or is the l10n layer itself." The doc's five new frontend areas at lines 837-841 are `ui/src/sw.ts`, `ui/src/runtime/`, `ui/src/storage/`, `ui/src/pairing/`, `ui/src/invites/` — none under those roots. The gate runs in CI (.github/workflows/ci.yml:78 "Verify web localization catalogs", :82 `node scripts/check-ui-i18n.mjs`), so it will PASS while pairing and storage copy sits outside the catalog. The scanner file itself flags the hazard at :73-75: "These are listed even where they sit outside `LITERAL_SCAN_ROOTS` today: the reason is the durable part, and widening the scan must not silently sweep them in." The doc never mentions widening it.

Proposed fix: Add to the change map at 837-841 an explicit note that each new directory must be added to `LITERAL_SCAN_ROOTS` in `scripts/check-ui-i18n.mjs` in the same PR that creates it, and add "web localization catalog gate covers all new UI directories" to the PR gate list at 643-658.

Dissent from the verification panel:

````text
REFUTE (high): Every technical leg checks out, but the finding does not survive the materiality lens at MISSING/high.

VERIFIED ACCURATE: scripts/check-ui-i18n.mjs:67 is literally `const LITERAL_SCAN_ROOTS = Object.freeze(['ui/src/App.tsx', 'ui/src/components']);` with the comment at :65-66 and the widening caveat at :73-75. Doc lines 837-841 do list ui/src/sw.ts, ui/src/runtime/, ui/src/storage/, ui/src/pairing/, ui/src/invites/, none under those roots. The gate does run in CI (.github/workflows/ci.yml:78 "Verify web localization catalogs", :82 `node scripts/check-ui-i18n.mjs`). And the doc is genuinely NOT self-aware: `grep -ni "i18n\|localiz\|translat\|l10n\|locale\|catalog" docs/production-deployment.md` returns zero hits across all 1087 lines, so the assumptions/unknowns block at 1032-1071 does not cover it. That silence is the finding's strongest leg.

WHY IT FAILS ANYWAY:

(1) The premise contradicts the repo's own convention. `ls ui/src/` shows App.tsx, components/, l10n/, lib/, main.tsx. `ls ui/src/lib/` shows 30+ modules — invite.ts, join.ts, rooms.ts, client.ts, roomList.ts, qr.ts, shell.ts — all deliberately OUTSIDE the scan roots, with only four named EXEMPT_FILES (mock.ts, diagnostics.ts, tokens.ts, format.ts). The scanner states the design at :65-66: "Everything else in `ui/src` is either not copy or is the l10n layer itself." The four proposed directories (runtime/, storage/, pairing/, invites/) are structural peers of lib/ — logic modules — and under the established convention their rendered copy lands in ui/src/components/, which IS scanned. The doc is not deviating from the convention; the investigator is asserting a convention change the repo has never made, then charging the doc for not adopting it.

(2) The headline overstates the gap. Doc line 643-644 already commits every PR to "the existing Rust, Dart/Flutter, TypeScript, documentation, secret, release, and dependency gates," and check-ui-i18n.mjs runs inside the `docs-ui` job named "docs + TypeScript + release contracts" (ci.yml:25-26). So the gate DOES run under this plan, over the location copy lives by convention. The true residual gap is only "the doc does not say to widen the roots," which is materially smaller than "the gate does not cover this and CI stays green on untranslated copy."

(3) Altitude/decision-relevance. This is a ~1087-line production-architecture approval doc covering hosting model, a six-crate split, pairing transcript, storage/eviction, relay economics, and signing. The change map at 837-841 is one line of responsibility per area. "Add each new dir to a lint's root array in the same PR" is implementation-PR hygiene that changes nothing about the go/no-go, phasing, cost, or risk posture the doc is asking approval for.

(4) Severity calibration. The doc's own highest-risk unknowns (1063-1071) are browser relay-auth token issuance and proof-of-possession, CDN/browser-origin compromise and maximum web-controller authority, recovery custody for an accountless identity. Ranking a lint-scope maintenance note alongside those is inflation.

RESIDUAL VALUE: ui/src/pairing/ is described in the doc's own words at line 840 as "revocation UI," and the repo demonstrably cares about EN/FR parity (issue #74 merged at HEAD 7248fb0). So a one-sentence forward-looking note is defensible — but as a low-severity hygiene remark, not a high-severity MISSING plan defect. Downgrade to low; do not present it as a gap that affects the decision.
````

### F38. Missing at lines 869-888 (Phase 0), 643-658

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The accessibility gate is documented as non-blocking and the plan never fixes that before a public launch.

Evidence: docs/accessibility-checklist.md:101-103: "`ui-e2e` is not currently in the repository's required status checks, so the accessibility gate runs on every pull request but does not yet BLOCK a merge. Adding it is a branch-protection change, outside any pull request's diff." Confirmed in CI: .github/workflows/ci.yml:102-103 defines the `ui-e2e` job ("UI browser regression (Playwright)"), which is where `ui/e2e/a11y-matrix.spec.ts` runs. The deployment doc's Phase 0 reconciles "status, threat, evidence, and platform documentation" (line 873) and confirms "DNS, CDN, relay, and signing ownership" (line 879) but never touches branch protection or the a11y gate; `grep -c -i` for "branch protection", "required check" = 0.

Proposed fix: Add to the Phase 0 deliverables at 872-879: make `ui-e2e` a required status check, closing the known gap recorded at docs/accessibility-checklist.md:101-103 before any surface is publicly reachable.

### F39. Missing at lines 202, 1044, 275-277, 1064

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "origin compromise can still read displayed content and use granted scopes" / "The product accepts that a hosted first-party origin can observe the content it renders and actions within its granted scope."

Evidence: "Use granted scopes" is a euphemism for the single most important residual risk in the design, and the document never states it plainly. The word "impersonate" appears exactly once (line 297) and is applied to server peers, never to the browser origin. What actually happens: the companion signs with the user's device key (crates/jeliya-core/src/identity.rs:46-51, `device` key "Signs events; signatures verify under `device_id`"; supervisor.rs:1568 `send_message`). Messages authored at a compromised origin's direction are therefore cryptographically indistinguishable from genuine ones to every other room member, and they are permanent: the doc's own TB4 (line 285) says "Signatures prevent forgery; they do not prevent an authorized peer from copying content", and line 499 says "Never rewrite signed room history during rollback." So the residual is not "the origin can act within scope" — it is "the origin can author non-repudiable statements attributed to the user, in signed history, that survive revocation, rollback, and the incident response at lines 707-711."

Proposed fix: Add an explicit paragraph under TB1 (after line 277): "Companion mode protects the KEY, not the SIGNATURE. A compromised origin cannot steal the identity key, but it can cause the companion to author signed events attributed to the user. Those events are non-repudiable, are replicated to every room member, and cannot be recalled by control-key revocation or by frontend rollback. This is the accepted residual risk of the companion model and must be stated in the product's security copy." Reword line 1044 accordingly and promote it into `docs/security-threat-model.md`'s residual-risk list (line 207).

Dissent from the verification panel:

````text
REFUTE (high): VERIFIED AS ACCURATE (the investigator's factual substrate):
- crates/jeliya-core/src/identity.rs:47-50 confirms `identity` "Signs the device binding (authorizes `device_id` under `sender_id`)" and `device` "Signs events; signatures verify under `device_id`". Confirmed by direct read.
- crates/jeliya-core/src/supervisor.rs:1568 `pub async fn send_message(&self, room_id_str: &str, body: &str) -> CoreResult<String>` exists. Confirmed via grep.
- Doc line 297 is the only occurrence of "impersonat*" (grep -n -i "impersonat" returns exactly one hit), and it is applied to the optional server peer, not the browser origin. Confirmed.
- Doc line 284-285 (TB4) and line 499 are quoted correctly.
So the underlying security property is real: a compromised origin can drive the companion into signing events attributed to the user, and those are unrecallable.

WHY IT IS REFUTED UNDER THE MATERIALITY LENS:

1. The doc is already self-aware, in BOTH sections my lens flags. Line 1043-1044 is a stated *planning assumption*: "The product accepts that a hosted first-party origin can observe the content it renders and actions within its granted scope." Line 1064 is *highest-risk unknown #4*: "Browser-origin/CDN compromise and the maximum authority granted to a web controller." The investigator cites both lines yet classifies the item as MISSING. A risk that appears as an explicitly accepted assumption AND as a top-10 unknown requiring resolution is not missing.

2. The claim "the document never states it plainly" is directly contradicted by two operational lines the investigator did not cite. Line 334: "Default scopes cover selected-room reads and idempotent chat sends only." Line 420: "Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection." Together these say, in plain language, that a browser controller's default grant is a write, and that the companion signs browser-originated sends. Line 917 repeats it ("scoped chat-only browser controller"). Every link in the investigator's chain is stated in the doc; what is absent is only the assembly of those links into one sentence containing the word "non-repudiable."

3. It does not move the decision the doc asks for (lines 216-219: "Adopt the hybrid model and use the companion-backed shell as the first production slice"). The comparison at line 202 puts the companion column against the static-PWA column, which reads "origin or CDN compromise can sign or exfiltrate" — i.e. unbounded signing plus key theft. The companion model is chosen precisely because it reduces that to a bounded grant. Stating the residual more forcefully makes the chosen option look no worse relative to the alternatives, because every alternative in the table is equal or worse on this exact axis. The finding cannot flip the recommendation.

4. The severity argument leans on scope-bounding the doc actually specifies. Lines 335-337 restrict the default grant (invites, files, pipes, identity operations, and agents each require separate approval) and make control keys "rate-limited, expire, and can be revoked immediately." The investigator is right that revocation cannot recall already-signed events — but that is an unavoidable property of any delegated-authority or signed-log system, and the doc concedes it explicitly at TB4 (line 284-285) and line 499. Line 1052 further defers "Multi-device and revocation event semantics" to an ADR. This is deferred-with-acknowledgement, not concealed.

WHAT SURVIVES (small): line 1044's phrasing "observe the content it renders and actions within its granted scope" is read-biased. "Observe ... actions" can be skimmed as passive watching rather than authorship. Since that line is the doc's designated plain-language statement of accepted risk, naming the write/authorship dimension there is a fair editorial improvement. That is a wording note, not a missing risk — consistent with the proposed fix itself, which is "add a paragraph" and "reword line 1044."

Classified MISSING/high is wrong on both axes: it is not MISSING (flagged at 1044 and 1064, operationalized at 334/420/917), and it is not high (does not affect the decision, and the chosen option dominates the alternatives on this axis). Downgrade to low as a one-line copy-precision note against line 1044; do not present it as an unstated residual risk.
````

### F40. Missing at lines 273-287, 856, 692

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: The trust boundary list TB1-TB5.

Evidence: Two more boundaries are absent. (1) Build/CI: TB1 covers "a compromised origin, CDN account, or frontend dependency" but not the pipeline that produces the artifacts. The companion is the root of the entire trust model, and `\.github/workflows/companion-release.yml` (line 856) plus the Apple/Windows signing material (lines 636-637) constitute a boundary whose compromise forges a trusted, notarized key-holding binary. The incident runbooks list "native signing-key compromise" (line 702) — a response for a boundary that is never declared. (2) The companion's own UPDATE channel is entirely absent: the doc specifies TUF-style root/targets/snapshot/timestamp with explicit "rollback protection" and "revocation" for COMPONENTS (lines 468-469) but nothing equivalent for the companion binary. This makes line 692 ("The companion can enforce a minimum-safe control-protocol version") a paper control: an attacker who can influence the update channel downgrades the COMPANION to a build whose pairing or scope logic is known-broken, and the minimum-version check is in the code being downgraded.

Proposed fix: Add TB6/TB7: "Build and release pipeline" (compromise yields a signed artifact that every other boundary trusts; controls are the line 640-671 gates, reproducible builds, and two-person release approval) and "Companion update channel" (compromise or downgrade yields arbitrary native code with key access; controls are signed update metadata with monotonic version/anti-rollback and a signed revocation list, mirroring lines 468-469 and 500-501). Add a companion anti-rollback requirement to Phase 2's deliverables (line 918).

### F41. Missing at lines 330, 337, 909

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "Both sides display a short authentication string" / "Control keys are rate-limited, expire, and can be revoked immediately."

Evidence:

````text
grep -n -i "digits\|SAS" docs/production-deployment.md returns lines 840, 909, 1005 and 330 — no length is specified anywhere in the document. RFC 6189 (https://www.rfc-editor.org/rfc/rfc6189.txt) sets the reference bar: "sasvalue = sashash [truncated to leftmost 32 bits]" and "A 16-bit SAS, for example, provides the attacker only one chance out of 65536 of not being detected." Separately, line 337's rate-limiting is scoped to "Control keys" — i.e. to an already-paired key — not to pairing ATTEMPTS. Nothing in the doc bounds how many times an attacker may restart the pairing handshake, which converts a one-shot 2^-n MITM gamble into an online guessing attack at n bits per try. At a plausible-looking 4 decimal digits (13.3 bits) an attacker succeeds within ~10^4 attempts.
````

Proposed fix: Specify in the pairing section: SAS is >= 20 bits rendered as 6 decimal digits or 4 words from a fixed wordlist; the companion aborts the pairing session and invalidates the QR on the FIRST SAS rejection (no retry on the same ephemeral); and the companion enters a fail-closed lockout (e.g. no new pairing for 60s, escalating) after 3 consecutive failed pairings. Add to the Phase 1 gate (line 909): "N consecutive failed SAS confirmations lock out further pairing attempts."

Dissent from the verification panel:

````text
REFUTE (high): The finding as framed does not hold up under the materiality lens, and its central factual assertion is wrong.

REFUTING EVIDENCE:

(1) FACTUALLY WRONG CORE ASSERTION. The investigator states "Nothing in the doc bounds how many times an attacker may restart the pairing handshake." Line 788, under the "### Abuse controls" heading at line 785, reads verbatim: "- per-IP and per-endpoint handshake, connection, byte, and rate limits;". Handshake attempt limiting IS specified in the document. The entire escalation from "SAS length unstated" (a real but small gap) to "converts a one-shot 2^-n MITM gamble into an online guessing attack" rests on that assertion, and it collapses. The investigator's grep pattern ("digits|SAS") could not have surfaced line 788, so this looks like an artifact of a too-narrow search rather than a read of the security section.

(2) THE DOC IS EXPLICITLY SELF-AWARE — the specific lens test. Line 1046 opens "### Decisions that require an ADR"; item 2 at line 1049 is "Companion control protocol and pairing transcript." SAS encoding length and retry/abort policy ARE the pairing transcript. The doc has not omitted this decision; it has deliberately scheduled it. Line 910 adds the catching gate: "independent security review approves the wire formats and key lifecycle." Line 909 already requires "replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed". Per the lens instruction, a doc that already flags the item makes the finding much weaker.

(3) THE CITED BAR DOES NOT SUPPORT THE PROPOSED FIX. I fetched https://www.rfc-editor.org/rfc/rfc6189.txt independently. Both investigator quotes are literally accurate: "sasvalue = sashash [truncated to leftmost 32 bits]" (Sec 4.5.2) and "A 16-bit SAS, for example, provides the attacker only one chance out of 65536 of not being detected" (Sec 4.4.1.1). But Sec 4.4.1.1's actual mechanism is a hash commitment that "constrains the attacker to only one guess to generate the correct Short Authentication String" — ZRTP specifies NO consecutive-failure lockout counter. The headline element of the proposed fix (3-strike escalating lockout) is absent from the very standard invoked as the reference bar. Line 329's "Noise XX-equivalent authenticated transcript" provides the equivalent per-session transcript binding.

(4) WRONG THREAT MODEL. Line 327-328: "The companion shows a QR or custom-protocol link containing an ephemeral public key, endpoint, and nonce, never a reusable bearer secret." The ephemeral key is delivered over an out-of-band optical channel, so the SAS is defense-in-depth on an already-authenticated channel, not the sole authenticator as in ZRTP's bare-voice model. An attacker must first defeat the QR display. Additionally, every one of the postulated ~10^4 attempts requires a human to be shown a mismatched string and to confirm it anyway — this is not an unattended guessing oracle.

(5) ALTITUDE / DECISION MATERIALITY. Verified by ls and grep: crates/ contains only jeliya-core, jeliyad, jeliya-ffi; crates/jeliya-control/ (doc line 850) and ui/src/pairing/ (doc line 840) do not exist; `grep -rn -i "sas\b|short auth"` over crates/ and ui/src returns zero hits. Frontmatter lines 7-8 are status: "proposal" / implementation_status: "planned", and lines 22-23 state "It does not authorize a production deployment by itself." The decision being asked for is acceptance of an architecture direction, not sign-off on an implementable pairing wire spec. Demanding a bit-count for an unbuilt component whose protocol is explicitly ADR-deferred is a spec-review comment misfiled as an architecture blocker.

WHAT LEGITIMATELY SURVIVES: the narrow observation that line 330 gives no length is true, and it is mildly inconsistent with the doc's own habit of quantifying security parameters elsewhere (256-bit recovery key, line 341; 30-minute/24-hour invite expiry, lines 386-387; 10,000 injected retries, line 906; 1,000 pairing/revocation cycles, line 924). "State a minimum SAS entropy the way you stated 256-bit for recovery" is a fair low-severity polish note. It is not a missing high-severity control, and the accompanying online-guessing attack narrative should not appear in the review at all.

Refuted as filed: classification MISSING at severity high is not supportable. Downgrade to low, retaining only the specificity nit.
````

### F42. Missing at lines 377-385, 401-402

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: The invite-fragment control set: fragment-only URL, `history.replaceState()`, `Referrer-Policy: no-referrer`, never stored in localStorage/IndexedDB/Cache/logs.

Evidence: `history.replaceState()` fixes the address bar and the session history entry; it does not undo the visit record. Mozilla bug 753264 (https://bugzilla.mozilla.org/show_bug.cgi?id=753264, "history.replaceState creates NEW history items") documents that replaceState URLs "get added to the global history" (places.sqlite) while affecting only session history — the original navigated URL is recorded at navigation time and replaceState does not remove it. With Firefox Sync or Chrome Sync enabled, that record is then uploaded off-device. The doc's disclosure list at lines 401-402 enumerates "Browser extensions, screenshots, copied links, and OS clipboard managers" and omits the two most automatic channels: the local history database and history sync. Session restore after a crash is a related, narrower window (the session store is flushed periodically and can capture the pre-replaceState URL).

Proposed fix: Add to lines 401-402: "the browser's own history database and any enabled history sync also retain the navigated URL including its fragment; `history.replaceState()` does not remove an already-recorded visit." Then add a structural mitigation to the control list: default invite delivery to the short 30-minute single-use live-pairing window (line 386) precisely because the URL cannot be un-recorded, and state in the product copy that an invite link that has been opened is spent.

### F43. Missing at lines 420, 899, 906

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection" / Phase 1 delivers "`client_msg_id` idempotency" with the gate "10,000 injected lost-response retries produce no duplicate message".

Evidence: The doc never states the SCOPE of the deduplication key, and the gate at line 906 only tests the cooperative case. Two adversarial cases follow. (1) Local suppression: if the companion dedupes on `client_msg_id` alone, a malicious controller can pre-burn ids and suppress a legitimate send from another paired device or from the native UI. (2) Cross-peer suppression, which is worse: PROTOCOL.md:364-367 says the field "requires a field on the signed `iroh-rooms` content", so `client_msg_id` becomes an attacker-choosable value inside SIGNED history, visible to every room member. Any receiving peer that dedupes on `client_msg_id` (or on anything less than the full sender+device tuple) can be made to drop a victim's genuine message by a hostile member who authors an event carrying the same id first. TB4 (line 285) already establishes that room members are semi-trusted, so this is in-model.

Proposed fix: Specify normatively: "The deduplication key is the tuple (sender_id, device_id, room_id, client_msg_id). Deduplication MUST NEVER be applied across senders or devices. `client_msg_id` is at least 128 bits of local randomness and is never used as an authorization or ordering input." Extend the line 906 gate with an adversarial case: "a hostile member replaying another member's client_msg_id does not suppress, reorder, or replace that member's message on any peer."

Dissent from the verification panel:

````text
REFUTE (high): VERIFIED AS FACT (the investigator's raw evidence checks out):
- docs/production-deployment.md:419 "Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection" — confirmed verbatim.
- Line 906 gate "10,000 injected lost-response retries produce no duplicate message" — confirmed verbatim; it is purely cooperative.
- docs/PROTOCOL.md:364-366 confirmed verbatim: "an optional `client_msg_id` the daemon echoes into the event for exactly-once reconciliation — but that requires a field on the signed `iroh-rooms` content, so it is named here, not yet implemented."
- TB4 at lines 283-285 confirmed: room peers are semi-trusted.
- No dedup-key scope is specified anywhere. `grep -rn "client_msg_id"` over the repo returns exactly 5 hits (crates/jeliya-ffi/src/lib.rs:16, docs/PROTOCOL.md:364 and :528, docs/production-deployment.md:419 and :895) — all of them naming the field as reserved headroom, none defining semantics. `grep -rn "deduplication key\|sender_id, device_id"` returns nothing relevant.

CITATION ERROR: the finding cites line 899 for "`client_msg_id` idempotency". Line 899 is "protocol version and capability negotiation"; the correct line is 895.

WHY IT FAILS THE MATERIALITY LENS:

1. Altitude mismatch. Lines 894-900 list all seven Phase 1 deliverables as bare noun phrases — "recovery bundle and OS-keystore abstraction", "incremental timeline cursor", "invite default expiry and cancellation", "companion pairing/control protocol", "store-hole detection, repair, or fail-loud response". None carry normative field semantics. Demanding a normative key tuple, a 128-bit entropy floor, and a MUST NEVER clause for one of the seven holds a single bullet to a spec-level standard the doc applies nowhere. That is a PROTOCOL.md-level edit misfiled against a phase plan.

2. The doc already routes this class of question through a named gate. Line 910: "independent security review approves the wire formats and key lifecycle." Adding a field to signed iroh-rooms content (PROTOCOL.md:365-366) is definitionally a wire format. The risk is not unowned; Phase 1 cannot exit without exactly this review. The finding does not surface an escape from the doc's own controls.

3. The attack requires an implementation nobody proposed. PROTOCOL.md:359-364 scopes the field's entire purpose to one case: "if a send fails with `connection_lost` *after* the daemon already authored the event, a retry authors a **second** event" — a sender reconciling its own pending send against its own echo. Cross-sender dedup on a receiving peer is not implied, hinted, or required by that design; it would be a novel misreading invented by the reviewer and then attributed to the doc.

4. Adversarial case (1) additionally self-refutes. Pre-burning another device's ids requires predicting them. The investigator's own proposed fix ("at least 128 bits of local randomness") is the mitigation, and any UUIDv4 — the default any implementer reaches for — defeats it. A concern that evaporates under the most obvious implementation is not a high-severity architecture gap.

5. It moves no decision. This doc asks for phase approval, sequencing, and budget. With or without the tuple, Phase 1 contains the same item, at the same 3-to-5-week estimate, in the same order, behind the same gates. Nothing about scope, cost, trust boundaries, or the risk ranking at lines 1058-1071 changes.

SELF-AWARENESS CHECK (lines 1032-1071): the specific issue is NOT listed. ADR item 4 "Multi-device and revocation event semantics" is adjacent but not the same. So the doc is not explicitly self-aware here — this is the finding's strongest remaining leg, and it is why I land on "low" rather than "drop".

RESIDUAL WORTH KEEPING (low): the gate at 906 is cooperative-only, while line 909 shows the doc does write adversarial gates where it judges them warranted ("replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed"). Gates are the doc's native altitude. And cross-sender dedup is the kind of latent bug honest testing never triggers, since 128-bit random ids never collide by accident. So one extra clause on the 906 gate is cheap, in-altitude, and defensible. That is a minor gate-hardening note — not a MISSING architectural gap, and not high.

VERDICT: refuted as filed. The classification (MISSING) and severity (high) do not survive; the underlying observation survives only as a low-severity suggestion to add a hostile-member case to the line 906 gate, with the normative-spec language redirected to PROTOCOL.md where the field is actually defined.
````

### F44. Missing at lines 539-543, 555-556, 787-788

- Kind: `MISSING` | Severity: `medium` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "A browser obtains a short-lived, endpoint-bound relay credential from `relay-auth.jeliya.ai` after proof of possession. The project API secret never enters static assets."

Evidence: The design keeps the project secret off the client and then hands out its output to anyone who asks. Proof of possession of WHAT? There are no accounts (line 184: "There is no tenant, account, authorization-domain, quota, or public audit model"; line 1066 lists "an accountless identity" as a risk), and a Jeliya/Iroh endpoint identity is just an Ed25519 keypair anyone can generate offline in microseconds. So a signature over a challenge proves only "I generated a keypair" — it is not an admission decision. The Worker holds the real credential: per iroh's own docs (https://docs.iroh.computer/add-a-relay) "Your dedicated relays require authentication by default. Your endpoint authenticates to them with your project's API key. The `iroh_services::preset()` builder ... mints a short-lived access token scoped to your endpoint's key." So `relay-auth.jeliya.ai` is, as specified, an open oracle that converts "any keypair" into "paid relay capacity billed to Jeliya". The abuse controls at line 788 do not close it: "per-endpoint" limits are meaningless when endpoints are free and unlimited, leaving only per-IP, which any proxy pool defeats. Cost consequence: line 802 and 807 state the $389/mo figure is "before bandwidth or SLA charges" and the formula at lines 820-824 has an unbounded `relayed GiB * provider egress rate` term.

Proposed fix: Replace "after proof of possession" with a stated admission rule and put it in Phase 0/3 gates. Options to record in ADR #1 (line 1048): bind minting to a first-party artifact the attacker does not have (e.g. the credential is issued only to a control key already paired to a companion, proven by a companion-countersigned attestation); or require a privacy-preserving rate token (Privacy Pass / PAT) so anonymity is preserved without free unlimited minting; or accept open minting and add a hard monthly egress cap with automatic shed. Add to line 788: "per-issuance and per-ASN quotas, a global daily minting budget, and an alert plus automatic cutoff at a stated GiB ceiling." Add to highest-risk unknowns: "unauthenticated relay-credential abuse as an uncapped cost channel."

Dissent from the verification panel:

````text
REFUTE (high): MISCATEGORIZED AND MASSIVELY OVER-SEVERE. The topic the finding calls "MISSING" is explicitly enumerated by the doc as an open question, in three separate places the investigator did not cite.

WHAT I VERIFIED AS ACCURATE IN THE FINDING:
1. Doc lines 539-543 read verbatim: "A browser obtains a short-lived, endpoint-bound relay credential from `relay-auth.jeliya.ai` after proof of possession. The project API secret never enters static assets." Confirmed at /home/sekou/AGI/jeliya/docs/production-deployment.md:539-541. The phrase "proof of possession" is indeed left undefined — the doc never states what is possessed or what admits.
2. Line 184 verbatim: "One daemon data directory represents one user identity. There is no tenant, account, authorization-domain, quota, or public audit model." (183-184). Accurate.
3. Line 1066 verbatim: "5. Recovery usability and user custody for an accountless identity." Accurate.
4. Line 788 verbatim: "per-IP and per-endpoint handshake, connection, byte, and rate limits;". Accurate.
5. Lines 801-802 and 807: "$389 per 30-day month before bandwidth or SLA charges" / "Approximately $389 before bandwidth/SLA". Accurate.
6. Lines 820-824 formula contains "+ relayed GiB * provider egress rate". Accurate.
7. The iroh URL quote is REAL. I fetched https://docs.iroh.computer/add-a-relay and it returns verbatim: "Your dedicated relays require authentication by default. Your endpoint authenticates to them with your project's API key." and "The `iroh_services::preset()` builder handles this for you: it mints a short-lived access token scoped to your endpoint's key and configures the endpoint to use your relays." The investigator quoted the source correctly. (It also adds that custom relay URLs are Pro/Enterprise-only, which is consistent.)

WHY THE CLASSIFICATION FAILS ANYWAY — the "MISSING" item is present:
- Line 1062, under "### Highest-risk unknowns" (heading at line 1058), item #2 reads verbatim: "Browser relay-auth token issuance and proof-of-possession behavior." The doc's own risk register already names relay-auth issuance AND proof-of-possession as an unresolved top-risk unknown. The investigator's proposed fix literally asks to "Add to highest-risk unknowns" an item on relay-credential minting — it is already there, one line above the line 1066 the investigator did cite. Charge is explicit: a MISSING item that is present elsewhere should be refuted.
- Lines 565-568 verbatim: "The provider choice is reversible. If provider-specific relay authentication or identity requirements cannot satisfy the threat model, Phase 0 must choose an equivalent static CDN, edge token service, and dedicated relay deployment before implementation starts." Relay-auth adequacy against the threat model is already a Phase 0 blocking reversal condition.
- Phase 0 (heading line 869) deliverable at 877-878: "prove browser-to-native Iroh connectivity with the intended relay authentication"; Phase 0 go/no-go gate at line 886: "a browser reaches a native test endpoint through an authenticated relay." The proposed fix "put it in Phase 0/3 gates" is partly already done.
- Line 949, Phase 3 go/no-go gate: "load tests stay inside resource and cost ceilings;" — a cost ceiling is already gate-blocking at the first production launch gate (line 953: "This is the first production launch gate").
- Line 1069, highest-risk unknown #8: "Relay bandwidth economics for browser file transfer." The cost channel is already a named unknown.

WRONG LENS, TOO: the load-bearing part of the argument is a design opinion, not an absence. "So `relay-auth.jeliya.ai` is, as specified, an open oracle" and "'per-endpoint' limits are meaningless when endpoints are free" are DISAGREE-class judgments about a design the doc has not specified. You cannot simultaneously hold that the design is under-specified (MISSING) and that the unspecified design is definitively an open oracle (WRONG). The doc declines to specify the admission rule and says so; the investigator treats the deferral as an unrecognized hole when the doc recognizes it by name at line 1062.

WHAT SURVIVES, AND AT WHAT SIZE: one narrow, genuine sharpening — the doc nowhere frames credential minting as an adversarial cost channel (distinct from line 1069's legitimate-traffic bandwidth economics), and has no hard monthly egress/spend cutoff, only an undefined "cost ceilings" at line 949. The observation that per-endpoint limits are weak against free Ed25519 keypairs is technically fair. That is a low-severity editorial suggestion to add one clause to line 788 and to sharpen the existing unknown at line 1062 — not a critical missing control in an architecture proposal that already gates the item at Phase 0. Nothing here blocks the doc, and "critical" should be reserved for gaps the document does not know it has.

Nothing in this finding touches the orchestrator's ground truth (HEAD, commit/file counts, the Cargo.toml:15 iroh-rooms pin, or the three workspace crates), so no conflict there.
````

### F45. Missing at lines 899, 890

- Kind: `MISSING` | Severity: `medium` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: Phase 1 ("3 to 5 weeks") delivers "`client_msg_id` idempotency" as an in-repo work item.

Evidence: docs/PROTOCOL.md:364-367 states the dependency the roadmap omits: "Reserved for a future minor (non-breaking): an optional `client_msg_id` the daemon echoes into the event for exactly-once reconciliation — but that requires a field on the signed `iroh-rooms` content, so it is named here, not yet implemented." The signed content format is upstream (Cargo.toml:15 pins iroh-rooms to a specific rev), so this Phase 1 item is blocked on the same upstream-acceptance risk the doc flags for storage traits at line 1060 — but `client_msg_id` is not listed among the highest-risk unknowns (lines 1058-1071) and Phase 1 has no upstream gate.

Proposed fix: Annotate line 899: "`client_msg_id` idempotency (requires a new field on the signed iroh-rooms content — upstream dependency, see docs/PROTOCOL.md idempotency note)." Add it to highest-risk unknown #1 (line 1060) or add a Phase 1 gate: "the upstream signed-content change is merged and pinned, or an accepted non-signed-content design is recorded in an ADR."

Dissent from the verification panel:

````text
REFUTE (high): REFUTED AS FRAMED. The finding's factual substrate is real, but its central inference — that Phase 1 is blocked on upstream acceptance and needs a gate / a slot in the highest-risk unknowns — is not established, and its materiality to the decision is small.

WHAT I CONFIRMED AS ACCURATE:
1. The PROTOCOL.md quote is verbatim correct. docs/PROTOCOL.md:359-367 reads: "**`message.send` has no idempotency key (normative gap).** ... Reserved for a future minor (non-breaking): an optional `client_msg_id` the daemon echoes into the event for exactly-once reconciliation — but that requires a field on the signed `iroh-rooms` content, so it is named here, not yet implemented."
2. The upstream-ownership premise is correct for THAT design. crates/jeliya-core/src/supervisor.rs:36 imports from `iroh_rooms::events`, and supervisor.rs:1593 calls upstream `build_message_text(...)`; `Content::MessageText` (materializer.rs:127) is an upstream enum variant. Cargo.toml:15 pins iroh-rooms to rev 71fbb500. So adding a field to the signed content would indeed be an upstream change.
3. Highest-risk unknown #1 is genuinely narrower than the finding needs. Lines 1060-1061 read "Whether Iroh Rooms will accept and maintain the portable browser **store, transport, and blob interfaces** upstream" — trait portability, not content schema. So client_msg_id is not covered there. The investigator is right on that point.

WHY IT STILL FAILS THE MATERIALITY LENS:

A. CITATION IS WRONG, AND THE PROPOSED FIX WOULD EDIT THE WRONG LINE. The finding cites "Doc line(s): 899, 890". Line 899 is "- protocol version and capability negotiation;" — a different deliverable. The actual `client_msg_id` bullet is docs/production-deployment.md:895. The proposed fix ("Annotate line 899: `client_msg_id` idempotency ...") would annotate the wrong bullet. Verified by `awk NR>=886 && NR<=900`.

B. THE "BLOCKED UPSTREAM" INFERENCE ASSUMES ONE DESIGN THE DOC NEVER ADOPTS. PROTOCOL.md describes a specific mechanism (daemon *echoes the id into the signed event*) to achieve "exactly-once reconciliation." The doc's Phase 1 gate is weaker: line 906 requires only "10,000 injected lost-response retries produce no duplicate message" — a same-daemon retry property, satisfiable by a daemon-local `client_msg_id -> event_id` table with zero signed-content change. The doc's own repository change map puts idempotency exactly there: line 844 assigns `crates/jeliya-core/src/supervisor.rs` responsibility for "Pluggable store/net/blob traits, **idempotency**, cursors, invite cancellation, and relay policy" — supervisor-level, not content-level. Line 419-420 likewise frames it client-side: "Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection." And supervisor.rs already carries local dedupe machinery (supervisor.rs:110 "pushed-event dedupe set", :960, :2440) that such a design would extend. The doc also proposes `crates/jeliya-protocol/` — "Pure protocol-v2 types, canonical encoding, signatures, and conformance fixtures" (line 846) — a jeliya-owned layer that could carry the id. So the claim "this Phase 1 item is blocked on the same upstream-acceptance risk" is an unproven leap, not a demonstrated dependency.

C. PARTIAL SELF-AWARENESS. The doc is not silent on upstream risk: line 527 already provides the escape hatch "Portable traits are introduced upstream **or in an audited short-lived patch**"; line 532 "Every release pins and qualifies an exact upstream revision"; lines 861-862 warn against a long-lived private fork; line 887 makes an upstream issue (#121) a Phase 0 gate. The doc's general upstream posture is stated, even though it is scoped to traits rather than content fields.

D. DECISION IMPACT IS MINOR. This is one sub-bullet among seven Phase 1 deliverables (lines 894-900) in a multi-phase, multi-quarter proposal whose critical path is dominated by browser store/transport portability, OS-keystore + recovery, companion pairing, and signing/notarization. Nothing about approving or rejecting this architecture turns on whether `client_msg_id` carries an upstream annotation. The finding's own proposed fix — folding it into unknown #1 — would actually conflate two distinct upstream asks (trait portability vs. content schema), degrading the section it aims to improve.

RESIDUE THAT SURVIVES (hence 'low', not 'drop'): there is a genuine, checkable doc-consistency nit — PROTOCOL.md:364-367 names a dependency that the roadmap bullet at line 895 does not carry forward, and the unknowns list does not cover content-schema changes. Worth a one-line cross-reference at line 895. It is a documentation cross-reference gap, not a medium-severity planning/schedule risk, and it should be reported with the corrected line number (895) and without the "blocked upstream / needs a gate" framing, which the doc's own change map at line 844 contradicts.
````

### F46. Missing at lines 163-166, 887-888

- Kind: `MISSING` | Severity: `medium` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: The doc's upstream-risk inventory (lines 163-166) and its Phase 0 gate (887-888) name only #121 and #119 as the upstream security residuals carried by the pinned rev.

Evidence: Upstream issue #126 is missing. `gh issue list --repo kortiene/iroh-room --label type/security --state all` shows #126 "[NET] Per-device provisional state races per-connection cleanup: a stale clear_provisional can bypass the join-bootstrap gate (double-connect TOCTOU)", labels [type/security, area/transport, priority/p2, risk/low], CLOSED 2026-07-16T15:10:31Z. Its body: "between its `clear_provisional(device)` and its `unregister(device)`, the pump may process a conn2 frame with `is_provisional == false` — the frame is served **un-gated** … Result: the bounded recent-chat window (or an arbitrary `WantEvents` pull) is served to an unproven dialer" and "connection churn is attacker-controlled and free, so it is repeatable for the duration of an open join window." Fixed by d0dde8797dfd "fix(net): guard link teardown with a connection generation (#126)" (PR #130) plus follow-up 85a3aedb18a6 (PR #131). `compare/71fbb500...d0dde8797dfd` → {"status":"ahead","ahead_by":9}: the pinned rev is vulnerable. `grep -n "#126" docs/production-deployment.md` → no match.

Proposed fix: Add to the upstream-residual bullet (lines 163-166) and to the Phase 0 gate: "Upstream issue #126 (type/security) lets a double-connect TOCTOU clear the provisional mark of a live successor link, serving one un-gated pull response (recent-chat window or arbitrary `WantEvents`) to an unproven dialer inside an open join window; repeatable via attacker-controlled connection churn. Present in the pinned rev; fixed upstream at d0dde879 (PR #130) with follow-up 85a3aedb (PR #131)."

## Low-severity findings

32 findings.

### F47. Wrong at lines 407-408

- Kind: `WRONG` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim under review: "Its persistent state is limited to view selection, aliases, and drafts; the daemon remains authoritative."

Evidence:

````text
The UI writes SEVEN distinct localStorage key families, not three. Verified by `grep -rn "localStorage" ui/src/`: (1) `jeliya.aliases.v1` — ui/src/lib/names.ts:5, identity_id → alias [doc's "aliases"]; (2) `jeliya.draft.<roomId>` — ui/src/components/Composer.tsx:34,83-84 [doc's "drafts"]; (3) `jeliya.lastRoom` — ui/src/App.tsx:64, written at App.tsx:392 [doc's "view selection"]; (4) `jeliya.rooms.v1` — ui/src/lib/roomFlags.ts:13,53, persists two SETS OF ROOM IDs (`{pinned: [...], archived: [...]}`) — NOT mentioned by the doc; (5) `jeliya.lastSeen` — ui/src/lib/lastSeen.ts:7,39, a `Record<room_id, unix_ms>` unread-watermark map — NOT mentioned; (6) `jeliya.textLocale` — ui/src/l10n/locale.ts:17 — NOT mentioned; (7) `jeliya.formattingLocale` — ui/src/l10n/locale.ts:18 — NOT mentioned. No IndexedDB or Cache Storage use exists (`grep -rn "indexedDB\|IndexedDB" ui/src ui/public` → no matches), so localStorage is the whole persistence surface.
````

Proposed fix: Replace lines 407-408 with: "Its persistent state is seven localStorage keys and no IndexedDB or Cache Storage: `jeliya.lastRoom` (view selection), `jeliya.aliases.v1` (identity_id → alias), `jeliya.draft.<room_id>` (per-room drafts), `jeliya.rooms.v1` (pinned/archived room-id sets), `jeliya.lastSeen` (room_id → last-seen timestamp), and `jeliya.textLocale`/`jeliya.formattingLocale`. The daemon remains authoritative for all signed state."

Dissent from the verification panel:

````text
REFUTE (high): The investigator's RAW FACTS are accurate — I reproduced them all — but the CLASSIFICATION ("WRONG") and SEVERITY ("medium") do not survive the materiality lens.

WHAT I VERIFIED AS TRUE (investigator's facts hold):
- `grep -rn "localStorage" ui/src/` reproduces exactly. All seven key constants confirmed at the cited lines: ui/src/App.tsx:64 `'jeliya.lastRoom'`; ui/src/lib/names.ts:5 `'jeliya.aliases.v1'`; ui/src/components/Composer.tsx:34 `` `jeliya.draft.${roomId}` ``; ui/src/lib/roomFlags.ts:13 `'jeliya.rooms.v1'`; ui/src/lib/lastSeen.ts:7 `'jeliya.lastSeen'`; ui/src/l10n/locale.ts:17-18 `'jeliya.textLocale'`/`'jeliya.formattingLocale'`.
- `grep -rn "indexedDB\|IndexedDB\|caches\.\|CacheStorage" ui/src ui/public` → zero matches. Confirmed.
- The paragraph's OTHER claims (doc 406-407) are ACCURATE: ui/public/site.webmanifest exists, and grep for `serviceWorker|service-worker|sw.js|workbox|vite-plugin-pwa` across ui/src, ui/public, ui/package.json, ui/vite.config.ts → zero matches. So "install manifest but no service worker or browser room runtime" is correct.

WHY IT IS NOT "WRONG":
The doc's sentence has two payloads: a category claim (persistent state is trivial device-local UI state) and an authority claim ("the daemon remains authoritative"). BOTH ARE CORRECT, and the omitted keys strengthen rather than undercut them. The source itself groups the omitted keys into exactly the doc's category — ui/src/lib/roomFlags.ts:1-6: "Device-local pin / archive marks... Stored only on this device, never on the wire... display preferences... exactly like the last-seen mark (lastSeen.ts) and aliases (names.ts) they sit beside." And ui/src/lib/lastSeen.ts:1-4: "stored only on this device... It never leaves this machine." None of the four omitted keys is signed or authoritative. This is an INCOMPLETE ENUMERATION, not a false statement — a distinction the review rules require me to keep separate.
Further, the doc enumerates by CATEGORY, not by key, and "view selection" charitably covers `jeliya.rooms.v1`: ui/src/lib/roomList.ts:25 shows pinned/archived are room-list SECTION keys (`'pinned' | 'active' | 'departed' | 'archived'`), i.e. literally view state. That reduces the genuine omissions to lastSeen and the two locale keys.

WHY SEVERITY IS NOT MEDIUM (the materiality core):
`grep -n -i "localStorage\|lastSeen\|unread\|pinned\|archived\|locale\|alias\|draft" docs/production-deployment.md` shows line 407 is the doc's ONLY statement about current UI persistence. Nothing depends on it. The section it sits in (404-439) is otherwise entirely about FUTURE modes that use IndexedDB/OPFS/Cache Storage. No ADR item (1046-1056), no risk (1058-1071), no phase, and no security control depends on the key count. The security-relevant storage statement is line 380 ("Never store a ticket in localStorage, IndexedDB, Cache Storage...") — a prohibition unaffected by how many preference keys exist. A reader who believes "three trivial device-local keys" versus "seven trivial device-local keys" makes IDENTICAL decisions on relay selection, companion protocol, browser signing strategy, and storage migration. Zero decision leverage.

SELF-AWARENESS CHECK (as instructed): the doc is NOT self-aware here. I read 1032-1071; unknown #9 ("PWA storage behavior across real Safari/iOS and low-storage devices") is about future storage behavior, not the current key inventory. This is the one factor keeping it from 'drop' rather than a downgrade.

RESIDUAL VALUE: "limited to" is a closed-enumeration phrasing that is under-inclusive, and the proposed fix is cheap, accurate, and makes the baseline auditable. The lastSeen/pinned keys are also a mild input to future ADR item 4 (multi-device semantics) since they are per-device and unsynced. That is worth a copy-edit at low severity — but it is a precision improvement to a one-line baseline, not a defect in the doc's reasoning, and it should not be presented to the decision-maker as a factual error in the proposal.
````

### F48. Internally inconsistent at lines 46-48, 214, 869, 890, 912, 932, 954

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim under review: "For a small team of two to three engineers, the companion-backed production slice is estimated at 11 to 17 engineering weeks" (46-48) and the table cell "Approximately 11 to 17 engineer-weeks" / "Approximately 16 to 24 engineer-weeks" (214).

Evidence: The phase headers sum exactly to the table cells. Phase 0 "1 to 2 weeks" (869) + Phase 1 "3 to 5 weeks" (890) + Phase 2 "5 to 7 weeks" (912) + Phase 3 "2 to 3 weeks" (932) = 11 to 17. Phase 0 + Phase 1 + Phase 3 + Phase 4 "10 to 14 weeks" (954) = 16 to 24. The exact match proves these are CALENDAR weeks for the whole team, not engineer-weeks. Line 197-198 states the team is "two core/full-stack engineers, one web/operations engineer at least part-time" (~2.5 FTE). Labelled as engineer-weeks at line 214, the numbers understate real effort by ~2.5x: slice 1 is ~28-43 engineer-weeks, not 11-17, and the full hybrid to browser peer is ~53-78 engineer-weeks.

Proposed fix: Relabel line 214 as "calendar weeks (team of ~2.5 FTE)" and add a separate "engineer-weeks" column showing 28-43 / 40-60 / 60+ / 53-78. A decision doc that misstates cost by 2.5x cannot support a build-vs-defer choice.

Dissent from the verification panel:

````text
REFUTE (high): MISCATEGORIZED. The finding's load-bearing evidence is a non-sequitur, and the correct classification is undefined-unit ambiguity (MISSING-ish), not INTERNALLY-INCONSISTENT.

WHAT I VERIFIED AS ACCURATE (all quotes verbatim, all arithmetic confirmed):
- :46-48 "the companion-backed production slice is estimated at **11 to 17 engineering weeks**. A robust browser-only peer adds approximately **10 to 14 weeks**."
- :214 "| First safe production | Approximately 16 to 24 engineer-weeks | Approximately 11 to 17 engineer-weeks | At least 24 weeks | First companion slice in 11 to 17 weeks; browser mode follows |"
- :197-198 "Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time, and an independent security review."
- Phase headers :869 (1-2), :890 (3-5), :912 (5-7), :932 (2-3), :954 (10-14) are verbatim.
- Arithmetic (python): P0+P1+P2+P3 = (11,17) exactly; P0+P1+P3+P4 = (16,24) exactly. The decomposition mapping column 1 to Phases 0+1+3+4 while SKIPPING the companion Phase 2 is correct and a genuinely sharp observation.

WHY IT DOES NOT HOLD UP:

1. The stated proof has zero discriminating power. "The exact match proves these are CALENDAR weeks for the whole team, not engineer-weeks" is invalid. Summation is unit-agnostic: engineer-weeks add across sequential phases exactly as calendar weeks do. A total equal to the sum of its parts is the expected result under BOTH readings, so the match distinguishes nothing. This inference is the entire basis for classifying the item as a factual internal inconsistency and rating it critical, and it does not bear that weight.

2. Under the engineer-weeks reading the doc is fully self-consistent. If the phase headers are engineer-weeks, then :46 ("engineering weeks"), :214 ("engineer-weeks"), and :869-:954 all agree; no two statements contradict. INTERNALLY-INCONSISTENT requires a demonstrated contradiction. There is none — there is an UNDEFINED UNIT. I grepped the whole 1087-line doc for calendar/FTE/effort/person-/parallel/sequential and read the "Planning assumptions" section (:1034-1045, five bullets: production matrix, signing services, relay credentials, no server peer, hosted-origin trust). The doc never states whether its weeks are calendar or effort. That is the real defect, and it is a different, much smaller one.

3. The investigator's secondary argument is interpretation, not fact. Reading :197-198 as implying calendar duration is an inference presented as established. It is also weak: that sentence is a skill-mix/resource list ("one web/operations engineer at least part-time", "an independent security review") that conditions EFFORT estimates too — Phase 3 (:932-948) is largely web/ops work and the Phase 1/3 gates (:906, :949) explicitly require external security review and penetration review. This is a DISAGREE-with-judgment dressed as a WRONG.

4. The proposed fix is as likely to inject the error as remove it. The "understates by ~2.5x" figure and the replacement numbers (28-43 / 40-60 / 53-78) come from an FTE count the investigator inferred, not one the doc states. The doc says "two to three engineers" (:46) and "at least part-time" (:197-198) — unbounded above; 2.5 FTE is a guess. If the author meant engineer-weeks, applying this fix inflates the stated cost by 2.5x, i.e. commits the exact error it accuses the doc of, in the opposite direction. "Misstates cost by 2.5x" cannot be asserted as fact when the direction of the alleged error is undetermined.

5. The one genuine literal inconsistency is a different, cosmetic one. On :214 itself the SAME quantity is labeled "engineer-weeks" in the companion column and bare "weeks" in the hybrid column ("First companion slice in 11 to 17 weeks"); :47 likewise says the browser peer "adds approximately 10 to 14 weeks" unqualified. This shows the author uses the terms interchangeably, which supports "unit never defined" (low) rather than "cost misstated 2.5x" (critical).

SURVIVING RESIDUE: a one-line low-severity note — the doc never defines whether its week figures are calendar or effort (add it to Planning assumptions, :1034), and :214 should use one unit label consistently across all four columns. Not critical, not an internal inconsistency, and the reviewer must not assert which unit was meant.
````

### F49. Internally inconsistent at lines 745 (vs. 583-598, 606-613)

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: Line 745 instructs operators to "Scrub CSP reports of `document-uri`, query values, and code samples" — but the header block (583-613) configures no CSP reporting at all, and `document-uri` is the wrong field name for the modern reporting mechanism.

Evidence: The CSP block (583-598) contains no `report-to` or `report-uri`; the additional-headers block (606-613) contains no `Reporting-Endpoints`. Confirmed by grep over the whole doc: matches for "report" are only lines 379, 381, 745, 795 — none configure an endpoint. MDN, HTTP Guides/CSP: "Reporting-Endpoints: csp-endpoint=\"https://example.com/csp-reports\"" plus "Content-Security-Policy: default-src 'self'; report-to csp-endpoint". MDN, default-src page: `report-to` and `report-uri` do NOT fall back to default-src. Separately, MDN's sample Reporting-API violation body uses the field `"documentURL"`, not `document-uri`; `document-uri` is the field name in the deprecated `report-uri` JSON payload (CSP3 §6.5.1: "The `report-uri` directive is deprecated in favor of the new `report-to` directive").

Proposed fix:

````text
Add `Reporting-Endpoints: csp-endpoint="https://<collector>"` to the headers block and `report-to csp-endpoint;` to the CSP. Ship the policy first as `Content-Security-Policy-Report-Only` in staging to shake out violations before enforcing. Then fix line 745 to say the scrub list is `documentURL`, `blockedURL`, `sample`, and `referrer` (Reporting API field names), keeping `document-uri`/`blocked-uri` only if a legacy `report-uri` collector is also run.
````

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as stated: the finding is miscategorized, and half of it (the "wrong field name" half) is factually wrong. A thin, genuine gap survives at low severity.

WHAT I VERIFIED AS ACCURATE (the investigator got this part right):
- Read docs/production-deployment.md:583-598 (the CSP block) and 606-613 (additional headers). Neither contains `report-to`, `report-uri`, nor `Reporting-Endpoints`. Confirmed independently: `grep -n -i "report\|CSP\|Content-Security" docs/production-deployment.md` returns only lines 276, 379, 381, 655, 745, 795, 936, 944, 1014 — none configures a reporting endpoint, and no `Content-Security-Policy-Report-Only` appears anywhere. So it is true that line 745 ("Scrub CSP reports of `document-uri`, query values, and code samples") presumes a report stream the doc never provisions.
- `grep -rn "report-to\|report-uri\|Reporting-Endpoints\|securitypolicyviolation\|Content-Security-Policy"` over the repo's ts/tsx/js/mjs/json/html/toml/yml files (excluding node_modules) returns ZERO hits, so nothing outside the doc supplies the missing config either.

WHY THE CATEGORY IS WRONG (my lens):
"INTERNALLY-INCONSISTENT" implies two doc statements that contradict. Nothing here contradicts. The doc nowhere says "we will not collect CSP reports"; it simply does not enumerate the reporting wiring. Line 581 titles the block "### Baseline Content Security Policy" — explicitly a starting point — and the doc twice treats these blocks as deliberately extensible later: line 600-602 ("When component UI is introduced, add only the reviewed isolated component origin to `frame-src`") and line 615 ("Add COEP only if Wasm threading requires cross-origin isolation"). Furthermore line 936 makes "DNS, TLS, CDN, CSP, and related headers" a Phase 3 *deliverable*, gated at line 944 on "external TLS/header/CSP assessment passes" — i.e. concrete header wiring is explicitly deferred to implementation. An operational privacy constraint (745) that presumes a capability the baseline doesn't spell out is an OMISSION (MISSING), not an inconsistency.

WHY THE SECOND HALF IS AFFIRMATIVELY WRONG:
The claim that "`document-uri` is the wrong field name" does not survive. MDN, Content-Security-Policy/report-uri, sample report body, quoted verbatim: `{"csp-report": {"blocked-uri": "...", "disposition": "report", "document-uri": "http://example.com/signup.html", "effective-directive": "style-src-elem", ... "script-sample" ...}}`. `document-uri` is the exact correct field name for `report-uri` payloads. The doc never states which reporting mechanism it uses, so labeling the term "wrong" presumes a mechanism the doc does not specify. MDN's report-to page itself says: "until `report-to` is broadly supported you can specify both directives", and marks report-to "Baseline 2026 - Newly available — Since March 2026" — four months before this doc's date, with Firefox's Reporting-API-for-CSP historically behind a default-off flag (Bugzilla 1922967). For a project whose CI tests "Chromium, Firefox, and WebKit" (line 648), running `report-uri` alongside `report-to` is the standard-recommended posture, and `document-uri` is then the correct name for the majority of received reports. The investigator's own proposed fix concedes this ("keeping `document-uri`/`blocked-uri` only if a legacy `report-uri` collector is also run") — that is a DISAGREE about modernization preference dressed up as a factual error, which is exactly the failure mode my lens is charged with catching.

SEVERITY:
Asserted "high" is unsupportable. The surviving kernel is a documentation gap: the doc references scrubbing CSP reports without ever specifying an endpoint or a Report-Only staging rollout. No security control is broken, no reader is misled into an unsafe configuration (absent an endpoint the browser simply sends nothing), and the doc already defers header implementation to a gated Phase 3. Keep it only as a low-severity MISSING item worded as "the doc should name a CSP reporting endpoint (Reporting-Endpoints + report-to, optionally with report-uri for older engines) and state that the policy ships as Content-Security-Policy-Report-Only in staging first." Strip the field-name assertion entirely.
````

### F50. Internally inconsistent at lines 46, 214, 197-198

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: "For a small team of two to three engineers, the companion-backed production slice is estimated at 11 to 17 engineering weeks" (line 46) vs. the comparison table's "Approximately 11 to 17 engineer-weeks" (line 214) for the same slice.

Evidence:

````text
grep -n -E 'weeks|engineer' docs/production-deployment.md returns: line 46 `slice is estimated at **11 to 17 engineering weeks**`; line 214 `| First safe production | Approximately 16 to 24 engineer-weeks | Approximately 11 to 17 engineer-weeks | At least 24 weeks | First companion slice in 11 to 17 weeks; browser mode follows |`; line 197-198 `Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time`. "Engineering weeks" scoped to "a small team of two to three engineers" reads as calendar duration; "engineer-weeks" is unambiguously person-weeks. The phase headings (lines 869, 890, 912, 932) are bare "1 to 2 weeks" etc., which reads as calendar. At the stated ~2.5 FTE the two readings differ by 2.5x: 11-17 calendar weeks = 27.5-42.5 person-weeks, vs 11-17 person-weeks = 4.4-6.8 calendar weeks. The document's single most-cited number is ambiguous by 2.5x.
````

Proposed fix:

````text
Pick one unit and use it everywhere. Recommended: state phase durations as calendar weeks at the stated staffing and add a person-week column. Replace line 46 with "...is estimated at 11 to 17 calendar weeks at the staffing in [Deployment-model comparison](#deployment-model-comparison) (approximately 28 to 43 engineer-weeks)" and change line 214's header cell to "First safe production (calendar weeks at 2.5 FTE)" with all four cells restated in the same unit.
````

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as characterized. The finding's headline claims — "INTERNALLY-INCONSISTENT" and "ambiguous by 2.5x" — do not survive. What remains is a one-word copy-edit defect.

WHAT I VERIFIED AS ACCURATE IN THE INVESTIGATOR'S EVIDENCE
The three quoted strings are real and exact. `grep -n -E "weeks?|engineer|FTE|month" docs/production-deployment.md` returns line 45-46 `For a small team of two to three engineers, the companion-backed production / slice is estimated at **11 to 17 engineering weeks**`; line 214 `| First safe production | Approximately 16 to 24 engineer-weeks | Approximately 11 to 17 engineer-weeks | At least 24 weeks | First companion slice in 11 to 17 weeks; browser mode follows |`; and lines 197-198 `Planning estimates assume two core/full-stack engineers, one web/operations / engineer at least part-time, and an independent security review.` The phase headings are also bare-"weeks" as claimed. The terminology genuinely varies across three forms: "engineering weeks" (46), "engineer-weeks" (214, cells 1-2), plain "weeks" (214 cell 4; 869/890/912/932).

REFUTATION 1 — there is no numeric inconsistency. `grep -n "^### Phase" docs/production-deployment.md` returns Phase 0 "1 to 2 weeks" (869), Phase 1 "3 to 5 weeks" (890), Phase 2 "5 to 7 weeks" (912), Phase 3 "2 to 3 weeks" (932). Min 1+3+5+2 = 11; max 2+5+7+3 = 17. That is EXACTLY the 11-17 at lines 46 and 214. Line 952, immediately after Phase 3's gate, reads "This is the first production launch gate." — confirming Phases 0-3 are precisely the "First safe production" row. The cross-check holds a second time: Phase 4 is "10 to 14 weeks" (954), exactly line 46-47's "A robust browser-only peer / adds approximately **10 to 14 weeks**". Two independent decompositions reconcile to the digit. Lines 46 and 214 state the same number; calling them "internally inconsistent" misdescribes a synonym mismatch as a data conflict.

REFUTATION 2 — the "2.5x ambiguity" is resolved inside the document. The investigator treats the number as free-floating, but §"Dependency-ordered roadmap and gates" (864) opens at 866-867 with "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase" and then supplies four sequentially gated phases summing to 11-17. A reader cannot reach 27.5-42.5 person-weeks without discarding the doc's own decomposition. The unit is fixed by construction, not left open.

REFUTATION 3 — "'engineer-weeks' is unambiguously person-weeks" is undercut in place. The same table row, same line 214, writes its fourth cell as "First companion slice in **11 to 17 weeks**" — no "engineer-" prefix, same figure. The doc demonstrably uses the two forms interchangeably within a single line, which is evidence of loose diction, not of a second intended quantity.

MATERIALITY (my assigned lens) — near zero for the decision asked. The doc's ask is the §Decision at 216-219: "Adopt the hybrid model and use the companion-backed shell as the first production slice." Line 214 is one row of a four-column option comparison, and all four cells use the same unit: 16-24 | 11-17 | at least 24 | 11-17. Under EITHER interpretation the ranking and the ratios between options are identical, so the comparison the table exists to drive is invariant. The unit question cannot flip the model choice.

DOC SELF-AWARENESS — partial, and adjacent rather than in 1032-1071. The staffing basis the investigator wants added is already stated at 197-198, in the two lines immediately preceding the table, so the table inherits it on the page. Lines 47-48 add "These are planning estimates, not release commitments." The proposed fix's substance ("at the staffing in Deployment-model comparison") is therefore largely already present. Note the honest limit: this is not listed under §Planning assumptions (1034-1044) or §Highest-risk unknowns (1058-1071), so the doc is not self-aware of the *terminology* variance specifically.

WHAT SURVIVES. A real, small copy-edit defect: "engineer-weeks" is an established term of art for person-weeks, and using it for a figure that is calendar duration is sloppy in a decision doc whose executive summary and comparison table are the parts most likely to be read standalone. An executive skimming only line 214 without reaching line 869 could mis-plan. That justifies one line in a review recommending the single word be normalized — not a high-severity internal-inconsistency finding. Severity high is inflated roughly two bands; corrected to low. I stop short of "drop" because the fix is one word and the exec-skim failure mode is genuine.
````

### F51. Unverifiable at lines 214

- Kind: `UNVERIFIABLE` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim under review:

````text
"First safe production | ... | At least 24 weeks | ..." for the hosted gateway / managed backend column.
````

Evidence: The other three cells in row 214 reconcile exactly to the phase table (verified: 11-17 = phases 0+1+2+3; 16-24 = phases 0+1+3+4). The gateway's "At least 24 weeks" reconciles to nothing — no phase, no decomposition, and no deliverable list for a gateway appears anywhere in the 1087-line document. It is also the only open-ended bound in a row of two-sided ranges, which biases the comparison: an unbounded estimate can never be beaten. Note also that a gateway would not need Phase 4 at all (the browser talks to the gateway, not to Iroh), so its true scope is smaller than the hybrid's 21-31, not larger.

Proposed fix: Either decompose the gateway estimate into phases the way the other options are decomposed, or replace the cell with "not estimated" and reject the gateway on the stated trust grounds alone (223-224), which is a sufficient and honest reason. Do not reject it on an unsupported number.

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as classified. The finding's textual observations are largely accurate, but it is miscategorized as UNVERIFIABLE, one of its two evidentiary pillars is overstated, and its supporting counter-argument is contradicted by the doc's own text.

WHAT I CONFIRMED (independently, not taking the investigator's word):
- Phase estimates at docs/production-deployment.md:869, 890, 912, 932, 954, 977 are P0 1-2, P1 3-5, P2 5-7, P3 2-3, P4 10-14, P5 8-16.
- Reconciliation is exact: P0+P1+P2+P3 = (11,17), matching "11 to 17" (line 214, companion column and reused in the hybrid column). P0+P1+P3+P4 = (16,24), matching "16 to 24" (static PWA column). The investigator's arithmetic is correct.
- `grep -n -i "gateway\|managed backend"` returns only lines 200, 204, 206, 222 across all 1087 lines. There is genuinely no gateway phase or deliverable list. "At least 24 weeks" is indeed the only open-ended bound in the row. These observations are accurate.

WHY IT IS REFUTED:

1. "Reconciles to nothing" is too strong. I ran the phase sums and found P0+P1+P3+P4+P5 = (24, 40) — a lower bound of exactly 24. Separately, 24 is also exactly the upper bound of the static-PWA cell in the same row (line 214). So "At least 24 weeks" has at least two plausible derivations from material the doc already contains: "the PWA path plus Phase 5 server/component work" (lower bound 24), or the comparative reading "at least as long as the longest alternative in this row." Neither is stated explicitly, but the claim that the number traces to nothing at all does not survive.

2. Miscategorized. UNVERIFIABLE implies the doc asserts something checkable that I could not check. But every cell in row 214 is an unverifiable forward estimate about a hypothetical build, and the doc says so twice: line 47-48 "These are planning estimates, not release commitments" and line 197-198 "Planning estimates assume two core/full-stack engineers...". Summing unverifiable numbers yields internal consistency, not verification — 11-17 and 16-24 are no more verified against reality than 24 is. Labeling only the gateway cell UNVERIFIABLE implies a contrast in epistemic status that does not exist. The real complaint is about estimation rigor and rhetorical framing (an undecomposed, one-sided bound in a row of decomposed two-sided ones). That is a DISAGREE-shaped judgment, not an UNVERIFIABLE-shaped factual gap. The primary proposed fix ("decompose the gateway estimate into phases") is also MISSING-shaped, further showing the bucket is unstable.

3. The supporting counter-argument is wrong on the doc's own evidence. The investigator asserts "a gateway would not need Phase 4 at all... so its true scope is smaller than the hybrid's 21-31, not larger." This silently assumes the gateway needs nothing beyond Phases 0-3, which the doc contradicts: line 202 "Central ingress, tenant isolation, and key custody become critical risks"; line 207 "HSM/server keys or client-side crypto required"; line 209 "Requires a hardened server sandbox and tenant scheduler"; line 212 "High: accounts, databases, isolation, backups, and abuse"; line 203 "unless a new encrypted-envelope protocol is built"; and lines 183-186 confirm no tenant, account, authorization-domain, quota, or public audit model exists today. None of that work appears in Phases 0-3, which are companion-shaped (pairing protocol, OS keystore, installers, relays, CDN/TLS). So "at least 24 weeks" is plausibly conservative rather than inflated, and the investigator's arithmetic rebuttal is unsupported.

4. Demanding decomposition of a rejected alternative asks the roadmap to scope work the project will never do. Section 864 is explicitly "Dependency-ordered roadmap and gates" for the adopted architecture. The static-PWA cell reconciles only incidentally, because its constituent phases are on the adopted path anyway; the gateway's are not, by construction.

5. The number is non-load-bearing. The Decision (216-224) rejects the gateway purely on trust grounds — "A gateway would gain browser reach by replacing Jeliya's current privacy and local-first boundaries with server trust" (222-224). The 24-week figure is never invoked. The investigator concedes this. An unsupported number that drives no conclusion, in a row the doc twice disclaims as planning estimates, cannot carry medium severity.

RESIDUAL VALUE: one real editorial point survives — a single open-ended bound in a row of two-sided ranges is a genuine presentational asymmetry, and the investigator's alternative fix ("replace the cell with 'not estimated' and reject on the stated trust grounds alone") is sound, low-cost advice. That warrants a low-severity style/rigor note reclassified as DISAGREE, not a medium UNVERIFIABLE finding.
````

### F52. Disagreement with a judgment at lines 444-445

- Kind: `DISAGREE` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `browser-platform-facts`

Claim under review: "Background Sync is unavailable in some major browsers, and browsers terminate long-running service-worker work."

Evidence: "some major browsers" understates a categorical engine split. https://caniuse.com/background-sync: Firefox NOT supported (v2-152), Safari NOT supported (v3.1-26.5), Safari on iOS NOT supported (v3.2-26.5); supported only in Chrome 49+, Edge 79+, Chrome for Android, Samsung Internet 5+, Opera 42+. Global usage 77.48%. MDN's own banner on the page cited at line 1081 reads: "Limited availability — This feature is not Baseline because it does not work in some of the most widely-used browsers." This is two of three engines (Gecko and WebKit), and because every browser on iOS is WebKit, it is 100% of iOS. The doc's own Phase 4 matrix at 966-967 requires Firefox, Safari, and iOS Safari, so Background Sync is absent from half the platforms the doc commits to supporting.

Proposed fix: Rewrite as: "Background Sync is a Chromium-only API — it is absent from Firefox and from all WebKit browsers, therefore from all of iOS. Deferred send-on-reconnect must be designed to work without it, using foreground reconciliation on next app activation as the baseline mechanism and Background Sync only as a Chromium-only optimization." This strengthens rather than weakens the doc's conclusion at 446-447, which is already the right one.

Dissent from the verification panel:

````text
REFUTE (high): The investigator's browser facts are ACCURATE — I verified both sources independently. https://caniuse.com/background-sync returns: Firefox not supported (2-152), Safari not supported (3.1-26.5), Safari on iOS not supported (3.2-26.5); supported Chrome 49+, Edge 79+, Samsung Internet 5+, Opera 42+; global usage 77.48%. MDN's banner is verbatim "Limited availability / This feature is not Baseline because it does not work in some of the most widely-used browsers." Background Sync is indeed Chromium-only. I am not disputing the facts.

The finding fails on MATERIALITY, on four independent grounds.

1. The doc's phrasing is a faithful paraphrase of the very source it cites. Doc line 444-445 says "unavailable in some major browsers"; MDN's banner — cited at doc line 1081 — says "does not work in some of the most widely-used browsers." The investigator quotes that banner AS evidence against the doc, but the banner uses the same hedge. A doc cannot be faulted for adopting the hedging of the authority it cites and links.

2. Nothing in the doc rests on Background Sync. `grep -n -i "background sync\|Background Synchronization"` over the whole file returns exactly two hits: line 444 (the sentence under review) and line 1081 (the citation). The API appears only as a reason NOT to treat the service worker as a peer. There is no design, gate, or deliverable to revisit — so no decision the doc asks for changes under either wording.

3. The proposed fix asks the doc to do something it already does, 24 lines earlier. The fix says "Deferred send-on-reconnect must be designed to work without it, using foreground reconciliation on next app activation as the baseline mechanism." Doc lines 419-420 already specify exactly that: "Keep offline drafts locally. Queued sends carry a stable `client_msg_id`; the companion signs them after reconnection." Reinforced at line 1009 ("idempotent sending") in the vertical slice. The doc is in fact MORE conservative than the fix: the fix would grant Background Sync status as "a Chromium-only optimization," whereas doc line 446 grants it nothing — "Browser peers are available while the application is active."

4. The doc is repeatedly self-aware of background unavailability, in the sections my lens directs me to check and beyond: line 974 (Phase 4 go/no-go gate) "product copy makes no durable background-availability claim"; line 1025 (explicit exclusions) "mobile background-availability claims"; line 1070 (highest-risk unknowns) "PWA storage behavior across real Safari/iOS and low-storage devices"; line 1056 (ADR required) "Supported browser, desktop OS, and mobile matrix."

The investigator's strongest move — that the Phase 4 matrix at 966-967 requires Firefox/Safari/iOS Safari, so Background Sync is missing on half the committed platforms — would be a real gap only if any Phase 4 gate depended on the API. None does. Gate line 972 reads "an ACTIVE browser peer works offline and converges after reconnection," which is the active-only stance restated. The doc engineered around the engine split rather than tripping over it; there is no contradiction to surface.

Separately, the second clause of the sentence — "browsers terminate long-running service-worker work" — is uncontested, applies even in Chromium where the API does exist, and carries the paragraph's conclusion on its own. So the conclusion at 446-447 survives regardless of how the first clause is worded.

The investigator effectively concedes all this: "This strengthens rather than weakens the doc's conclusion at 446-447, which is already the right one." A finding whose own author states the conclusion is already correct, against a doc that already implements the proposed mechanism and already flags the risk in four places, is not a medium-severity DISAGREE.

Residual value, and why low rather than drop: "some major browsers" is genuinely vaguer than "Chromium-only," the doc is precise elsewhere, and naming the engine split costs one clause. It is worth an optional wording note so a future reader does not re-open "could we use Background Sync for deferred send?" — but it is editorial polish, not a substantive disagreement with the architecture. Downgrade to low, and it should be filed as a wording suggestion rather than a DISAGREE.
````

### F53. Disagreement with a judgment at lines 329

- Kind: `DISAGREE` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "The peers establish a Noise XX-equivalent authenticated transcript over Iroh."

Evidence: Iroh already provides exactly what this layer is being added to provide. Per iroh's own documentation (https://www.iroh.computer/blog/iroh-0-34-0-raw-public-keys and https://docs.iroh.computer/about/faq): "Direct peer-to-peer QUIC uses TLS 1.3 Raw Public Keys (RFC 7250) with mutual endpoint authentication" and "Dialing by public key also makes them mutually authenticated, because each endpoint's public key is its TLS identity." The QR at line 328 already carries the companion's "endpoint". So the browser dialing that EndpointId gets a mutually authenticated, end-to-end-encrypted channel to exactly that companion with zero user interaction and zero new cryptography. Layering a second, hand-rolled handshake over it does not add an authentication property — it adds a second state machine, a second key-derivation, and a second thing to get wrong, and the doc's own Phase 1 gate (line 910) then has to pay for an independent review of "the wire formats and key lifecycle" that would otherwise be inherited from a reviewed library.

Proposed fix:

````text
Either (a) drop the Noise layer and specify that the control protocol runs on a dedicated Iroh ALPN over the already-authenticated QUIC connection, with the browser's control key = its Iroh EndpointId and the SAS derived from KDF(exported TLS keying material || both EndpointIds); or (b) keep Noise but state in ADR #2 the specific property it adds that Iroh's TLS-RPK does not (e.g. cryptographic independence from a QUIC/TLS implementation bug), AND require channel binding so the Noise transcript covers both Iroh EndpointIds.
````

Dissent from the verification panel:

````text
REFUTE (high): The finding's technical premise is correct but its materiality framing collapses on four doc-internal checks, and two of its four supporting legs are factually wrong.

VERIFIED TRUE (I did not take the investigator's word): Iroh 1.0.1 (per Cargo.lock) does provide mutual endpoint authentication. In ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-1.0.1/src/tls/verifier.rs:119-122: `impl ClientCertVerifier for ClientCertificateVerifier { fn offer_client_auth(&self) -> bool { true } }`, with TLS 1.2 rejected (verifier.rs:143-149) and raw public keys. The proposed fix (a) is also implementable: `export_keying_material` exists at iroh-1.0.1/src/endpoint/connection.rs:1075. The docs.iroh.computer/about/faq fetch confirms "iroh uses self-signed certificates with Endpoint IDs to authenticate both ends of the connection" and "all connections in iroh are end-to-end encrypted."

WHY IT STILL FAILS THE MATERIALITY LENS:

(1) The doc ALREADY specifies the Iroh ALPN. Line 851: "`crates/jeliya-companion/` | Signed native service with Iroh control ALPN and no public HTTP listener". The first clause of proposed fix (a) — "specify that the control protocol runs on a dedicated Iroh ALPN over the already-authenticated QUIC connection" — is already the doc's stated design. The investigator appears not to have read the repository change map.

(2) The doc ALREADY credits Iroh, not Noise, with the property. Line 36: "over a new mutually authenticated, end-to-end-encrypted Iroh control protocol." The finding's central premise ("Iroh already provides exactly what this layer is being added to provide") argues against a claim the doc never makes. The doc nowhere attributes mutual auth or E2EE to the Noise layer.

(3) The doc is ALREADY self-aware — this is decisive under my lens. Line 1049, in "Decisions that require an ADR": "2. Companion control protocol and pairing transcript." That is verbatim the subject of this finding. The investigator's own fix (b) concedes this by saying "state in ADR #2..." A finding whose ask is "decide this in an ADR," filed against a doc that lists it as unresolved ADR decision #2, is not a finding against the doc.

(4) The asserted cost is factually wrong. The investigator claims the Noise layer forces the doc to "pay for an independent review of 'the wire formats and key lifecycle'" at line 910. But line 910 gates the ENTIRE Phase 1 deliverable set (lines 894-900): recovery bundle + OS-keystore abstraction, client_msg_id idempotency, timeline cursor, invite expiry/cancellation, pairing/control protocol, protocol version negotiation, store-hole detection. Line 343 alone ("a versioned authenticated-encryption bundle containing the profile root, room membership index, device authorization state, and relay config") independently mandates a crypto review. Dropping Noise eliminates zero percent of that gate's necessity. The claimed saving does not exist.

(5) Line 329 says "Noise XX-equivalent" — hedged property language (mutual auth where neither party pre-knows the other's static key, which is precisely what the pairing needs per line 324-325: "The browser control identity is separate from the Jeliya profile or room-device identity"). It names a property rather than mandating the Noise Protocol Framework. Fix (a)'s KDF-over-exported-keying-material construction is arguably itself XX-equivalent, so the doc's text does not foreclose the investigator's preferred design.

SOURCING DEFECT: the quote attributed to https://www.iroh.computer/blog/iroh-0-34-0-raw-public-keys — "Direct peer-to-peer QUIC uses TLS 1.3 Raw Public Keys (RFC 7250) with mutual endpoint authentication" — does not appear on that page. My fetch reports the page "does not specifically discuss mutual authentication mechanics" and "doesn't provide detailed technical specifications about bidirectional authentication flows." The substance holds via the FAQ and via source inspection, but the finding presents a fabricated quotation as literal evidence.

SURVIVING NUGGET (why 'low', not 'drop'): channel binding appears nowhere in the doc — grep for "channel bind" over docs/production-deployment.md returns no hits, and "transcript" appears only at lines 329, 850, 1049. If a Noise handshake is run inside an Iroh stream without covering both Iroh EndpointIds, it is in principle proxyable across two Iroh connections. That is a genuine, cheap-to-state technical detail an ADR #2 reviewer would want. It belongs in the review as a one-line drafting note on line 329 ("state what the transcript adds over Iroh's TLS-RPK, and bind it to both EndpointIds"), not as a medium-severity DISAGREE with the architecture.

IMPACT ON THE DECISION BEING ASKED: none. The doc's executive decision (lines 27-48) and the Phase 0 gate's ask ("accept or reject the hybrid architecture through an ADR", line 875) are unaffected. No phase gate, cost, or schedule estimate changes. Under materiality this is a drafting refinement to a bullet the doc has already marked ADR-pending.
````

### F54. Disagreement with a judgment at lines 989, 928, 952

- Kind: `DISAGREE` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: Phase 5 go/no-go gate: "sandbox escape and confused-deputy review passes" — the only confused-deputy review in the roadmap.

Evidence: grep -n -i "confused" docs/production-deployment.md returns only line 989, inside Phase 5 ("components and optional server peers, 8 to 16 weeks"). But the confused-deputy surface that actually ships first is the browser-controller/companion scope boundary, delivered in Phase 2 and launched in Phase 3 — line 952: "This is the first production launch gate." Phase 2's substitute is line 928, "a malicious controller cannot invoke files, pipes, agents, or identity reset", which is a test of four named methods, not a review of the authority-resolution model; as shown above it would not surface `room.join`, the parameter-free methods, or `save_dir`. Phase 1 (line 910) and Phase 3 (line 950) do commission independent reviews, so the gap is one of explicit framing rather than of any review at all — but "wire formats and key lifecycle" and a penetration test are not the same exercise as a deputy analysis.

Proposed fix: Move a confused-deputy review into Phase 2's gate: "a confused-deputy review of the control protocol passes: for every RPC reachable by a paired controller, the review identifies which party names the resource and which party's authority resolves it, and confirms no browser-named string is resolved with companion authority outside an approved scope." Keep line 989 for the component host.

### F55. Missing at lines 426, 435

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `browser-platform-facts`

Claim under review: "Use atomic journals, versioned migrations, signed-event validation on boot, and crash-safe checkpoints" over an IndexedDB event store.

Evidence: IndexedDB does not provide crash-safety by default. https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase/transaction documents the `durability` option: "relaxed — The user agent may consider that the transaction has successfully committed as soon as all outstanding changes have been written to the operating system, without subsequent verification"; "strict — The user agent may consider that the transaction has successfully committed only after verifying that all outstanding changes have been successfully written to a persistent storage medium. This is recommended where the risk of data loss outweighs the impact of its use on performance and power"; and "default — The user agent should use its default durability behavior for the storage bucket. This is the default for transactions if not otherwise specified." A transaction that has fired `complete` under the default/relaxed setting can still be lost to a power failure — which, for an append-only signed-event log with cursors, means a committed cursor advance can survive while the events it points past do not.

Proposed fix: Add to the bullet: "Event-log and cursor-advance transactions use `{ durability: 'strict' }`; the default UA durability does not survive power loss. Cursors are only advanced in the same strict transaction that persists the events they cover, so that a lost tail is re-fetched rather than skipped."

Dissent from the verification panel:

````text
REFUTE (high): The underlying technical FACT is correct and I verified it independently — but the finding does not survive the materiality lens as a medium-severity MISSING item. Downgrade to low (polish/nit), do not carry it as a gap.

WHAT I VERIFIED AS ACCURATE (investigator's evidence holds):
- I fetched https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase/transaction and the three quoted passages are verbatim-correct. MDN: strict = "may consider that the transaction has successfully committed only after verifying that all outstanding changes have been successfully written to a persistent storage medium"; relaxed = "as soon as all outstanding changes have been written to the operating system, without subsequent verification"; default = "The user agent should use its default durability behavior for the storage bucket. This is the default for transactions if not otherwise specified." So the default is UA-determined, NOT strict. The investigator did not misquote or overstate the spec.
- Doc lines cited are real. Line 426-427: "IndexedDB stores signed event records, membership indexes, cursors, and package metadata." Line 434-435: "Use atomic journals, versioned migrations, signed-event validation on boot, and crash-safe checkpoints."

WHY IT FAILS MATERIALITY:

1. It targets a phase explicitly excluded from the decision under review. Line 954: "### Phase 4: browser peer and multi-device identity, 10 to 14 weeks". Line 1017-1019, the "It explicitly excludes:" list for the smallest production-worthy vertical slice, leads with "a browser-owned room identity". The go/no-go this doc asks for is on a companion-mode PWA where, per lines 416 and 408, "Keep the companion authoritative" and "the daemon remains authoritative." No IndexedDB event store ships in the slice being decided. I confirmed there is nothing to harden yet: `ls ui/src/` returns App.tsx, components, l10n, lib, main.tsx, styles.css, vite-env.d.ts — no `ui/src/storage` (the path the doc itself projects at line 839), and `grep -rn "indexedDB\|IDBDatabase\|durability" ui/src/` returns zero hits.

2. The doc already asserts the requirement; the finding only supplies the API flag. Line 434 already says "crash-safe checkpoints" and "signed-event validation on boot". This is not a missed requirement — it is a missing implementation detail one altitude below a deployment-architecture proposal. The doc names patterns elsewhere at the same altitude ("Use TUF-like root, targets, snapshot, and timestamp metadata", line 467-468) without naming APIs.

3. The doc's own adjacent bullets already specify the mitigation for the exact failure scenario alleged. The finding's asserted harm is that "a committed cursor advance can survive while the events it points past do not," i.e. a silently skipped tail. But line 434 mandates "signed-event validation on boot," and lines 436-438 mandate: "Maintain an eviction sentinel. Missing critical state stops authorship and offers recovery import or peer resynchronization. Never silently create a new identity." A boot-time validation pass plus a missing-critical-state sentinel that halts authorship is precisely the mechanism that detects a cursor pointing past absent events. The "silent" part of the failure scenario is already designed out by the two bullets immediately following the one being criticized.

4. Wrong failure class for a replicated log. This is a p2p signed-event system whose events are replicated by construction, not a database of record. A lost tail is re-fetchable from peers, and the doc already gates on exactly that at line 972: "an active browser peer works offline and converges after reconnection". Cost of the bug is a resync, not corruption or data loss.

5. Partial mis-targeting. The finding is framed as crash-safety "over an IndexedDB event store" and attaches to the line-434 journals/checkpoints bullet — but per line 428-429 the doc puts journals and checkpoints in OPFS ("OPFS stores blobs, component packages, journals, checkpoints, and large snapshots"), and `durability` is an IDB-transaction option that does not apply to OPFS. The fix text is still partly on-target because events and cursors ARE in IndexedDB per line 426, so a same-transaction cursor/event write is a coherent suggestion — but the coupling to the cited bullet is loose.

ON SELF-AWARENESS (the check I was asked to run): this is the one point favoring the finding. The unknowns section does NOT flag transaction durability. Line 1070 lists "PWA storage behavior across real Safari/iOS and low-storage devices," which is about quota and eviction, not commit durability — related but not the same thing. So the doc has not explicitly hedged this. However, line 434's standing "crash-safe checkpoints" requirement is a stronger self-awareness signal than an unknowns-list entry would be: the doc has stated the goal, it just has not named the mechanism.

NET: correct fact, verified citation, cheap fix, and the doc's citation list (lines 1075-1087) does not currently include the IDBDatabase/transaction page — so it is legitimately worth one line in a polish/nits bucket, which is why I say low rather than drop. But classifying it as MISSING at medium overstates a design-detail refinement to an out-of-slice Phase 4 component whose failure mode the doc already routes into an existing detection-and-resync path. It should not influence the go/no-go.
````

### F56. Missing at lines 576-577, 607

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: "Consider preload after an observation period" (line 577) for `app.jeliya.ai`, with header `Strict-Transport-Security: max-age=63072000; includeSubDomains` (line 607).

Evidence: hstspreload.org submission requirements: "The `max-age` must be at least `31536000` seconds (1 year)." — satisfied (63072000). "The `includeSubDomains` directive must be specified." — satisfied. "The `preload` directive must be specified." — the header at line 607 has no `preload` token. Critically, only registrable domains (eTLD+1) are accepted; subdomains cannot be submitted separately, and "The preload applies to all subdomains and nested subdomains." Also: "Inclusion in the preload list cannot easily be undone. Domains can be removed, but it takes months for a change to reach users with a Chrome update and we cannot make guarantees about other browsers." The doc's own topology has `staging.app.jeliya.ai` (line 573) and `relay-auth.jeliya.ai` (line 556) in scope.

Proposed fix: Rewrite 576-577 to: "Preload is not available for `app.jeliya.ai` — hstspreload.org accepts only the registrable domain. Preloading therefore means submitting `jeliya.ai` with `includeSubDomains` + `preload`, which permanently forces HTTPS on `relay-auth.jeliya.ai`, `staging.app.jeliya.ai`, and every future `*.jeliya.ai` label, and is effectively irreversible on a months-long timescale. Defer preload until every planned subdomain is HTTPS-only, and note that it is an organization-wide decision, not an app.jeliya.ai one."

Dissent from the verification panel:

````text
REFUTE (high): FACTS HOLD, MATERIALITY DOES NOT. I independently confirmed the technical substrate from primary sources, then refuted the finding on its asserted severity and framing.

CONFIRMED FACTS (verified, not taken on the investigator's word):
- chromium/hstspreload `domain.go` (https://github.com/chromium/hstspreload/blob/main/domain.go) contains error code `domain.is_subdomain` with literal message: "`%s` is a subdomain. Please preload `%s` instead. (Due to the size of the preload list and the behaviour of cookies across subdomains, we only accept automated preload list submissions of whole registered domains.)" and the comment "We don't currently allow automatic submissions of subdomains." `.ai` is the eTLD, so eTLD+1 is `jeliya.ai` and `app.jeliya.ai` is indeed not self-submittable.
- hstspreload.org confirms: max-age >= 31536000 (doc's 63072000 at line 607 PASSES), includeSubDomains required (PASSES), `preload` directive required (absent at 607), and "Domains can be removed, but it takes months for a change to reach users with a Chrome update and we cannot make guarantees about other browsers."
- Topology confirmed by grep: app.jeliya.ai (34, 572), staging.app.jeliya.ai (573), relay-auth.jeliya.ai (556) all under jeliya.ai. Lines 576-577 are the ONLY HSTS/preload discussion in all 1087 lines.

WHY IT FAILS THE MATERIALITY LENS:
1. THE DOC IS ALREADY SELF-AWARE. Line 576-577 reads "Enable HSTS with `includeSubDomains` only after every affected subdomain is HTTPS. Consider preload after an observation period." The proposed fix's substantive recommendation ("Defer preload until every planned subdomain is HTTPS-only") is essentially already the doc's own text. The doc already sequences subdomains-HTTPS-first, then includeSubDomains, then only "consider" preload. Per my instructions, a doc that already flags the hazard makes the finding much weaker.
2. THE CITED HEADER DEFECT IS ACTUALLY THE DOC BEING CORRECT. The evidence leans on "the header at line 607 has no `preload` token" as if that were a gap. It is not — the doc explicitly defers preload, and shipping the `preload` token before submitting is the real-world error. Line 607 correctly omits it. Evidence presented as a defect is in fact conformance.
3. NO DECISION TURNS ON IT. The doc's ask is architectural (lines 27-48: capability-aware hybrid, PWA + companion, Wasm peer gating, relays). Line 22 states it "does not authorize a production deployment by itself." Preload appears in neither the ADR list (1046-1056) nor the highest-risk unknowns (1058-1071) — correctly, since it is deferred routine ops. Accepting this doc creates zero irreversible HSTS commitment, so the (real) months-long-removal hazard is never triggered by anything the doc authorizes.
4. SELF-REVEALING AT POINT OF ACTION. Cost of the omission is near zero: whoever revisits preload pastes app.jeliya.ai into the form and gets the `domain.is_subdomain` error instantly, which names the correct parent domain in the error text itself.
5. INVESTIGATOR OVERREACH IN THE EVIDENCE. It lists relay-auth.jeliya.ai as in-scope alongside staging.app.jeliya.ai. But relay-auth.jeliya.ai is a SIBLING of app.jeliya.ai, not a descendant — it is not covered by the line 607 includeSubDomains header at all, only by a hypothetical jeliya.ai preload submission. Only staging.app.jeliya.ai is a descendant. The evidence blurs header scope with preload scope.
6. MINOR ABSOLUTISM: the proposed fix's "Preload is not available for app.jeliya.ai" is true only of the automated path. The Chromium wiki (chromium/hstspreload.org Preload List Processes) explicitly contemplates manual subdomain entries — "A large, old site can't move their top domain to `includeSubDomains` at once, and temporarily wants to preload sensitive subdomains like `www.example.com` or `login.example.com`" — while cautioning "Unless there is high user benefit, we should not preload a large number of subdomains for a given domain." This doesn't rescue the doc but does make the fix's flat assertion slightly overstated.

WHAT SURVIVES: one genuinely non-obvious fact worth a single clarifying clause — that preload is a jeliya.ai-wide, organization-level decision rather than an app.jeliya.ai one. That is a polish nit on a deferred, non-binding line, not a medium-severity MISSING gap. Downgrade to low; do not drop entirely, since the clause is cheap and informative.
````

### F57. Missing at lines 583-598 (cf. 600-602)

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: The proposal specifies one origin-wide CSP (583-598) with no per-response hardening for peer-supplied content, even though the repo already ships exactly that pattern for the same threat.

Evidence: crates/jeliyad/src/serve.rs:450-453 already sets, on every peer-supplied blob the daemon serves: `.header("x-content-type-options", "nosniff")` and `.header("content-security-policy", "default-src 'none'; sandbox; frame-ancestors 'none'")`, alongside `Content-Disposition: attachment`. The in-code comment at serve.rs:434-439 states the reasoning verbatim: "a peer-supplied `text/html` / `image/svg+xml` opened as a top-level document would run script with the daemon's origin and could exfiltrate the auth token." The proposal's header section contains no analogue, and MDN's default-src page confirms `sandbox` does not fall back to `default-src`, so `default-src 'none'` at line 584 does not supply it.

Proposed fix: Add to the CSP section: "Any response at app.jeliya.ai that carries peer- or user-supplied bytes (file download, preview, component manifest) must carry its own response CSP `default-src 'none'; sandbox; frame-ancestors 'none'` plus `X-Content-Type-Options: nosniff` and `Content-Disposition: attachment`, mirroring `crates/jeliyad/src/serve.rs:450-453`. The origin-wide policy protects the shell; it does not protect against content the shell hands to the browser."

Dissent from the verification panel:

````text
REFUTE (high): VERIFIED ACCURATE (all sub-claims check out literally):

1. `docs/production-deployment.md:583-598` is indeed a single origin-wide CSP block, and 600-602 is the only follow-on guidance ("When component UI is introduced, add only the reviewed isolated component origin to `frame-src`"). CONFIRMED by reading the lines.

2. `crates/jeliyad/src/serve.rs:450-453` sets exactly what is quoted. Actual text:
   `.header("x-content-type-options", "nosniff")` (450) and `.header("content-security-policy", "default-src 'none'; sandbox; frame-ancestors 'none'",)` (451-453), preceded by `CONTENT_DISPOSITION` at 449 built from `content_disposition_value(&display_name, "attachment")` (443). CONFIRMED.

3. The in-code rationale is real and quoted correctly, but the line range is off by one: the comment runs 433-439, not 434-439 (`grep -n` shows line 433 = "// This blob came from a remote room peer, and `file.mime` is that peer's"). Trivial, non-load-bearing.

4. MDN claim CONFIRMED. Fetched https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/default-src — the fallback list is child-src, connect-src, font-src, frame-src, img-src, manifest-src, media-src, object-src, prefetch-src, script-src(-elem/-attr), style-src(-elem/-attr), worker-src. `sandbox` is a Document directive, not a fetch directive, and does not fall back.

5. The mitigation is genuinely absent from the doc as a *header* recommendation. `grep -n -iE "sandbox|nosniff|content-disposition|attachment|peer-supplied|user-supplied|untrusted"` over all 1087 lines returns no analogue. Line 608 (`X-Content-Type-Options: nosniff`) is the origin-wide default; line 485 ("opaque-origin sandboxed iframe") is about rendering *component UI*, not peer file bytes. So the investigator did not miss a literal duplicate.

WHY IT STILL FAILS MY LENS — the threat IS addressed elsewhere in the doc, by a different and stronger mechanism, so "MISSING" misreads the architecture:

- Line 293 (Component responsibilities table) scopes the origin explicitly: "`app.jeliya.ai` | Serve immutable PWA/Wasm assets, public environment config, publisher trust roots, and signed revocation metadata | **Prohibited:** Store room events, identities, invites, or user keys; proxy `jeliyad`; contain secrets".
- Line 210: "File handling | Picker and OPFS; no arbitrary native path; quota-sensitive".
- Lines 428-430: "OPFS stores blobs, component packages, journals... **Cache Storage contains only exact content-hashed shell and Wasm assets.**"
- Lines 553-554: the static host is Cloudflare Pages "receiving an already built immutable artifact rather than rebuilding source."

Peer bytes reach the browser over the end-to-end-encrypted Iroh p2p path into OPFS. They are never an HTTP response from the origin. The response class the proposed fix targets — "any response at app.jeliya.ai that carries peer- or user-supplied bytes" — is empty by the doc's own design constraint stated 290 lines earlier. The daemon needs serve.rs:450-453 precisely because `jeliyad` *is* an HTTP server that hands peer blobs to a browser over loopback; app.jeliya.ai by design is not. The finding's "the repo already ships exactly that pattern for the same threat" is a false equivalence between an HTTP blob server and a static asset CDN. Applying the proposed paragraph verbatim would add a spec requirement that governs nothing.

RESIDUAL KERNEL (which the investigator did NOT identify): the real analogue of serve.rs in this architecture is client-side — a peer-supplied blob read out of OPFS and handed to the browser via a `blob:`/object URL inherits the app.jeliya.ai origin, and line 207 states that in browser mode "active origin can invoke usable keys" (WebCrypto + IndexedDB wrapping). The doc never specifies how peer bytes are safely presented (forced download, MIME neutralization à la `safe_download_mime` at serve.rs:464, sandboxed preview). That gap is real but is a *client-side rendering* finding, not a response-header finding, and response headers cannot fix it — a blob: URL document has no server response to carry them.

CATEGORIZATION VERDICT: the label "MISSING" is formally the right bucket for an absent recommendation and is not a judgment dressed as a factual error. But it fails the second half of my charge: the threat is handled elsewhere in the doc (293, 210, 428-430) by architectural exclusion rather than by header, so the origin-wide CSP is not the doc's only defense as the finding asserts. As literally written — with its evidence, mechanism, and fix all response-header-shaped — the finding should not appear. I set 'low' rather than 'drop' only because a reframed version (client-side handling of OPFS-resident peer bytes, citing serve.rs:433-439 and 464 as the prior-art rationale) is a legitimate small gap worth one sentence. Medium severity is not defensible.
````

### F58. Missing at lines 584-596 (roadmap at 210, 428)

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: The fetch-directive list (584-596) is complete for the app's roadmap. `media-src` is absent and `blob:` appears nowhere.

Evidence:

````text
MDN default-src confirms `media-src` falls back to `default-src`, which is `'none'` (line 584) — so `<audio>`/`<video>` are blocked outright. Doc line 210 promises "File handling | Picker and OPFS" and line 428 says "OPFS stores blobs, component packages, journals, checkpoints, and large" content. Rendering a locally-held file almost always goes through `URL.createObjectURL` → a `blob:` URL, which `img-src 'self' data:` (587) and the absent `media-src` both reject. Verified this is a future break, not a current one: `grep -rn "createObjectURL|blob:" ui/src/` returns zero hits today.
````

Proposed fix: Add a forward-looking note under the block, parallel to the existing frame-src note at 600-602: "When local file preview lands, add `img-src 'self' blob:`, `media-src 'self' blob:`, and — if PDF or document preview is in scope — an explicit decision on `object-src`, which stays `'none'`. Previewing peer-supplied bytes must go through blob: URLs with a sanitized Content-Type, never data: URLs and never an inline document."

Dissent from the verification panel:

````text
REFUTE (high): The two narrow technical sub-claims are ACCURATE and I credit them: (a) `media-src` is indeed absent from 584-596 and falls back to `default-src 'none'` (standard CSP Level 3 fallback; uncontested, I did not re-fetch MDN because my refutation does not rest on disputing it); (b) `blob:` does appear nowhere in the block. The investigator's grep is also CONFIRMED — `grep -rnE "createObjectURL|blob:" ui/src/` returns zero hits, so this is correctly labeled a future, not current, break. But under the materiality lens the finding collapses on four independent grounds.

1. FILES ARE EXPLICITLY OUT OF SCOPE FOR THE SLICE THIS CSP SERVES. Line 1017-1020: "It explicitly excludes: - a browser-owned room identity; - files;". Line 684 makes it a production-promotion gate: "absence of files, pipes, agents, and components when disabled." Line 421: "Mark files, pipes, membership actions, and agents unavailable while the companion cannot be reached." The doc is not merely silent on file preview — it actively gates on its absence. A CSP that blocks blob: file rendering is CORRECT for the release it is written for.

2. LINE 210 IS NOT A ROADMAP PROMISE — THIS IS A MISREADING. Line 210 is a cell in the four-column comparison matrix opened at line 200, whose column 2 header is "Static PWA with Wasm peer". "File handling | Picker and OPFS" describes a candidate architecture the doc did NOT select. The Decision at 216-219 reads "Adopt the hybrid model and use the companion-backed shell as the first production slice." The investigator quoted a descriptive matrix cell about a non-chosen option and called it a promise.

3. LINE 428 DOES NOT SAY WHAT THE FINDING SAYS. It sits under "### Browser-peer mode" (heading at 424), which maps to "### Phase 4: browser peer and multi-device identity, 10 to 14 weeks" (line 954) — beyond the first slice. And "blobs" there is a storage noun, not a URL scheme. `grep -nEi "blob"` returns 16 hits, every one of them storage-layer: "per-room filesystem blob" (101), "blob store" (132), "SQLite/blobs" (261), "Pluggable store/net/blob traits" (844), "browser event, blob, and sync adapters" (958). The doc never once refers to `blob:` URLs. The leap from "OPFS stores blobs" to "will need blob: URL rendering" is the investigator's inference, presented as a doc citation.

4. THE `media-src` HALF HAS ZERO ROADMAP BASIS. `grep -nEi "\baudio\b|\bvideo\b|media-src|<video|<audio|voice|playback" docs/production-deployment.md` returns ZERO hits across all 1087 lines. The document never contemplates media playback anywhere. The finding asserts the directive list is incomplete "for the app's roadmap" while the roadmap it cites contains no media feature. That item is invented.

SELF-AWARENESS (the lens's explicit test): strongly present, and not via 1032-1071 but via better mechanisms. The heading at 581 is literally "### Baseline Content Security Policy" — declaring it a floor to be extended. Lines 600-602 already demonstrate the exact convention the proposed fix asks for: "When component UI is introduced, add only the reviewed isolated component origin to `frame-src`. Do not loosen the main origin..." The doc has stated its methodology (deny-by-default baseline, deliberately loosened per feature at the time that feature lands) and modeled it once. The proposed fix adds a second instance of an already-established convention for a feature the doc explicitly excludes.

INVERTED POLARITY: the finding penalizes the doc for a correct security posture. `default-src 'none'` with explicit enumeration is deny-by-default; "`<audio>`/`<video>` are blocked outright" is the intended behavior, not a defect. Pre-enumerating directives for unbuilt features is precisely how deny-by-default baselines rot. Note the proposed fix's own good advice ("never data: URLs") would actually argue the doc's existing `img-src 'self' data:` at 587 is the looser line — but that is a different finding, not this one.

DECISION IMPACT: nil. The decision the doc asks for is Phase 0 approval of the hybrid/companion-backed model. Nothing about a Phase 4+ CSP directive for an explicitly-excluded feature bears on it. Recommend drop; if the reviewer wants the forward-looking note anyway it is an editorial nit, not a medium.
````

### F59. Missing at lines 590 (see also 621, 651, 837)

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: `worker-src 'self'` (line 590) is the whole service-worker story: the doc never states that the SW script needs the CSP header on its own response, nor that fetches made inside the SW are checked against `connect-src` rather than the resource-type directive.

Evidence: MDN, Content-Security-Policy header page (verbatim): "Workers are in general _not_ governed by the content security policy of the document (or parent worker) that created them. To specify a content security policy for the worker, set a `Content-Security-Policy` response header for the request which requested the worker script itself. The exception to this is if the worker script's origin is a globally unique identifier (for example, if its URL has a scheme of data or blob). In this case, the worker does inherit the content security policy of the document or worker that created it." On the connect-src point: qubyte.codes/blog/content-security-policy-and-service-workers (verbatim): "the worker is using the `connect-src` policy when performing the request for the image, and not the `image-src` policy I expected" and "The initial request is checked for `image-src` violation. A request with the same URL is checked in the service worker for `connect-src` violation. The response from the service worker is checked for `image-src` violation." The doc plans `ui/src/sw.ts` (line 837) and SW install/update/offline tests (line 651) but says nothing about either behavior.

Proposed fix: State in the headers section that the CSP and the other security headers must be applied to *all* responses including `/sw.js`, not only `text/html` — a static host that scopes headers to HTML leaves the SW with no policy. Add a sentence that a cache-first SW re-fetch is evaluated against `connect-src`, which is satisfied here only because `'self'` is listed (line 589); any future cross-origin asset the SW caches needs an explicit `connect-src` entry in addition to its resource-type entry. Note that a Worker created from a `blob:` URL would inherit the page CSP and be blocked by `worker-src 'self'` — the Wasm peer's worker must be a real same-origin script URL.

### F60. Missing at lines 612

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: `Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()` (line 612) is offered as the baseline.

Evidence: MDN Permissions-Policy documents 49 directives; the doc denies 5. Absent and relevant to this threat model (a p2p app that must not be a device-access or tracking surface): `serial`, `hid`, `bluetooth`, `idle-detection`, `screen-wake-lock`, `local-fonts`, `display-capture`, `midi`, `xr-spatial-tracking`, `publickey-credentials-create`, `publickey-credentials-get`, `otp-credentials`, `browsing-topics`, `attribution-reporting`, `window-management`, `compute-pressure`, `speaker-selection`, `accelerometer`, `gyroscope`, `magnetometer`. MDN on syntax: "`()` (empty allowlist) — The feature is disabled in top-level and nested browsing contexts."

Proposed fix: Extend to a deny-by-default list, e.g. `camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=(), bluetooth=(), midi=(), idle-detection=(), display-capture=(), local-fonts=(), xr-spatial-tracking=(), publickey-credentials-create=(), publickey-credentials-get=(), otp-credentials=(), browsing-topics=(), attribution-reporting=(), window-management=(), compute-pressure=(), accelerometer=(), gyroscope=(), magnetometer=()`. Two exclusions to make deliberately: do NOT blanket-deny `screen-wake-lock` without checking whether long file transfers need it, and note that `local-fonts=()` is verified safe today (`grep -o "@font-face" ui/dist/assets/*.css` = 0 matches; the only font-family in the built CSS is `font-family:var(--mono)`). Correction to the review brief: `interest-cohort` should NOT be added — it is not in MDN's directive list (FLoC was withdrawn); the live equivalent is `browsing-topics`. Likewise `clipboard-read`/`clipboard-write` are not MDN Permissions-Policy features, which is fortunate because ui/src/components/ui.tsx:151 and ui/src/App.tsx:149 call `navigator.clipboard.writeText`.

### F61. Missing at lines 187

- Kind: `MISSING` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim under review: "The current UI transports bearer material in WebSocket and upload query URLs." (enumeration of where the daemon token travels in a URL)

Evidence:

````text
The enumeration is incomplete — there is a THIRD, arguably worse transport the doc never names: the token is embedded in file-download links that are rendered into the DOM as user-visible anchors.

ui/src/App.tsx:160-166:
```
function localFileUrl(roomId: string, fileId: string): string {
  const params = new URLSearchParams({ room_id: roomId, file_id: fileId });
  // The daemon token-gates /api/files/*; by the time a file link renders, the
  // WS client has fetched the session token (links come from protocol data).
  const token = daemonToken();
  if (token) params.set('token', token);
  return `/api/files/local?${params.toString()}`;
}
```
That string is stored as `FetchState.url` (ui/src/App.tsx:175 and :742) and rendered as an anchor href in two places:
- ui/src/components/ui.tsx:398-406: `<a className="btn btn-sm btn-primary" href={state.url} target="_blank" rel="noreferrer">` (the "Open file" button)
- ui/src/components/ui.tsx:489-497: `<a className="fetch-path-link" href={state.url} target="_blank" ...><code>{state.path}</code></a>`

This is materially different from the two transports the doc does name. A WebSocket URL and a `fetch()` POST URL are never surfaced to the user; a rendered `<a href>` is right-click → "Copy link address" copyable, is opened via `target="_blank"` (so the token-bearing URL enters browser history for that tab), and — because crates/jeliyad/src/serve.rs:443 sets `Content-Disposition: attachment` on `/api/files/local` — the token-bearing URL is recorded as the source URL in the browser's persistent download manager. The doc's own stated concern at line 187 is that "query strings land in logs"; browser history and the download manager are exactly that class of durable sink, and they outlive the daemon process in a way daemon logs do not.
````

Proposed fix:

````text
Replace line 187 with: "The current UI transports bearer material in query URLs on three surfaces: the WebSocket connect URL (`ui/src/lib/client.ts:255`), the file-upload POST URL (`ui/src/lib/client.ts:95`), and — most exposed — the `/api/files/local` download link rendered as a user-copyable `<a href target=\"_blank\">` (`ui/src/App.tsx:160-166`, `ui/src/components/ui.tsx:401` and `:492`), which places the token in browser history and the download manager record." Then add the download link to the Repository change map row for `ui/src/lib/client.ts` (doc line 834), or add a row for `ui/src/App.tsx`, so the remediation is tracked.
````

Dissent from the verification panel:

````text
REFUTE (high): Every code fact the investigator cites is ACCURATE — I verified each one independently:

- `ui/src/App.tsx:160-166` `localFileUrl()` does embed the daemon token as a `token` query param on `/api/files/local`. Verified verbatim.
- It is stored as `FetchState.url` (`ui/src/App.tsx:175`) and rendered as `<a href={state.url} target="_blank" rel="noreferrer">` at `ui/src/components/ui.tsx:398-406` (the "Open file" button) and `:489-497` (`fetch-path-link`). Verified verbatim.
- `crates/jeliyad/src/serve.rs:443-444` does set `Content-Disposition: attachment`. Verified.
- The two transports the doc DOES name are real: `ui/src/lib/client.ts:252-256` (WS connect URL) and `ui/src/lib/client.ts:95-96` (upload POST URL). Verified.
- The doc at line 187 genuinely does not mention the download link, and `grep -n "api/files\|App.tsx\|download manager\|browser history" docs/production-deployment.md` returns nothing (exit 1). So it is genuinely absent, not already flagged in the assumptions/unknowns section (1032-1071) either.

The finding is nonetheless refuted on MATERIALITY, on four independent grounds:

1. INCREMENTAL RISK IS ~ZERO. `crates/jeliyad/src/lifecycle.rs:60-62` documents the token as "Generated per daemon start and published only through the 0600 portfile and the browser-only `/api/session` endpoint," and `main.rs:229` confirms "it lives in the 0600 portfile." Any process that can read the browser's download-manager DB or history is running as the same OS user — and that user can already just read the 0600 portfile. Doc lines 176-177 explicitly concede this: the threat model "explicitly excludes hostile same-user processes." The token is also only usable against `127.0.0.1`, so a token leaked via a copied link is unusable to any remote party. The "durable sink" framing is further undercut by the token being per-daemon-start: a download-manager record surviving a restart holds a dead credential.

2. THE CENTRAL RHETORICAL CLAIM RESTS ON A MISQUOTE. The investigator writes: "The doc's own stated concern at line 187 is that 'query strings land in logs'." Line 187 reads in full: "- The current UI transports bearer material in WebSocket and upload query URLs." It says nothing about logs. The only "query strings" text (lines 380-381, "Never store a ticket in `localStorage`, IndexedDB, Cache Storage, logs, crash reports, query strings, or URL paths") is in the invite-fragment section and governs join TICKETS on the public origin — a different secret with a different threat model. The "browser history and the download manager are exactly that class of durable sink" argument is therefore attributing a concern to line 187 that line 187 does not express.

3. THE DECISION DOES NOT MOVE. Line 187 is the eighth of eight illustrative bullets supporting one conclusion at lines 189-195: "A reverse proxy would not add the missing security model... The production companion control protocol must be designed as a separate surface. Do not add a public-listen flag, proxy `/ws`, or reuse the daemon token remotely." That conclusion is already carried far more forcefully by lines 183 ("identity creation, daemon shutdown, room history, native file operations, pipes, and agent projections") and 185-186 ("no public TLS, remote pairing, device approval, abuse controls"). A third instance of the same defect class changes no recommendation, phase, cost estimate, or ADR in the doc.

4. THE PROPOSED FIX CONTRADICTS THE DOC'S OWN POSTURE. The investigator asks to track remediation in the Repository change map. But the change map's jeliyad row (line 843) reads "Remain a loopback-only legacy/local sidecar; never receive a public bind option," and the `ui/src/lib/client.ts` row at 834 is scoped to "Replace the production same-origin `/ws` assumption with transport interfaces and capability negotiation" — future architecture, not current-token remediation. The doc deliberately does not propose remediating the current daemon token transport, because its thesis is that this surface is not the production path. Adding a remediation row would misrepresent the proposal.

Secondary technical doubt: the investigator asserts `target="_blank"` means "the token-bearing URL enters browser history for that tab." Because `serve.rs:443` returns `Content-Disposition: attachment`, browsers cancel the navigation and hand off to the download manager, typically leaving no history entry. I did not empirically test this, so I flag it as a probable overstatement rather than a verified error — but it means one of the two named durable sinks is likely not real.

Net: a real, correctly-cited code observation about an incomplete enumeration, but it is an editorial completeness note on an illustrative bullet, not a high-severity gap. Keep it at LOW as a one-clause suggested addition to line 187, with the security-risk framing and the change-map remediation request stripped. Not a candidate for drop only because the underlying fact is verified and the sentence would be marginally more accurate with it.
````

### F62. Missing at lines 304-305

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim under review: "It stores both seeds in a plaintext JSON secret protected by filesystem permissions."

Evidence:

````text
Same unstated platform gap as the portfile. The permission enforcement is Unix-only.

crates/jeliya-core/src/identity.rs:251-262 (write_new_owner_only):
```
fn write_new_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
```
crates/jeliya-core/src/identity.rs:72-81 (ensure_dir) is likewise `#[cfg(unix)]`-gated for the `0o700` directory mode. The crate's own test asserting the mode is `#[cfg(unix)]`-gated too (crates/jeliya-core/src/identity.rs:324-334: `fn files_are_owner_only`). No Windows ACL code exists.

Everything else in this sentence is confirmed exactly: plaintext hex seeds for BOTH keys in one JSON file (crates/jeliya-core/src/identity.rs:236-248, `secret_file_contents`, which formats `{"version":1,"identity_secret":"<hex>","device_secret":"<hex>"}`).
````

Proposed fix: Amend to: "It stores both seeds as plaintext hex in a single JSON secret (`identity.secret`), protected by `0600` file / `0700` directory permissions on Unix only (`crates/jeliya-core/src/identity.rs:251-262`, `:72-81`). Windows relies on inherited per-user directory ACLs with no code enforcement." This strengthens rather than weakens the doc's Phase 1 gate at line 906 ("native production mode no longer leaves the root secret plaintext").

### F63. Missing at lines 404-410, 839

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim under review: Browser persistence section (lines 404-410) and the repository change map (lines 834-841) plan new browser storage but never account for the room IDs and identity IDs the current UI already keeps in plaintext localStorage.

Evidence:

````text
Four of the seven existing keys hold protocol identifiers in cleartext at the web origin: `jeliya.rooms.v1` stores room-id sets (ui/src/lib/roomFlags.ts:53 `JSON.stringify({ pinned: [...flags.pinned], archived: [...flags.archived] })`), `jeliya.lastSeen` stores a `room_id → ts` map (ui/src/lib/lastSeen.ts:9 comment: "room_id → the newest signed-event ts (Unix ms) this device has acknowledged"), `jeliya.draft.<roomId>` puts a room id in the KEY NAME (ui/src/components/Composer.tsx:34), and `jeliya.aliases.v1` is "keyed by identity_id" (ui/src/lib/names.ts:1-2). The doc's own TB1 (lines 274-277) states a compromised origin "controls the browser session", and line 730 forbids room IDs even in aggregate telemetry — yet nothing in §"Browser persistence, PWA, and offline behavior" or the `ui/src/storage/` change-map row (line 839) says this pre-existing plaintext identifier state must be migrated, namespaced per profile, or cleared on unpair. The Phase 3 gate (lines 943-950) likewise does not test it.
````

Proposed fix: Add a bullet under "Companion mode" (after line 422): "Migrate the existing plaintext `localStorage` state (`jeliya.rooms.v1`, `jeliya.lastSeen`, `jeliya.draft.<room_id>`, `jeliya.aliases.v1`) into the encrypted local projection. Room IDs and identity IDs must not remain readable to any script on the origin, and unpairing or control-key revocation must clear them." Add a matching Phase 3 gate line: "no room or identity identifier remains in unencrypted browser storage after pairing."

### F64. Missing at lines 864-996

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: The roadmap omits user-facing documentation and support.

Evidence: `grep -n -i 'documentation' docs/production-deployment.md` returns only: line 25 (the doc profile), 71/79 (assessment), 88 (citations), 123 (`documentation, downloads, checksums, and installation instructions` — listed under what can be deployed TODAY, not as roadmap work), 645 (the existing CI doc gate), 873 (`reconcile status, threat, evidence, and platform documentation` — internal reconciliation). No phase delivers install guides, pairing walkthroughs, recovery-kit instructions, or troubleshooting. This matters more than usual here because the product's core failure modes are user-custody problems the document itself flags: line 347 `Require a successful test restore before setup is called complete`, line 349 `Explain that the recovery bundle restores identity authority, not unique unreplicated events or blobs`, line 439 `Explain that unreplicated local blobs disappear if browser storage is lost`, line 402 `Browser extensions, screenshots, copied links, and OS clipboard managers remain disclosure risks that the product must state`. Every one of those "explain" and "state" obligations is unassigned documentation work. Support is likewise absent: `grep -n -i support` shows no support-channel, triage, or on-call-for-users item; line 385 (`support ... tooling`) refers only to redaction in diagnostics.

Proposed fix: Add a Phase 3 deliverable: "install, pairing, recovery, and troubleshooting guides for the supported platforms, in EN and FR; a support intake channel and triage rotation; and a stated response expectation for beta." Budget 1-2 person-weeks plus ongoing support load, and note that the recovery-kit guide is on the critical path for the Phase 1 gate at line 904.

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as filed at medium; a thin residual survives at low.

WHAT I CONFIRMED (finding's one true core): I reran the greps. No phase deliverable in lines 869-996 names an install guide, pairing walkthrough, troubleshooting guide, or a user support channel. `grep -n -i 'support channel|support intake|on-call|triage|helpdesk|customer'` returns exactly one hit, line 814 ("move availability and on-call cost to the team"), which is operator on-call, not user support. So the narrow observation is true.

GROUND 1 — the central evidence is MIS-TYPED, and the claim about it is false twice over. The investigator's load-bearing argument is that lines 347/348-349/401-402/439 are "unassigned documentation work." Read in context they are in-product copy, not documentation:
- 401-402 literally says the disclosure risks are what "the product must state."
- 348-349 sits inside the Recovery design bullets immediately after 347 "Require a successful test restore before setup is called complete" — an in-product setup-flow requirement.
- 439 sits beside 431 "display storage estimates" and 436-438's eviction sentinel — all in-product.
And they ARE assigned: Phase 2 deliverable line 919 "recovery and re-pair user interfaces"; line 840 `ui/src/pairing/` "... expiry, and revocation UI"; and the doc enforces copy at gate level twice — line 974 "product copy makes no durable background-availability claim" and line 994 "the server-peer UI states precisely whether the server can read content." The assertion "Every one of those 'explain' and 'state' obligations is unassigned documentation work" is wrong on both the type and the assignment.

GROUND 2 — the doc IS self-aware (the lens's explicit test). Line 1066, highest-risk unknown #5: "Recovery usability and user custody for an accountless identity." That is the doc flagging the finding's strongest sub-claim in its own unknowns section.

GROUND 3 — the "missing" premise is overstated. The investigator dismisses line 123 ("documentation, downloads, checksums, and installation instructions") as "listed under what can be deployed TODAY, not as roadmap work." That is backwards: it is existing practice, and the repo confirms it. README.md:88-129 is end-user install documentation ("You install one program and run it — no building required") with a per-OS table (brew, macOS/Linux install scripts, PowerShell); docs/agent-guide.md is a published guide; docs/glossary-fr.md shows FR practice already exists. An architecture roadmap not re-enumerating ongoing docs practice is not a gap.

GROUND 4 — the "critical path" claim in the proposed fix is unsupported. Line 904 is "recovery succeeds from a fresh install on every supported OS" — a functional test of the mechanism, with no docs dependency. Worse for the finding, line 347's enforced test-restore-before-setup-completes is the doc's in-product answer to recovery-custody risk, deliberately reducing reliance on a written guide.

GROUND 5 — MATERIALITY. Phase durations sum to 29-47 weeks (1-2, 3-5, 5-7, 2-3, 10-14, 8-16), with 11-17 weeks to the first production launch gate (line 952). The proposed 1-2 person-weeks is below the doc's own per-phase estimate spread and changes no go/no-go criterion, no ADR in the list at 1046-1056, no architecture choice, and no cost model. The doc scopes itself at line 4/19-20 to assessment, trust architecture, infrastructure, and measurable gates; user-docs staffing is program-ops.

RESIDUAL (why low, not drop): there is a real asymmetry worth one line — the doc plans operator-facing incident runbooks (line 859, 940, 1015) and budgets operator on-call (814), but names no user support intake path, for a product whose signature failure mode is irreversible identity loss. That is a routine launch-checklist note, not an architecture defect. Report at low, with the corrected framing (support intake only), and drop the four "explain" lines from the evidence entirely — citing them would be inaccurate.
````

### F65. Missing at lines 932, 950, 944, 890, 910

- Kind: `MISSING` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 3 is scoped at "2 to 3 weeks" (line 932) while its gate requires "an external penetration review has no unresolved critical or high finding" (line 950) and "external TLS/header/CSP assessment passes" (line 944); Phase 1 (3-5 weeks, line 890) requires an "independent security review" (line 910).

Evidence:

````text
No calendar allowance for third-party turnaround appears anywhere. `grep -n -E 'weeks|engineer'` shows every duration in the document is a phase heading; none is attributed to external review scheduling, execution, reporting, remediation, or retest. Line 198 lists `an independent security review` as a staffing assumption only. An external penetration test is a sequence of externally-controlled steps — scoping/contracting, scheduling into the vendor's queue, the test window, the report, remediation, and a retest — and the gate requires the *retest* outcome ("no unresolved critical or high finding"). Placing that entire sequence inside a 2-3 week phase alongside DNS/TLS/CDN/CSP, a service worker, two relays, a relay-auth service, promotion/smoke/rollback, metrics, and eight runbooks (lines 936-940) is not schedulable. Phase 3 is also the last phase before line 952 `This is the first production launch gate.`, so this slip lands directly on the launch date.
````

Proposed fix: Split each external review into its own calendar-time line item that is explicitly not engineering effort, e.g. "Phase 3: 2 to 3 engineering weeks, plus 4 to 8 calendar weeks of external penetration review and remediation running from the start of Phase 2." Start vendor scoping in Phase 0 (add it to the Phase 0 deliverables at lines 871-879) so the test window overlaps Phase 2 rather than serializing after Phase 3.

Dissent from the verification panel:

````text
REFUTE (high): MISCATEGORIZED, and the supporting evidence is contradicted by the investigator's own grep output.

WHAT VERIFIES. The cited lines are quoted accurately. Line 932 `### Phase 3: production web and relay operations, 2 to 3 weeks`; line 944 `- external TLS/header/CSP assessment passes;`; line 950 `- an external penetration review has no unresolved critical or high finding.`; line 952 `This is the first production launch gate.`; line 890 Phase 1 `3 to 5 weeks`; line 910 `- independent security review approves the wire formats and key lifecycle.`; lines 936-940 do list DNS/TLS/CDN/CSP, service worker, two relays + relay-auth, promotion/smoke/rollback, metrics and runbooks. It is also true that no vendor lead time, procurement, or turnaround language appears anywhere: `grep -n -i -E 'calendar|vendor|third.party|turnaround|lead time|schedul|parallel|overlap'` returns nothing on point.

WHY IT DOES NOT HOLD AS "MISSING/high".

1. The evidence's central assertion is false on its own output. The finding states: "`grep -n -E 'weeks|engineer'` shows every duration in the document is a phase heading." I ran that exact grep. It returns lines 45-48, 197-198, and 214 — none of which are phase headings. Line 46 reads "the companion-backed production slice is estimated at **11 to 17 engineering weeks**"; line 214 reads "Approximately 11 to 17 engineer-weeks". The unit is explicitly *engineering effort*, not calendar time — which is precisely the distinction the finding claims the doc fails to make.

2. The proposed fix asks for something the doc already does. The fix proposes rewriting as "Phase 3: 2 to 3 engineering weeks, plus N calendar weeks of external review." But Phases 0-3 sum to exactly 11-17 weeks (1+3+5+2=11, 2+5+7+3=17), matching line 46's "11 to 17 engineering weeks" verbatim. The phase headings ARE the decomposition of an explicitly engineering-effort figure. The doc has already declared these are not calendar durations.

3. Lines 47-48 defuse the severity argument: "These are planning estimates, not release commitments." The finding's closing escalation — "this slip lands directly on the launch date" — presupposes a launch date. `grep -i -E 'launch date|release date|deadline|target date|ship by'` finds none in 1087 lines. There is no date to slip.

4. The item the finding calls absent is partly present at the line it cites as evidence against itself. Line 197-198: "Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time, and an independent security review." The security review is listed as a resourced assumption *inside* the planning estimate, parallel to the part-time ops engineer — i.e. a standing/retained reviewer, not a procurement event. The finding acknowledges this and dismisses it as "a staffing assumption only." That dismissal is a judgment about whether a staffing assumption suffices; it is not a demonstration of absence.

5. The "not schedulable" conclusion rests on a serialization premise the doc does not make. Line 866-867 reads: "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase." That constrains *dependent implementation work* only. Nothing forbids vendor scoping in Phase 0 or a test window overlapping Phase 2 — which is exactly what the fix proposes as a remedy. The doc is silent on a tactic it never prohibits.

6. The "4 to 8 calendar weeks" figure anchoring the severity is unsourced. No vendor SOW, quote, or citation was produced; per the evidence rule that alone would make the quantitative core UNVERIFIABLE.

CORRECT CLASSIFICATION. This is DISAGREE, not MISSING. The doc made a stated planning judgment — external security review is a staffed assumption folded into estimates that are explicitly engineering-effort and explicitly not release commitments — and the reviewer thinks that judgment is too optimistic. That is a legitimate opinion, but under my charge it is a judgment call presented as an omission, and the omission claim is undercut by lines 45-48, 197-198, and 214.

RESIDUAL VALUE. Small and real: even granting the engineering-week framing, a doc whose launch gate (line 952) depends on two externally-controlled outcomes (lines 944, 950) would be improved by one sentence noting that external assessments carry vendor lead time and should be scoped early. That is a low-severity editorial note under DISAGREE, not a high-severity scheduling defect. Recommend keeping it only if reworded to drop the false "every duration is a phase heading" claim, the nonexistent launch date, and the unsourced 4-8 week figure.
````

### F66. Missing at lines 989

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 5 go/no-go gate: "sandbox escape and confused-deputy review passes" (line 989).

Evidence: ASPIRATIONAL. No reviewer, no method, no criteria, no artifact, and no definition of "passes". It is the weakest of the three external-review gates: line 950 at least specifies a severity threshold (`no unresolved critical or high finding`) and line 910 at least names a subject matter. This gate guards TB5, which the document describes as untrusted third-party code execution (line 287 `Package signatures prove provenance, not harmlessness`). Severity is medium rather than high only because Phase 5 is well past the first production launch gate (line 952).

Proposed fix: Restate as: "an adversarial review by a reviewer who did not author the host, working from a written threat model of the WIT world, produces a report with no unresolved critical or high finding; the review explicitly covers guest-to-host memory escape, host-function argument confusion, capability re-delegation between components, and time-of-check/time-of-use in the policy broker."

Dissent from the verification panel:

````text
REFUTE (high): REFUTED as stated. The finding's central evidence — "No reviewer, no method, no criteria, no artifact" — is factually wrong because the investigator read line 989 in isolation and did not read the section the gate reviews.

1. Line 989 is NOT the sole gate for TB5. It is item 1 of a six-item Phase 5 gate list (lines 989-996). The very next item, lines 990-991, is a concrete testable criterion covering the same authority-containment property: "a component cannot access a secret, file, room, network, process, or pipe without the corresponding import and grant". Line 992: "quota violation terminates cleanly without corrupting host state". The framing that TB5's untrusted-code risk rests on one aspirational line does not survive reading the adjacent six lines.

2. The "method" and "artifact" the finding says are absent are specified at docs/production-deployment.md:471-490 ("### Permissions and sandbox"), which is what such a review would review: line 473-474 "Define a narrow Jeliya WIT world. A missing import means the component cannot ask the host for that facility."; line 475 "Never give components an identity key or a generic signing primitive."; lines 476-477 "A component proposes an action; the policy broker validates it, and the runtime/user signs the resulting event."; line 480 "Deny network by default."; lines 484-485 dedicated workers + "opaque-origin sandboxed iframe and sanitized message boundary"; lines 486-487 "Wasmtime Component Model without ambient WASI filesystem, environment, process, or network access." Lines 475-477 are precisely the confused-deputy mitigation; 484-487 are precisely the sandbox-escape mitigation. The proposed fix's four bullets largely re-label what the doc already names ("sandbox escape" = guest-to-host memory escape; "confused deputy" = host-function argument confusion), adding only capability re-delegation and TOCTOU.

3. DOC IS SELF-AWARE, in exactly the section my lens directs me to check. ADR item 7, line 1055: "Component package metadata, trust-root custody, and WIT world." The proposed fix demands the review work "from a written threat model of the WIT world" — but the doc correctly defers the WIT world to a future ADR. You cannot specify a threat model of an undesigned artifact; demanding it at proposal time inverts the doc's own sequencing. Line 876 also makes threat-model updating explicit Phase 0 deliverable work.

4. MATERIALITY — decisive. The decision this doc asks for is stated at lines 22-25 ("It does not authorize a production deployment by itself... remains a proposal until the architecture decision is accepted") and line 875 ("accept or reject the hybrid architecture through an ADR"). Third-party components are excluded from that decision twice: line 504 "Third-party components are excluded from the first production slice." and line 1023 (explicit exclusion list). Phase 5 begins only after Phase 0 (1-2w) + Phase 1 (3-5w) + Phase 2 (5-7w) + Phase 3 (2-3w) + Phase 4 (10-14w), i.e. 21-31+ weeks out, and line 866-867 gates each phase on the prior one. The wording of line 989 cannot change the accept/reject decision the doc requests.

5. The finding's own ranking argument is contestable. It claims 989 is weakest because 910 "at least names a subject matter" — but 989 names TWO specific attack classes (sandbox escape, confused deputy), which is narrower and more actionable than 910's "wire formats and key lifecycle", and 910 carries no severity threshold either. 989 is weaker than its siblings on exactly one axis, not overall.

WHAT SURVIVES: a genuine but minor drafting-consistency nit. Line 989 omits the reviewer-independence qualifier its siblings carry (line 910 "independent security review", line 950 "an external penetration review") and omits line 950's severity threshold ("no unresolved critical or high finding"). Given the doc lists "security-reviewers" in its audience (line 11), aligning 989 with the house convention is a cheap one-line edit. But it must be re-characterized: this is a WORDING CONSISTENCY issue, not MISSING, and low, not medium.

ACCURATE claims I confirmed while checking: the finding correctly quotes line 989 verbatim; correctly quotes line 287 "Package signatures prove provenance, not harmlessness"; correctly quotes line 950's severity threshold; correctly quotes line 952 "This is the first production launch gate"; and its severity-reduction rationale (Phase 5 sits well past the first launch gate) is sound reasoning — it simply did not go far enough.
````

### F67. Missing at lines 1077, 800-801

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: Line 1077 attributes "current managed-relay starting price" to the Iroh hosting page, and line 800-801 derives a two-relay total from it.

Evidence: https://www.iroh.computer/services/hosting states only "$0.27/hour and up" — it does NOT state the billing unit. The per-relay unit that the doc's x2 multiplication depends on appears only on https://www.iroh.computer/pricing: "$0.27/relay/hour", with a worked example "$197.10/mo" for 1 relay. That page is not cited anywhere in the doc.

Proposed fix: Add https://www.iroh.computer/pricing to the citation list as the source for the per-relay unit and the plan tiers, and cite it at line 800. Keep /services/hosting for the public-service limitations claim.

### F68. Missing at lines 800-802, 817-824

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: The cost model's only variable relay cost is bandwidth: "$389 before bandwidth or SLA charges" and the formula "relay instance-hours + relayed GiB * provider egress rate + managed support or SLA charges".

Evidence: https://www.iroh.computer/pricing documents two usage-metered dimensions the model omits entirely: "Concurrent endpoints: $0.50/100 endpoints" beyond a base of 100, and "Metrics: $1.49/1K DPM" beyond a base 10K DPM, on the Pro plan ("Pay as you go pricing"). For a hosted consumer product every browser tab is a concurrent endpoint, so this scales directly with active users and is plausibly a larger driver than relay instance-hours.

Proposed fix: Add concurrent-endpoint and metrics overage to both the table and the formula, e.g. "+ max(0, concurrent_endpoints - 100)/100 * $0.50 + metrics overage". Note explicitly that concurrent endpoints scale per active user, unlike the fixed relay hours.

Dissent from the verification panel:

````text
REFUTE (high): FACTS CONFIRMED, MATERIALITY REFUTED. The investigator's raw evidence is real: https://www.iroh.computer/pricing lists on Pro "Concurrent endpoints ... $0.50/100 endpoints" (included base 100) and "Metrics ... $1.49/1K DPM" (included base 10K DPM), and the doc's formula at lines 820-824 does omit both. I verified this by fetching the page directly. But the finding's load-bearing justification is affirmatively wrong.

1) THE CORE RATIONALE IS FALSE. The investigator asserts concurrent endpoints are "plausibly a larger driver than relay instance-hours" while never establishing the billing period — the entire claim hinges on it. Iroh's own cost estimator resolves it: the charge is MONTHLY, not hourly. Vendor worked example (1 relay, 500 concurrent endpoints): Relays $197.10/mo ("1 x $0.27/hour x 730 hrs"), Connections $2.00/mo ("400 extra x $0.5/100"), Metrics $0.00 ("Included in base plan"), total $218.10/mo. Connections are ~1% of relay hours in the vendor's own illustration — the opposite of the finding's claim. Computed magnitudes: 1,000 endpoints -> $4.50/mo; 10,000 endpoints (TOP of iroh's own calculator slider) -> $49.50/mo, against the doc's $389 relay baseline and $400-600 band. Matching $389 needs ~77,900 concurrent endpoints, ~7.8x beyond the vendor's slider max, for a product the doc scopes (lines 1036-1042) as a pre-launch, desktop-focused first slice with no server peer. The omission cannot flip any decision the doc asks for.

2) THE METRICS HALF IS SELF-NEGATING. Estimator shows 8,700 DPM = $0.00, included. DPM = metrics-per-endpoint x endpoints x push frequency — a driver the doc's privacy architecture structurally suppresses: line 729 bans "identity, device, or endpoint IDs, including shortened values" and line 741 mandates "Aggregate metrics inside the service where possible." A design forbidding endpoint-scoped series and requiring pre-aggregation cannot generate large DPM. The proposed fix would add a formula term the doc's own constraints drive to ~$0.

3) PARTIAL SELF-AWARENESS (weighed honestly, not overclaimed). Line 810 already budgets "Privacy-reviewed monitoring | $0 to $150"; line 811's "$400 to $600" band is $200 wide and absorbs the endpoint charge even at 10k endpoints; lines 827-828 explicitly defer per-user pricing "until real room size, online time, and file-transfer distributions are measured" — room size x online time is functionally concurrent endpoints; line 1069 flags relay bandwidth economics as a top unknown. None NAME endpoint metering, so this is partial mitigation, not a complete defense.

4) TWO POINTS FOR THE DOC. The doc cites /services/hosting (line 1077); I fetched it and it states only "$0.27/hour and up" plus "Negotiated bandwidth", with NO endpoint or DPM line items — the doc accurately represents its cited source, and the investigator's facts come from a different page. Provider selection is also still open (line 1048 ADR; lines 1040-1041 keep self-hosting live; line 813 costs self-hosted at $50-200/mo), so importing Pro-SKU line items is premature precision for a provider that may not be chosen.

ACCURATE CLAIMS VERIFIED IN PASSING (coverage, not silence): $0.27/hour confirmed on both pages; $0.27 x 720 x 2 = $388.80 ~ "$389" for a stated 30-day month, arithmetic correct (iroh's 730-hr convention gives $394.20, trivial delta); "before bandwidth or SLA charges" is a genuinely accurate hedge — no egress line appears in published rates and SLAs are Enterprise-only.

RESIDUAL: a real but minor editorial gap — the three-term formula omits a fourth small metered dimension and never states that any cost scales with concurrent users. Worth one clause, not a medium-severity gap. I chose low over drop because the omission is factually real; I set refuted=true because the finding AS WRITTEN (medium, "larger driver than relay instance-hours", plus a metrics-overage term) does not survive. Confidence high: I fetched both pages myself and corroborated the estimator via independent search; minor caveat that the estimator breakdown was rendered through a summarizing fetch rather than raw HTML, though search independently confirmed the "$0.5/100" connections math and the 0-10,000 endpoint slider range.
````

### F69. Missing at lines 807, 804-811

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review:

````text
"Two managed Iroh relays | Approximately $389 before bandwidth/SLA" — the table has no line item for an Iroh platform subscription.
````

Evidence: https://www.iroh.computer/pricing lists dedicated relays at "$0.27/relay/hour" under the paid tiers, with plan pricing "Free - $0/month", "Pro - $19/month" (marked "Most popular"), "Enterprise - Contact Us". The Free tier is capped at "10" concurrent endpoints and "1K DPM". A production deployment on dedicated relays therefore also carries the $19/month Pro base fee, which appears nowhere in the table.

Proposed fix:

````text
Add a table row: "Iroh Services plan (Pro) | $19" and adjust the total accordingly.
````

### F70. Missing at lines 358, 361-362, 972

- Kind: `MISSING` | Severity: `low` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: A v1 peer sharing a room with a v2 peer will silently ignore device.revoked and keep accepting events from a revoked device; no gate tests a mixed-version room.

Evidence: Doc line 358 asserts without qualification: "Room peers receive and validate the device-authorization update." docs/PROTOCOL.md:236-239 is normative and contradicts that for older peers: "Clients MUST ignore `TimelineEvent` `kind` values they do not recognize rather than erroring — render them as an inert 'unsupported event' or skip them. This is what lets a lower-`protocol` peer coexist with a higher one in the same P2P room: you fold the events you understand and pass over the rest." A v1 peer therefore folds past `device.revoked` and continues to accept authored events from the revoked device. The Phase 4 gate at line 972 tests only "a revoked device cannot author an accepted future event" — which passes in an all-v2 room and says nothing about a mixed room. `grep -c -i` for "v1 peer" and "mixed" in the doc = 0.

Proposed fix: State the mixed-version revocation limit explicitly beside line 361-362 (revocation is only enforced by peers that understand v2), and add a mixed-version test to the Phase 4 gate: a room containing one v1 peer and one v2 peer, where the v1 peer must be shown to either enforce revocation or be excluded from rooms that rely on it.

Dissent from the verification panel:

````text
REFUTE (high): MISCATEGORIZED on three independent grounds, and its factual premise is refuted by the pinned implementation.

WHAT I CONFIRMED AS ACCURATE (quote fidelity is fine):
- docs/production-deployment.md:358 reads exactly "- Room peers receive and validate the device-authorization update." and 361-362 read "- Device revocation blocks future authorship and future encrypted epochs. It / cannot recall material already received." Verified.
- docs/PROTOCOL.md:236-239 is quoted verbatim and correctly. Verified.
- The doc contains zero occurrences of "v1 peer" or "mixed". Verified by grep.
- Citation error: the revoked-device gate is line 973 ("- a revoked device cannot author an accepted future event;"), not 972 (972 is the offline-convergence gate). Minor.

GROUND 1 — the stated mechanism ("silently ignore") is WRONG at the P2P validation layer. The proposal builds on the pinned iroh-rooms rev 71fbb50. /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/event/content.rs:38-39: "The closed MVP event-type registry (Event Protocol §7). Unknown strings map to [`RejectReason::UnknownEventType`]." And content.rs:8: "any unknown key is rejected ([`RejectReason::InvalidContent`])". A v1 peer does not "fold past" device.revoked — it rejects it loudly as UnknownEventType. The finding's verb ("silently ignore", "folds past") is contradicted by the actual code.

GROUND 2 — PROTOCOL.md rule 2 was applied to the wrong layer. `TimelineEvent` is the daemon-to-UI *display* view-model, not the peer validation record: crates/jeliya-core/src/materializer.rs:1-9 ("Pure `StoredEvent` -> `TimelineEvent` JSON view-models... Event kinds outside the protocol's displayed set (`member.removed`) fold to `None` and are omitted from timelines") and materializer.rs:99-100 ("or `None` for kinds the protocol does not enumerate (`member.removed`)"). `member.removed` is security-load-bearing and is NOT a TimelineEvent kind at all, yet is fully enforced from the log fold — see crates/jeliya-core/src/supervisor.rs:2964-2973, which derives removed-member sets by scanning `EventType::MemberRemoved` from the store. Rule 2 governs what a UI renders, not what a peer accepts. The inference "rule 2 therefore means revocation is unenforced" does not follow.

GROUND 3 — the investigator quoted rule 2 and stopped one line short of the rule that actually governs v1-vs-v2. PROTOCOL.md:240-242 rule 3: "A higher `protocol` is only assumed backward-compatible across the **same major**. A major bump may remove or reshape fields and requires an explicit client update." Plus PROTOCOL.md:224-225: a client "MUST read `daemon.status` once after connecting and treat a `protocol` it does not support as a hard incompatibility." Rule 2's coexistence guarantee is scoped to additive changes within a major; the doc's line 353 explicitly frames this as "Protocol v2", i.e. a major bump, which rule 3 places outside rule 2's coexistence promise. The normative text cited as contradicting the doc in fact prescribes the exact mitigation the finding asks for.

GROUND 4 (the charge's explicit refutation trigger) — the residual concern IS present elsewhere in the doc, at proposal altitude and correctly sequenced. Line 899 makes "protocol version and capability negotiation" a Phase 1 deliverable — three phases BEFORE Phase 4 ships device revocation. Line 1051 lists ADR item 4 "Multi-device and revocation event semantics." Line 1063 lists highest-risk unknown 3 "Multi-device compatibility with existing room membership history." A "MISSING" item covered three times is not missing; the finding's grep only failed because it searched for vocabulary ("v1 peer", "mixed") the doc does not use.

WHAT ACTUALLY REMAINS, and its correct class: after the above, the only live residue is "the Phase 4 go/no-go bullet at line 973 should have named a mixed-version case explicitly." That is a preference about gate granularity in a phase whose version-negotiation prerequisite already landed in Phase 1 — a DISAGREE, not a MISSING, and certainly not critical. Severity "critical" is unsupportable for a stylistic gate-wording preference whose asserted mechanism is refuted by the pinned code. Recommend drop; if the reviewer insists on keeping it, it must be re-labeled DISAGREE at low and stripped of the "silently ignore" claim.
````

### F71. Missing at lines 367-402, 785-796

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: No age gating or minor-safety posture for a public messaging product.

Evidence: `grep -c -i` over the doc = 0 for: minor, "age gating", COPPA, "age verification". The product is public messaging reachable by link: line 371 defines an invitation URL `https://app.jeliya.ai/join#v1.<...>` and line 387-388 permits invites valid "no more than 24 hours" for asynchronous use. There are no terms of service (see the legal finding) and therefore no stated minimum age, in a product whose own abuse section (785-796) anticipates block/report tools.

Proposed fix: State a minimum age in the terms of service and record the decision in the abuse-controls section at 785-796, including whether any age assurance is performed at account (identity) creation.

### F72. Missing at lines 560-563, 636-637 vs 798-828

- Kind: `MISSING` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: Code-signing and developer-program fees required by the plan appear nowhere in the cost model.

Evidence: Line 560-563 requires "Native signing: Apple Developer ID/notarization, HSM-backed Windows Authenticode such as Azure Trusted Signing, and signed Linux repository/checksum plus provenance." Line 636-637 requires Apple and Windows material in "platform-approved secret/HSM services". Line 634 requires component root trust "offline or in an HSM". The cost table at 804-811 has no row for any of these; `grep -c` over the doc = 0 for "$99", "Developer Program", "EV cert". These are recurring annual costs, not one-offs, and Apple Developer ID enrollment is a prerequisite for the Phase 2 deliverable at line 918 ("signed macOS and Windows packages").

Proposed fix: Add an annual/one-time cost table alongside the monthly one covering: Apple Developer Program enrollment, Azure Trusted Signing (or equivalent) subscription, any EV/OV certificate, and the HSM the doc itself mandates at lines 634-637. Note which are blocking prerequisites for Phase 2 rather than launch-time purchases.

Dissent from the verification panel:

````text
REFUTE (high): FACTUALLY TRUE BUT IMMATERIAL, AND THE DOC IS ALREADY SELF-AWARE. Downgrade high -> low.

CONFIRMED parts: lines 561-563 do require "Apple Developer ID/notarization, HSM-backed Windows Authenticode such as Azure Trusted Signing"; lines 636-637 do require signing material in "platform-approved secret/HSM services"; the cost table at 804-811 has no signing row; line 918 does list "signed macOS and Windows packages" as a Phase 2 deliverable. I reproduced the investigator's greps: "$99"=0, "Developer Program"=0, "EV cert"=0.

MATERIALITY FAILS. Verified magnitudes: Apple Developer Program is 99 USD/year (developer.apple.com/support/compare-memberships: "Enrollment is 99 USD (or in local currency where available) per membership year") = ~$8.25/mo. Azure Trusted Signing / Artifact Signing Basic is $9.99/mo and per Microsoft's pricing page covers "identity validation, certificate lifecycle management and signing". Total omitted recurring cost ~$18/month, against line 811's "Approximately $400 to $600 plus relay bandwidth". That is 3-4% of the low end, inside a range the table itself states as $200 wide, and an order of magnitude below the explicitly unbounded bandwidth term the doc flags at line 815 ("Browser peers are always relayed, so file traffic can dominate cost") and at 1069. It cannot move any go/no-go gate; line 949's cost ceiling concerns load/relay, not signing.

TWO OF THE FOUR PROPOSED COST LINES ARE NOT COSTS THIS PLAN INCURS. The evidence says "the HSM the doc itself mandates at lines 634-637", but line 634 reads "Keep component root trust offline OR in an HSM" -- offline is permitted at ~$0, and component work is explicitly out of the first slice (line 1023 "third-party components" under "It explicitly excludes"). The proposed "any EV/OV certificate" line is likewise moot: the doc's named Windows route (line 562, Azure Trusted Signing) bundles certificate lifecycle into the subscription, so there is no separate EV cert purchase.

DOC SELF-AWARENESS (the lens's explicit weakening test) -- four independent places, including inside the assumptions section named in the lens: (1) lines 1038-1039, Planning assumptions: "The team can obtain Apple and Windows signing services and operate protected production environments."; (2) line 1068, Highest-risk unknowns #7: "Native signing, notarization, SmartScreen, and Linux distribution timing."; (3) line 879, a Phase 0 deliverable: "confirm DNS, CDN, relay, and signing ownership" -- acquisition is already sequenced three phases before the Phase 2 deliverable at 918, which is precisely what the proposed fix asks to add; (4) line 158 links to /home/sekou/AGI/jeliya/docs/signing-notarization.md, which at line 53 lists "Apple Developer Program membership." as a required Apple asset.

The grep-for-absence proves only that the TABLE lacks a row, not that the concern is unaddressed. Residual valid point: line 811 calls its figure an "Initial fixed total" while omitting a genuine ~$18/mo recurring item, so a one-line addition would improve completeness. That is a copy-edit nit, not a high-severity MISSING finding.
````

### F73. Missing at lines 670, 662-666, 869-888

- Kind: `MISSING` | Severity: `low` (asserted `high`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The plan assumes continuous release authority from what the repo documents as a single maintainer, with no deputy, escrow, or succession.

Evidence: Line 670: "Use a protected production environment with manual approval." Line 666: "Release candidates bake for at least 24 hours." Against that, the repo records a single-person bottleneck: SECURITY.md:11 "That opens a private advisory only the maintainer can see"; SECURITY.md:24 "This is a small open-source project, not a company"; SECURITY.md:27-28 "Best-effort response. The aim is an acknowledgment within a week... there is no SLA"; docs/known-gaps-roadmap.md:96 "explicit release authority is granted to the sole publishing job". `grep -c -i` over the deployment doc = 0 for "single maintainer", "bus factor", "hand-off", escalation.

Proposed fix: Add a continuity subsection: who holds production-approval authority when the maintainer is unavailable, how signing and release-authority credentials are escrowed, and what the documented degraded mode is (e.g. no promotions, rollback-only) during that window.

Dissent from the verification panel:

````text
REFUTE (high): Refuted as classified. The underlying observation is real but the finding is miscategorized and its premise is contradicted by the doc.

MISCLASSIFIED (MISSING -> DISAGREE). A MISSING finding is strongest when the doc is silent on the axis. This doc is not. It raises staffing explicitly and parks it: line 45 "For a small team of two to three engineers, the companion-backed production slice is estimated at **11 to 17 engineering weeks**"; line 1038 under the "### Planning assumptions" heading (line 1034) "The team can obtain Apple and Windows signing services and operate protected production environments"; line 879 Phase 0 gate "confirm DNS, CDN, relay, and signing ownership"; line 814 "move availability and on-call cost to the team"; and line 859 allocates `docs/runbooks/` for "Deployment, rollback, relay failure, key rotation, and incident procedures" — the designated home for approval-authority procedure. Arguing that naming this as an assumption is insufficient depth is a judgment about treatment, not an unraised topic.

PREMISE CONTRADICTED BY THE DOC. The claim is that "the plan assumes continuous release authority from what the repo documents as a single maintainer." The plan does not assume a single maintainer; line 45 states its staffing basis as a team of two to three engineers. The finding attributes to the doc a premise the doc does not hold.

EVIDENCE ERROR (one of three items). docs/known-gaps-roadmap.md:96 "explicit release authority is granted to the sole publishing job" is cited as proof of a single-person bottleneck. It is not about a human. docs/release-vs-main.md:86-88 disambiguates: "only the final publishing job can write; it verifies the sealed receipt without executing candidate bytes, and only its final step receives the token after explicit release authority." That is CI least-privilege — exactly one job holds the publish token. The investigator read "sole" as "sole human."

WHAT SURVIVES. I confirmed the doc quotes are exact (line 670 "Use a protected production environment with manual approval."; line 666 "Release candidates bake for at least 24 hours.") and that no succession/escrow/deputy/bus-factor/degraded-mode content exists anywhere in the 1087-line doc — I enumerated all 60 headings and grepped the continuity term set; the incident runbooks section (695-712) covers compromise scenarios only, never human unavailability. The SECURITY.md quotes at :11, :24, :27-28 are verbatim and do evidence a thin support posture, and git history corroborates a near-solo project (161 of 168 commits from one human across two emails, rest dependabot). So "the doc mandates manual approval without naming who holds it or what happens when they are unavailable" is a legitimate minor completeness note worth one line.

SEVERITY. High is not defensible: the premise misstates the doc's own staffing basis, a third of the evidence is misread, and the residual point is a depth-of-treatment complaint about an explicitly named assumption with a designated runbooks home. Keep at low, reframed as DISAGREE (the "team" planning assumption at line 1038 deserves decomposition into approval authority, credential escrow, and degraded mode), with the known-gaps-roadmap.md:96 evidence struck and the "single maintainer" framing dropped in favor of "a two-to-three-person team is still a thin approval bus factor."
````

### F74. Missing at lines 858-859, 340-364

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: No end-user documentation, support channel, or data-deletion path is planned.

Evidence: The change map plans only internal docs: line 858 `docs/adr/` and line 859 `docs/runbooks/`. `grep -c -i` over the doc = 0 for: "user guide", "install instructions", uninstall, contact, "first run", training, "documentation for users". Nothing covers how a user installs the companion, what to do with the recovery kit, or how to delete their identity and data — the identity/recovery section (340-364) has creation, recovery, and revocation but no deletion, and line 362 states "It cannot recall material already received." Deletion is also a GDPR erasure obligation, which compounds the legal finding above.

Proposed fix: Add end-user documentation to the Phase 3 deliverables (934-940): companion install/uninstall, recovery-kit instructions, and a data-deletion path — with an explicit, user-facing statement of what deletion cannot undo, given line 362.

Dissent from the verification panel:

````text
REFUTE (high): REFUTED AS WRITTEN — overbroad headline contradicted by repo files the investigator never opened, plus three unequal items bundled under one severity.

WHAT HOLDS UP (verified independently):
- docs/production-deployment.md:858-859 does list only `docs/adr/` and `docs/runbooks/`. Confirmed.
- Lines 340-364 contain creation/recovery/revocation and no deletion. Line 362 reads exactly "It cannot recall material already received." Confirmed verbatim.
- All six of the investigator's `grep -c -i` zeros reproduce ("user guide", "uninstall", "contact", "first run", "training", "documentation for users" = 0).
- Deletion appears in neither the ADR list (1046-1056) nor the highest-risk-unknowns list (1058-1069). Repo-wide grep of the doc for `GDPR|erasure|retention|legal|complian|regulat|jurisdiction|consent` returns zero. The doc genuinely has no legal/retention vocabulary.

NEAR-MISSES I CHECKED THAT DO NOT REFUTE (so the investigator was not sloppy here):
- Line 433 "does not prevent user deletion" is browser-storage eviction, not a deletion feature.
- Line 394 "New-user onboarding" is the two-step identity-bound invite protocol, not docs.
- Line 795 "user block/report tools" is abuse reporting, not a support channel.
- Lines 348, 401-402, 439, 919, 974 impose user-facing explanation duties ("Explain that...", "the product must state", "recovery and re-pair user interfaces") — these are UI/copy obligations, not documentation deliverables.

WHY IT IS REFUTED: the investigator grepped only the proposal and never checked the repo. End-user documentation covering exactly the three things the proposed fix asks for already exists:
- README.md:99 — per-platform install instructions ("brew install kortiene/jeliya/jeliya | Easiest to update and uninstall.")
- README.md:351-355 — "**Reset everything and start fresh.** Stop `jeliyad`, then remove the data directory for your platform: macOS: `rm -rf "$HOME/Library/Application Support/Jeliya"`" (plus Linux/Windows). That IS a documented end-user data-deletion path.
- README.md:356-359 — "**Uninstall.** Homebrew users can run `brew uninstall jeliya`..."
- SECURITY.md:9-10 — private vulnerability reporting via GitHub Security tab (a real, if narrow, inbound channel).
So "No end-user documentation, support channel, or data-deletion path is planned" is false as a blanket statement, and the proposed fix largely duplicates an established project pattern.

CLASSIFICATION CRITIQUE (my assigned lens): the MISSING bucket is right for at most one of the three bundled items. (a) End-user docs and (b) support channel are scope-boundary DISAGREE for an infrastructure/deployment architecture proposal — deferring user docs and support staffing to a product plan is normal and defensible, especially when README.md already carries them. (c) Data deletion is the only element with a genuine MISSING residual, and even it is narrower than stated: README's `rm -rf` recipe covers the current native daemon's local directory but not the NEW surfaces this proposal introduces — browser IndexedDB/OPFS/CacheStorage (426-430), the optionally cloud-hosted encrypted recovery envelope (346), relay-observed metadata (203), and peer-replicated copies (362). Bundling three items of unequal strength into one medium finding is itself a defect in construction.

EVIDENCE OVERREACH: "Deletion is also a GDPR erasure obligation" is an unqualified legal conclusion the investigator did not establish and cannot establish from the repo. Whether an operator of a local-first p2p system is a controller for peer-held content is genuinely contested; the hosted origin and relays likely do create obligations for the metadata at line 203, but that argument was never made. Asserting the conclusion flatly, then using it to "compound the legal finding above," inflates severity on an unproven premise.

CORRECTED SEVERITY low, not drop: a narrower rewritten finding is legitimate — the proposal introduces browser, cloud-envelope, and relay-metadata surfaces that the existing README deletion recipe does not reach, and no phase deliverable or ADR item addresses them. That is worth one low-severity note scoped to new-surface deletion only, dropping the end-user-docs and support-channel claims and the GDPR assertion.
````

### F75. Missing at lines 899

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim under review: The document's own Phase 1 deliverable "protocol version and capability negotiation" ignores extension points the protocol has already reserved for exactly that.

Evidence:

````text
Line 899 (Phase 1 deliverable): "protocol version and capability negotiation". docs/PROTOCOL.md:244-252 already reserves the mechanism: "**Reserved for a future minor (named now so adding them is non-breaking, not implemented today):** `daemon.status` / `ready` line / portfile MAY gain `min_protocol` (the lowest major the daemon still speaks) so a forward client can range-check a peer without a new endpoint. `/ws` MAY accept a `?protocol=<n>` connect param, or the server MAY send a first `hello` frame carrying `{ protocol, min_protocol }`, replacing the post-connect `daemon.status` round-trip." `grep -c` for min_protocol and "hello" in the deployment doc = 0. Designing a second, independent negotiation risks two incompatible schemes.
````

Proposed fix: Cite PROTOCOL.md:244-252 at line 899 and state whether the companion control protocol adopts the reserved `min_protocol` / `hello` design or deliberately diverges, with the reason.

Dissent from the verification panel:

````text
REFUTE (high): The investigator's raw evidence is literally accurate — I verified every piece — but the finding as framed (a MISSING gap at medium severity) does not survive the materiality lens. Three independent grounds.

VERIFIED ACCURATE (crediting the investigator):
1. docs/production-deployment.md:899 does read "- protocol version and capability negotiation;" as a Phase 1 deliverable (Phase 1 header at :890, "Deliver:" at :892, bullets :894-900).
2. docs/PROTOCOL.md:244-252 is quoted verbatim and correctly, including "Reserved for a future minor (named now so adding them is non-breaking, not implemented today)", the `min_protocol` bullet (:247-249) and the `?protocol=<n>` / `hello` frame bullet (:250-252).
3. The grep claim is correct. `grep -n "min_protocol\|hello\|PROTOCOL.md" docs/production-deployment.md` returns exactly one line — :410, which is only the link `[Daemon protocol](PROTOCOL.md)`. So min_protocol=0 and hello=0 in the deployment doc, as asserted.

GROUND 1 — THE DOC IS ALREADY SELF-AWARE, inside the exact range the lens flags. docs/production-deployment.md:1046 opens "### Decisions that require an ADR" and :1049 reads "2. Companion control protocol and pairing transcript." That sits at lines 1046-1049, within the 1032-1071 self-awareness section. Line 899 is adjacent to and continuous with :898 "- companion pairing/control protocol;". The finding's stated risk is "Designing a second, independent negotiation risks two incompatible schemes" — but the doc has not designed a second scheme, it has explicitly ADR-gated the design. You cannot "ignore reserved extension points" in a design that the doc deliberately declines to write. The risk is contingent on a design decision the doc routes through a gate rather than pre-empting.

GROUND 2 — SCOPE MISMATCH; "exactly that" is overstated. PROTOCOL.md's frontmatter (:3-4) scopes it as "Jeliya daemon protocol (v1)… contract between jeliya-core and every web, Flutter, FFI, script, and test client", and the reserved mechanisms are same-host sidecar constructs: "`daemon.status` / `ready` line / portfile" and "/ws" (PROTOCOL.md:247, :250), where /ws "validates only Origin + token" (:223). The deployment doc's line 899 governs a browser-origin-to-native-`jeliya-companion` control channel across a different trust boundary — `jeliya-companion` is a NEW binary (doc :916) and does not exist: `Cargo.toml:2` is `members = ["crates/jeliya-core", "crates/jeliyad", "crates/jeliya-ffi"]` and `ls crates/` confirms only those three. Further, "capability negotiation" in this doc consistently means runtime capability PROFILE, not a wire version integer — :718 "capability mode, such as companion or browser peer", :209 "per-runtime capability profiles", :205 "explicit capability degradation", :982 "browser/native capability profiles". PROTOCOL.md reserves nothing for that. The two surfaces overlap but are not the same problem; the investigator asserts identity rather than showing it.

GROUND 3 — NO IMPLEMENTATION EXISTS ON EITHER SIDE, so "two incompatible schemes" is doubly speculative. PROTOCOL.md:244-245 says the mechanism is "not implemented today"; the companion protocol is a roadmap bullet for a crate that does not exist. There is no artifact to become incompatible with. Contrast a real conflict, which would require at least one shipped side.

MATERIALITY VERDICT: the doc asks for accept/reject on a capability-aware hybrid architecture plus a phased roadmap with go/no-go gates. This finding changes no phase boundary, no gate criterion (Phase 1 gate is :902-910; none of the seven bullets touch version negotiation), no cost figure, and no risk ranking. No reader decides differently because a Phase-1 line item omits a cross-reference to a reserved-but-unbuilt field name. It lands wholly inside ADR #2.

WHAT LEGITIMATELY SURVIVES: a purely editorial cross-reference nit. Adding "see PROTOCOL.md:244-252" near :899 is harmless and mildly helpful so the ADR #2 author does not overlook the reserved names. That is worth at most a low-severity editorial note, not a medium MISSING architectural gap. I set refuted=true because the finding's classification (MISSING), its severity (medium), and its stated risk ("ignores"/"two incompatible schemes") all fail; I set corrected_severity=low rather than drop because the underlying cross-reference suggestion is factually grounded and costless to adopt. If the review has a nit/editorial bucket it belongs there; if the review only carries decision-relevant findings, it should be dropped.
````

### F76. Missing at lines 273-287

- Kind: `MISSING` | Severity: `low` (asserted `high`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: The trust boundary list TB1-TB5.

Evidence: The companion<->OS boundary is missing, and the companion inherits an explicitly-out-of-scope hole. docs/PROTOCOL.md:89-95 states it plainly for jeliyad: "The trust boundary is a single-user machine: any process running as the same user can already read the 0600 portfile... A shared multi-user machine is therefore out of scope." The doc quotes this at lines 177-178 as a reason jeliyad must stay local — but then designs a companion that holds the same identity keys (line 280), and TB3 is written entirely as companion-vs-browser-controller ("Browser controllers receive narrow, revocable grants"), never companion-vs-local-process. Today's key storage is plaintext JSON under 0600 (crates/jeliya-core/src/identity.rs:4-6 "Seeds are stored plaintext under owner-only permissions (the SDK MVP threat model)"), and the pairing state the companion must persist (control keys, scopes, expiry — line 332) has no stated protection at all. The doc's OS-keystore proposal (lines 312-315) partially raises this bar without ever naming the boundary it is defending.

Proposed fix: Add TB6: "Companion and local OS. The companion holds identity keys, the paired-controller table, and the local store. Same-user processes are outside the current threat model (docs/PROTOCOL.md, single-user machine). OS keystore wrapping (lines 312-315) raises the cost of key extraction on macOS/Windows; it does not make a hostile same-user process out of scope. State this in product copy and in docs/security-threat-model.md." Add the paired-controller table to the list of assets protected by the keystore, not just the identity seeds.

### F77. Missing at lines 329-331

- Kind: `MISSING` | Severity: `low` (asserted `critical`) | Verdict: PLAUSIBLE — 1 of 3 verifiers voted to refute
- Source: track `T3-Judgment`, investigator `security-attack`

Claim under review: "Both sides display a short authentication string and require user confirmation." (the SAS is never said to be derived from the completed handshake transcript)

Evidence: Line 329 says the peers "establish a Noise XX-equivalent authenticated transcript over Iroh" and line 330 says "Both sides display a short authentication string" — but the doc never states that the SAS is a function OF that transcript. The word "transcript" appears at lines 329, 850, 1049 and never in the same sentence as the SAS; grep for "channel binding" in docs/production-deployment.md returns zero hits. Contrast the normative reference the doc is imitating, RFC 6189 §4.5.2 (fetched from https://www.rfc-editor.org/rfc/rfc6189.txt): "sashash = KDF(s0, \"SAS\", KDF_Context, 256)" and "sasvalue = sashash [truncated to leftmost 32 bits]" — the SAS is computed from s0, the negotiated shared secret, precisely so that a MITM who substitutes ephemerals cannot force a match. The Phase 1 gate (line 909) tests "wrong-SAS" but a wrong-SAS test passes trivially against an unbound SAS.

Proposed fix:

````text
Replace lines 330-331 with a normative statement: "The SAS is the leftmost N bits of KDF(handshake_hash || initiator_static || responder_static || iroh_initiator_endpoint_id || iroh_responder_endpoint_id, \"jeliya-pair-sas-v1\"), where handshake_hash is the final Noise `h` after the last handshake message. A SAS derived from anything other than the completed handshake hash (e.g. from the QR nonce, or from the static keys alone) is a protocol violation." Add a Phase 1 gate: "a test vector proves that flipping any byte of any handshake message changes the SAS."
````

Dissent from the verification panel:

````text
REFUTE (high): REFUTED AS ASSERTED (critical/MISSING). The investigator's literal observations are ACCURATE, but the materiality case collapses on four independent checks, two of which the investigator's own evidence walked past.

WHAT I CONFIRMED AS ACCURATE (so coverage is legible):
- Lines 329-331 read exactly as quoted (Read of docs/production-deployment.md:322-338).
- `grep -n -i "transcript"` → lines 329, 850, 1049 only; never in the same sentence as the SAS. Correct.
- `grep -c -i "channel binding"` → `0`. Correct.
- Line 909 does read "replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed;". Correct.
- The underlying crypto point is real in the abstract: an SAS not bound to the completed handshake is MITM-defeatable. I did NOT re-fetch RFC 6189 (UNVERIFIED by me), but the convention is not in dispute — materiality is.

WHY IT FAILS THE MATERIALITY LENS:

1. THE DOC EXPLICITLY SELF-FLAGS THIS, inside the exact section the lens named. Line 1046 "### Decisions that require an ADR", item 2 at line 1049: "Companion control protocol and pairing transcript." The pairing transcript design — the precise artifact the finding says is unspecified — is named as an undecided item requiring an ADR. Reinforced at line 858: "`docs/adr/` | Accepted decisions for hosting, identity, **pairing**, encryption, and server-peer trust". Per the lens, a doc that already flags the gap makes the finding "much weaker"; here it flags it by name.

2. SELECTIVE EVIDENCE ON THE GATE. The investigator quoted line 909 to argue the gates miss unbound SAS, and stopped one line short. Line 910: "independent security review approves the wire formats and key lifecycle." An unbound SAS is precisely a wire-format defect, and Phase 1 cannot pass without that review. The "the gate passes trivially" argument requires suppressing the adjacent line.

3. ALTITUDE MISMATCH — decisive. `grep -iE "kdf|hkdf|domain.separ|truncat|leftmost|handshake hash"` over all 1087 lines returns **exit 1, zero hits**. The doc specifies no KDF, no domain-separation label, no truncation length, and no byte layout for ANY construction. BLAKE3/SHA-256 appear only as file-digest algorithms (lines 108, 459). The proposed fix ("leftmost N bits of KDF(handshake_hash || initiator_static || … , \"jeliya-pair-sas-v1\")") would be the single most cryptographically detailed sentence in the document, dropped into a bullet list whose neighbors operate at "the peers establish a Noise XX-equivalent authenticated transcript over Iroh." That is spec-altitude detail demanded of a proposal-altitude document that has already deferred the spec to an ADR.

4. IT DOES NOT MOVE THE DECISION BEING ASKED. Lines 22-25: "It does not authorize a production deployment by itself. The page remains a proposal until the architecture decision is accepted." The decision (line 875) is "accept or reject the hybrid architecture through an ADR" — companion-backed pairing vs. browser-only Wasm peer vs. exposing `jeliyad`. No reviewer flips accept/reject on the SAS derivation formula. Nothing is at risk today either: `grep -rn -i "sas|noise" crates/ docs/` outside this file returns ZERO hits, and `crates/jeliya-control/` (line 850) is itself a proposed crate. There is no code that could have implemented this wrongly.

5. UNCHARITABLE READING. Bullets 329 and 330 are adjacent: a Noise XX handshake immediately followed by a displayed short authentication string is the universal construction (ZRTP, Signal safety numbers, Magic Wormhole, WebRTC DTLS-SRTP). Treating that adjacency as licensing a SAS derived from the QR nonce reads a conventional shorthand as an affirmative omission.

RESIDUAL VALUE → LOW, not drop. One normative clause ("the SAS MUST be derived from the completed handshake hash, not from the QR nonce or static keys alone") plus a binding test vector is cheap and genuinely worth carrying into the ADR named at line 1049. But that is an ADR input, not a defect blocking acceptance of this proposal. Classified correctly it is a low-severity suggestion against lines 330-331, not a critical MISSING finding. Anything at high/critical here would imply the doc failed to address something it in fact routes to an ADR by name and gates behind an independent security review.
````

### F78. Missing at lines 883-888, 899

- Kind: `MISSING` | Severity: `low` (asserted `medium`) | Verdict: CONFIRMED — all 3 verifiers upheld it
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: The doc's Phase 0 go/no-go gate lists five conditions and never mentions upstream version skew between the hosted product and already-published Jeliya clients.

Evidence:

````text
The pinned rev's own commit message (`gh api repos/kortiene/iroh-room/commits/71fbb500` → commit.message) states: "Mixed-version note: rc.2 and rc.3 must not be mixed in one room — an rc.2 joiner never sends the capability proof an rc.3 admin now requires, and an rc.3 bootstrap against an rc.2 responder hard-stalls once the joiner holds more than ~1k events." The public Jeliya release v0.5.0 (`gh release list` → v0.5.0, prerelease, 2026-07-14T23:11:15Z) is rc.2-era: docs/release-vs-main.md:63-67 says the post-release candidate on main "repins `iroh-rooms` to `v0.1.0-rc.3`" and that "`v0.5.0` behavior is exactly what its archives contain, including its known join-after-chat limitation." `grep -n -i "mixed.version\|upgrade together" docs/production-deployment.md` → no match; only line 899 "protocol version and capability negotiation" as a Phase 1 deliverable.
````

Proposed fix: Add to the Phase 0 gate: "a coordinated fleet-upgrade plan exists — rc.2-era clients (public Jeliya v0.5.0) and the rc.3+ hosted companion must not share a room; an rc.2 joiner cannot present the capability proof an rc.3 admin requires, and an rc.3 bootstrap hard-stalls against an rc.2 responder past ~1k events."

## Low-severity findings passed through unverified

21 findings were recorded at low severity and deliberately not put through the
adversarial verification pass. They are reproduced for completeness. They carry no verdict
and should be treated as unconfirmed reviewer observations.

### U1. Disagreement with a judgment at lines 67-69

- Kind: `DISAGREE` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim under review: "Some status pages contain stale contradictions: the top of capability status records fresh candidate direct and relay runs while lower rows still say that no candidate run exists or that another is required."

Evidence:

````text
The first half is exactly right; the second half is imprecisely attributed. Verified accurate half — docs/capability-status.md:50 (row "Identity, room create/join/open, membership, and messages"): "the rc.3 candidate on `main` fixes this and has no network run yet", contradicting docs/capability-status.md:19-22 ("fresh certifying direct and forced-relay runs were executed at the candidate on 2026-07-16"), :30 ("signed schema 2 direct (`1ca39cfa`) and forced-relay (`cf28bc63`) runs of 2026-07-16; `certifiable: true`") and :32 ("Candidate network verification | certified"). Imprecise half — the genuinely contradictory "another is required" text is NOT in a row: it is docs/capability-status.md:78-79, in the closing "Preview publication rule" section: "For the next release the same bar applies to the rc.3 candidate: fresh signed network evidence at its pin". The one row that does say a further run is required, docs/capability-status.md:56 ("a fresh signed schema 2 qualification is required before the claim transfers to the next release"), is NOT stale: it is scoped to room-scoped synchronization isolation, which the top of the same page explicitly excludes at :32 ("Neither run certifies room-scoped synchronization isolation (`synchronization_isolation_claimed: false`)"). That exclusion is independently true in the signed manifests: `functional_evidence/foreign_room_non_disclosure/synchronization_isolation_claimed = False` in both docs/evidence/v0.6.0/direct.json and relay.json.
````

Proposed fix: Rewrite lines 67-69 to: "the top of capability status records fresh candidate direct and relay runs (capability-status.md:19-22, :30, :32) while the identity/rooms/messages row still says the rc.3 candidate 'has no network run yet' (:50) and the preview publication rule still demands 'fresh signed network evidence at its pin' (:78). The synchronization-isolation row (:56) is correctly scoped, not stale."

### U2. Unverifiable at lines 76

- Kind: `UNVERIFIABLE` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim under review: "React: 87 Vitest tests passed and the Vite production build succeeded."

Evidence:

````text
87 is the collected total, not necessarily the pass count. I reproduced the suite at the exact assessed commit (detached worktree at 4d4621c9, `npx vitest run`) and got: "Test Files  7 passed | 1 skipped (8) / Tests  81 passed | 6 skipped (87)", with stderr "[conformance] daemon suite skipped: .../target/debug/jeliyad not built (run `cargo build`)". The 6 skipped are src/lib/conformance/conformance.daemon.test.ts, which self-skips unless target/debug/jeliyad exists. The doc's own environment plausibly had that binary (lines 80-83 describe running the core and daemon test suites separately), in which case 87 would indeed all pass — so I can neither confirm nor refute "87 passed" as written. The Vite half is confirmed: `npx vite build` at 4d4621c9 -> "vite v7.3.6 ... ✓ 55 modules transformed ... ✓ built in 475ms". Note also that `include: ['src/**/*.test.ts']` in ui/vitest.config.ts excludes ui/e2e/*.spec.ts, so 87 covers unit tests only.
````

Proposed fix: Replace line 76 with: "React: the Vitest unit suite collected 87 tests (`ui/vitest.config.ts` scopes it to `src/**/*.test.ts`; Playwright e2e is separate); 81 passed and the 6-test daemon conformance suite ran only because `target/debug/jeliyad` was present. The Vite production build succeeded."

### U3. Missing at lines 179-180

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim under review: "Host and Origin checks defend a loopback application from DNS rebinding and cross-site WebSocket hijacking." (stated without scope, in a bulleted list characterizing the daemon's defenses)

Evidence:

````text
The judgment is correct but the scope is unstated, and the two checks have different coverage. Verified by reading crates/jeliyad/src/serve.rs:

Host gate — applies to `/ws` and every `/api/*` route, before any other dispatch (serve.rs:120-125):
```
    if (path == "/ws" || path.starts_with("/api/")) && !host_is_loopback(&req) {
        return text(StatusCode::FORBIDDEN, ...);
    }
```
Static UI paths are deliberately not Host-gated (harmless: a rebound page runs at the attacker's origin and is then blocked from every `/api/*` and `/ws` read by this same gate).

Origin gate — applies to only three of six routes:
- `/ws` upgrade: serve.rs:360-368, rejects any non-loopback Origin.
- `/api/files/share` (upload): serve.rs:495-504, rejects any non-loopback Origin.
- `/api/session`: serve.rs:213-229, a loopback Origin is one of the two accepted shapes.
NOT applied to `/api/health` (serve.rs:130-135, deliberately unauthenticated and secret-free) and NOT applied to `/api/files/local` (serve.rs:159-168 — Host + token only, no Origin check).

The `/api/files/local` omission is not exploitable on its own (serve.rs:295-299 `token_ok` still gates it and a cross-origin page cannot learn the token), but a reader of line 179-180 would reasonably infer uniform Origin enforcement across the API, which is not the case.
````

Proposed fix: Amend to: "Host checks gate `/ws` and all `/api/*` against DNS rebinding (`crates/jeliyad/src/serve.rs:120`). Origin checks gate the WebSocket upgrade (`:360`), the file upload (`:495`), and `/api/session` (`:213`) against cross-site hijacking; `/api/health` and `/api/files/local` rely on the Host gate and the token gate alone. Neither check is remote account authentication."

### U4. Missing at lines 176-177

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim under review: "`/api/session` gives the token only to the expected loopback browser shape."

Evidence:

````text
Accurate as written, but "the expected loopback browser shape" is broader than a reader will assume: the handler also accepts `Sec-Fetch-Site: none`, which browsers send on a user-initiated top-level navigation (address bar, bookmark, link from a non-web app).

crates/jeliyad/src/serve.rs:217-222:
```
    let same_origin_browser = headers.get(ORIGIN).is_none()
        && headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .map(|site| site == "same-origin" || site == "none")
            .unwrap_or(false);
```
Consequence: typing `http://127.0.0.1:7420/api/session` into the address bar renders the raw token as a JSON document and writes that URL into browser history. This is not a cross-origin hole — a cross-site `<script>`, `<iframe>`, `window.open()`, or link navigation all carry `Sec-Fetch-Site: cross-site` and are refused, and the DNS-rebinding path is independently blocked by the Host gate at serve.rs:120 — but it is a wider shape than "the expected loopback browser".

The doc's accompanying threat-model sentence (line 177-178) is fully confirmed by docs/PROTOCOL.md:90-96: "any process running as the same user can already read the 0600 portfile... a multi-user machine is therefore **out of scope**: a different local user who can reach `127.0.0.1` could obtain the token via `/api/session`."
````

Proposed fix: Amend to: "`/api/session` serves the token only to a loopback-`Origin` fetch or a browser-set `Sec-Fetch-Site: same-origin`/`none` request (`crates/jeliyad/src/serve.rs:210-229`); the `none` case includes direct user navigation, so the token can land in browser history. Neither header is forgery-proof against a non-browser local process, and its documented threat model (docs/PROTOCOL.md:90-96) explicitly excludes hostile same-user processes and shared multi-user service operation."

### U5. Missing at lines 101-103

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim under review: "Native storage uses a shared SQLite WAL database and per-room filesystem blob stores. See crates/jeliya-core/src/supervisor.rs."

Evidence:

````text
The claim is true, but the cited file does not contain the WAL enforcement, so a reader auditing at the citation cannot confirm it. supervisor.rs only REFERENCES WAL in comments (crates/jeliya-core/src/supervisor.rs:272 "one shared WAL `rooms.db`", :414, :431, :1164, :2289) and sets a busy_timeout (supervisor.rs:276-281 `StoreOptions::new(Some(Duration::from_millis(5000)))`). `grep -rn "journal_mode\|PRAGMA" crates/jeliya-core/src/supervisor.rs` returns nothing. The `PRAGMA journal_mode = WAL;` is set upstream, inside the pinned dependency: /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/store/schema.rs:42 (that checkout's HEAD is 71fbb5007bef4ce83631c94762ec68c2beef3d79, the pinned rev). The per-room blob half IS in the cited file: supervisor.rs:65 `const BLOBS_DIR: &str = "blobs";` and supervisor.rs:259-267 `fn room_blobs_dir(...) -> self.data_dir.join(BLOBS_DIR).join(hex_part)`.
````

Proposed fix: Amend line 101-103 to: "Native storage uses a shared SQLite WAL database (`rooms.db`, single file selected at [`supervisor.rs:63`]; `PRAGMA journal_mode = WAL` is set by the pinned upstream at `iroh-rooms-core/src/store/schema.rs:42`) and per-room filesystem blob stores ([`supervisor.rs:259-267`])." The distinction matters for the doc's own portability plan (lines 842-849): WAL is an upstream property that a browser adapter cannot inherit.

### U6. Internally inconsistent at lines 76

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim under review: "React: 87 Vitest tests passed and the Vite production build succeeded."

Evidence:

````text
Reproduced at the doc's own assessed commit 4d4621c9 in a clean git worktree: `npx vitest run` reports `Test Files  7 passed | 1 skipped (8)` / `Tests  81 passed | 6 skipped (87)`. The 6 skipped are `src/lib/conformance/conformance.daemon.test.ts`, which self-skips with stderr `[conformance] daemon suite skipped: .../target/debug/jeliyad not built (run \`cargo build\`)`. So 87 is the exact COLLECTED total at that commit, but "87 passed" holds only if the jeliyad binary was built first; otherwise it is 81 passed + 6 skipped. The doc is scrupulous about exactly this distinction one line later for Rust (line 77-78: "71 unit tests passed; one opt-in performance test was ignored"), so the asymmetry is an inconsistency in evidentiary rigor, not a substantive error.
````

Proposed fix: Rewrite line 76 as: "React: 87 Vitest tests collected, all passing with the jeliyad binary built (81 pass and 6 daemon-conformance tests skip when it is not); the Vite production build succeeded."

### U7. Missing at lines 74-78

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim under review: The test counts on lines 76-78 are presented as the document's local verification evidence.

Evidence: The counts are correctly scoped to the assessed commit by lines 52-54, but the doc never states the drift magnitude at repo HEAD. Measured: at current HEAD 7248fb0, `cd ui && npm test` gives `Test Files  16 passed (16)` / `Tests  219 passed (219)` — 2.5x the documented 87, from the localization work in commits #111/#112. The Rust figure did not drift: `cargo test -p jeliya-core -p jeliyad` at HEAD gives `63 passed; 0 failed; 1 ignored` (jeliya-core) plus `8 passed; 0 failed; 0 ignored` (jeliyad) = exactly 71 passed and 1 ignored.

Proposed fix: Add one sentence after line 78: "These counts are bound to `4d4621c9`. At the time of writing the Vitest suite has since grown to 219 tests at repository HEAD; the Rust core/daemon count is unchanged. Phase 0 re-runs all gates on the selected candidate SHA."

### U8. Internally inconsistent at lines 804-811

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review:

````text
Table rows for DNS/CDN, relays, worker, storage and monitoring, summing to "Initial fixed total | Approximately $400 to $600 plus relay bandwidth".
````

Evidence: Summed the doc's own rows: low end = $0 + $389 + $0 + $0 + $0 = $389; high end = $25 + $389 + $25 + $10 + $150 = $599 (computed). The stated floor of $400 does not follow from rows that all bottom out at $0 except the $389 relay line.

Proposed fix: Change line 811 to "Approximately $389 to $600 plus relay bandwidth", or raise the optional-row floors so the total is derivable. If the Pro platform fee is added (see separate finding) the correct range becomes ~$408 to ~$618.

### U9. Internally inconsistent at lines 152-153

- Kind: `INTERNALLY-INCONSISTENT` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: "Browser Iroh is supported, but current browser connections are relay-only and require an application-specific `wasm-bindgen` wrapper."

Evidence: Upstream states a conditional recommendation, not a requirement: https://docs.iroh.computer/languages/wasm-browser — "Should you need javascript APIs, we recommend that you write an application-specific rust wrapper crate that depends on iroh and exposes whatever the javascript side needs via wasm-bindgen." The doc's own line 511-512 renders this correctly ("recommends an application-specific `wasm-bindgen` wrapper"); line 153 upgrades "recommends" to "require", contradicting itself.

Proposed fix: Line 153: change "require an application-specific `wasm-bindgen` wrapper" to "and Iroh recommends an application-specific `wasm-bindgen` wrapper crate where JavaScript APIs are needed".

### U10. Disagreement with a judgment at lines 508-509, 815

- Kind: `DISAGREE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: "browser connections currently traverse a relay because browser sandboxes do not provide the UDP hole-punching path" and, downstream, "Browser peers are always relayed, so file traffic can dominate cost."

Evidence: The stated reason matches upstream exactly for the UDP path — https://docs.iroh.computer/languages/wasm-browser: "This is because we can't port our hole-punching logic in iroh to browsers: They don't support sending UDP packets to IP addresses from inside the browser sandbox." But the same page continues: "There are other ways of getting direct connections going, such as WebTransport with `serverCertificateHashes`, or WebRTC. We may expand iroh's browser support to make use of these to try to generate direct connections even when a browser node is involved in the connection." So the barrier is that Iroh has not implemented non-UDP direct paths, not that browsers cannot do direct connections. The doc's causal framing presents a current implementation gap as a browser-platform law, and line 815 builds a permanent cost assumption on it.

Proposed fix: Line 508-509: "...because Iroh's hole-punching is UDP-based and browsers cannot send UDP from the sandbox. Iroh notes that WebTransport (with `serverCertificateHashes`) and WebRTC could provide direct browser paths and that it may expand browser support to use them, so relay-only is a current implementation state rather than a permanent platform limit." Add a hedge at line 815 that relay bandwidth economics could improve if Iroh ships direct browser transports.

### U11. Disagreement with a judgment at lines 800-802, 807

- Kind: `DISAGREE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: "Two continuously running relays therefore start near $389 per 30-day month" — the 30-day (720-hour) normalization.

Evidence: Arithmetic verified: 0.27 * 24 * 30 * 2 = 388.8, so $389 is right for a 30-day month. But Iroh's own pricing page normalizes on a 730-hour month: https://www.iroh.computer/pricing shows "$197.10/mo" for 1 relay (= 0.27 x 730). On the vendor's own basis two relays are 0.27 * 730 * 2 = $394.20 (computed), and a 31-day month is 0.27*24*31*2 = $401.76. A budget built on 720 hours under-provisions every longer month.

Proposed fix: Use the vendor's basis: "Two continuously running relays start near $394 per month (Iroh bills on a 730-hour month; its own page shows $197.10/mo per relay), rising to about $402 in a 31-day month." Update the table row at line 807 to match.

### U12. Disagreement with a judgment at lines 801-802

- Kind: `DISAGREE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: "$389 ... before bandwidth or SLA charges" — implying an SLA is a surcharge on top of the $0.27/hr rate.

Evidence: https://www.iroh.computer/pricing lists "SLAs" only under the Enterprise plan ("Enterprise - Contact Us", with "Custom retention", "SLAs", "Dedicated Support Engineer", "On-prem and multi-cloud"); Pro lists "8x5 support tickets" and no SLA. https://www.iroh.computer/services/hosting says only "Uptime SLAs available". An uptime SLA therefore appears to require moving to Enterprise custom pricing, not paying a surcharge on a Pro-plan rate — so the $389/$394 figure is not a valid base for an SLA-backed deployment.

Proposed fix: Reword to: "...before bandwidth. An uptime SLA is listed only under the Enterprise plan (custom pricing), so an SLA-backed deployment is not this figure plus a surcharge — it requires a separate quote." If production requires an SLA, the whole managed-relay line item should be marked TBD pending a quote.

### U13. Unverifiable at lines 813-814

- Kind: `UNVERIFIABLE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim under review: "Self-hosted relays may reduce the direct infrastructure bill to roughly $50 to $200 per month plus egress"

Evidence: I confirmed the software is free — https://www.iroh.computer/services/hosting: "Self-host your relays for free. Forever." I did NOT verify the $50-$200 compute figure against any live cloud provider price list; no provider, instance class, or region is named in the doc, so the estimate is not checkable as written. It is plausible for two small VMs but is currently an unsourced number.

Proposed fix:

````text
Name the assumed provider and instance class (e.g. "two 2 vCPU / 4 GB instances on <provider>, ~$X each") and link the price page, or mark the range as an unvalidated placeholder pending a Phase 0 sizing exercise. Also cross-reference the self-hosted credential gap noted above.
````

### U14. Missing at lines 68-72

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `upstream-issues`

Claim under review: The doc discusses release-state contradictions and stale status pages (lines 68-72) but never states what the currently-published Jeliya releases actually do.

Evidence:

````text
Jeliya v0.5.0 (released 2026-07-14T23:11:15Z, `gh release list`) is pinned to iroh-rooms d0ceb0b320f1ff3a576b63d8b24aa1bf76a2d3bb (2026-07-14T18:34:11Z, merge of PR #107). Upstream PR #111 "fix(sync): joins deadlock once a conversation has started in a room" merged 2026-07-15T20:37:36Z at 7d706dd5 — AFTER d0ceb0b. `compare/d0ceb0b...71fbb500` → {"status":"ahead","ahead_by":21}. Confirmed by docs/verification-evidence.md:320-324: "rc.3 carries the join-after-conversation deadlock fix (upstream PR #111 — at `d0ceb0b`, and therefore in released `v0.5.0`, an invite minted after any non-admin chat cannot complete `room.join`)". `grep -n "v0\.5\|deadlock" docs/production-deployment.md` → no match.
````

Proposed fix: Add one sentence to the assessment boundary: "The join-after-conversation deadlock (upstream PR #111) is fixed at the current pin but is present in the published v0.5.0 artifacts (pinned to d0ceb0b, pre-#111); v0.6.0 is the first release carrying the fix. Any onboarding path that reaches existing v0.5.0 installs must account for it." This is not a blocker for the hosted product, which builds from the rc.3 pin.

### U15. Missing at lines 428 (with 435)

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `browser-platform-facts`

Claim under review: "OPFS stores blobs, component packages, journals, checkpoints, and large snapshots."

Evidence: The doc assigns journals and checkpoints to OPFS without noting the threading constraint that makes that performant. https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system : "Note: This feature is available in Web Workers", and "Web Workers don't block the main thread, which means you can use the synchronous file access APIs in this context. Synchronous APIs are faster as they avoid having to deal with promises." `FileSystemSyncAccessHandle` — the only low-latency read/write path, and the one journaling and checkpointing want — is reachable only from a dedicated worker. The same page also notes: "Despite having 'Sync' in its name, the createSyncAccessHandle() method itself is asynchronous."

Proposed fix: Add: "All OPFS journal and checkpoint I/O runs in a dedicated Web Worker; `FileSystemSyncAccessHandle` is unavailable on the main thread. The storage adapter is therefore worker-resident and the main thread talks to it over a message port." This also interacts with the COEP/cross-origin-isolation decision at line 615.

### U16. Disagreement with a judgment at lines 587

- Kind: `DISAGREE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: `img-src 'self' data:` (line 587) — `data:` is included in the baseline.

Evidence: Nothing in the current build needs it. `grep -o "url(data:[a-z/+-]*" ui/dist/assets/*.css` returns no matches. No `toDataURL` or `data:image` anywhere in ui/src. QR codes — the one obvious candidate — render as inline SVG, not a data URI: ui/src/components/QrCode.tsx:60 `<svg`. All icons are same-origin files (ui/index.html lines 12-14 reference `/favicon.svg`, `/favicon-32.png`, `/apple-touch-icon.png`; ui/public/ contains exactly those plus og.png and the two PWA icons).

Proposed fix:

````text
Drop `data:` from img-src until something demonstrably needs it, making the line `img-src 'self';`. The risk is low in isolation (an `<img src="data:image/svg+xml,...">` cannot execute script), but it is an unused grant in a policy whose whole value proposition is that every source is justified, and it is the usual first step toward a data:-based UI-spoofing or exfil gadget. When file previews land, add `blob:` — not `data:`.
````

### U17. Unverifiable at lines 643, 655 (about the block at 583-613)

- Kind: `UNVERIFIABLE` | Severity: `low` | Verdict: not verified
- Source: track `T2-External`, investigator `csp-and-headers`

Claim under review: "Every pull request runs: ... CSP and Trusted Types tests" (lines 643, 655) is stated in the present tense.

Evidence:

````text
No such test exists today and no header config exists to test. `grep -rn -i "content-security-policy|Permissions-Policy|Strict-Transport" --exclude-dir=node_modules --exclude-dir=.git --exclude-dir=target --exclude-dir=dist .` over the repo returns exactly three hits: docs/production-deployment.md:607, docs/production-deployment.md:612, and crates/jeliyad/src/serve.rs:452 — the daemon's per-file header, not an app.jeliya.ai policy. `ls ui/e2e` shows 23 specs (a11y-matrix, invite, rooms, settings, …); none is CSP- or Trusted-Types-related. I cannot tell from the doc whether line 643 describes current CI or the target state, so I am not calling it WRONG.
````

Proposed fix: Disambiguate the tense in the CI section — mark line 655 as a Phase 0 deliverable rather than an existing gate — and add the concrete gate: a Playwright spec that loads the built app under the exact production header set and fails on any `securitypolicyviolation` event. Without that, nothing prevents a future dependency from reintroducing an inline-style or inline-script requirement that silently breaks the policy.

### U18. Missing at lines 974, 995-996

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 4 gate: "product copy makes no durable background-availability claim" (line 974); Phase 5 gate: "no blind-backup privacy claim is made before encrypted-envelope and key-epoch interoperability tests pass" (lines 995-996).

Evidence: ASPIRATIONAL in their copy-policing halves. Neither names an owner, a surface (in-app strings? marketing site? app-store listing? release notes?), a method, or a definition of the prohibited claim. Line 995-996 is half-objective — `before encrypted-envelope and key-epoch interoperability tests pass` is decidable — but the "no claim is made" half is not. Both are mechanizable in this repo, and both currently have a blind spot: any copy check that reads only `ui/src/l10n/en.ts` misses `fr.ts`, and the literal scan does not cover the proposed new module paths (scripts/check-ui-i18n.mjs:67, `LITERAL_SCAN_ROOTS = ['ui/src/App.tsx', 'ui/src/components']`).

Proposed fix: Replace with: "a banned-phrase lint over ui/src/l10n/en.ts and fr.ts plus the marketing copy source fails CI on background/always-on/durable-availability phrasing (list maintained in docs/), and a named product owner signs the pre-launch copy review covering in-app strings, the site, and store listings, in both locales."

### U19. Missing at lines 887-888

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 0 gate: "production work does not continue with upstream issue #121 exploitable and unmitigated" (lines 887-888).

Evidence: Not falsifiable as written: "exploitable" has no stated criterion, no test is named, and the bullet is phrased as a prohibition on continuing rather than as a condition that can be checked. The document already contains the concrete mitigation elsewhere — line 390-391 `Fix upstream issue #121 or suspend normal room fanout while an unproven provisional connection exists` — which IS testable, but the gate does not reference it.

Proposed fix: Restate as: "either upstream #121 is fixed at the pinned revision and a regression test demonstrates it, or the fanout-suspension mitigation of [Secure invitation links] is implemented and a test proves that no room event reaches an unproven provisional dialer during an open join window."

### U20. Missing at lines 975

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: Phase 4 gate: "the exact upstream/browser-adapter revision receives security qualification" (line 975).

Evidence: ASPIRATIONAL as written — "receives security qualification" names no procedure, reviewer, or output. Partially mitigated by repository precedent: the concept of exact-revision qualification is established (line 58-61 cites `capability-status.md` naming a `network-qualified v0.6.0 pair`, and `docs/verification-evidence.md` exists), so the gate is repairable by reference rather than by invention.

Proposed fix: Restate as: "the browser-adapter revision completes the qualification procedure defined in docs/capability-status.md, producing signed direct and forced-relay evidence bound to that exact SHA in docs/evidence/, plus a security review of the wasm-bindgen boundary."

### U21. Missing at lines 45-48, 977

- Kind: `MISSING` | Severity: `low` | Verdict: not verified
- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim under review: The executive summary's totals stop at Phase 4; the roadmap's full span is never stated.

Evidence: Line 46-47 gives 11-17 for the companion slice and `approximately 10 to 14 weeks` for the browser peer. Line 977 adds `### Phase 5: components and optional server peers, 8 to 16 weeks`, which appears in no summary. Summing all six phases gives 29-47 weeks, a figure that appears nowhere in the document. A reader working only from the executive decision section will underestimate the full program by 8-16 weeks.

Proposed fix: Add one sentence at line 47: "Components and optional server peers (Phase 5) add a further 8 to 16 weeks; the full roadmap spans 29 to 47 weeks in the same unit."

## Refuted findings

47 candidate findings were dropped because two or more verifiers refuted them.
They are recorded because a review that hides what it rejected cannot be audited, and
because several of them are claims a future reader is likely to raise again. The quoted
reasoning is the first verifier opinion on record for each, which may itself be an uphold
vote that was outvoted.

### R1

Candidate finding: "The repository assessment was performed on 2026-07-17 and 2026-07-18 from Jeliya HEAD `4d4621c929e6f9678b31b7e4a3ee1c8d751b545b` on branch `feat/69-fleet-attention-projection`."

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: the finding SURVIVES. Every command in the evidence reproduces byte-for-byte, and both doc citations say what the finding claims.

VERIFIED EVIDENCE (all re-run independently):
- Doc lines 52-54 confirmed verbatim: "The repository assessment was performed on 2026-07-17 and 2026-07-18 from / Jeliya HEAD `4d4621c929e6f9678b31b7e4a3ee1c8d751b545b` on branch / `feat/69-fleet-attention-projection`."
- `git branch -a --contains 4d4621c9` -> empty. `git merge-base --is-ancestor 4d4621c9 HEAD` -> NO. `git ls-remote origin | grep -i "4d4621c9\|fleet-attention"` -> no match. Full `git ls-remote --heads origin` contains main@7248fb0, feat/72-web-landmarks, feat/73-flutter-a11y, feat/74-react-i18n, feat/75-design-conformance, test/76-a11y-matrix, dependabot/* -- and NO refs/heads/feat/69-fleet-attention-projection. Exactly as quoted.
- Squash 77501d7e8870069247ad29ff6504f54a53499949 "feat: Agent Fleet - attention-first, truthful redesign across React + Flutter (#69)" dated 2026-07-17T22:45:01-04:00 -- exact match, and it IS an ancestor of HEAD.
- `git diff --shortstat 0277577f 4d4621c9` -> "24 files changed, 1534 insertions(+), 58 deletions(-)"; `git diff --shortstat 3059d51 77501d7` -> "24 files changed, 1533 insertions(+), 225 deletions(-)". Both identical to the quoted output.
- docs/verification-evidence.md:17-19 does read "A result is transferable to a release candidate only when it binds / the exact public Jeliya commit, public immutable dependency revisions, / environment, timestamps..." CONFIRMED.
- docs/PROFILE.md:206-207 does read "Status reports must identify the / tested Git commit, dependency revisions, environment, timestamp, assertions..." CONFIRMED (working tree; PROFILE.md is modified but the cite is correct for what a reviewer reads).

CHECKS THE INVESTIGATOR DID NOT STATE, which I added: 0277577f is genuinely `git merge-base main 4d4621c9` (a correct branch-point base, not arbitrary). Unreachability is total: 37 local refs (7 heads / 20 remotes / 10 tags), origin/main current at 7248fb0, `git for-each-ref --contains 4d4621c9` returns 0 refs, and 4d4621c9 does not appear in `git log --all`. It survives only as a dangling object on this machine, so the "fresh clone cannot run git show 4d4621c9" consequence is real.

TWO CORRECTIONS THAT LOWER SEVERITY (but do not refute):

1. Classification "WRONG" is miscalibrated. The doc asserts the assessment WAS PERFORMED from that HEAD/branch. That is almost certainly TRUE as historical fact: the commit exists, the branch existed and was deleted on merge (orchestrator ground truth), and I re-derived that the doc's own "14 commits and 142 changed files" figure is EXACTLY right at 4d4621c9 (14 commits; "142 files changed, 12331 insertions(+), 2679 deletions(-)"), corroborating that the assessment really happened there. Nothing stated is false. The actual defect is that the anchor is UNPUBLISHED and therefore third-party-unreproducible. Per the review rules this belongs in the UNVERIFIABLE/provenance bucket, not WRONG -- the finding blurs exactly the distinction the rules require be kept separate.

2. The proposed fix would introduce a NEW inaccuracy. "Contents are substantively identical" holds for the delta (24 files each way) but NOT for the tree: `git diff --shortstat 4d4621c9 77501d7` -> "38 files changed, 4599 insertions(+), 667 deletions(-)". Main's order is 0277577f (#95) -> 2ee52df (#63) -> 3059d51 (#64) -> 77501d7 (#69), so the branch forked at 0277577f and its work was replayed atop two PRs it never contained. 77501d7 is the assessed CHANGE on a newer base, not the assessed TREE. The doc metric becomes 13 commits / 155 files there (not 14/142) -- the fix anticipates that -- but it does NOT address that the test results at doc lines 74-85 (87 Vitest, 71 Rust) were run against 4d4621c9's tree, so blanket re-anchoring would attribute them to a tree never tested. The honest repair is to keep 4d4621c9 as the tested SHA while disclosing it is unreachable, and cross-reference 77501d7 as where the work landed.

SEVERITY: high -> medium. Real and worth reporting: the section is titled "Assessment boundary and evidence", its whole function is auditability, and an unfetchable anchor defeats that. But no claim is false, no architectural conclusion changes, the fix is essentially a one-line disclosure, and the verification-evidence.md standard cited is a release-qualification bar being applied to a doc explicitly marked status: proposal / verification_status: partial. Medium, reported as a reproducibility/provenance defect rather than a false claim.
````

### R2

Candidate finding: Frontmatter `timestamp: "2026-07-18T20:29:18Z"` combined with the assessment boundary at lines 52-54 describing a live branch and a current HEAD.

Verification reasoning:

````text
UPHOLD (high): EVIDENCE VERIFIED — I independently reran every cited command in /home/sekou/AGI/jeliya and each output matches the finding exactly. `git log -1 --format='%H %ad %s' --date=iso-strict 77501d7` -> `77501d7e8870069247ad29ff6504f54a53499949 2026-07-17T22:45:01-04:00 feat: Agent Fleet — attention-first, truthful redesign across React + Flutter (#69)` (= 2026-07-18T02:45:01Z). `git log -1 --format='%h %ad' --date=iso` -> `7248fb0 2026-07-18 16:12:24 -0400` (= 2026-07-18T20:12:24Z). `git rev-list --count 77501d7..HEAD` -> `13`. `git log -1 4d4621c9` -> `2026-07-17T16:41:17-04:00` (= 2026-07-17T20:41:17Z). `date -u` -> 2026-07-18 22:43:21 UTC. Doc line 6 reads `timestamp: "2026-07-18T20:29:18Z"` (confirmed by Read of /home/sekou/AGI/jeliya/docs/production-deployment.md). Arithmetic confirmed: doc stamp is 17h44m17s after the squash-merge and 16m54s after the current HEAD commit (finding says "17 minutes" — rounding, immaterial).

CORROBORATION I ADDED (not run by the investigator, all strengthening): (1) `git branch -a --list '*69-fleet-attention*'` returns empty — the branch named at doc line 53 genuinely does not exist, so the anchor is unreproducible by a reader. (2) `git merge-base --is-ancestor 4d4621c9 HEAD` -> NOT an ancestor, and `git log -1 --format='%P' 77501d7` -> single parent `3059d51` — confirms squash-merge topology, consistent with the repo's LINEAR-HISTORY/squash policy. (3) `git rev-list --count 55024a4..4d4621c9` -> 14 and `git diff --shortstat 55024a4 4d4621c9` -> `142 files changed` — the doc's line 62 numbers are literally CORRECT at its stated boundary (26 / 231 at HEAD), so the defect is currency, not falsity, exactly as the finding states. (4) grep of all 1087 lines for `stale|supersed|as of|current|merged` confirms NO self-currency caveat anywhere; the two "stale" mentions at lines 67 and 70 refer to OTHER pages (capability-status.md, security-threat-model.md), not to this page's own boundary. So the proposed remedy addresses a real gap.

THREE CORRECTIONS, none refuting: (a) The claim summary says lines 52-54 describe "a live branch and a current HEAD". This OVERSTATES the text. Lines 52-54 are past tense: "The repository assessment was performed on 2026-07-17 and 2026-07-18 from Jeliya HEAD `4d4621c9...` on branch `feat/69-fleet-attention-projection`." The doc never calls the branch live or the HEAD current. The only present-tense exposure is line 62 ("The assessed HEAD **is** 14 commits and 142 changed files after that Jeliya commit"), and that sentence is TRUE of the assessed tree. (b) The classification INTERNALLY-INCONSISTENT is mislabeled — the finding's own body concedes "The dates themselves are otherwise coherent... The defect is the boundary text, not the clock." Nothing in the doc contradicts anything else in the doc; this is a STALE / UNREPRODUCIBLE-ANCHOR defect. (c) The proposed fix says "the assessed branch was squash-merged to `main` as `77501d7`" — supported (same PR #69, single-parent squash, branch deleted, 4d4621c9 not an ancestor) but incomplete: `git diff --shortstat 4d4621c9 77501d7` -> `38 files changed, 4599 insertions(+), 667 deletions(-)`, so 4d4621c9 was NOT the branch tip at merge time; the branch advanced (Flutter work) before being squashed. The fix sentence should say so.

SEVERITY: downgrade high -> medium. Against high: this is an uncommitted document with frontmatter `status: "proposal"` and `verification_status: "partial"` (lines 7-9); every number it states is literally accurate at its declared boundary; the boundary prose is past tense and therefore not a false assertion; there is no security, correctness, or plan defect. Against dropping or low: the anchor is genuinely unreproducible (branch deleted), there is no currency caveat anywhere in 1087 lines, and — materially — the understatement directly undercuts the doc's OWN central argument at lines 62-63 ("Evidence bound to the earlier commit does not qualify the later tree"), making the qualification gap appear roughly half its true size (14 commits/142 files vs 26/231 at HEAD) in a document whose stated purpose is gating a production deployment on evidence currency. That is a real substantive weakening of the doc's thesis, which is why it clears low. Medium.
````

### R3

Candidate finding: The doc names only capability status and the security threat model as carrying stale contradictions, while citing Platform matrix as authority at line 156 and requiring "platform documentation" reconciliation at line 873.

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: every cited file:line was read directly and every quoted string is verbatim correct.

VERIFIED EVIDENCE:
- production-deployment.md:67-72 names only capability status and the security threat model. Confirmed; `grep -n -i "stale\|contradict\|reconcil" docs/production-deployment.md` returns only 67, 70, 72, 873, 883, and `grep -n -i "platform matrix\|platform-matrix\|platform documentation"` returns only 156 and 873. The doc never flags platform-matrix.md as contradictory.
- production-deployment.md:156: "scaffold. See [Platform matrix](platform-matrix.md)." — confirmed as the authority cite for the Android/iOS classification (154-156).
- production-deployment.md:873: "- reconcile status, threat, evidence, and platform documentation;" — confirmed verbatim.
- platform-matrix.md:17-19 header quote — verbatim correct.
- platform-matrix.md:27 and :29 both end "certified for `v0.5.0` and re-certified at the `v0.6.0` candidate" — verbatim correct, both rows.
- platform-matrix.md:72-73 "The certifying runs qualify `v0.5.0` exactly; they do not transfer to the rc.3 candidate, whose pin differs." — verbatim correct.
- security-threat-model.md:122-124 — exact range; and Cargo.lock:2015 shows `source = "git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef4ce83631c94762ec68c2beef3d79"` with version 0.1.0-rc.3, so "the public Jeliya lockfile does not yet resolve that code" is genuinely stale.

THE CONTRADICTION IS REAL. I tested the strongest counter-reading: platform-matrix.md:36-39 already correctly scopes "The `v0.5.0` manifests ... do not transfer to the rc.3 pin", so :72 could be read as shorthand for the v0.5.0 runs. That defense fails: :72 drops the scoping, adds the exclusive quantifier "exactly", and immediately follows row :66, which itself labels the runs at `55024a4…` + `71fbb500…` (the candidate pair) as certifying. So :72 negates the header's "it is qualified" (:19), rows :27/:29, and row :66. This is structurally identical to the capability-status.md defect the doc does describe (top :19-21/:29-32/:53-54 assert candidate certification while :50 says "has no network run yet" and :56 says "a fresh signed schema 2 qualification is required"), so the doc's own claim at 67-70 is ACCURATE and platform-matrix.md is the same class of defect, unflagged.

WHAT DOES NOT HOLD: the consequence clause. The finding asserts that omitting platform-matrix "understates the Phase 0 scope." It does not — line 873 already lists "platform documentation" among the Phase 0 reconciliation deliverables, so platform docs are in scope regardless of whether the summary names the specific contradiction. Combined with the hedge "Some status pages contain stale contradictions" (line 67), this is a non-exhaustive exemplar list in an executive-summary bullet, not a scope gap. No downstream gate or decision in the proposal changes if it is added.

Also noting the proposed fix's second sentence (the threat-model lockfile statement) merely restates what production-deployment.md:70-72 already says, so it adds nothing.

Net: evidence is real and correctly read, so the finding survives the accuracy lens and should appear in the review, but the asserted medium severity rests on the one claim that fails. Downgrade to low: a genuine but cosmetic completeness gap in a hedged summary bullet.
````

### R4

Candidate finding: "It creates one per-process bearer token and writes that token to the local owner-only portfile."

Verification reasoning:

````text
UPHOLD (high): Independently verified every cited artifact; all are real and correctly read. (1) Doc lines 174-175 read verbatim "It creates one per-process bearer token and writes that token to the local / owner-only portfile." (2) crates/jeliyad/src/lifecycle.rs write_portfile spans 72-91; the #[cfg(unix)] block is lines 79-83 with opts.mode(0o600) at line 82 — the permission call is genuinely Unix-gated as claimed. (3) I ran grep -rnE "set_permissions|PermissionsExt|0o7|0o6" crates/jeliyad/src/ and got exactly one hit: crates/jeliyad/src/lifecycle.rs:82. (4) The source comment quote is verbatim ("Contains the auth token, so it is created user-readable only (0600) on Unix; on Windows the per-user data dir ACLs carry the same intent.") though its actual span is lines 23-26, not the cited 24-27 — a trivial off-by-one that does not affect the substance. (5) docs/PROTOCOL.md line 59 is verbatim "Written atomically after bind, removed on graceful shutdown, `0600` on Unix." (6) Doc line 918 is verbatim "- signed macOS and Windows packages and a verified Linux package;". (7) generate_token at lifecycle.rs:64-68 is [0u8; 32] + getrandom::fill + hex::encode, matching the proposed fix's "256-bit, OS CSPRNG, hex-encoded".

Two independent checks I added, both supporting the finding: a broader grep -rniE "windows|acl|security_attributes|winapi|windows-sys" over all of crates/ --include=*.rs finds no Windows ACL code anywhere in the workspace (only the lifecycle.rs:26 comment itself and an unrelated MOVEFILE_REPLACE_EXISTING comment at crates/jeliya-core/src/localstate.rs:111); and grep -niE "0600|acl|owner-only|portfile" docs/production-deployment.md returns line 175 alone, so the doc states the property exactly once with no platform qualifier anywhere else and no place for a reader to recover the nuance.

One fact that strengthens the finding beyond what the investigator cited: crates/jeliyad/src/main.rs:60 declares data_dir as Option<PathBuf> (a user-supplied flag), main.rs:116 resolves it via unwrap_or_else(default_data_dir), and main.rs:117 calls create_dir_all with no mode or ACL. So the data dir is user-relocatable and its creation sets no permissions either — meaning the source comment's "on Windows the per-user data dir ACLs carry the same intent" is itself conditional on the default path. The Windows gap is concrete rather than theoretical.

Nothing to refute under the accuracy lens. The MISSING (not WRONG) classification is correct: "owner-only" is true on Unix, and the defect is the dropped platform qualifier that both the source and PROTOCOL.md explicitly carry. Severity medium is slightly generous — the sentence sits in a list arguing the daemon must not be publicly exposed, and the Windows gap strengthens rather than undermines that conclusion, so no recommendation in the doc turns on it — but Windows is an explicit Phase 2 launch platform (line 918), both sources of truth carry the qualifier, and the relocatable data dir makes the exposure real, so medium stands. The only correction to the proposed fix is the source-comment line range: 23-26, not 24-27.
````

### R5

Candidate finding: "Preserve the ability to move to self-hosted relays through configuration and infrastructure-as-code." — presented as a like-for-like fallback for the credential design in lines 539-543.

Verification reasoning:

````text
REFUTE (high): The quoted passages are genuine — I fetched both URLs and every attributed sentence appears verbatim in the iroh-relay README and docs.iroh.computer/concepts/relays. But the evidence was read selectively and does not support the conclusion drawn.

The investigator quoted the README's section "Shared token (local, no external service)" and generalized its limitations to ALL self-hosted access control. The README documents four access modes (raw README lines 46, 52, 62, 86): "everyone", allowlist/denylist by endpoint ID, shared token, and — omitted entirely by the investigator — "### HTTP callout (external auth service)" at line 86. That section reads: "The relay calls an external HTTP endpoint for each incoming connection, passing the connecting endpoint's ID. The token below authenticates the relay to your auth service (machine-to-machine), not the connecting client." with config `access.http.url = "https://your-auth-service.example.com/relay-auth"`.

That mechanism supplies exactly what doc lines 539-543 require: (a) endpoint-bound — the decision is made per connection keyed on the connecting endpoint's ID; (b) no secret in static assets — the README states the bearer token authenticates the relay to the auth service, "not the connecting client", so nothing reaches the browser, dissolving the "shared secret must never reach a browser" objection; (c) revocation without restart — the auth service simply answers differently on the next callout.

The revocation caveat was also unscoped. Line 84 is the ONLY revocation mention in the entire README (verified by grep): "**Note:** this shared token does not support revocation other than updating the config and restarting the service." It is scoped to the shared-token mode, not to self-hosting generally.

The proposed fix would insert a false statement into the doc: "the self-hosted path requires building our own token-issuing proxy or an upstream contribution before it can carry browser traffic." The capability exists upstream today via one TOML field, and the doc already provisions relay-auth.jeliya.ai at lines 555-556, so no new component is needed. Doc line 544-545's actual wording — "through configuration and infrastructure-as-code" — is literally accurate for the callout mechanism, which is a config field.

One narrow real delta survives but is not this finding: managed relays claim revocation reaches "connections that are already open," whereas the callout fires "for each incoming connection," so established sessions may persist past revocation. That is a different, much smaller claim; rewriting the finding into it is replacement, not survival. I did NOT verify whether the relay cryptographically proves possession of the key behind the claimed endpoint ID (doc line 540's "proof of possession") — marked UNVERIFIABLE — but this does not rescue the finding, whose argument rested entirely on static shared secrets being the only self-hosted option.

Given the doc already frames the provider choice as reversible with a Phase 0 gate (lines 565-568), acting on this finding would actively mislead a reader into believing self-hosting requires upstream work it does not require. Drop.
````

### R6

Candidate finding: "A browser obtains a short-lived, endpoint-bound relay credential from `relay-auth.jeliya.ai` after proof of possession. The project API secret never enters static assets." Flagged only as assumption #3 (line 1040) and risk #2 (line 1062).

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: the finding's evidence is real and correctly read. I fetched both cited URLs myself and confirmed every quoted passage.

VERIFIED VERBATIM on https://docs.iroh.computer/add-a-relay: "Your dedicated relays require authentication by default. Your endpoint authenticates to them with your project's API key."; the preset builder "mints a short-lived access token scoped to your endpoint's key and configures the endpoint to use your relays"; "In production, load the key from a config file or environment variable instead of hardcoding it."

VERIFIED VERBATIM on https://docs.iroh.computer/concepts/relays: "When you build an endpoint with the iroh_services::preset() builder and your API key, the SDK mints a short-lived, signed access token scoped to that endpoint's identity"; "Your API key never leaves your application." The phrase "proof of possession" does NOT appear — confirming it is a Jeliya-side design addition, as the finding states.

The inferential step also holds: on both pages every documented mint is in-process by the SDK holding the API key, and neither documents an API for a separate server to mint a token scoped to a different, untrusted endpoint. I extended the check beyond the investigator's two pages: https://docs.iroh.computer/languages/wasm-browser says nothing whatsoever about relay auth, API keys, or credentials; https://www.iroh.computer/services/hosting says only "Dedicated relays are authenticated by default. Only your project's endpoints can connect, using your API key." So the universal negative survives a broader search than the investigator performed.

ONE MINOR EVIDENCE IMPRECISION (not disqualifying): the finding quotes /concepts/relays as "Your endpoint presents a token, not the key itself." The live page renders this as "The endpoint presents this token when it connects to the relay." Same substance, and the load-bearing sentence ("Your API key never leaves your application") is verbatim correct. This is wording variance, not a misreading.

WHAT THE INVESTIGATOR MISSED (does not refute; does affect severity): self-hosted iroh-relay DOES document an external-auth mode — "The relay calls an external HTTP endpoint for each incoming connection, passing the connecting endpoint's ID," with the relay authenticating to your service via access.http.bearer_token / IROH_RELAY_HTTP_BEARER_TOKEN. There is also RelayConfig::with_auth_token plus shared-secret Bearer/?token= admission. This is a genuine documented route to admit browser endpoints without shipping a secret in static assets. It does NOT rescue lines 539-541, because it is authorization-by-callout rather than issuance — the relay "does not mint or issue tokens to connecting clients" — and it applies to self-hosted relays, not the managed dedicated relays committed at line 557. So the finding's core claim (delegated issuance of an endpoint-bound token by relay-auth.jeliya.ai, after proof of possession, is undocumented) stands.

SEVERITY DOWNGRADE high -> medium. The doc hedges this substantially more than the finding credits: line 1041 places the fallback inside the assumption itself ("or an equivalent self-hosted design is selected"); line 1062 lists it as highest-risk unknown #2; and lines 565-568 already impose a Phase 0 gate ("If provider-specific relay authentication or identity requirements cannot satisfy the threat model, Phase 0 must choose an equivalent static CDN, edge token service, and dedicated relay deployment before implementation starts") — which is most of the investigator's proposed fix, already present. Combined with a documented self-hosted fallback, this is not a project-threatening gap. The residual real defect is narrower and worth reporting: lines 539-541 and 555-556 state the mechanism in the declarative present as settled architecture and committed infrastructure, and line 787 lists "short-lived endpoint-bound relay tokens" as a live abuse control, while the assumptions section treats the same mechanism as unproven. That tone mismatch, plus the fact that "proof of possession" is a Jeliya invention with no Iroh counterpart, justifies medium.

RECOMMENDED FIX ADJUSTMENT: keep the reword of line 1040 separating "endpoint-bound short-lived tokens exist" from "third-party issuance is undocumented," but drop the demand for a new Phase 0/1 gate (lines 565-568 already have one) and add the concrete fallback the investigator omitted: self-hosted iroh-relay's HTTP-callout admission, which achieves secret-free browser admission by a different mechanism than the one lines 539-541 describe.
````

### R7

Candidate finding: "$389 per 30-day month before bandwidth or SLA charges" — i.e. bandwidth is excluded from the $0.27/hr rate and metered at a "provider egress rate" (line 822).

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: the stated evidence is real and correctly read on every point. Finding SURVIVES.

DOC LINES CONFIRMED (read /home/sekou/AGI/jeliya/docs/production-deployment.md:798-828):
- L800-802: "Current Iroh managed relay pricing starts at $0.27 per hour. Two continuously running relays therefore start near $389 per 30-day month before bandwidth or SLA charges."
- L807: "| Two managed Iroh relays | Approximately $389 before bandwidth/SLA |"
- L811: "| Initial fixed total | Approximately $400 to $600 plus relay bandwidth |"
- L822: "  + relayed GiB * provider egress rate"
The doc does structurally assert bandwidth is excluded from the hourly rate and metered at a per-GiB "provider egress rate", so the investigator characterized the claim correctly.

URL 1 — https://www.iroh.computer/services/hosting (fetched independently): Cloud tier is "$0.27/hour and up". Its feature bullets are "Authenticated by default...", "Multi-region or multi-cloud deployment", "Fully managed infrastructure", "Negotiated bandwidth", "Client version locking & diagnostics", "Uptime SLAs available". The investigator's quote of "Negotiated bandwidth" and its adjacency to "Fully managed infrastructure" and "Uptime SLAs available" is EXACT. The page does not state whether bandwidth is included in or billed separately from the hourly rate — confirmed.

URL 2 — https://www.iroh.computer/pricing (fetched twice, second time with a targeted billing-dimension prompt): tiers Free $0/month, Pro $19/month, Enterprise "Contact Us"; relay rate "$0.27/relay/hour". First fetch: "The page contains no mentions of bandwidth, egress, data transfer, GB, GiB, or TB." The investigator's "lists no bandwidth/egress line at all" is EXACT.

ADVERSARIAL PROBES THAT FAILED TO REFUTE:
1. I tried to find a published per-GB rate elsewhere (two web searches for Iroh/n0 egress pricing). None exists; results only pointed back to /pricing and to unrelated cloud-egress comparison sites.
2. I enumerated the pricing page's actual metered dimensions to see whether egress is quietly billed: they are exactly three — "$0.27/relay/hour", "$0.50/100 endpoints" (concurrent endpoints), "$1.49/1K DPM" (metrics). Bandwidth is affirmatively NOT a metered dimension. This POSITIVELY STRENGTHENS the finding: line 822's egress term cannot be populated from public pricing, exactly as claimed.
3. I checked whether the doc's directional assumption is outright WRONG rather than unverifiable. It is not. "Negotiated bandwidth" leans toward bandwidth being a separately negotiated commercial term, which is closer to supporting the doc than refuting it. UNVERIFIABLE is therefore the correct classification and the investigator did not overclaim it as WRONG — that restraint is right.
4. Arithmetic sanity on the $389: 2 x $0.27 x 720h = $388.80, so the doc's own math is internally consistent. The finding does not rest on an arithmetic error, and I found none.

SEVERITY: medium is correct. Not lower, because the doc itself says at L815 "Browser peers are always relayed, so file traffic can dominate cost" — the doc identifies bandwidth as the potentially DOMINANT cost, and that is precisely the term with no public rate, leaving L811's "$400 to $600 plus relay bandwidth" unbounded on its largest variable. Not higher, because the doc is honest that bandwidth is excluded rather than hiding it; the remedy is a rewording plus an open-commercial-question flag, not a structural correction. The proposed fix is appropriate and I would adopt it as written.

ADJACENT OBSERVATION (out of scope for this finding, do not merge into it): my dimension enumeration also showed the pricing page's own example totals one relay at "$197.10/mo" (730-hour month) plus "400 extra x $0.5/100" = "$2.00/mo" connections, "Estimated total $218.10/mo". That implies the doc's $389 also omits the concurrent-endpoint and metrics-DPM meters and the Pro $19/month base, and uses 720h where Iroh uses 730h. That is a separate completeness issue about the $389 figure, not evidence for or against this bandwidth finding.
````

### R8

Candidate finding: Phase 4 go/no-go gate: "browser and native peers produce byte-compatible signatures and membership folds" — offered as the test that discharges the "exact wire interoperability" hedge at line 316-317.

Verification reasoning:

````text
REFUTE (high): REFUTED on the accuracy lens: the load-bearing external evidence does not say what the finding claims it says.

CENTRAL REFUTATION. The finding asserts: "the WebCrypto spec text only mandates the A check. A signature with small-order R is therefore acceptable to a spec-conforming browser verifier and rejected by the native verifier." I downloaded https://wicg.github.io/webcrypto-secure-curves/ (281625 bytes) and stripped tags. The Ed25519 Verify algorithm contains TWO small-order steps. The investigator quoted the first and stopped one normative step short of the second. Literal text, consecutive steps in the same algorithm:

  "If the key data of key represents an invalid point or a small-order element on the Elliptic Curve of Ed25519, return false."
    Issue 27: Make small-order checks in EdDSA optional? Not all implementations perform this check.
  "If the point R, encoded in the first half of signature, represents an invalid point or a small-order element on the Elliptic Curve of Ed25519, return false."
    Issue 27: Make small-order checks in EdDSA optional? Not all implementations perform this check.
  "Perform the Ed25519 verification steps, as specified in [RFC8032], Section 5.1.7, using the cofactorless (unbatched) equation, [S]B = R + [k]A' ..."

(The identical pair appears in the Ed448 section, confirming it is deliberate spec structure, not an artifact.) So the spec mandates the R small-order check with its own dedicated step. The one concrete divergence the finding offers cannot occur against a spec-conforming implementation. Spec-conforming WebCrypto verify = cofactorless + reject small-order A + reject small-order R, which is exactly verify_strict's semantics. The claimed "fork in the membership fold" evaporates, and the asserted "high" severity rested entirely on it.

SECONDARY REFUTATION. "The gate tests signing, which cannot fail" is also wrong. /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/event/signed.rs:251 defines the signing message as EVENT_CONTEXT || signed (canonical signed bytes): `pub fn event_signing_message(signed: &[u8]) -> Vec<u8>`. A browser reimplementation must reproduce the domain-separation context AND the canonical serialization byte-exactly. Determinism of Ed25519 makes the primitive reproducible, but NOT the message construction. Byte-compatible-signature comparison is precisely the test that catches CSB/context divergence, likely the most probable real browser/native mismatch. The gate is the opposite of vacuous.

TWO FURTHER OVERSTATEMENTS. (a) "omits verification" — the gate at line 968 also requires byte-compatible "membership folds", a verification-dependent property: if the two peer classes disagreed on event validity the folds would diverge. (b) The doc is not silent on conformance: line 962 "browser/native protocol conformance" is a Phase 4 deliverable and line 656 lists "protocol conformance, fuzzing, and malformed-frame tests".

WHAT DID CHECK OUT (so coverage is legible): doc line 968 gate text and 316-317 hedge text quoted correctly; keys.rs:153 `vk.verify_strict(message, &signature.to_dalek())` verbatim correct at pinned rev 71fbb50, as is its doc comment "Uses strict verification (rejects non-canonical signatures and small-order points). This is the **only** event-signature verification entry point"; verify_strict genuinely rejects small-order R and A per /home/sekou/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ed25519-dalek-3.0.0-rc.1/src/verifying.rs:372 `if signature_R.is_small_order() || self.point.is_small_order()`; RFC 8032 §8.2 determinism and §5.1.7 "sufficient, but not required" quotes are real. Minor citation slip: "signed-event validation on boot" is doc line 434, not 435.

RESIDUAL KERNEL (why 'drop' rather than 'low'). One true observation survives: Issue 27 flags that real browsers may not implement the small-order checks the spec mandates, so a conformance corpus is defensible engineering. But that is a standards-uncertainty argument, not the spec-text-gap argument the finding makes, it is already largely covered by lines 962 and 656, and it would rate low at best. Publishing the finding as written would put a false claim about a public spec into the review, and its fix rationale ("keep WebCrypto for signing only - that direction is genuinely safe") is itself misleading given the CSB canonicalization risk. It should be dropped and, if desired, re-filed from scratch with correct grounding at low severity.
````

### R9

Candidate finding: "Otherwise wrap the seed with a nonextractable WebCrypto key and load it into Wasm only while active", supported by citation "MDN: SubtleCrypto unwrapKey — Wrapped and nonextractable browser-key behavior."

Verification reasoning:

````text
REFUTE (high): REFUTED under the accuracy lens: the evidence is real but misread, and the misreading is contradicted by the very page cited.

1) The finding's central assertion — "unwrapKey() cannot perform the operation the doc describes" — is false per MDN's own unwrapKey page. I fetched https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/unwrapKey and extracted the raw page text. The signature is: unwrapKey(format, wrappedKey, unwrappingKey, unwrapAlgo, unwrappedKeyAlgo, extractable, keyUsages). The page documents the parameter verbatim: "extractable — A boolean indicating whether it will be possible to export the key using SubtleCrypto.exportKey() or SubtleCrypto.wrapKey()." The prose also states: "you specify the key's import format and other attributes of the key to import details such as whether it is extractable, and which operations it can be used for."

The investigator's own supporting sentence ("raw bytes are only reachable via exportKey() and only if the key was created extractable") is true, but is exactly the reason unwrapKey CAN do this: extractable is a caller-supplied argument. unwrapKey(..., extractable=true, ...) followed by exportKey("raw", key) yields the raw seed bytes, which can then be loaded into Wasm linear memory. The investigator quoted the Return value section correctly ("A Promise that fulfills with the unwrapped key as a CryptoKey object" — verified verbatim) but omitted the extractable parameter sitting in the same signature they were quoting from. That omission is the whole finding.

2) The finding attacks a mechanism the doc never specifies. `grep -n -i "unwrapkey|wrapkey|subtlecrypto|nonextractable|webcrypto" docs/production-deployment.md` yields hits at lines 207, 316, 318, 1053, 1084, 1085 — the ONLY occurrence of "unwrapKey" is line 1085, the citation entry itself. Doc lines 317-318 read "Otherwise wrap the seed with a nonextractable WebCrypto key and load it into Wasm only while active" — using "wrap" in the ordinary descriptive sense, consistent with line 207 ("WebCrypto plus IndexedDB wrapping") and line 1053 ("nonextractable WebCrypto signer or wrapped Wasm"). The doc names no API call at all. The investigator inferred unwrapKey() as the prescribed mechanism and then refuted the inference. The citation label at 1085 is "Wrapped and nonextractable browser-key behavior" — a topical pointer, and the unwrapKey page does genuinely document both wrapped-key handling and the extractable/nonextractable attribute. The citation is defensible as attached.

3) The proposed fix would reduce doc quality: rewriting 317-318 to mandate a specific AES-GCM + decrypt() construction injects implementation specificity into a deliberately architecture-level section and would foreclose a valid wrap/unwrap implementation.

Note for completeness: a real (unraised) wrinkle exists on the wrapping side — wrapKey() requires the key being wrapped to have extractable=true ("To export a key, it must have CryptoKey.extractable set to true", per the wrapKey MDN page I also fetched), and raw format is not supported for Ed25519 private keys, so one would import the 32-byte seed as an oct JWK or AES-GCM key to wrap it. That is a practicality wrinkle in any wrap-based scheme, not an error in the doc's sentence, and it is not the argument the finding makes.

Separately verified as ACCURATE and unaffected: doc lines 319-320 ("Treat browser key protection as at-rest defense. A malicious same-origin script can still invoke a usable key and may observe active memory") — this is the correct security framing for nonextractable WebCrypto keys.

Severity should be 'drop': the claim is not WRONG, the citation is not misattached in any way a reader would be misled by, and including this would push a change that makes the document less correct.
````

### R10

Candidate finding: Line 316-317 recommends "a nonextractable WebCrypto Ed25519 key" for the browser identity, while line 343-344 requires the recovery flow to "Export a versioned authenticated-encryption bundle containing the profile root".

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: every piece of stated evidence is real and correctly read. I verified each independently.

DOC ANCHORS — ALL CONFIRMED VERBATIM (/home/sekou/AGI/jeliya/docs/production-deployment.md):
- 316-318: "3. In the browser, prefer a nonextractable WebCrypto Ed25519 key when browser / compatibility and exact wire interoperability pass. Otherwise wrap the seed / with a nonextractable WebCrypto key and load it into Wasm only while active." Quoted accurately, including the two-option structure.
- 343-344: "Export a versioned authenticated-encryption bundle containing the profile / root, room membership index, device authorization state, and relay config." Accurate.
- 971: "clearing storage triggers recovery and never silent identity replacement" — verbatim, and it is indeed a Phase 4 go/no-go gate.
- 1053-1054: "6. Browser signing strategy: nonextractable WebCrypto signer or wrapped Wasm / seed." Accurate.
- 353: root-signed `device.authorized` and `device.revoked` — accurate.

EXTERNAL SOURCE — CONFIRMED. Fetched https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/unwrapKey; it contains the quoted string verbatim: "A boolean indicating whether it will be possible to export the key using SubtleCrypto.exportKey() or SubtleCrypto.wrapKey()." Notably this is the doc's OWN cited reference (line 1085: "MDN: SubtleCrypto unwrapKey - Wrapped and nonextractable browser-key behavior"), so the finding is holding the doc to its own source.

THE ONE INFERENTIAL STEP, INDEPENDENTLY CLOSED. The unwrapKey page states only the positive form; it does not literally state the inverse. I therefore verified the inverse at https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/exportKey: "To export a key, the key must have CryptoKey.extractable set to true" and "InvalidAccessError DOMException — Raised when trying to export a non-extractable key." So "nonextractable is definitionally unexportable" is correct normative behavior, not hand-waving.

INDEPENDENT EVIDENCE STRENGTHENING THE FINDING (not cited by the investigator):
- Line 311 introduces the numbered list as covering BOTH keys: "Generate a long-lived profile/root key and a per-device endpoint/event key locally." Item 3 (browser) is unscoped — it does not restrict itself to the device key — while item 2 (native) says "wrap secrets" (plural). So the recommendation does reach the profile root.
- Phase 4, "browser peer and multi-device identity" (954-962), lists as browser deliverables both "root-signed device authorization and revocation" (960) and "browser recovery and eviction handling" (961). This closes the main escape hatch a defender would use — the doc does contemplate the browser holding root authority and performing recovery.

REFUTATION ATTEMPTS I MADE AND REJECTED:
(a) "Item 3 means only the event/device signing key." Rejected on lines 311 and 960 above.
(b) STRONGEST COUNTER: nonextractable is compatible with recovery if the seed is generated in Wasm, exported once at setup (line 347 "Require a successful test restore before setup is called complete" implies a setup-time bundle), then imported with extractable:false. This does soften the investigator's absolutism ("no recovery bundle ... can ever be produced"). But under that reading option 1 collapses into option 2 — the seed transits the Wasm/JS heap either way — which is precisely the distinction the doc draws at 316-318 and at OQ6 ("nonextractable WebCrypto signer OR wrapped Wasm seed"). Under the only reading where option 1 is materially distinct (generateKey with extractable:false, seed never existing outside WebCrypto), the conflict is total. Verdict unchanged.
(c) "OQ6 already flags this as open." Only partially mitigating: the selection criteria the doc actually states at 316-317 are "browser compatibility and exact wire interoperability" — recoverability appears in neither 316-317 nor 1053. The doc still asserts a preference ordering ("prefer X ... Otherwise Y") on criteria that omit the one constraint that disqualifies the preferred option. That is the inconsistency, and it survives.

The classification INTERNALLY-INCONSISTENT is correct, and the proposed fix (scope nonextractable to per-device event keys, which are replaceable via the 353 machinery; require the wrapped-seed form for the profile root; resolve OQ6 on that basis) follows from the evidence.

ONE CALIBRATION NOTE FOR THE PARENT: the phrase "no recovery bundle containing that root can ever be produced" is strictly true only for the generateKey-with-extractable:false form, not for an import-then-discard form. Worth softening in the write-up; it does not change the verdict.

SEVERITY: medium is correct. Not high — OQ6 (1053) means the choice is not yet locked in and the fix is a scoping clarification. Not low — if the stated preference is followed literally, a hard Phase 4 go/no-go gate (971) becomes unreachable and browser-first users lose their identity irrecoverably on a storage clear.
````

### R11

Candidate finding: Phase 4 support matrix: "the latest two Chrome, Edge, Firefox, and Safari releases plus current iOS Safari and Android Chrome pass the supported matrix".

Verification reasoning:

````text
REFUTE (high): ACCURACY lens: the cited evidence is real, and most of it was transcribed correctly, but the single load-bearing cell was misread and the quantitative inference is wrong by an order of magnitude.

WHAT CHECKS OUT (verified by fetching https://caniuse.com/mdn-api_subtlecrypto_sign_ed25519):
- "Global usage for the feature is 82.88%" — exact match.
- "Chrome 137+, Edge 137+, Firefox 129+, Safari 17.0+, Safari on iOS 17.0+, Chrome for Android 150" — every figure matches the rendered page verbatim.
- Doc lines 966-967 are quoted accurately: "the latest two Chrome, Edge, Firefox, and Safari releases plus current iOS Safari and Android Chrome pass the supported matrix". Doc lines 317-318 are also quoted accurately (the wrapped-seed Wasm fallback).
So the investigator did visit the page and did not fabricate numbers.

WHY IT STILL FAILS — the Samsung Internet cell is an artifact, not an affirmative negative:
1. MDN BCD source (https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/SubtleCrypto.json) records "samsunginternet_android": "mirror" for sign Ed25519. "mirror" is a DERIVED value computed from the Chromium-version release table — it is explicitly not a declared "false". The investigator read a computed/unknown state as a positive statement of non-support.
2. https://raw.githubusercontent.com/mdn/browser-compat-data/main/browsers/samsunginternet_android.json lists 29.0 with engine_version "136", release_date "2025-10-25", status "current" — genuinely one Chromium below the 137 floor, so "not supported" is correct for v4-29. But there is NO 30.0 release record in BCD at all. With no release row, mirroring cannot resolve and caniuse renders the v30 cell as unsupported by default.
3. Samsung's primary source contradicts the v30 reading: developer.samsung.com/internet/release-note/windows-release-note.html states "Samsung Internet for Windows 30.0.0.48 (Feb 26, 2026) - Web engine upgrade (M143)". Chromium 143 is far above the 137 threshold, so Samsung Internet 30 almost certainly DOES support WebCrypto Ed25519. (Caveat: that entry is the Windows line; the Android v30 build is a strong but not airtight inference. It does not matter to the verdict — the investigator's evidence never established non-support for v30 in the first place, since the value is "mirror" with a missing release row.)
4. caniuse's own usage-table shows Samsung Internet 30 at 0.99% of a 1.30% total, i.e. ~76% of Samsung Internet users sit on exactly the version the finding assumes lacks the primitive.
So the parenthetical "NOT supported across v4-30 (its entire shipped range)" is literally what the page renders but substantively wrong, and it is the sole premise for the finding.

THE 17% INFERENCE IS ALSO WRONG: "the ~17% gap is concentrated in exactly the population the doc's matrix does not enumerate" is unsupported and off by an order of magnitude. Per caniuse's usage table Samsung Internet totals 1.30% globally — at most ~7.6% of the 17.12% gap, and realistically ~0.31% (v<=29 only) or under 2% of it. The residual 17% is dominated by older Chrome/Safari/WebView versions, not Samsung handsets.

TWO SECONDARY INACCURACIES: Samsung Internet is a Blink/Chromium browser, not "the one significant mobile engine" — calling it a distinct engine is wrong. And the finding's own note concedes the doc's line 317-318 fallback ("Otherwise wrap the seed with a nonextractable WebCrypto key and load it into Wasm only while active") already covers any browser lacking Ed25519 — a general architectural fallback that would blunt the severity to a gate-enumeration nit even if the Samsung premise had held.

DISPOSITION: drop rather than downgrade. As filed this is "MISSING: Samsung Internet omitted, medium" justified by "Samsung Internet lacks Ed25519" — a premise that is false, and acting on it would push the author to write a Samsung-specific carve-out that the platform facts do not warrant. The one salvageable fragment is the tail of the proposed fix ("record the concrete floors so the gate is testable: Chrome/Edge 137, Firefox 129, Safari/iOS Safari 17.0"), which is a legitimate but separate low-severity editorial nit about the Phase 4 gate at 966-967 being untestable as written; it stands on its own and is independent of any Samsung Internet claim. If the reviewer wants that, it should be re-filed as its own low-severity finding with the Samsung framing removed.
````

### R12

Candidate finding:

````text
The CSP at line 589 (`connect-src 'self' https://relay-auth.jeliya.ai https://<relay-hosts> wss://<relay-hosts>`) is presented as the baseline for app.jeliya.ai, but the current UI's only transport is the loopback daemon, which this policy blocks. The doc never states this as a required migration.
````

Verification reasoning:

````text
REFUTE (high): REFUTED on the accuracy lens, on two independent grounds. Both the code reading and the doc reading are wrong.

GROUND 1 — the cited code evidence is misread; the investigator quoted line 22 without reading lines 32-43.

The finding asserts "the current UI's only transport is the loopback daemon, which this policy blocks," resting on /home/sekou/AGI/jeliya/ui/src/lib/client.ts:22. Line 22 exists verbatim as quoted. But it is the NON-PROD fallback, not the hosted-build transport. client.ts:38-42 (uncited by the investigator):

    if (import.meta.env.PROD && typeof window !== 'undefined' && window.location.host) {
      const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws';
      return `${scheme}://${window.location.host}/ws`;
    }
    return DEFAULT_DAEMON_URL;

The inline comment at client.ts:32-37 states the intent explicitly: "In a production build the daemon serves this SPA from its own loopback origin, so the control socket is same-origin: derive it from the page host (and matching ws/wss scheme)... The Vite dev server serves the UI on a different origin than the daemon, so there we keep the fixed default instead."

So a PROD bundle served from https://app.jeliya.ai resolves daemonUrl() to `wss://app.jeliya.ai/ws` — same-origin, NOT loopback. Per CSP Level 3 §1.3 (Changes from Level 2), quoted from https://www.w3.org/TR/CSP3/: "Likewise, `'self'` now matches `https:` and `wss:` variants of the page's origin, even on pages whose scheme is `http`." Therefore `connect-src 'self'` at doc line 589 ALLOWS that connection rather than blocking it. (MDN's connect-src page carries a caveat — "`connect-src 'self'` does not resolve to websocket schemes in all browsers" — but that is a browser-inconsistency note, not support for the finding, which claims the policy blocks the transport by omission of loopback hosts.)

Same defect for client.ts:73: `fetch(new URL('/api/session', daemonHttpBase()))` exists verbatim, but daemonHttpBase() derives from daemonUrl() (client.ts:46), so under a hosted PROD build it is `https://app.jeliya.ai` — same-origin, allowed by 'self'. client.ts:263 `ws = new WebSocket(url);` exists verbatim but is transport-agnostic and proves nothing about the URL scheme.

The only genuinely CSP-blocked path is client.ts:27 (`?daemon=<port>` → `ws://127.0.0.1:${value}/ws`), an opt-in query-string escape hatch — not "the only transport." A hosted bundle does fail at runtime, but because nothing serves /ws at app.jeliya.ai and doc line 293 forbids proxying jeliyad — a dead endpoint, not a CSP violation. The finding's causal mechanism is wrong.

GROUND 2 — "The doc never states this as a required migration" is factually false; the doc states it more forcefully than the proposed fix does.

- Lines 29-30: "The current React build is **not** a deployable functional web application, and `jeliyad` must **never** be exposed through a public listener or reverse proxy."
- Lines 34-38: numbered target architecture — static PWA, then companion over "a new mutually authenticated, end-to-end-encrypted Iroh control protocol," then "A browser-resident Wasm room peer follows only after browser storage, signing, synchronization, and Iroh Rooms adapters pass independent gates."
- Line 120: "Only the following surfaces can be deployed without architectural work:" — the list includes "existing daemon artifacts for local loopback use" — followed by line 127: "None of those surfaces satisfies the goal of a safe, functional Jeliya product at `app.jeliya.ai`."
- Line 135: "a secure browser-to-companion control protocol" is listed under "What requires architecture changes".
- Line 293: the component table prohibits app.jeliya.ai from "proxy `jeliyad`".

The proposed fix would have the doc add "The hosted build must not ship the loopback daemon transport." The doc already says the entire current React build is not deployable — a strictly stronger statement that subsumes the proposed addition.

CLAIMS I CHECKED AND FOUND ACCURATE (calibration): the CSP block is quoted correctly from doc lines 583-598, and line 589 reads exactly `connect-src 'self' https://relay-auth.jeliya.ai https://<relay-hosts> wss://<relay-hosts>;`. Doc lines 616-617 are quoted correctly ("Do not depend on hosted-page access to loopback addresses; the relevant browser policy is still experimental and platform-dependent."). Line 293's "proxy `jeliyad`" prohibition is quoted correctly. client.ts lines 22, 27, 73, and 263 all exist verbatim at the stated line numbers — the line citations are honest; the interpretation is what fails. I did not independently fetch the MDN Permissions-Policy/loopback-network page, so that sub-claim is UNVERIFIABLE by me, but it is not load-bearing: it supports the doc's caution, not the finding.

DISPOSITION: drop, not downgrade. The mechanism is wrong (CSP does not block the hosted path), and the gap alleged to be MISSING is present in the doc in four places. There is no residual kernel worth an editorial note at this severity. If anything survives for a separate finding, it is the unrelated observation that the CSP's connect-src does not yet enumerate a transport for the browser-to-companion Iroh control protocol of target-architecture item 2 — but the doc's `<relay-hosts>` placeholders and its "Phase 0 must choose... before implementation starts" language (lines 565-568) already frame that as an open placeholder rather than a settled baseline.
````

### R13

Candidate finding: The cache policy (619-625) covers `index.html`, the service worker, public config, and "Content-hashed JavaScript, CSS, Wasm, and images" — but never states the Content-Type requirement for `.wasm`, which `X-Content-Type-Options: nosniff` (line 608) makes fail-closed.

Verification reasoning:

````text
REFUTE (high): ACCURACY lens: the quoted evidence is real but does not say what the finding claims, and the finding's central mechanism is factually wrong.

VERIFIED ACCURATE: (1) Doc line 608 does read `X-Content-Type-Options: nosniff` within the "Additional headers" block at 604-613. (2) Doc 619-625 is "### Cache policy" and line 623 does read "Content-hashed JavaScript, CSS, Wasm, and images use `public, max-age=31536000, immutable`." (3) The doc genuinely never states a Content-Type for .wasm — `grep -n -i "content-type\|mime\|application/" docs/production-deployment.md` returns exactly one line, 608, and nothing else. (4) The MDN quote is transcribed correctly.

REFUTED: The finding asserts nosniff (608) is what makes the wrong wasm MIME "fail-closed" and "forbids the browser from recovering." This is false. The WebAssembly Web API spec rejects unconditionally: "If mimeType is not a byte-case-insensitive match for `application/wasm`, reject returnValue with a TypeError and abort these substeps." I fetched https://webassembly.github.io/spec/web-api/ and asked specifically about sniffing: the spec does not mention X-Content-Type-Options, nosniff, or MIME sniffing anywhere. Browsers never MIME-sniff for WebAssembly streaming, so there is no recovery path for nosniff to remove. Removing line 608 changes nothing. The asserted "interaction with 608" — the finding's own framing in its doc-lines field — does not exist.

The investigator's own evidence fails to support its mechanism: I fetched the cited MDN instantiateStreaming page and confirmed it does not mention nosniff or MIME sniffing at all. The nosniff causal link was supplied by the investigator, not by any source. Secondary errors: the failure is not "silent" (TypeError rejection), and calling it a consequence of a security header misattributes a plain host misconfiguration.

RESIDUAL (why low, not drop): the bare observation that the doc omits the application/wasm Content-Type is literally true. But the doc specifies MIME types for no asset at all — not JS, CSS, images, or webmanifest — so wasm is not a singled-out gap, and MIME mapping is conventionally left to the host. The repo already handles it correctly at /home/sekou/AGI/jeliya/crates/jeliyad/src/serve.rs:826 (`"wasm" => "application/wasm"`), so it is not an unrecognized project hazard. Doc line 585 (`script-src 'self' 'wasm-unsafe-eval'`) also permits non-streaming compile paths. If retained, the finding must be rewritten to strip the nosniff mechanism, leaving a one-line nit — the novel part of the claim is precisely the part that is wrong.
````

### R14

Candidate finding: The "Capability-aware hybrid" column wins on Security ("Bounds authority by mode"), Offline ("Browser mode can work offline"), and Identity ("Root/device keys remain on the selected execution peer").

Verification reasoning:

````text
REFUTE (high): ACCURACY lens: the finding's literal quotations are real, but its load-bearing generalization is false, and two quotes are materially truncated.

VERIFIED REAL (all read directly at /home/sekou/AGI/jeliya/docs/production-deployment.md):
- L202 col1 "Keys and plaintext exist in the browser origin; origin or CDN compromise can sign or exfiltrate" — verbatim.
- L206 col1 "Good while storage survives; active-browser execution only" — verbatim.
- L207 col1 "WebCrypto plus IndexedDB wrapping; active origin can invoke usable keys" — verbatim.
- L207 hybrid "Root/device keys remain on the selected execution peer" — verbatim.
- L32-43 enumerate both modes (item 2 companion, item 3 browser Wasm peer); hybrid is a union — correctly read.
- L1070 unknown #9 "PWA storage behavior across real Safari/iOS and low-storage devices." — verbatim.
- "Phase 4 ships browser-peer mode" — confirmed, L954 "### Phase 4: browser peer and multi-device identity, 10 to 14 weeks".

REFUTED — the thesis "every hybrid cell is written as the best case of both" / "the table cannot lose". I read all 13 hybrid cells (L202-214). Four contradict it outright:
- L205 hybrid "Broad, with explicit capability degradation" vs col3 "Broadest browser reach" — hybrid NOT best.
- L208 hybrid "Browser relay-only; native direct/relay; optional server peers" — carries col1's relay limitation verbatim in substance.
- L211 hybrid "Browser never claims always-on work; native/server may" — states the browser worst case explicitly; this is exactly the pattern the finding asserts is absent.
- L212 hybrid "Medium-high, but can launch incrementally" vs col1 "Low" and col2 "Medium" — hybrid scores WORSE than both.
- L214 hybrid matches col2's 11-17 weeks, and L46-47 say the browser peer "adds approximately 10 to 14 weeks" (21-31 total), worse than col1's 16-24.
A table where the chosen column loses on operational complexity, browser compatibility, total timeline, and carries browser relay-only and no-background-work limitations is not a table that "cannot lose."

TRUNCATED QUOTES: L202 hybrid in full is "Bounds authority by mode; highest design complexity"; L206 hybrid in full is "Browser mode can work offline; companion mode has truthful limited offline behavior". Both carry cost clauses the finding elided, weakening "written as the best case."

OMITTED COUNTER-EVIDENCE: L1064 highest-risk unknown #4 is "Browser-origin/CDN compromise and the maximum authority granted to a web controller" — the doc ranks the exact risk the finding says it hides in its top four unknowns. L446 "Browser peers are available while the application is active" and L972 gate ("an active browser peer works offline and converges after reconnection") document the offline caveat. L38-39 and L218-219 phase browser mode last behind independent gates.

SURVIVING CORE (minor): within the table, the hybrid Security/Offline/Identity cells are terser about browser-mode downside than rows L208/L211 are. That is a real internal-consistency nit worth one line of editorial feedback, not a "rationalized rather than derived decision." Severity high is unsupportable because the underlying risks are documented elsewhere in the doc and the decision already sequences companion-first; correcting to low rather than drop because the three-cell asymmetry is genuine.
````

### R15

Candidate finding:

````text
"Browser compatibility | ... | Broad modern-browser coverage after companion installation | ..." for the hosted-shell-with-companion column.
````

Verification reasoning:

````text
REFUTE (high): All four literal citations are real and correctly quoted: production-deployment.md:205 cell 2 is exactly "Broad modern-browser coverage after companion installation"; platform-matrix.md:58 is "| iOS app | no scaffold or engine build | none | none | excluded |"; platform-matrix.md:56 contains "no cross-network, NAT, direct, or relay evidence" with "excluded"; production-deployment.md:1036 says "The first supported production matrix is desktop-focused". The finding nonetheless fails the accuracy lens because its load-bearing premise and two of its inferences misread the document.

(1) PREMISE REFUTED — "The row is used in the decision as a proxy for addressable reach." The Decision is lines 216-224 and never references the browser-compatibility row. `grep -n -i "reach|addressable"` over the whole doc returns only lines 205, 223, 422, 886. The single reach argument in the Decision is line 222-224, and it is about the GATEWAY column: "A gateway would gain browser reach by replacing Jeliya's current privacy and local-first boundaries with server trust" — i.e. the doc explicitly concedes the gateway has greater reach and rejects it on trust grounds, not on the table row. Line 221 states "A browser peer remains the intended zero-install capability, but the repository does not yet contain its storage or network runtime" — the stated sequencing reason is the missing browser runtime, not reach. The row is not doing the work the finding assigns it.

(2) "the static PWA it is being preferred over" — FACTUALLY WRONG. Lines 1000-1003 put BOTH in release one: "1002:- an installable static PWA;" and "1003:- a signed local companion for the supported desktop platforms;". Nothing is preferred over anything; the companion supplies the peer runtime the browser lacks. Line 1003 also already scopes the companion to desktop in the doc's own words, so the proposed fix's core content is already stated twice (1003 and 1036). Additionally the adopted column is the hybrid, whose cell reads "Broad, with explicit capability degradation" — not the column-2 cell attacked.

(3) "unsatisfiable on mobile ... 0% of mobile" — OVERSTATES the cited evidence. platform-matrix.md:56 says the Android app "application and three-ABI build path exist" and is excluded for lack of cross-network evidence and no published APK/AAB. That is unverified and unreleased, not architecturally impossible. platform-matrix.md is a current shipped/verified status matrix, not a bound on what a companion can target. Only iOS (line 58) is genuinely absent.

(4) IGNORES CONTRARY DOC TEXT. Lines 966-967 make "current iOS Safari and Android Chrome pass the supported matrix" an explicit Phase 4 go/no-go gate, and line 1025 explicitly excludes "mobile background-availability claims" from release one. The doc treats mobile openly and honestly rather than hiding it behind line 205.

(5) On its own literal dimension ("Browser compatibility") the cell is defensible: a hosted shell is a rendering client needing far less than the PWA's WebCrypto + OPFS + Wasm + relay-capable transport stack, which is why cell 1 says "feature detection required". The investigator substituted "addressable reach", a dimension the row does not claim.

Residue is a pure wording nit — "after companion installation" could read "on desktop platforms where the companion is installable" — but with 1003 and 1036 already saying so, it does not merit a review slot, and certainly not the asserted claim that the cell is "backwards" or that the decision may not survive. Drop.
````

### R16

Candidate finding:

````text
"Identity and key storage | WebCrypto plus IndexedDB wrapping; active origin can invoke usable keys | ..." stated as a settled property of the static-PWA option.
````

Verification reasoning:

````text
REFUTE (high): ACCURACY lens: the quotes are real and transcribed correctly, but they do not say what the finding claims, so the finding fails.

Verified literally: line 207 col 1 = "WebCrypto plus IndexedDB wrapping; active origin can invoke usable keys"; lines 1053-1054 = ADR #6 "Browser signing strategy: nonextractable WebCrypto signer or wrapped Wasm seed"; the conditional text is at lines 316-318 (finding cites 317-318, sentence starts at 316); the Phase 4 gate sentence is at 968-969 (finding cites 969). Line-number slips are minor.

Fatal misreading #1: The finding's thesis is that line 207 asserts as settled what ADR #6 leaves open. It does not. ADR #6's open question is WHICH branch (nonextractable signer vs. wrapped seed). Line 207 commits to neither. Line 318 in full: "Otherwise wrap the seed with a nonextractable WebCrypto key and load it into Wasm only while active" — WebCrypto is used in BOTH branches, and IndexedDB is the storage medium in both. "WebCrypto plus IndexedDB wrapping" is therefore the branch-agnostic common substrate, not a premature commitment. The investigator's own cited evidence refutes the reading it is offered for.

Fatal misreading #2: The clause "active origin can invoke usable keys" is not a pending decision either. It restates the doc's own invariant at lines 319-320 ("A malicious same-origin script can still invoke a usable key and may observe active memory"), which holds under both branches. The cell states the browser model's weakness — the conservative direction — not an overclaim.

Overstatement #3: "neither the mechanism nor its wire compatibility is established" and "the hybrid does not retire the risk" are both too strong. Signing goes through iroh_rooms::identity::SigningKey (/home/sekou/AGI/jeliya/crates/jeliya-core/src/identity.rs:12, used :48-50, :119-120, :226). The fallback branch compiles that same Rust code to Wasm, so byte-compatibility with native peers holds by construction. The doc names this escape hatch inline at 317-318. Worst case on a failed WebCrypto Ed25519 spike is "take branch B," so exposure is bounded, not a latent Phase 4 blowup.

Fix would inject an error: line 207 column 4 reads "Root/device keys remain on the selected execution peer" — a mode-boundary claim about key location, true regardless of ADR #6's resolution. Annotating it "Ed25519 wire interop unproven" attaches a browser-signing caveat to a cell that is not about browser signing.

Residue (a DIFFERENT finding, not this one): the "Highest-risk unknowns" list at 1058-1069 omits WebCrypto Ed25519 interop, and neither Phase 0 (869) nor Phase 1 (890-900) schedules a byte-compat spike. A narrow low-severity roadmap note could stand alone, but it is a scheduling suggestion, not the medium DISAGREE about line 207 that was submitted, and the by-construction fallback deflates its urgency. As written, drop.
````

### R17

Candidate finding: "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase" — Phase 0 gate: "a browser reaches a native test endpoint through an authenticated relay."

Verification reasoning:

````text
REFUTE (high): ACCURACY lens verdict: every quoted line is real and quoted correctly, but two of the readings that carry the conclusion are wrong, and the single most on-point line was omitted.

WHAT CHECKED OUT (all verified by reading the file directly):
- L866-867: "No phase starts implementation work that depends on an unresolved go/no-go gate / from the previous phase." — real, quoted correctly.
- L869: "### Phase 0: freeze the claim boundary, 1 to 2 weeks" — real.
- L886: "- a browser reaches a native test endpoint through an authenticated relay;" — real, quoted verbatim.
- L938: "- two dedicated relays and the relay-auth service;" under Phase 3 Deliver — real.
- L959: "- Wasm signing and Iroh endpoint wrapper;" under Phase 4 Deliver — real.
- L849: "| `crates/jeliya-web/` | `wasm-bindgen`, IndexedDB/OPFS, browser signing, and browser Iroh adapters |" — real.
- L511-512 (cited as 512): "recommends an application-specific `wasm-bindgen` wrapper / rather than an off-the-shelf npm package." — real, accurate quote.
So the finding is not fabricating evidence. It fails on inference, not on citation.

REFUTATION 1 — the finding omits Phase 0's own deliverable, which is the direct rebuttal. Lines 877-878 (never cited by the investigator) read: "- prove browser-to-native Iroh connectivity with the intended relay / authentication;". Gate item L886 is a 1:1 restatement of deliverable L877-878. The assertion "Phase 0's gate cannot be evaluated with what Phase 0 builds" is therefore contradicted by Phase 0's own Deliver list. The gate is not orphaned; it maps to a scheduled Phase 0 work item.

REFUTATION 2 — L866 is misread, so "the roadmap's opening claim (866) is falsified by its own first gate" is false. L866 asserts a specific forward constraint: no phase starts work depending on an *unresolved gate from the previous phase*. Phase 0 has no previous phase and no preceding gate, so Phase 0's gate cannot falsify L866 as written. The investigator's actual concern is an inverted dependency (Phase 0 touching capability whose production form lands later), which is a different proposition from the one L866 states. Citing L866 as "falsified" is a misreading of the cited line — dispositive under the accuracy lens.

REFUTATION 3 — "Phase 3/4 deliverable" conflates production instantiation with feasibility proof. L557 states the relays are "two Iroh managed dedicated relays" — a hosted third-party service, not something the team builds. L539-541 describes relay auth as a short-lived endpoint-bound credential from a Cloudflare Worker (L555-556). Phase 3's L938 deliverable is the *operational* form: two regions with failover (gate L948: "a regional relay outage fails over within 2 minutes") and 99.9% availability (L757). Phase 0's L886 needs one browser, one relay, one credential. Likewise "prove ... connectivity" (L877) is validation language, distinct from Phase 4's production browser peer (L958-962: adapters, conformance, revocation, eviction). The finding treats "must exist in some form" as equivalent to "must be the Phase 3/4 production deliverable"; the doc does not.

WHAT SURVIVES (reduced, and reclassified). I ran `grep -n -i "spike\|prototype\|throwaway\|proof of concept\|proof-of-concept\|probe\|timebox" docs/production-deployment.md` — zero matches. So the doc genuinely never labels the L877 work as throwaway, and a 1-2 week Phase 0 (L869) that must demonstrate browser-Wasm Iroh through an authenticated relay is tight given the doc's own L508-512 (default features must be disabled, app-specific wasm-bindgen wrapper required) and L514-522 ("This does not make the current Iroh Rooms runtime browser-compatible" — persistence, sync transport, blob storage, clocks, task spawning all need browser adapters). That is a legitimate schedule-feasibility note and the "say it's a timeboxed spike" half of the proposed fix is reasonable editorial advice.

But per the orchestrator's rules this belongs in DISAGREE-WITH-JUDGMENT, not WRONG: no doc statement is false. The "move the gate to Phase 3" half of the fix should be dropped — it would delete the roadmap's only early de-risking checkpoint for its single largest technical unknown, which is the opposite of good sequencing. High severity is not supportable when the load-bearing inference misreads L866 and ignores L877-878.
````

### R18

Candidate finding: Phase 1 gate: "independent security review approves the wire formats and key lifecycle", freezing `crates/jeliya-protocol/` "Pure protocol-v2 types, canonical encoding, signatures, and conformance fixtures."

Verification reasoning:

````text
REFUTE (high): ACCURACY AUDIT OF THE CITED EVIDENCE

Verbatim quotes that check out (6 of 7):
- L846 (`sed`/Read of docs/production-deployment.md): "| `crates/jeliya-protocol/` | Pure protocol-v2 types, canonical encoding, signatures, and conformance fixtures |" — ACCURATE.
- L899: "- protocol version and capability negotiation;" — ACCURATE, Phase 1 Deliver.
- L910: "- independent security review approves the wire formats and key lifecycle." — ACCURATE, Phase 1 gate, last bullet.
- L960: "- root-signed device authorization and revocation;" — ACCURATE, Phase 4 Deliver.
- L1051: "4. Multi-device and revocation event semantics." — ACCURATE, ADR item 4.
- L1063: "3. Multi-device compatibility with existing room membership history." — ACCURATE, highest-risk unknown #3; index correct.
- MISCITED: the quote "Protocol v2 needs root-signed `device.authorized` and `device.revoked` events and multiple active device bindings per identity" is at L353-354, not L355-356. `grep -n "Protocol v2 needs root-signed" docs/production-deployment.md` → `353:`. Text is right, line numbers are off by two (minor).

WHERE THE EVIDENCE DOES NOT SAY WHAT THE FINDING CLAIMS (load-bearing):

1. "Phase 1 gate ... freezing `crates/jeliya-protocol/`" is not in the document. `grep -ni "freez\|frozen" docs/production-deployment.md` returns exactly ONE hit: `869:### Phase 0: freeze the claim boundary, 1 to 2 weeks` — which concerns the release claim and candidate commit, not the protocol crate. The Repository change map header (L832) is `| Existing or new area | Proposed responsibility |` — no phase column, and no surrounding prose (L830-862) assigns any row to a phase. So the doc never places `crates/jeliya-protocol/` in Phase 1, and never says any wire format is frozen there. The finding's central framing is the investigator's construction presented alongside real doc quotes as if the doc asserted it.

2. "a third of its event vocabulary is designed in Phase 4" is quantitatively false. The pinned upstream rev (Cargo.toml:15 → 71fbb5007bef…, confirmed by orchestrator) defines exactly 10 Content variants at /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/event/content.rs:279-299 — RoomCreated, MemberInvited, MemberJoined, MemberLeft, MemberRemoved, MessageText, FileShared, PipeOpened, PipeClosed, AgentStatus. Adding device.authorized + device.revoked yields 2/12 ≈ 17%, roughly half the asserted fraction. The doc itself never enumerates a v2 vocabulary, so the number is unsupported from either direction.

3. The reading of L910 "the wire formats" as "the complete protocol-v2 wire format including membership events" is one interpretation, not what the text says. Phase 1's heading (L890) is "production identity and protocol **primitives**", and its deliverables (L894-900) that actually carry wire formats are the recovery bundle, `client_msg_id`, timeline cursor, invite expiry/cancellation, and the companion pairing/control protocol. Device authorization events appear nowhere in that list. The scope-limited reading — "the wire formats [delivered in Phase 1]" — is at least as natural. Worse, the finding cites L899 "protocol version and capability negotiation" as proof the format is being frozen, when capability negotiation is precisely the extensibility mechanism that lets later phases add event kinds WITHOUT reopening an earlier review. The cited evidence is closer to the opposite of the inference drawn from it.

4. Classification error under the task's own taxonomy: nothing in the doc is factually false, so "WRONG" does not apply. There is no contradiction between L910 and L960 unless one first imports assumption (1).

WHAT SURVIVES (real, but different and much smaller): Phase 4 introduces new root-signed event types (L960) whose semantics are an undecided ADR (L1051) against a top-3 unknown (L1063), yet Phase 4's gate (L964-975) contains no independent wire-format security review — only behavioral test L973 "a revoked device cannot author an accepted future event" and L975 "the exact upstream/browser-adapter revision receives security qualification". Phase 1's gate does get such a review (L910). Recommending that Phase 4's gate add a second wire-format review — essentially the finding's proposed fix (b) — is a reasonable, actionable improvement. But that is a MISSING gate-scoping observation, not a high-severity WRONG, and it is mitigated by the external penetration review at L950 and the revision qualification at L975.

VERDICT: refuted as framed. The quotes are real but two load-bearing characterizations built on them are unsupported ("freezing" in Phase 1) or contradicted by source ("a third"), the severity is inflated, and the category is wrong. Not "drop", because the residual Phase 4 gate gap is genuine — it should appear as a low-severity MISSING item recommending a second wire-format review in Phase 4's gate, with the correct citation L353-354.
````

### R19

Candidate finding: "Adopt the hybrid model and use the companion-backed shell as the first production slice" because "the repository does not yet contain its storage or network runtime."

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: I independently read every cited doc line and ran every named command. The finding's core survives; two supporting evidence statements do not.

VERIFIED ACCURATE (6 of 8):
1. Doc 221-222 verbatim: "A browser peer remains the intended zero-install capability, but the repository does not yet contain its storage or network runtime." Correctly characterized as an effort/readiness argument.
2. Doc 46-48 verbatim: "estimated at **11 to 17 engineering weeks**. A robust browser-only peer adds approximately **10 to 14 weeks**." The verb "adds" makes 21-31 the DOC'S arithmetic, not the investigator's construction.
3. Doc 214 verbatim: static PWA column "Approximately 16 to 24 engineer-weeks."
4. Doc 912 verbatim: "### Phase 2: companion-backed vertical slice, 5 to 7 weeks."
5. The 5-7 week detour is STRUCTURALLY EXACT, stronger than the investigator argued. `grep -n "^### Phase"` yields Phase 0 (1-2) + Phase 1 (3-5) + Phase 2 (5-7) + Phase 3 (2-3) = 11-17, exactly reproducing line 46; plus Phase 4 (10-14) = 21-31; removing Phase 2 gives 16-24, exactly reproducing line 214. The identity is derived from the doc's own phase decomposition, not a numerical coincidence.
6. Cargo.lock:2022-2034 confirms the `iroh-rooms-core` 0.1.0-rc.3 package block (source git+https://github.com/kortiene/iroh-room?rev=71fbb500...) lists `rusqlite` at line 2032 among its direct dependencies. `grep -n "rusqlite\|sqlite" crates/jeliya-core/Cargo.toml` returns no match. Both halves of the Cargo.lock evidence CONFIRMED.
7. Doc 1060 verbatim: unknown #1 is "Whether Iroh Rooms will accept and maintain the portable browser store, transport, and blob interfaces upstream." Correctly cited.
8. The omission is real: grep for every figure (11 to 17 / 10 to 14 / 16 to 24 / 21 to 31) shows the doc NEVER sums the hybrid total nor compares it to 16-24. Line 214's hybrid cell reports only "First companion slice in 11 to 17 weeks; browser mode follows" — no total. So a reader of 218-224 is left with the false impression that companion-first is the cheaper route to the stated goal, when the doc's own numbers make it 5-7 weeks more expensive.

EVIDENCE DEFECT 1 (material misread): The finding states jeliya-companion consumes "the same `jeliya-runtime` + `jeliya-platform-native` (847-848) that `jeliyad` already consumes (845)". Line 845 reads ONLY: "| [`crates/jeliyad`](../crates/jeliyad) | Remain a loopback-only legacy/local sidecar; never receive a public bind option |" — it says nothing about consuming any crate. Line 851 likewise says nothing about jeliya-companion's dependencies ("Signed native service with Iroh control ALPN and no public HTTP listener" — that quote itself IS accurate). The present-tense claim is also factually false: crates/jeliyad/Cargo.toml:26 reads `jeliya-core = { path = "../jeliya-core" }`, and jeliya-runtime/jeliya-platform-native do not exist (workspace members are exactly jeliya-core, jeliyad, jeliya-ffi per Cargo.toml:2 and `ls crates/`). This misread is the sole support for the "SECOND consumer of the SAME single platform implementation" sub-argument. The sub-argument's CONCLUSION (Phase 2 de-risks little of Phase 4) remains supportable by a different route — line 848 is the native adapter crate while line 849 (`crates/jeliya-web/`) is the browser one, and Phase 4's deliverables at 958-962 are all browser adapters Phase 2 never touches — but the stated evidentiary chain must be struck and replaced.

EVIDENCE DEFECT 2 (overbroad): "The real justification, which the doc never states, is risk ownership." The doc does state the upstream dependency and its risk, repeatedly: 517-518 ("The current runtime assumes SQLite, filesystem blobs, and native Tokio networking"), 527-528, 861-862 ("Portable Iroh Rooms storage, network, and blob interfaces should preferably land upstream. A long-lived private fork is a security and maintenance liability"), and 1060. The finding cites 1060 itself, which is self-undercutting. Only the narrower claim survives: the doc never states it AS THE RATIONALE at the decision point 218-224.

PROPOSED FIX ALSO OVERSTATES: "That schedule is not ours to control" is contradicted by the doc's own line 527, which offers an explicit non-upstream path — "Portable traits are introduced upstream OR IN AN AUDITED SHORT-LIVED PATCH" — and by 861's hedge "should PREFERABLY land upstream." The rewrite must preserve that fallback rather than assert pure external gating.

SEVERITY: asserted high, corrected to medium. Downgrade reasons: (a) this is a DISAGREE about rationale framing, not a factual error in the doc — the finding explicitly concedes companion-first is the right call; (b) the remedy is a rewrite of 4 lines of justification prose, not a plan change; (c) one of the two supporting evidence chains is wrong and must be removed before publication; (d) the proposed replacement text conflicts with doc line 527. It is a genuine and well-grounded finding — the arithmetic inversion is exact and the doc's silence on the hybrid total is real — but it does not rise to high.
````

### R20

Candidate finding: The first production release requires "an installable static PWA" plus "a signed local companion" plus "secure SAS-confirmed pairing with a scoped browser control key" — i.e. the hosted browser shell is mandatory in slice 1.

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens. Every literal citation checks out verbatim: doc 1002/1003/1005 (slice-1 list), 275-277 (TB1), 1043-1044 (accepted disclosure), 850 (crates/jeliya-control/), 112, 334, and ui/src/lib/client.ts:22 (`export const DEFAULT_DAEMON_URL = 'ws://127.0.0.1:7420/ws';`). The core reading — that slice 1 requires a native install (1003) and therefore delivers no zero-install value — is correctly read and is in fact corroborated by lines the investigator did not cite: 221 ("A browser peer remains the intended zero-install capability, but the repository does not yet contain its storage or network runtime"), 205 ("Broad modern-browser coverage after companion installation"), 206 ("signing and sync require the companion"). The narrow claim that cross-device browser access is never stated as a launch requirement is also correct (grep: cross-device occurs only at 137, 299, 307, 351, none as a slice-1 requirement).

Two parts do NOT hold up.

(1) Evidence misread. The finding lists the relay-auth service (556) among costs/attack surface "that would not otherwise exist" without the shell. Lines 542-543 state: "Native companions use the same short-lived credential policy rather than embedding a global project secret." Relay-auth is therefore required for the native companion regardless and does not disappear if the shell is cut. The "CDN dependency" item is likewise weak: line 123 already contemplates hosting "documentation, downloads, checksums, and installation instructions." TB1 and jeliya-control remain fair shell-attributable costs.

(2) The central verdict — "the decision is unfalsifiable because the requirement that would justify it is missing" — is refuted. A justifying objective is stated at 16-17 ("making Jeliya safely usable from https://app.jeliya.ai") and 127-128 ("the goal of a safe, functional Jeliya product at app.jeliya.ai"). The decision at 218 is backed by an explicit four-model comparison (200-214) including relative cost (214: "First companion slice in 11 to 17 weeks; browser mode follows") and rationale at 221-224 (gateway alternative "would gain browser reach by replacing Jeliya's current privacy and local-first boundaries with server trust"). The doc also carries an incremental-launch rationale the finding ignores: Phase 3 (932-940) stands up DNS/TLS/CDN/CSP, service worker, relays, and rollback on the companion slice so the later browser peer lands on an existing production surface. The decision is motivated and falsifiable against a stated goal; disagreeing with the goal is a legitimate DISAGREE but not "unfalsifiable."

Residual valid content: slice 1 accepts TB1 and the 1043-1044 disclosure without delivering zero-install, and the doc would be improved by stating the requirement that justifies the hosted shell in slice 1. That is a "make the rationale explicit" comment. Severity drops from high to medium because the high rating rested on the unfalsifiability claim (refuted) and one cost item is misattributed. The proposed fix's second branch ("cut the shell from slice 1") argues against the document's stated premise rather than an internal inconsistency, so it should be presented as an alternative to justify, not a defect to correct.
````

### R21

Candidate finding: "Smallest production-worthy vertical slice" — the 13 bullets at lines 1002-1015.

Verification reasoning:

````text
REFUTE (high): REFUTED under the accuracy lens. The quotes are transcribed correctly, but the two load-bearing sub-claims rest on misreadings of the cited lines, and both are contradicted by the doc's own text.

**Sub-claim (b), "two relays contradicts the doc's OWN availability objective" — REFUTED, and it is the finding's strongest bullet.**
Line 757 reads `| Relay authentication plus at least one relay | 99.9 percent monthly availability |`. That is the *Surface* column of the "Initial service objectives" table — the label of the composite thing being measured (relay-auth plus >=1 relay reachable), not a provisioning requirement that one relay suffices. The finding reads a measurement subject as a capacity ceiling.
Four pieces of evidence contradict the reading, three of them inside the same document:
- Line 761, four rows below the line the finding cites, in the *same table*: `| Relay regional failover | At most 2 minutes |`. A regional-failover objective is unsatisfiable with one relay. So the doc's availability objectives *require* two relays; they do not forbid them. The finding cites the table as its evidence and misses the row that kills it.
- Line 252 (architecture diagram): `| at least two regions     |`.
- Lines 536-537: "Start with two dedicated managed relays, one in North America and one in Europe."
- External source. I fetched https://docs.iroh.computer/add-a-relay (cited by the doc at line 1076): "For production, run at least two relays in different geographic regions, for example one in North America and one in Europe. iroh clients try multiple relays automatically, so if one becomes unreachable they'll seamlessly fall back to another." The doc's two-relay choice is a verbatim adoption of upstream production guidance, not padding.
The finding also calls gate 948 ("regional relay outage fails over within 2 minutes") something that two relays "drags in" — inverted. 761 commits to that objective independently in the objectives table; the second relay is how it is met, not an incidental cost of it. The cost arithmetic is the only part that checks out ($389/2 ≈ $194, consistent with 0.27/hr × 720h × 2 at line 800-801/807).

**Sub-claim (c), "encrypted cached room view is most likely rewritten in Phase 4" — REFUTED; cited evidence does not say it.**
Lines 425-430 are the "Browser-peer mode" storage list (IndexedDB / OPFS / Cache Storage). They say nothing about superseding or replacing the companion-mode view cache. The claim that browser-peer mode "replaces a cache-of-remote-projection" is the reviewer's inference, and the doc contradicts it: the architecture diagram at 242-245 presents "Companion mode" and "Browser-peer mode, later" as two coexisting columns of the same browser runtime; line 718 lists "capability mode, such as companion or browser peer" as an ongoing telemetry dimension; line 838 lists `ui/src/runtime/` as "Companion and browser-peer client adapters" (both, plural). Companion mode survives Phase 4, so its cache (line 414, "an encrypted local projection of recently viewed rooms") is not throwaway work. "Its offline value is low" is an unsupported judgment, and it runs against line 206 ("companion mode has truthful limited offline behavior") and gate 946.

**Sub-claim (a) — quotes accurate, conclusion over-read.**
1003, 918, and 1068 are all quoted correctly. But (i) 918 says "signed macOS and Windows packages and **a verified Linux package**" — two signing pipelines plus a verification step, not the "three signing pipelines" asserted; (ii) the slice at 1003 says "the supported desktop platforms" and the doc explicitly defers what that set is — line 1056, open unknown #8, "Supported browser, desktop OS, and mobile matrix". The slice therefore does not hard-commit to three platforms, so "one platform suffices for slice 1" is arguing against a commitment the doc has not made.

**Sub-claim (d) — quotes accurate, evidence used backwards.**
904 and 306 are transcribed correctly, but 306 is cited in inverted sense. Its full context (303-307, section titled "Current identity boundary") is: "The current UI truthfully states that the identity is unrecoverable. There is no export, recovery, rotation, device authorization, or same-identity cross-device flow." That is the doc's statement of the *defect being fixed*, not a launch posture it endorses. The doc's actual position is the opposite of the finding's: recovery is named top risk #5 at line 1066 ("Recovery usability and user custody for an accountless identity"). The finding's supporting premise — "a recovery bundle can be generated from an existing identity at any later time" — carries no citation and ignores lines 776-777: "Jeliya infrastructure cannot restore a client-only identity or unique local file." Deferring recovery is irreversible for any user who loses a device before the fast-follow; the finding does not engage with that line.

**Proposed fix is also inaccurate on the doc's contents.** It says to cut runbooks 697-706 to "the three that can actually fire at launch". At least three of the excluded five demonstrably can fire at launch given the slice the finding itself accepts: "native signing-key compromise" (701) applies from the moment the signed companion at 1003 ships; "browser-storage loss or corruption" (706) applies to the cache/drafts at 1012 (and the finding retains drafts); "dependency advisories" (705) applies continuously.

**What could survive:** only a cosmetic nit — bullet 13 (1014-1015) does pack eight distinct workstreams into one line, so "decompose it" is factually fair. That is an editorial formatting note, not a medium-severity architecture-judgment finding, and it is not what this finding argued. The "four bullets are padding, cut to 9" thesis does not hold up.
````

### R22

Candidate finding: The repository change map proposes eight new crates: jeliya-protocol, jeliya-runtime, jeliya-platform-native, jeliya-web, jeliya-control, jeliya-companion, jeliya-components, jeliya-server-peer.

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: the finding's quantitative evidence is real and correctly read, but its characterization of the cited section is not, which deflates it from medium to low.

VERIFIED ACCURATE (I re-derived each):
- Doc 846-853 does propose exactly the eight named crates. Confirmed by Read and by `grep -n` for all eight names, which returns hits ONLY at 846-853 plus `jeliya-companion` at 916.
- LOC measurements are exact. `find crates/<c> -name '*.rs' -type f -exec cat {} + | wc -l` returns jeliya-core 7043, jeliyad 1696, jeliya-ffi 998, total 9737 — matching the finding digit for digit.
- Workspace membership content is correct; 3 existing + 8 new = 11 crates.
- Three new workflows (854-856) and infra/ (857) confirmed.
- "Later" quotes are verbatim: 852 "Later signed package, WIT policy, quota, and native component host"; 853 "Later explicitly invited availability or hosted-agent peer".
- 977 "### Phase 5: components and optional server peers, 8 to 16 weeks" and 952 "This is the first production launch gate" — exact.
- The uncited "2-3 engineer team" premise is genuinely in the doc (45 "For a small team of two to three engineers"; 197-198 "two core/full-stack engineers, one web/operations engineer at least part-time").
- The jeliya-control single-consumer inference holds: 850 names no consumers, and the browser control client is TypeScript (840 `ui/src/pairing/`), so jeliya-companion is the only Rust consumer pre-Phase-4.

MISREADINGS:
1. Minor citation error: `members = [...]` is at Cargo.toml line 2, not line 3 (line 3 is `resolver = "2"`).
2. Minor: "five new UI directories (837-841)" — 837 is `ui/src/sw.ts`, a file. Four directories plus one file.
3. Material: "Listing Phase 5 crates in a Phase 3 launch plan" misdescribes the section. `## Repository change map` (830) precedes `## Dependency-ordered roadmap and gates` (864), has no framing sentence, no phase column, and is document-scoped across all five phases. It is not a Phase 3 plan.
4. Material: the proposed fix largely already exists. `## Smallest production-worthy vertical slice` (998) explicitly excludes "third-party components" (1023) and "optional server peers" (1024) — exactly the two Phase 5 crates — and "a browser-owned room identity" (1019), which excludes jeliya-web. The doc nowhere commits all eight crates to slice 1, so the premise that it "takes a 2-3 engineer team to eleven crates" for launch overstates the document.

The finding does not collapse: the change map genuinely lacks phase attribution, and jeliya-web (clearly Phase 4 per 958-959) carries no "Later" marker while two peers do. The "at most five new crates for slice 1" arithmetic is correct. But that residual is a table-legibility improvement, not an architecture disagreement, since the doc already performs the slice separation the fix demands.
````

### R23

Candidate finding: Split `crates/jeliya-core` to "Split host-independent protocol/runtime behavior from native persistence and network assumptions" into jeliya-protocol + jeliya-runtime + jeliya-platform-native.

Verification reasoning:

````text
REFUTE (high): REFUTED. The finding's individual citations are mostly real, but its load-bearing assertion — "the native assumption is not in jeliya-core" — is false, and it is refuted by the crate's own source, which the investigator never opened.

WHAT CHECKED OUT (accurate evidence):
- Doc lines quoted correctly. 842 = "Split host-independent protocol/runtime behavior from native persistence and network assumptions"; 846-848 = jeliya-protocol / jeliya-runtime / jeliya-platform-native ("SQLite, filesystem, native Iroh, and OS keystore adapters"). Verified verbatim.
- Dep list of crates/jeliya-core/Cargo.toml: exact, including no rusqlite declaration.
- Cargo.lock:2023-2034: `iroh-rooms-core` 0.1.0-rc.3 at rev 71fbb500... does list `rusqlite` (line 2032). Accurate.
- Header description "the only crate in the project that talks to the iroh-rooms SDK": accurate (Cargo.toml:7).
- Line counts exact: 7043 total, supervisor.rs 4580, identity.rs 335.
- Consumers exact: crates/jeliyad/Cargo.toml:25 and crates/jeliya-ffi/Cargo.toml:16 both `jeliya-core = { path = "../jeliya-core" }`; workspace members confirm only those two.
- Doc 63 and 884 quoted accurately.

WHY IT STILL FAILS — jeliya-core is pervasively native:
1. crates/jeliya-core/src/engine.rs:111 — `pub fn new(data_dir: PathBuf, loopback: bool, config: EngineConfig)`. The crate's top-level public constructor takes a native filesystem path. That IS the "native persistence assumption" doc 842 names.
2. crates/jeliya-core/src/supervisor.rs:256-258 `fn db_path(&self) -> PathBuf { self.data_dir.join(DB_FILE) }` and :270-282 `open_store()` → `EventStore::open_with(&self.db_path(), ...)`. Even the SQLite assumption is partly here: upstream owns rusqlite, but jeliya-core owns the native path handed to it. The seam the split targets is exactly that path.
3. supervisor.rs:260-268 `room_blobs_dir()` — jeliya-core's own per-room FsStore directory layout.
4. supervisor.rs:291-293 `assert_shareable_path` — a security control built on `std::fs::canonicalize`.
5. crates/jeliya-core/src/identity.rs:68-83 `ensure_dir` with `#[cfg(unix)] std::fs::Permissions::from_mode(0o700)`; module doc :1-6 "Device identity persistence under `--data-dir` ... `identity.json` ... `identity.secret`, both owner-only (`0600`, dir `0700`)". That is an OS-specific keystore inside jeliya-core — precisely doc 848's "OS keystore adapters".
6. crates/jeliya-core/src/localstate.rs:115 `atomicwrites::AtomicFile::new(...)` plus `std::fs::read` (:82), `OpenOptions` (:102), Unix mode (:104). This is a filesystem store. The finding wrote "no rusqlite and no filesystem-store dependency (deps are: ... atomicwrites)" — it listed the disconfirming item inside its own parenthetical and asserted the opposite conclusion.
7. Filesystem call sites by file: supervisor.rs 26, localstate.rs 10, identity.rs 8, engine.rs 3. Zero `wasm32`/`target_arch` cfgs anywhere in the crate; the only cfgs are 5x `#[cfg(unix)]`. jeliya-core cannot compile to a browser target as written.

So the doc's proposed `jeliya-platform-native` maps onto real, identified content living in those 7,043 lines. The finding's core claim that the split "rearranges 7,043 lines ... without removing the blocker" is wrong: the blocker material is demonstrably in them. Absence of a rusqlite *declaration* in Cargo.toml is not absence of native assumptions — that is the misread.

SECONDARY POINTS DON'T RESCUE IT:
- The "invalidates exact-revision qualification" objection quotes 63/884 accurately but is a non-sequitur: the Phase 0 gate freezes the *claim* boundary, and every phase from 1 onward necessarily changes code. It would forbid all implementation work.
- "Two consumers" is accurate but is a cost/risk observation, not evidence the proposal is WRONG.
- "browser one arriving 15+ weeks later" is loose: summing 869/890/912/932, Phase 4 begins 11-17 weeks in, so 15+ is the top of the range, not the floor.
- The proposed fix ("land the portable traits upstream FIRST") is already the doc's own stated position at 861-862 ("Portable Iroh Rooms storage, network, and blob interfaces should preferably land upstream"), which the finding cites without noting the doc agrees.

Classifying this as WRONG/high inverts the truth. The residual — "design the trait seam against a second implementation, and sequence upstream work ahead of the split" — is an ordinary design caution the doc partly makes itself; it does not survive as written and would need a full rewrite at low severity to be defensible. Drop.
````

### R24

Candidate finding: "The companion shows a QR ... containing an ephemeral public key, endpoint, and nonce" then "The peers establish a Noise XX-equivalent authenticated transcript over Iroh."

Verification reasoning:

````text
REFUTE (high): REFUTED under the accuracy lens. The cited evidence is real but is not correctly read, and the misreading is the finding's load-bearing step.

WHAT IS ACCURATE (verified independently, so coverage is visible):
1. The Noise spec quotations are verbatim correct. Fetched https://noiseprotocol.org/noise.html and confirmed the second-character convention: "N" = "No static key for responder"; "K" = "Static key for responder Known to initiator"; "X" = "Static key for responder Xmitted ("transmitted") to initiator". The gloss "XX is by definition the pattern for when NEITHER party knows the other's static key in advance" is correct (XX = `-> e / <- e, ee, s, es / -> s, se`).
2. The doc-line citations are correct. docs/production-deployment.md:327-329 reads exactly: "- The companion shows a QR or custom-protocol link containing an ephemeral / public key, endpoint, and nonce, never a reusable bearer secret. / - The peers establish a Noise XX-equivalent authenticated transcript over Iroh."
3. The ADR pointer is correct. Line 1049 is "2. Companion control protocol and pairing transcript." — the right home for such a decision.

WHY IT STILL FAILS — the central inference does not follow from the cited text:
The finding asserts "The QR at line 328 delivers the responder's key to the initiator ... that is by construction the 'K' case, i.e. NK/XK/IK." But line 327-328 says the QR carries an **ephemeral** public key, not a static key. Every definition the finding itself quotes is scoped explicitly to the **static** key. The finding silently substitutes "key" for "static key," and the entire NK/XK conclusion rests on that substitution.

The same spec the finding cites refutes the inference directly. Pre-message patterns are defined as one of `"e"`, `"s"`, `"e, s"`, or empty — public keys "somehow performed prior to the handshake." An out-of-band *ephemeral* is therefore a first-class, named construct in the framework, and the XX-family pattern for exactly this situation is XXfallback: `-> e / ... / <- e, ee, s, es / -> s, se`. XXfallback moves the ephemeral into the pre-message and remains an XX pattern precisely because neither *static* is known in advance. So OOB delivery of an ephemeral does not convert the pattern to the K family. The doc's design (QR-pinned ephemeral + mutual static transmission in-handshake + SAS confirmation) maps onto XX/XXfallback, not onto a misuse of XX.

The proposed fix would also degrade the design, not improve it. "Noise_XK (companion static known from the QR pre-message)" requires putting the companion's long-lived static key into the QR — a persistent, scannable, linkable identifier — replacing a deliberately per-pairing ephemeral. The adjacent constraint on line 328 ("never a reusable bearer secret") and the surrounding privacy posture indicate the ephemeral is intentional. Security-wise the swap buys nothing for a one-shot pairing: a QR-pinned ephemeral binds the handshake and resists an active MITM for that session just as a pre-shared static would, so the premise that XX "discards the authentication the QR already provides" is unsupported.

A further accuracy problem the finding does not address: "over Iroh" makes a literal Noise pattern prescription incoherent. Cargo.lock:1792-1793 pins iroh 1.0.1, and its docs.rs page states "The connection is encrypted using TLS, like standard QUIC connections" and "Unlike standard QUIC there is no client, server or server TLS key and certificate chain. Instead each iroh endpoint has a unique `SecretKey` used to authenticate and encrypt the connection." Iroh is QUIC + TLS 1.3 with raw public keys and does not implement Noise at all. That is exactly why the doc writes "XX-**equivalent**" — hedged language describing the authentication *shape* (mutual, both statics authenticated in-handshake, no prior static knowledge required) that iroh's mutual raw-public-key TLS already supplies. Rewriting line 329 to "`Noise_XK`" would not be an editorial fix; it would mean replacing iroh's transport crypto or layering a second handshake, which the finding neither acknowledges nor scopes.

One steelman considered and rejected: one could argue the QR's "endpoint" implicitly carries the companion's iroh EndpointId, which per docs.rs *is* the node's static public key ("Since the `PublicKey` is also used to identify the iroh endpoint it is also known as the `EndpointId`") — that would be a genuine pre-shared static. But the finding does not make that argument (it points at the ephemeral key), and even granting it, the doc explicitly separates identities at lines 324-325: "The browser control identity is separate from the Jeliya profile or room-device identity." So the pairing-layer statics still are not known in advance, and the K-case claim fails as stated.

SEVERITY: drop, not merely downgraded. The finding's thesis is "XX is the wrong pattern; specify XK/NK," and that thesis is technically wrong on the doc's own text and on the spec it cites. Shipping it even at low severity would push the author toward a strictly worse design (long-lived static key in the QR) on the strength of a misreading. There is a legitimate but much weaker residual observation — line 329's "Noise XX-equivalent over Iroh" is imprecise since iroh is not Noise, and ADR #2 (line 1049) should pin down the actual transcript including how the QR ephemeral is bound as a pre-message — but that is a different, low-severity clarity note with the opposite remedy, and should be raised on its own terms rather than carried in under this finding.
````

### R25

Candidate finding: The pairing flow (QR display -> Noise handshake -> SAS -> scopes recorded) as the whole of the pairing threat model.

Verification reasoning:

````text
UPHOLD (high): PARTIALLY HOLDS. The quotations are real and verbatim, but two of the four evidentiary limbs are misread, and the framing overstates what the doc claims for itself. Severity critical is not supportable.

WHAT I CONFIRMED (quotes are genuine):
- Lines 327-331 read exactly as characterized: "- The companion shows a QR or custom-protocol link containing an ephemeral / public key, endpoint, and nonce, never a reusable bearer secret. / - The peers establish a Noise XX-equivalent authenticated transcript over Iroh. / - Both sides display a short authentication string and require user / confirmation." ACCURATE.
- Line 924 quoted verbatim: "- 1,000 automated pairing/revocation cycles accept no unauthorized controller;" ACCURATE.
- The 401-402 quote is real, though it actually spans lines 400-402 ("...implicitly. Browser extensions, screenshots, copied / links, and OS clipboard managers remain disclosure risks that the product must / state."). Off-by-one, immaterial.
- The core gap is REAL and I verified it by exhaustive grep. `grep -n -i "QR" docs/production-deployment.md` returns exactly one hit (line 327). `grep -n -i "single-use"` returns line 190 (unrelated, "single-user") and line 386 only. Line 386 — "- Default to single-use with a 30-minute expiry for live pairing and no more / than 24 hours for asynchronous invites" — sits inside the "Secure invitation links" section (366-402) and governs invite tickets, not the pairing QR. Every expiry mention in the pairing section (332, 337, 840, 909) refers to the *granted control key*, not the QR ephemeral. So: the doc genuinely never states a QR validity window or single-use invalidation. That asymmetry is notable precisely because the author DID specify single-use+TTL for invites at 386 and did not carry it to the QR. This is a legitimate MISSING finding worth one line in a review.

WHERE THE EVIDENCE IS MISREAD (three defects):
1. "grants strictly more authority (a durable control key, lines 332-334) than a single invite" is contradicted by the very lines around it. Line 337: "- Control keys are rate-limited, expire, and can be revoked immediately." Not durable. Line 334: "- Default scopes cover selected-room reads and idempotent chat sends only." Lines 335-336: "- Invite creation, file access, pipes, identity operations, and agents require / separate approval." A scoped, rate-limited, expiring, instantly-revocable read+chat delegate is not "strictly more authority" than an invite, which confers room membership. This limb fails.
2. "no defense against an attacker who simply READS the QR ... so the user confirms it" understates line 330-331's explicit "require user confirmation" on BOTH sides. A QR-capturing attacker cannot pair without an affirmative human approval of a prompt the victim did not initiate. Worse for the finding, its own headline scenario is partly self-defeating: if the victim is concurrently pairing their own browser, the companion shows the attacker's SAS while the victim's browser shows a different one — mismatch, victim rejects. The attack reduces to opportunistic later reuse of a still-valid QR at a moment the victim is not pairing, still requiring the victim to affirmatively approve an unsolicited dialog. That is a soft defense, not "no defense."
3. "The Phase 2 gate at line 924 does not model this attacker at all" cherry-picks. The investigator did not engage the Phase 1 gate at line 909: "- replay, wrong-SAS, expired-key, and revoked-key pairing tests fail closed;". The doc does require pairing replay-resistance and expiry tests to fail closed. That does not fully cover a fresh honest handshake from a QR reader, but "not at all" is wrong.

FRAMING DEFECT: the claim evaluated treats lines 327-331 "as the whole of the pairing threat model." Line 876 makes updating the threat model an explicit Phase 0 deliverable: "- update the threat model for browser origin, companion, and relays;". The doc does not present these bullets as a complete threat model. (I did confirm docs/security-threat-model.md currently has no QR/SAS/pairing content — grep for qr|pair|sas|shoulder|screen hits only line 36, a commit-pair table row — so the gap is real today, but the doc has already scheduled the work.)

NET: keep it, demoted to low, and rewritten as a narrow doc-completeness point — "mirror line 386's single-use + TTL rule for the pairing QR, and specify that the confirmation dialog says a device is pairing now." Drop the "strictly more authority", "durable", "no defense", and "not modeled at all" assertions; they do not survive reading the cited lines. This is a proposal document with no implementation behind it, the attack requires affirmative victim confirmation (330-331), replay/expired-key pairing tests are already gated (909), and the threat model is already a scheduled deliverable (876). Nothing here approaches critical.
````

### R26

Candidate finding: "Default scopes cover selected-room reads and idempotent chat sends only."

Verification reasoning:

````text
UPHOLD (high): SURVIVES, but materially overstated. I opened every cited location.

VERIFIED ACCURATE: engine.rs:47-68 is `requires_room_access_preflight`; 24 dispatch arms total (grep of `"x.y" =>` returns exactly 24, lines 195-328); `daemon.shutdown` at 196-205 never reads `raw_params` (genuinely parameter-free); `room.list` at 220 is an unfiltered `sup.list_rooms()`; `agents.fleet` at 321 -> supervisor.rs:2560, whose doc-comment at 2548-2551 reads "every room this identity belongs or belonged to, open or not"; `daemon_status` at 650-666 emits all five named fields verbatim (`data_dir` 660, `identity_id`/`device_id` 654, `endpoint` 663, `rooms_open` 664). The 7 named non-preflight methods are exactly the right 7.

EVIDENCE ERRORS: (1) "lists exactly 16 methods" is wrong — engine.rs:50-66 lists 17. (2) "the remaining 8 dispatch arms" is wrong — there are 7 (24-17), and the investigator then lists 7, contradicting their own preceding "7 of the engine's 24". Cosmetic, not load-bearing, but the finding as written is internally inconsistent.

OVERREACH: (a) "A scope model expressed as 'selected-room' is keyed on a room id" is the investigator's inference, not a doc statement. Doc:334 says default scopes cover those things "only" — an "only" clause that is already default-deny in intent. "Would silently fall open under any room-keyed check" is speculation about an implementation the doc never specifies. (b) The proposed fix demands `identity.create` and `agents.fleet` be enumerated as denied, but doc:335-336 already states "Invite creation, file access, pipes, identity operations, and agents require separate approval" — covering both. (c) "The doc never says where scope enforcement lives" is overstated: doc:850 assigns "scoped RPC" to `crates/jeliya-control/`, and doc:181 already lists "identity creation, daemon shutdown ... agent projections" as the RPC surface. (d) The fix says the layer should sit "ABOVE Engine::dispatch", but doc:847 proposes `crates/jeliya-runtime/` to replace the current engine — the fix targets a component the proposal retires.

RESIDUAL GENUINE GAP: after subtracting what doc:334-336 already covers, five methods are addressed by neither the default-scope clause nor the separate-approval carve-out: `daemon.shutdown`, `daemon.status`, `room.create`, `room.list`, `room.join`. `daemon.shutdown` as a parameter-free remote kill from a paired browser controller is a real authority gap in an identity-boundary section, and `room.list`/`daemon.status` are real disclosure gaps against a "selected-room" framing.

SEVERITY: not critical. This is a bulleted design sketch for a not-yet-built stack; the doc already carries default-deny framing, a separate-approval carve-out covering 2 of the 7, and a named crate for scoped RPC. The finding is "tighten this spec to enumerate the daemon-lifecycle and room-index methods", which is medium. Keep it, but rewrite: drop the room-id-keyed premise, drop identity.create and agents.fleet from the enumeration ask, fix 16->17 and 8->7, and retarget the fix at jeliya-control/jeliya-runtime rather than Engine::dispatch. The conformance-test suggestion is sound and worth keeping.
````

### R27

Candidate finding: The slice "explicitly excludes" files, pipes, agents, components, and server peers (a product-scope statement with no build-level requirement).

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: every cited piece of evidence is real and correctly read. I verified each independently.

DOC LINES (all verbatim): 1017-1026 is a bare product-scope list ("It explicitly excludes: … files; pipes; agents; third-party components; optional server peers; …") with no build-level requirement — correctly characterized. Line 847 = "`crates/jeliya-runtime/` | Host-independent engine over store, network, blob, clock, and key traits". Line 851 = "`crates/jeliya-companion/` | Signed native service with Iroh control ALPN and no public HTTP listener". Line 928 = "a malicious controller cannot invoke files, pipes, agents, or identity reset" — genuinely runtime-behavior phrasing, so the proposed strengthening is apt. The grep reproduces exactly: sole hit is line 1075, the Iroh WASM-browser docs citation, which is indeed unrelated.

REPO EVIDENCE (all exact, zero line drift): main.rs:33-34 is `#[cfg(feature = "relay-only-test")]` on RELAY_ONLY_VERIFICATION_MARKER. main.rs:78-80 is the same cfg on the hidden `verification_relay_only_build` arg. main.rs:107-115 is the gated branch, returning before `create_dir_all` at 117 — the in-code comment even states it "intentionally runs before data-dir creation, logging, token generation, or networking." main.rs:429-443 is the test `ordinary_build_does_not_accept_the_relay_attestation_flag` containing the literal quoted string "release/default binary must reject the hidden verifier flag". engine.rs has `match method {` at 193 and the `other =>` fallback at 333; I counted the arms and it is EXACTLY 24, matching the claim. All four named methods exist as arms: file.share:285, file.fetch:294, pipe.expose:301, pipe.connect:310. The proposed fix's expected wire code also checks out: `other =>` returns CoreError::invalid -> ErrorKind::InvalidParams -> "invalid_params" (error.rs:48,125). The "unconfined save_dir sink" cross-reference is real: engine.rs:296 forwards p.save_dir into fetch_file and supervisor.rs:1981 does `save_dir.map_or_else(|| self.data_dir.join(DOWNLOADS_DIR), PathBuf::from)` — caller-supplied absolute path, no confinement.

MISSING CLASSIFICATION STRESS-TESTED: I ran a broader grep than the investigator (compiled out|not built|absent from|built without|shipped without|no dispatch|unreachable|#[cfg) across the whole doc — zero hits. The requirement is absent under any wording, so MISSING (not WRONG, not DISAGREE) is the correct bucket.

SEVERITY HELD AT HIGH. The strongest available objection is altitude: "it is a planning doc; enforcement mechanism belongs in implementation." That objection fails on the doc's own terms, per evidence I derived independently. Line 923, in the very same Phase 2 gate list, states a structural build/deployment requirement ("the companion has no non-loopback TCP or HTTP control listener"), proving the doc operates at an altitude where this class of requirement is stated. And line 295 explicitly designs the companion to "expose native files/pipes/agents only with approval" — an avowed runtime-approval model, which independently corroborates that the excluded surfaces remain reachable code behind unwritten policy. The doc states the structural requirement for the listener but not for the excluded method surfaces; that asymmetry is precisely the gap, in a document governing a public production deployment whose reachable surface includes an arbitrary-write sink.

TWO PRECISION CAVEATS (framing, not misread evidence, so not grounds to refute): (1) "the companion is proposed to embed the whole engine" is an inference from the crate map — the doc never literally states that jeliya-companion depends on jeliya-runtime. (2) engine.rs:193-335 is today's jeliya-core engine, not the proposed new crate. Both inferences are sound (line 842 describes jeliya-runtime as the host-independent split of exactly this engine, and line 295 corroborates the runtime-approval model), but the published finding should mark them as inference rather than present them as quoted doc claims.
````

### R28

Candidate finding: "Invite creation, file access, pipes, identity operations, and agents require separate approval." / "Control keys are rate-limited, expire, and can be revoked immediately."

Verification reasoning:

````text
REFUTE (high): ACCURACY LENS: the two load-bearing factual assertions in the evidence are contradicted by the doc's own text, and the proposed fix's headline element is already in the doc.

WHAT CHECKS OUT (verified directly):
1. The quoted text is verbatim correct. /home/sekou/AGI/jeliya/docs/production-deployment.md:335-336 reads "Invite creation, file access, pipes, identity operations, and agents require / separate approval." and :337 reads "Control keys are rate-limited, expire, and can be revoked immediately."
2. `grep -n -i "fatigue" docs/production-deployment.md` returns zero hits (exit 1). Confirmed, and zero hits repo-wide under docs/. But keyword absence is weak evidence — the doc simply does not use that vocabulary.
3. The doc genuinely does not specify an elevation *request* protocol. Accurate.

WHAT DOES NOT CHECK OUT:
4. "A malicious origin therefore has an unbounded, free, SILENT retry loop." Refuted by line 693: "The companion can enforce a minimum-safe control-protocol version, but the web / origin cannot rewrite native state or silently elevate scopes." The doc explicitly forecloses silent elevation. "Unbounded" and "free" also sit directly against line 337's literal "Control keys are rate-limited."
5. "with a plausible first-party UI ('Jeliya needs file access to finish syncing') rendered at app.jeliya.ai. Since the origin also controls the surrounding UI, it can make declining look like a bug." Refuted by lines 266-267, which state as trust boundary TB3: "native files, pipes, and agent execution require / explicit LOCAL approval and stronger permissions", and by line 295, where the approval is a *Native companion* responsibility ("expose native files/pipes/agents only with approval") and "Expose public HTTP/WS; give a browser a daemon token" is listed as PROHIBITED for the companion. The doc already locates the consent surface on the native companion, not at the web origin. The attack as narrated is not the architecture the doc describes.
6. "Line 337's rate-limiting is applied to control keys (session/RPC volume), not to APPROVAL PROMPTS." The doc says only "Control keys are rate-limited" — it never says "session/RPC volume." That scope restriction is the investigator's, presented as the doc's. Since every elevation request necessarily originates from a control key, the natural reading of "control keys are rate-limited" plausibly already covers elevation-request rate. The defensible statement is "the doc does not specify what the rate limit covers," which is materially weaker than what was asserted.

FIX IS LARGELY REDUNDANT:
7. The fix's primary element — "the elevation prompt is rendered by the COMPANION's native UI (not by the web origin)" — is already stated at line 267 ("explicit local approval") and line 295.
8. The fix's "names the requesting control key, its creation time, and its last use" is already at lines 332-333: "The companion records the browser public key, granted scopes, expiry, / creation time, and last use."
9. "Add a Phase 2 gate case for it" ignores the existing Phase 2 gate at line 928: "a malicious controller cannot invoke files, pipes, agents, or identity reset."

SURVIVING RESIDUE (why 'low', not 'drop'): one narrow element is genuinely absent. `grep -i "backoff\|cooldown"` returns zero hits, and the Abuse controls section (lines 785-796) rate-limits relays, invitations, authoring, and storage but says nothing about repeated declined scope-elevation requests. No decline cooldown, no escalating backoff on elevation prompts, and no auto-revoke after repeated declines. That is a real hardening gap worth one sentence — but it is a detail-level omission in an explicitly phased design proposal that already puts consent on native UI, already rate-limits and revokes control keys, already records key provenance, and already gates malicious-controller behavior in Phase 2. It is not a 'high'. As written the finding overstates the architecture's exposure by attributing to the doc the opposite of what lines 267, 295, and 693 say, so it should not survive in its current form; at most it should reappear as a low-severity hardening note.
````

### R29

Candidate finding: Abuse control: "no arbitrary relay egress or generic TCP proxying".

Verification reasoning:

````text
REFUTE (high): REFUTED on two independent accuracy grounds: the cited evidence does not exist at the cited URL, and the cited URL — read correctly — affirmatively SUPPORTS doc line 793 rather than refuting it.

**1. The quote is misattributed.** The finding attributes to https://docs.iroh.computer/about/faq the sentence: "They accept encrypted traffic for iroh endpoints which are connected to them, forwarding it to the correct destination based on the EndpointId only..." I fetched that page (curl -sL, 402761 bytes, saved to scratchpad/faq.html), stripped tags, and searched. Every distinctive fragment is absent:
  'accept encrypted traffic' -> NOT FOUND
  'correct destination'      -> NOT FOUND
  'can not decode'/'cannot decode' -> NOT FOUND
  'only forward it'          -> NOT FOUND
  'EndpointId only'          -> NOT FOUND
The text is real, but it is from the docs.rs/iroh crate docs (confirmed via WebSearch for the exact phrase, which returns docs.rs/iroh and jetstream.rs, not docs.iroh.computer). The investigator cited a URL they did not read.

**2. The cited page directly contradicts the finding's conclusion.** The actual FAQ, verbatim, under "What are the risks of running a public relay?": "The traffic you relay is fully end-to-end encrypted and cannot be decrypted by the relay. The only information a relay has is what it needs to function: the endpoint IDs and IP addresses of the endpoints currently connected to it, plus which endpoints are paired. **A relay has no egress to the open internet, so if you're comparing it to Tor, running a relay is like running a guard/middle relay, not an exit node.**" That last sentence is upstream's own categorical statement of exactly the property doc line 793 asserts. Line 793 is not merely defensible — it restates the vendor's documented architectural guarantee, using the vendor's own framing.

**3. The finding equivocates on the terms of art.** "Arbitrary relay egress" and "generic TCP proxying" mean egress to arbitrary internet destinations and CONNECT-style proxying to arbitrary host:port. The abuse they name is open-proxy abuse: your infrastructure reaching/attacking third parties, IP blacklisting, exit-node legal exposure. An iroh relay structurally cannot do this — its address space is EndpointId, and only endpoints already connected to it. The FAQ's guard/middle-vs-exit-node analogy draws precisely this line. The investigator substitutes a different scenario — two attacker-controlled endpoints tunneling bytes to each other — and calls that "exactly what a generic relay is." It is not. No third party is reachable; nothing egresses. That is bandwidth abuse, not proxying, and it is already the named target of lines 787-788 ("short-lived endpoint-bound relay tokens"; "per-IP and per-endpoint handshake, connection, byte, and rate limits") — which the finding itself concedes at line 788 before discarding the concession.

**What survives:** nothing at the asserted level. The residual observation — that line 793 states an inherited structural property of iroh rather than a control Jeliya implements — is a mild framing nit, and listing structural properties that bound abuse is normal practice in an abuse-control section. It does not make the claim WRONG, and the proposed fix would replace an accurate, vendor-corroborated statement with weaker text that concedes a proxying capability the relay does not have. Verified as ACCURATE: doc line 793 (exact text confirmed: "- no arbitrary relay egress or generic TCP proxying;"), and consistent with doc lines 278-279 and 296, which already scope relays to metadata visibility and routing only.
````

### R30

Candidate finding: "A minimal first-party bootstrap reads the fragment into memory and calls `history.replaceState()` before React startup, service-worker registration, error reporting, or telemetry." plus the CI assertion "that invite fragments never enter HTTP requests, logs, or crash evidence".

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: the evidence is real and correctly read. The finding survives.

VERIFIED DOC CITATIONS (all read directly):
- 377-379 verbatim: "A minimal first-party bootstrap reads the fragment into memory and calls `history.replaceState()` before React startup, service-worker registration, error reporting, or telemetry." Line 375 frames the list as "Required controls are:" — i.e. presented as sufficient.
- 653-654 verbatim: "an assertion that invite fragments never enter HTTP requests, logs, or crash evidence". (Investigator cited 654; sentence spans 653-654.)
- 371 ticket format confirmed; 1082 MDN URI-fragment citation exists exactly as characterized; 34 and 404-410 confirmed.
- MINOR: the fragment-not-on-the-wire claim is at 374-375, not "line 373" as the finding says. Immaterial.

WORKBOX QUOTE — VERBATIM EXACT. GoogleChrome/workbox#488 states word-for-word: "The service worker specification was changed to start exposing hash fragments as part of the `Request.url` that's passed in to the `fetch` event handler." Source names w3c/ServiceWorker#854 as the driver, Firefox 52, Chrome 59 — exactly as the investigator reported.

INDEPENDENT PRIMARY-SOURCE CONFIRMATION (I pulled these myself; a web-search summarizer asserted the OPPOSITE — that Request.url strips fragments — and was wrong, conflating Response.url with Request.url):
- Fetch Standard, current text, curled and grepped: "The url getter steps are to return this's request's URL, serialized." — NO exclude fragment for Request. Contrast, same spec: "The url getter steps are to return the empty string if this's response's URL is null; otherwise this's response's URL, serialized with exclude fragment set to true." — the asymmetry is the spec-level proof that event.request.url retains the fragment.
- HTML Standard "create navigation params by fetching" (browsing-the-web.html): "Let request be a new request, with url = entry's URL ... destination \"document\" ... mode \"navigate\"" — no fragment-stripping step. This closes the specific NAVIGATION case, which the workbox issue (largely about precaching subresources) leaves open. So the /join navigation request genuinely carries the ticket into the SW fetch handler before any page script exists.

NO MITIGATION ELSEWHERE: grep of every "service worker"/"scope"/"kill switch" mention in the doc shows it never constrains SW scope on /join and never requires stripping before SW handling. Line 294 assigns "Remove invite fragments" to the Browser tier generically; line 837 plans `ui/src/sw.ts`.

CORRECTIONS (substance unaffected):
1. Proposed fix says "which is why this route needs the SW kill switch below" — the doc has NO SW kill switch. Line 691's kill switch is for component metadata only. That cross-reference is wrong.
2. Classification "WRONG" is loose and conflicts with the orchestrator's WRONG-vs-MISSING rule. The sentence at 377-379 is not a false statement of fact; the ordering it prescribes is achievable and true. The defect is (a) line 375 presents the control set as sufficient while omitting the SW channel, and (b) the line 654 assertion is mis-scoped. This should be reported as MISSING control + inadequate verification, not WRONG.
3. Line 406 confirms the UI has no service worker today, so this is a design-review finding on unbuilt code, not a live exploitable vulnerability.

SEVERITY: high, not critical. The hostile-CDN framing adds little over what the doc already concedes at line 202 ("origin or CDN compromise can sign or exfiltrate") — a hostile CDN could read the fragment from page JS directly without any SW. The load-bearing, genuinely novel part is the benign case: a routine Workbox-style SW that cache-keys or logs event.request.url silently captures identity-bound tickets into first-party cache/log storage, and the doc's own CI gate at 654 passes while it happens. Real, actionable, well-evidenced — but a first-party storage leak in a proposed design, not attacker-controlled exfiltration of a shipped system.
````

### R31

Candidate finding: TB1: "A compromised origin, CDN account, or frontend dependency controls the browser session. CSP reduces injection risk but cannot make a deliberately malicious first-party build trustworthy." plus the component responsibility "Serve immutable PWA/Wasm assets, public environment config, publisher trust roots, and signed revocation metadata".

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: I read every cited line myself. The literal quoted evidence is real and correctly transcribed, but the finding's load-bearing INTERPRETATION of one cited passage is wrong, and it missed two directly on-point lines. Core gap survives; severity and proposed fix do not.

WHAT I VERIFIED AS ACCURATE
- Lines 275-277: verbatim match for the TB1 quote, including "cannot make a deliberately malicious first-party build trustworthy."
- Line 293: verbatim match for the `app.jeliya.ai` responsibility cell, including "publisher trust roots, and signed revocation metadata."
- Lines 1064-1065: highest-risk unknown #4 is "Browser-origin/CDN compromise and the maximum authority granted to a web controller." Correctly cited.
- Pairing section is exactly lines 327-337. Confirmed it contains no build-identity element: line 332-333 records "the browser public key, granted scopes, expiry, creation time, and last use." The "authenticates the CONTROL KEY, never the CODE" characterization of THAT SECTION is accurate.
- Component-responsibility table is exactly lines 291-297. Correct.
- My own grep of docs/production-deployment.md: "binary transparency" = 0 hits, "subresource" = 0 hits. Confirmed. ("integrity" appears only at lines 166, 204, 839, all unrelated.)
- Line 621-622 no-cache index.html confirmed. The SRI-is-not-the-answer argument is sound: an attacker controlling the CDN rewrites the bundle and the no-cache index.html carrying the `integrity` attribute in the same operation.

CITATION ERRORS
- "line 662" is wrong. Line 662 is "1. Merge to `main` builds the static artifact once." The quoted "CI emits its digest, SBOM, and signed provenance" is line 663. The finding quotes it twice against the wrong line.
- "lines 468-469" truncates the TUF sentence, which begins at line 467 ("Use TUF-like root, targets, snapshot, and timestamp metadata...").

MATERIAL EVIDENCE THE FINDING MISSED
- Line 692-693: "The companion can enforce a minimum-safe control-protocol version, but the web origin cannot rewrite native state or silently elevate scopes." The proposed fix offers "an anti-rollback minimum version" as new; a companion-enforced version floor already exists. Different object (protocol version vs. build digest), so not a full refutation, but the investigator's absolute framing did not survey the doc for it.
- Lines 1043-1044, a stated planning assumption: "The product accepts that a hosted first-party origin can observe the content it renders and actions within its granted scope." This is the doc consciously accepting the residual risk. A MISSING finding that never engages the author's explicit accept-the-risk statement is not fully "correctly read."

THE DECISIVE REASONING ERROR
The finding asserts the doc "already specifies the correct machinery for a DIFFERENT artifact class" (TUF, 467-469) and merely fails to carry provenance to a verifier — "emitted and dropped." That misreads lines 456-469. For Wasm components the verifier is the native host, which RECEIVES THE COMPONENT BYTES and hashes them against the signed manifest (line 459: "Wasm component bytes plus SHA-256 and BLAKE3 digests") before executing. The verifier is EXTERNAL to the artifact and holds the bytes.

The proposed web analogue has no such property. The companion never receives the web bundle's bytes; it receives only what the web shell chooses to send over the control channel. A signed build manifest presented by the build being attested is unauthenticated self-attestation. Under the doc's own TB1 — compromised origin, CDN account, or frontend dependency — the attacker serves malicious JS that replays the legitimate, publicly-fetchable signed manifest of the reviewed build. The Ed25519 signature verifies against the pinned publisher key (the offline/HSM root at line 634 is never needed by the attacker), the anti-rollback minimum version passes, and the companion displays the "correct" build version in the paired-devices UI — an actively misleading assurance surface. The fix therefore does not close the gap it claims to close, and the TUF disanalogy is the reason.

Real controls for this problem are external verifiers that independently hash delivered resources (the Code Verify browser-extension pattern) or append-only binary-transparency logs making a malicious build publicly detectable after the fact — detection and auditability, not handshake-time prevention. Peer attestation of web code via self-reported manifests is not achievable.

WHAT SURVIVES
A narrower, real MISSING: line 663 emits signed provenance that nothing consumes or publishes; there is no transparency log anywhere in the doc (0 grep hits, verified); and TB1 is named as highest-risk unknown #4 with no corresponding mitigation line item in the CI/CD (639-671), cache (619-625), rollback (686-693), or Phase 2 gate (921-930) sections. A secondary real-but-small benefit of a handshake manifest is that it forces an attacker to compromise the genuine origin rather than stand up a lookalike shell at another origin — a phishing control, not a TB1 control, and the finding should say so.

SEVERITY: high -> medium. The gap is inherent to web delivery rather than a negligent omission of a standard control; the doc names it at 1064-1065 and explicitly accepts the adjacent risk at 1043-1044; and the asserted "high" rests on the premise that an available control was dropped, which is false. Worth a paragraph recommending transparency-log publication of the line-663 provenance plus an explicit note that peer-verified build attestation is not achievable — not a blocking architectural hole. The proposed fix must be rewritten before it goes in the review; as written it would ship security theater.
````

### R32

Candidate finding: Phase 2 go/no-go gate: "the companion has no non-loopback TCP or HTTP control listener".

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: the load-bearing evidence is real and correctly read, so the finding survives, but two supporting sub-claims over-read the doc and the severity is inflated.

VERIFIED ACCURATE (all read by me, verbatim):
- docs/production-deployment.md:923 = "the companion has no non-loopback TCP or HTTP control listener;"
- :851 = "`crates/jeliya-companion/` | Signed native service with Iroh control ALPN and no public HTTP listener"
- :295 = "Native companion | ... run native Iroh; enforce paired-client scope ... | Expose public HTTP/WS; give a browser a daemon token; accept an unpaired controller"
- :208 = "Native direct, NAT traversal, and relay remain available"
- :36-37 confirms the control protocol is carried "over a new mutually authenticated, end-to-end-encrypted Iroh control protocol"
- crates/jeliyad/src/main.rs:5-7 = "Local-only by construction: the listener binds `127.0.0.1` and nothing else — there is no flag to bind another interface, so the protocol's \"MUST refuse to bind non-loopback interfaces\" holds trivially." Exact.
- `grep -n "async fn bind_loopback\|Ipv4Addr::LOCALHOST" crates/jeliyad/src/main.rs` -> 364: `async fn bind_loopback(...)`, 371: `SocketAddr::from((Ipv4Addr::LOCALHOST, candidate))`. Function body spans 364-388. Cited range exact.
- The investigator's uncited transport premise independently CONFIRMS: Cargo.lock iroh 1.0.1 dependency list contains `noq`, `noq-proto`, `noq-udp`, `portmapper`, `netwatch`, and `grep -ic quinn Cargo.lock` = 0 (only `quick-xml` matches "qu"). iroh's transport is UDP-based QUIC with NAT port mapping. A TCP-listener predicate therefore provably does not measure the companion's inbound surface. Stronger still: iroh relay paths make the companion reachable via an OUTBOUND connection, so line 923 can be satisfied with zero inbound listeners while the node is internet-reachable.

MISREAD EVIDENCE (the finding's real defects):
1. "the DoS/rate-limit bullets at line 788 are written for relays, not for the companion's inbound endpoint" is FALSE. :785 heads a generic "Abuse controls" section; :788 ("per-IP and per-endpoint handshake, connection, byte, and rate limits") is unscoped, and its siblings are plainly non-relay (:789 owner-enforced invitation limits, :791 event/body/file authoring limits, :792 browser/component/server-peer quotas, :794 pipes/agents, :795 block/report). Only :787 and :793 are relay-scoped. So "the unauthenticated pre-pairing attack surface has no stated limits" is overstated, and the proposed fix's "add per-endpoint inbound handshake rate limits to 787-796" is partly redundant with text already present.
2. Same over-read at :656 — "protocol conformance, fuzzing, and malformed-frame tests" is unscoped and plausibly already covers the companion ALPN; adding it is a clarification, not a gap fill.
3. Also relevant and unmentioned: :337 "Control keys are rate-limited, expire, and can be revoked immediately" (post-pairing, so it does not close the pre-pairing gap, but it shows the doc is not silent on rate limiting).

CLASSIFICATION IS LOOSE: :851 and :923 do not contradict each other — both are consistent statements about TCP/HTTP. The defect is a gate whose wording implies more assurance than it delivers, i.e. an incomplete/protocol-mismatched gate, not an internal inconsistency.

IMPACT IS NARROWER THAN ASSERTED: "would pass on a companion that is fully reachable from the public internet" is literally true, but reachability does not imply unauthorized control — that half is gated at :924 (1,000 pairing/revocation cycles accept no unauthorized controller), :928 (malicious controller cannot invoke files, pipes, agents, or identity reset), :295 (prohibits accepting an unpaired controller), and :329-331 (Noise XX-equivalent transcript + SAS + user confirmation). What genuinely remains ungated is pre-pairing DoS and parser/state-machine exposure on the inbound ALPN.

SURVIVING CORE: a Phase 2 go/no-go gate phrased in TCP/HTTP terms measures nothing about the companion's actual inbound QUIC/ALPN surface, and no gate bounds unauthenticated pre-pairing frames. That is a genuine, actionable wording fix. But with two of three supporting sub-claims over-reading the doc and the authorization half already gated, this is medium, not high.
````

### R33

Candidate finding: Phase 2 go/no-go gate: "a malicious controller cannot invoke files, pipes, agents, or identity reset" (line 928).

Verification reasoning:

````text
REFUTE (high): REFUTED under the accuracy lens. The finding's three evidentiary legs are misread; the flagship example is affirmatively wrong.

WHAT IS ACCURATE (credit where due):
1. Line 928 verbatim: "- a malicious controller cannot invoke files, pipes, agents, or identity reset;" — correct.
2. Lines 990-991 verbatim: "a component cannot access a secret, file, room, network, process, or pipe / without the corresponding import and grant;" — correct, and the investigator reads its structure correctly.
3. The quoted RPC-surface sentence is verbatim present in the doc.

FAILURE 1 — wrong line number (minor). The quoted passage "The RPC surface includes identity creation, daemon shutdown, room history, native file operations, pipes, and agent projections." is at lines 181-182, NOT 183. Line 183 reads "One daemon data directory represents one user identity." Verbatim text is right; the anchor is off by two.

FAILURE 2 — wrong surface (load-bearing, fatal). Lines 181-182 sit under the heading at line 170, "`jeliyad` is local-only by construction:", describing the EXISTING legacy daemon. Lines 191-193 state explicitly: "The production companion control protocol must be designed as a separate surface. Do not add a public-listen flag, proxy `/ws`, or reuse the daemon token remotely." Line 845 reinforces: jeliyad must "Remain a loopback-only legacy/local sidecar; never receive a public bind option." The Phase 2 gate at 928 guards `jeliya-control` / `jeliya-companion` (lines 850-851), a different, newly-designed surface. So the investigator's headline example — "daemon shutdown, for one, is named in the RPC surface and absent from the gate" — imports a capability from a surface the doc deliberately excludes from the design. A browser controller cannot invoke jeliyad shutdown because it never reaches jeliyad. Shipping this example would put a demonstrably false statement in the review.

FAILURE 3 — "no equivalent structural hook" is false. The finding asserts line 928 lacks a Phase-5-style structural precondition. The doc supplies one at lines 332-337, and as an ALLOWLIST, which is strictly stronger than the denylist reading imposed: line 332-333 "The companion records the browser public key, granted scopes, expiry, creation time, and last use."; line 334 "Default scopes cover selected-room reads and idempotent chat sends only."; lines 335-336 "Invite creation, file access, pipes, identity operations, and agents require separate approval." Line 850 specifies `crates/jeliya-control/` as "Pairing transcript, scoped RPC, nonce/counter replay protection, and revocation". Line 295 requires the companion to "enforce paired-client scope" and prohibits it from "accept[ing] an unpaired controller". Under default-deny scoping, the gate's enumeration need not be exhaustive — anything outside "selected-room reads and idempotent chat sends" is denied by construction. Line 928 reads naturally as a targeted regression test over the four highest-value capabilities named at 335-336, not as a complete denylist. The premise "the surface it guards is large" collapses once the correct surface is identified.

FAILURE 4 — the "no stated test method" asymmetry does not hold either. Line 990-991, the investigator's exemplar of a well-formed gate, also states no test method. Neither gate does. The section's convention throughout (lines 904-910, 944-950) is terse assertions. There is no asymmetry between 928 and 990-991 on this axis.

WHAT SURVIVES: a genuine but small drafting nit. Line 990-991 restates its precondition inline ("without the corresponding import and grant") while line 928 does not restate "outside its granted scope", leaving 928 self-contained-ambiguous even though the doc resolves it 600 lines earlier. Appending "outside its granted scope" to 928, or adding a positive allowlist-conformance assertion, would tighten it. The proposed generated-negative-test-matrix fix is reasonable engineering advice on independent merits. But that is a low-severity wording suggestion on a phase gate in a planning proposal — not a MISSING/high defect. Classification should change from MISSING to a judgment/wording nit, and the daemon-shutdown example must be struck entirely.
````

### R34

Candidate finding: Phase 0 go/no-go gate: "no contradictory release claim remains" (line 883).

Verification reasoning:

````text
REFUTE (high): ACCURACY lens: the finding's primary citations are real, but two of the four load-bearing evidentiary sub-claims are misread, and they are precisely the ones that carry MISSING+high.

WHAT CHECKS OUT (verified myself):
- Doc line 883 is verbatim `- no contradictory release claim remains;` under `Go/no-go gate:` at line 881. Quote and line number exact.
- The `check-docs.mjs` greps are real: `errors.push` at 201, 209, 216, 222, 229, 243; `'document-orphan'` at scripts/check-docs.mjs:968 with `'document is not reachable from docs/index.md'` at :969. Correctly transcribed.
- `check-docs.mjs` genuinely contains no cross-document claim-consistency analysis: no reference to capability-status.md or security-threat-model.md anywhere in scripts/, and no SHA/status cross-comparison.
- The doc's contradiction enumeration is grounded in real repo state, so the investigator was right to lean on it: docs/capability-status.md:53-54 assert certified candidate direct/relay runs while :50 says "the rc.3 candidate on `main` fixes this and has no network run yet" and :56 says "a fresh signed schema 2 qualification is required"; docs/security-threat-model.md:43-44 says "immutable safe repinning ... all now complete at this pin" while :122-124 says "Because the public Jeliya lockfile does not yet resolve that code, upstream publication and Jeliya repinning are mandatory before release qualification."

WHAT IS MISREAD:
1. "no scope (which documents?)" is WRONG. The gate bullet at 883 pairs with the Deliver bullet directly above it at docs/production-deployment.md:873: `- reconcile status, threat, evidence, and platform documentation;`. That names the scope as four document classes. The investigator read the gate bullet in isolation from the Deliver list it belongs to (871-879).
2. "no automated backing ... nothing in CI can decide this gate" is WRONG as stated. `.github/workflows/ci.yml:59` runs `node scripts/check-release.mjs --source`, and scripts/check-release.mjs:112-148 performs exactly the kind of release-claim consistency check the finding says does not exist: it reads `docs/verification-evidence.md` (:120), fails on `implementation_status`/`verification_status` mismatch (:121-126), fails if `| Release evidence gate | READY |` is absent (:127-129), fails if the Candidate identity section still contains "pending" (:130-132), and extracts the 40-hex "Network-qualified commit" and "Candidate upstream remediation revision" (:141-142) to bind them to the signed evidence manifests. Partial automated backing exists and runs in CI. This also means the proposed fix's "optionally add scripts/check-claims.mjs that cross-checks the qualified-commit SHA ... across ... verification-evidence.md" partly duplicates shipped machinery the investigator did not look for.
3. "scripts/check-docs.mjs validates only the front-matter profile" is FALSE and self-contradicting — the same sentence then cites the orphan-reachability check. The file also validates link resolution (validateLinks at :717), anchor targets (markdownAnchors at :469), raw HTML (validateRawHtml at :681), UTF-8 encoding (:83-89), and title/heading match (:663-671). Not load-bearing (none are claim-consistency checks), but it shows the file was skimmed, not read.
4. Minor: cited "lines 68-72"; the sentence starts at :67. And "three specific contradictions" over-counts — the passage names two documents, and the proposed fix adds "platform-matrix.md ..." which that passage does not mention.

RESIDUE: the only sub-claim that fully survives is "no decider" — I grepped for decider/sign-off/maintainer/accountable/RACI across the doc and found nothing governing the gates. That is a genuine but minor gap in a roadmap document, worth at most a one-line note, not a high-severity MISSING finding asserting an unscoped, unenumerated, unbacked gate.
````

### R35

Candidate finding: Service objective: "99 percent of accepted chat events visible to both online peers within 10 seconds" (line 758).

Verification reasoning:

````text
REFUTE (high): EVIDENCE IS REAL BUT MISREAD. All seven quoted strings exist verbatim in the doc, though three line cites are off by one: "message bodies or event payloads" is line 727 (cited 726), "room IDs" is line 728 (cited 727), "identity, device, or endpoint IDs, including shortened values" is line 729 (cited 728-729). Lines 719, 679, 735, 746 are cited correctly. The negative claim also holds: grep -ni "canary|synthetic|probe" over docs/production-deployment.md returns zero hits, so the doc genuinely names no measurement method.

THE INFERENCE FAILS. The finding's load-bearing premise is that "establishing that a specific event authored by peer A became visible to peer B requires correlating two clients' reports," from which it concludes the SLO is unmeasurable because every correlator is forbidden. In this codebase no correlator is needed — the author timestamp travels inside the event itself: crates/jeliya-core/src/lib.rs:53 defines now_ms(); crates/jeliya-core/src/supervisor.rs:841 stamps `let created_at = now_ms();` at authoring; crates/jeliya-core/src/materializer.rs:83 emits `obj.insert("ts".into(), json!(ev.created_at));` to the receiving client. A receiving peer therefore computes (local_render_time - ev.created_at) purely from local state and reports a bucket into the "result/error code and latency histogram" that line 719 explicitly ALLOWS. No room ID, identity ID, payload, or stable cross-session identifier is attached. The "unlinkable session ID" of line 746 is irrelevant because nothing needs linking.

THE FINDING CONTRADICTS ITSELF. Its own resolution (a) concedes the receiving-peer histogram is "allowed." A finding cannot simultaneously assert "the SLO is not measurable with the telemetry the document permits" and enumerate a permitted measurement. Once (a) is admitted, the objection collapses to two residuals, neither of which is an internal inconsistency: (1) clock skew — a measurement-accuracy caveat universal to distributed latency metrics, not a conflict with the privacy section; (2) "cannot establish the other peer was online" — weak, since the receiver's own continuous-connection state is local knowledge it can bucket on, and the author sees its own event by local echo.

CLASSIFICATION IS ALSO WRONG. INTERNALLY-INCONSISTENT requires two doc assertions that cannot both be true. Here the convergence objective (758) and the telemetry restrictions (725-735) are simultaneously satisfiable via the line-719 histogram. Line 764 ("These are launch objectives to measure during beta") frames the table as objectives to operationalize later, and lines 767-769 already carve out the all-peers-offline case — so the doc never asserts the SLO is measured by any forbidden mechanism.

RESIDUAL VALUE. A much weaker note ("line 758 could name its measurement method and caveat clock skew") could be written, but it is a nice-to-have on an objectives table in an architecture proposal, not a defect. As written and classified at high severity, the finding does not survive the accuracy lens.
````

### R36

Candidate finding: The comparison table (line 214) presents "Approximately 16 to 24 engineer-weeks" for the Static-PWA-with-Wasm-peer path and "Approximately 11 to 17 engineer-weeks" for the recommended hybrid under the same "First safe production" row.

Verification reasoning:

````text
REFUTE (high): The finding's evidence does not say what it claims, on the load-bearing point.

(1) COLUMN MISATTRIBUTION. Line 200 header: `| Dimension | Static PWA with Wasm peer | Hosted shell with native companion | Hosted gateway or managed backend | Capability-aware hybrid |`. Line 214: `| First safe production | Approximately 16 to 24 engineer-weeks | Approximately 11 to 17 engineer-weeks | At least 24 weeks | First companion slice in 11 to 17 weeks; browser mode follows |`. The bare "Approximately 11 to 17 engineer-weeks" belongs to column 3, "Hosted shell with native companion" — NOT the recommended hybrid (column 5). The recommended hybrid's actual cell reads "First companion slice in 11 to 17 weeks; browser mode follows". The finding's premise is that "one row header invites a direct 16-24 vs 11-17 comparison" with the partial scope undisclosed; in fact the hybrid's own cell in that row states the number covers only the companion slice and that browser mode comes later. The disclosure the finding says is missing is inside the cell it cites.

(2) THE FINDING CONTRADICTS ITS OWN CITED LINES. It asserts the doc "never discloses the 21-31 total", citing line 46 for the 11-17 and line 954 for the 10-14 as though they were scattered across the document. Lines 45-48 actually read: "the companion-backed production slice is estimated at **11 to 17 engineering weeks**. A robust browser-only peer adds approximately **10 to 14 weeks**. These are planning estimates, not release commitments." Both addends sit adjacent in the executive summary, and the second is explicitly labeled an *add* on top of the first. Only the literal sum "21 to 31" is never written.

(3) THE PROPOSED FIX ENCODES THE SAME ERROR: "16-24 / not applicable / not applicable / 21-31" only parses if 11-17 had been the hybrid's cell; under the real header order the companion column is one of the two marked "not applicable" despite being the path that number describes.

CLAIMS I CHECKED AND CONFIRMED ACCURATE: phases 0-3 sum exactly to the table's 11-17 (line 869 "Phase 0 ... 1 to 2 weeks", line 890 "Phase 1 ... 3 to 5 weeks", line 912 "Phase 2 ... 5 to 7 weeks", line 932 "Phase 3 ... 2 to 3 weeks"), and line 952 "This is the first production launch gate" confirms the boundary — so the 11-17 is internally consistent, as the finding says. Line 954 is "### Phase 4: browser peer and multi-device identity, 10 to 14 weeks". Line 221 does read "A browser peer remains the intended zero-install capability". The Decision section (216-224) is 9 lines and does not restate week counts. `grep -n "16 to 24"` returns only line 214.

RESIDUAL: the doc never performs the addition 21-31 in a single place. Against a hybrid cell that already says "browser mode follows" (line 214) and a summary sentence that already says the browser peer "adds approximately 10 to 14 weeks" (lines 46-47), that is an editorial nicety, not a material undisclosed trade-off. Not a MISSING finding, and nowhere near high severity. Recommend drop; at absolute most a low-severity presentational suggestion, and it would have to be rewritten from scratch since its stated premise and fix are both built on the column misread.
````

### R37

Candidate finding: Phase 0 deliverable: "confirm DNS, CDN, relay, and signing ownership" (line 879), with signed macOS/Windows packages due in Phase 2 (line 918).

Verification reasoning:

````text
UPHOLD (high): The finding's load-bearing structural claim is fully verified; one corroborating external quote is misattributed and misquoted and MUST be corrected before publication, but the error runs against the investigator's own calibration and does not undermine the defect.

VERIFIED ACCURATE (read directly in /home/sekou/AGI/jeliya/docs/production-deployment.md):
- L879 Phase 0 deliverable: "confirm DNS, CDN, relay, and signing ownership." Correct.
- L883-888 Phase 0 go/no-go gate is exactly five bullets (no contradictory release claim; CI twice on one SHA; signed direct/forced-relay evidence bound to that SHA; browser reaches native endpoint via authenticated relay; upstream #121 not exploitable+unmitigated). None mentions signing, DNS, CDN, or relay ownership. The deliverable/gate asymmetry is real.
- L918 Phase 2 deliverable "signed macOS and Windows packages and a verified Linux package." Correct.
- L1038-1039 and L1068 quoted correctly verbatim.
- ADDITIONAL corroboration the investigator missed, strengthening the finding: L561-562 specifies "Apple Developer ID/notarization, HSM-backed Windows Authenticode such as Azure Trusted Signing" (so the Azure Artifact Signing FAQ is the correct service to cite); L930 gates signature verification only at the END of Phase 2, i.e. the first hard check lands after the work is done. And docs/signing-notarization.md (140 lines, referenced from L158) has zero occurrences of enroll/D-U-N-S/lead time/prerequisite/business day — so no other repo artifact gates procurement either. The MISSING classification holds.

APPLE URL — CONFIRMED VERBATIM. Fetched https://developer.apple.com/help/account/membership/D-U-N-S/. All three quoted sentences appear exactly: "After requesting a D-U-N-S Number, please allow up to 5 business days to receive your number from D&B."; "Expediting your D-U-N-S Number creation process will not shorten this waiting period."; "If your application has taken longer than two weeks to process, please email D&B."; "Once you receive your D-U-N-S Number, please allow up to 2 business days for Apple to receive your information from D&B."

MICROSOFT URL — TWO OF THREE QUOTES CONFIRMED, ONE REFUTED. I fetched the rendered page AND the raw source (https://raw.githubusercontent.com/MicrosoftDocs/azure-docs/main/articles/artifact-signing/faq.yml, 26,777 bytes) and grepped it.
- CONFIRMED verbatim, faq.yml line 81: "Note: creating more identity validation requests for the same entity that is in progress doesn't help. Identity validation requests can't be expedited."
- CONFIRMED verbatim, faq.yml line 52: "For Public Trust certificates, Artifact Signing is currently available to organizations in the USA, Canada, the European Union, and the United Kingdom, as well as individual developers in the USA and Canada. This limitation isn't applicable to Private Trust certificates."
- REFUTED: "identity validation request takes from 1 to 7 business days (possibly longer if we need to request more documentation from you)" attributed to the FAQ "Identity validation section". `grep -iE "business day|[0-9]+ days|weeks"` over the raw FAQ returns only an unrelated line about 60-day renewal reminders. The FAQ states no processing duration anywhere. The actual sentence is on a DIFFERENT page — https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart, "Important information for public identity validation" table, Processing time row: "Processing your identity validation request takes from 1 to 20 business days (possibly longer if we need to request more documentation from you)." The parenthetical is verbatim correct, but the number is wrong (7 vs 20) and the URL is wrong. WebSearch indicates "1 to 7 business days" circulates only in Microsoft Q&A community answers, not Learn docs.

WHY THIS DOES NOT REFUTE: the misquote is corroborating color for severity, not the basis of the defect. The defect is established entirely by doc lines 879 vs 883-888, which I read directly. Moreover the correction cuts against the investigator: 1-to-20 non-expeditable business days is up to ~4 calendar weeks, versus a Phase 0 budgeted at "1 to 2 weeks" (L869). So the investigator's CALIBRATION sentence ("days-to-a-few-weeks and largely parallelizable") understates it, and the schedule risk is real rather than nominal.

ALSO UNSUPPORTED (fix text, not evidence): the proposed fix cites "the organization-history requirement Microsoft applies." No organization-age/history requirement exists in the current FAQ or quickstart; the quickstart only requires submitted documents be "issued within the previous 12 months." This appears carried over from older Trusted Signing docs. Drop that clause.

FIX-QUALITY CAVEAT: requiring enrollment "COMPLETED, not merely requested" as a Phase 0 EXIT gate is in tension with the stated 1-2 week Phase 0, given up to 20 business days for Microsoft alone. Better framing: gate Phase 0 exit on eligibility confirmed + enrollment SUBMITTED, and gate Phase 2 ENTRY on completed enrollment plus a throwaway binary signed and notarized end to end.

SEVERITY: medium is correct. The defect is a verified gating omission with a genuine plan-invalidating branch nothing checks (the jurisdiction eligibility hard stop, which forces an alternative CA and changes the Phase 2 plan). Not high, because the doc does acknowledge the risk twice (L1038-1039, L1068) and Phase 2 cannot start before week 4 (Phase 0 1-2wk + Phase 1 3-5wk), giving natural slack that absorbs most of the lead time. Not low, because no artifact in the repo gates it and the eligibility check is binary, not a delay.
````

### R38

Candidate finding:

````text
Service objective: "Companion pairing | At least 99 percent success on the supported OS/browser matrix" (line 759).
````

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: every piece of cited evidence is real and correctly read. I verified each quote against the file directly.

VERIFIED VERBATIM:
- Line 759 reads exactly: `| Companion pairing | At least 99 percent success on the supported OS/browser matrix |`. Quoted correctly.
- Line 715 reads exactly `Allowed aggregate metrics are:` — the investigator's characterization "phrased as exhaustive" is a fair reading of "Allowed X are:" followed by a closed list.
- Lines 717-723 are quoted accurately, item for item: 717 `frontend build and runtime version;`, 718 `capability mode, such as companion or browser peer;`, 719 `result/error code and latency histogram;`, 720 `byte-size buckets;`, 721 `relay region and aggregate direct/relay ratio;`, 722 `storage/quota state bucket;`, 723 `first-party component name/version when components are enabled.` The investigator truncated 723's trailing conditional, which is immaterial to the argument.
- "Operating system and browser family/version are not among them" — CONFIRMED. None of the seven items carries an OS or browser dimension. The nearest candidate, line 718 `capability mode, such as companion or browser peer`, is a capability role (companion vs. browser peer), not a browser family or OS; it cannot slice a pairing success rate by the OS/browser matrix.
- The investigator's careful disclaimer that this is NOT a conflict with the never-attach list is also correct: I read lines 725-735 and no entry there mentions OS, browser, or platform. The finding correctly distinguishes itself from the line 758 case.

INDEPENDENT CHECKS THAT COULD HAVE REFUTED IT, BUT DID NOT:
1. I grepped for any second or alternative allow-list (`grep -n -i "allowed\|aggregate\|platform\|fingerprint"`). Line 715 is the document's only allow-list. No other section rehabilitates a platform dimension. So there is no rescuing text elsewhere.
2. I checked whether the doc scopes this objective to CI rather than production, which would have made the allow-list irrelevant. It does the opposite: line 764, immediately under the table, reads "These are launch objectives to measure during beta." That is production/beta measurement, which is exactly what the "Privacy-safe observability" allow-list at 715 governs. This line STRENGTHENS the finding — the investigator did not cite it and could have.
3. I checked the other "matrix" occurrences to see whether the matrix is defined as a pre-release gate only. Lines 966-967 use the supported matrix as a Phase 4 go/no-go gate, and line 1056 lists "Supported browser, desktop OS, and mobile matrix" as an undecided ADR. Neither converts line 759's beta-measured SLO into a CI-only acceptance criterion, so the gap stands. (These do make the investigator's alternative fix — restate 759 as a CI acceptance criterion — a coherent option.)

SEVERITY CORRECTION: medium is overstated. The defect is real, verifiable, and actionable, but its blast radius is one missing bullet in a proposal document's allow-list. It carries no security, correctness, or architectural consequence; nothing downstream is built wrong because of it; and the doc itself flags the matrix as an unresolved ADR (line 1056), so the observability dimension would naturally be settled alongside it. It does deserve to appear in the review — an explicitly exhaustive privacy allow-list that cannot express one of the document's own stated SLOs is a genuine internal tension a reviewer should resolve — but it is a low-severity documentation gap, not a medium one.

Minor taxonomy note (not grounds for refutation under my lens): the classification INTERNALLY-INCONSISTENT is defensible but borderline; the investigator's own wording, "a gap in the allow-list," reads closer to MISSING. Either label points at the same real text.
````

### R39

Candidate finding: Phase 3 go/no-go gate: "external TLS/header/CSP assessment passes" (line 944).

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: every literal quotation and line number in the finding's evidence is real and correctly read. The core claim survives, but three characterizations are overstated, and the severity is inflated.

VERIFIED ACCURATE (I read each cited line myself in /home/sekou/AGI/jeliya/docs/production-deployment.md):
- Line 944 is verbatim `- external TLS/header/CSP assessment passes;` — quoted correctly.
- Line 947 `- N-to-N-1 rollback completes within 15 minutes;` — "within 15 minutes" confirmed at the exact line cited.
- Line 948 `- a regional relay outage fails over within 2 minutes;` — confirmed at the exact line cited.
- Line 924 `- 1,000 automated pairing/revocation cycles accept no unauthorized controller;` — confirmed at the exact line cited.
- Header set at 607-612 — exactly right: 607 Strict-Transport-Security through 612 Permissions-Policy, six headers, no off-by-one.
- Smoke tests at 675-684 — exactly right: 675 "DNS, certificate, HTTP redirect, and security headers" through 684.
- "No tool, grade, or scoring system is named" — CONFIRMED independently. `grep -niE "ssl labs|observatory|qualys|testssl|hardenize|securityheaders|grade|score|scan"` over the whole doc returns only three lines (162, 492, 983), and all three are false positives where "grade" matched inside "upgrade". Zero external graders, scans, or scores are named anywhere in the 1087-line document. The gate genuinely has no defined pass criterion.

OVERSTATEMENTS FOUND (accuracy defects, but not disqualifying):
1. "the CSP is fully specified at lines 584-598" — NOT fully specified. Line 589 reads `connect-src 'self' https://relay-auth.jeliya.ai https://<relay-hosts> wss://<relay-hosts>;` — `<relay-hosts>` is an unresolved placeholder. Also 584-598 is off by one at the tail: 583 opens the ```text fence, content runs 584-597, 598 is the closing fence.
2. "an exact-match assertion is trivially writable" / proposed fix's "byte-for-byte" — not literally achievable as written. Beyond the placeholder, lines 600-602 anticipate adding a component origin to `frame-src`, and line 615 says "Add COEP only if Wasm threading requires cross-origin isolation" — the header set is deliberately conditional and evolving. The assertion is mechanizable only after deploy-time values are bound, which is weaker than "trivially".
3. "unusual for this document" — materially undercut. Three sibling gates share the identical defect, one of them structurally verbatim: line 989 `- sandbox escape and confused-deputy review passes;` (same "X review passes" shape, Phase 5), line 910 `- independent security review approves the wire formats and key lifecycle;` (Phase 1), and within Phase 3's own gate, line 949 `- load tests stay inside resource and cost ceilings;` is equally unquantified — I checked the cost model at lines 798-826 and it gives estimates ("Approximately $400 to $600") but never defines a ceiling. So 944 is not the lone soft bullet even inside the phase it belongs to.

SEVERITY: medium → low. MISSING implies a requirement the doc omits, but the substance of the gate is recoverable from the document itself: line 575 "Permit TLS 1.2 and 1.3 only", line 574 "Enable DNSSEC and restrictive CAA records", lines 576-577 and 607 for HSTS, the CSP template at 584-597, the header set at 607-612, and line 675 already puts "DNS, certificate, HTTP redirect, and security headers" in the production smoke tests. Nothing material is absent — what is missing is the binding of the gate wording to specs the doc already states. That is a wording/rigor tightening, not an omitted requirement. The one factor arguing upward is line 952, "This is the first production launch gate," which raises the stakes of imprecision at Phase 3 specifically; that keeps it above 'drop' but not at medium. A calibrated review should report this as the pattern across 944/949/989/910 rather than singling out 944.
````

### R40

Candidate finding: The document's cost model (lines 798-828) covers infrastructure only and omits the external penetration review that is a HARD launch gate at line 950.

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: every piece of stated evidence is real and correctly read. I verified each independently.

QUOTES — all verbatim, no paraphrase drift:
- L950: "- an external penetration review has no unresolved critical or high finding." EXACT.
- L952: "This is the first production launch gate." EXACT.
- L910: "- independent security review approves the wire formats and key lifecycle." EXACT.
- L198: "engineer at least part-time, and an independent security review." EXACT; correctly characterized as a *planning input*, since L197 reads "Planning estimates assume". The investigator did not misrepresent this as a budget line.
- L811: "| Initial fixed total | Approximately $400 to $600 plus relay bandwidth |" EXACT.

COUNTS — `awk NR 804..811 | grep -c "^|"` = 8 pipe lines = header (804) + separator (805) + 6 data rows (806-811). Investigator's "six rows" is correct, and the enumeration (DNS/CDN, relays, Worker, object storage, monitoring, total) matches 806-811 exactly. "None is a security review" holds; note L810 "Privacy-reviewed monitoring" contains "reviewed" but is a monitoring line, correctly excluded.

NEGATIVE GREPS — confirmed. "penetration" appears exactly once in all 1087 lines, at L950. Zero hits for "pen test"/"pentest". "audit" hits only L114 (CI dependency-audit job), L184 (public audit model), L527 (audited patch) — none a cost. All dollar amounts in the doc are at L800-814, all infrastructure. No other section prices the reviews: the Planning-assumptions block (L1034-1044) lists obtainables (e.g. L1038 Apple/Windows signing services) but never a security review; Highest-risk unknowns (L1058-1071) prices relay bandwidth economics (L1069) but no review.

ARITHMETIC — $400x12=$4,800, $600x12=$7,200. The "~$4.8k-$7.2k/yr" figure is correct.

Nothing in the evidence is fabricated, misquoted, or misread. The finding is NOT refuted.

SEVERITY CORRECTION critical -> medium. Three things the finding's framing understates, none of which touch evidence accuracy:
1. Scope-by-construction. L798 titles the section "Initial monthly cost model" and L804's column header is "Monthly starting estimate". A one-time pen test is out of scope for a recurring-cost table by design, so this is incompleteness in the doc's overall budgeting, not a defect in the table. The investigator's fix anticipates this ("or a new one-time cost table"), so the substance survives — but the omission is a scoping choice, not an oversight.
2. The gate is not invisible. L197-198 explicitly names the independent security review as a resource the plan assumes. The proposed fix's rationale — "a hard gate with no budget is a gate that will be quietly dropped" — is rhetoric, not evidence; the requirement is stated twice as a hard gate (910, 950) and once as a planning input (198).
3. Consistent doc-wide pattern. L1038 assumes the team "can obtain Apple and Windows signing services" — also a real recurring paid external cost, also unpriced. The pen test is not singled out for special neglect.

It remains a genuine, well-evidenced gap worth reporting: external pen tests routinely cost multiples of this doc's entire annual infra estimate, so a reader taking "Initial fixed total ~$400 to $600" as the launch budget will materially under-plan. That is medium. "Critical" would require the doc to claim total launch cost (it does not) or to contain a false statement (it does not — this is correctly classified MISSING, not WRONG).
````

### R41

Candidate finding: The cost model excludes the 2-3 engineers it commits for 11-17 weeks, making the stated total off by roughly two orders of magnitude.

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: I independently read every cited line and re-ran every grep. The stated evidence is real and correctly transcribed, so the finding is not refuted outright — but its headline characterization misreads the artifact and the severity is badly inflated.

VERIFIED ACCURATE:
- Line 798 is exactly "### Initial monthly cost model".
- Line 811 is exactly "| Initial fixed total | Approximately $400 to $600 plus relay bandwidth |".
- Lines 45-46 (investigator cited 46-48; the sentence starts at 45) read verbatim "For a small team of two to three engineers, the companion-backed production / slice is estimated at **11 to 17 engineering weeks**."
- Lines 197-198 (cited as 198) read verbatim "Planning estimates assume two core/full-stack engineers, one web/operations / engineer at least part-time, and an independent security review."
- Grep counts re-run and confirmed 0 for: salary, labor, "people cost", headcount, staffing, hire, "one-time", "up-front", capital. I added "fully loaded" (0) and "burn" (0). Note: "FTE" superficially returns 10, but all 10 are the substring inside "after" — false positive, not counter-evidence.
- The gap is genuine doc-wide: I enumerated all 50 headings (no delivery/effort-cost section exists) and read `### Planning assumptions` (1034-1044) and the ADR list (1046-1056), the two places a labor-cost caveat would plausibly live. Neither mentions it.

WHERE IT OVERREACHES (why severity drops):
1. "making the stated total off by roughly two orders of magnitude" misreads line 811. The doc never presents $400-600 as a program total. Line 798 says "Initial MONTHLY cost model", line 804's column header is "MONTHLY starting estimate", and the row is labeled "Initial FIXED total". A monthly recurring infrastructure figure is not "off" by any factor — it is correctly scoped and merely narrowly titled.
2. The engineering commitment is far from hidden. It is stated prominently four separate times: lines 45-48 (inside `## Executive decision`, the doc's opening section), lines 197-198, line 214 as a first-class comparison-table row ("First safe production | ... | Approximately 11 to 17 engineer-weeks | ..."), and in every phase heading from 869 to 977 ("1 to 2 weeks" ... "8 to 16 weeks"). The asserted risk — "a funding decision made against the $400-600 figure alone" — requires a reader who ignores the word "monthly" twice over and four prominent effort statements.
3. Minor: the claim "the word 'cost' in the roadmap refers only to infrastructure" has one partial exception the investigator missed — line 814, "but move availability and on-call cost to the team" — which does gesture at human cost, though never monetized. Does not refute, but shows the sweep was not exhaustive.

SURVIVING RESIDUE: the doc genuinely never converts engineer-weeks into money anywhere, and the cost section's title omits "infrastructure". That is a real but modest editorial clarity gap; architecture proposals commonly quote effort in weeks and leave rate-loading to the reading org since loaded rates vary widely. The proposed retitle to "Initial monthly INFRASTRUCTURE cost model" is the right cheap fix and worth keeping in the review — as a low-severity clarity item, not a critical omission.
````

### R42

Candidate finding: No error-state or empty-state design exists for any new surface, contradicting an accepted decision record and a stated product principle that already bind this codebase.

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: the load-bearing evidence is real and correctly read, so the finding is not refuted — but it is materially overstated and must drop from critical to medium.

VERIFIED ACCURATE (I ran/read each):
1. All nine grep counts reproduce as exactly 0 against docs/production-deployment.md ("error state", "empty state", "user sees", "what the user", "error message", "user-facing", "loading", "spinner", "retry").
2. docs/room-workbench.md:297 = "## Decision 5 — truthful states per destination". The quote "Every destination defines all six. A destination that renders its empty state while it is actually loading, offline, or unauthorized is lying" is verbatim at 299-300. The six-row table is at 306-311 (Empty/Loading/Offline/Stale/Failed/Unauthorized) — matches the finding's list exactly.
3. PRODUCT.md:70-72 is verbatim principle 5: "Every state is designed... Each gets a deliberate presentation — including empty and failure states." (Note: file is at repo root; docs/PRODUCT.md does not exist — the finding's path was written without the docs/ prefix, so it is correct.)
4. "The doc never links" either record: confirmed, grep -n -i "room-workbench\|PRODUCT.md\|Decision 5\|design principle" over the doc returns zero hits (exit 1).
5. Binding force is real: room-workbench.md:24 "This document is normative for clients", and the doc's own change map at line 840 claims "Browser control keys, SAS pairing, scopes, expiry, and revocation UI" — so UI states are in the doc's self-declared scope, not purely out of band.
6. The substantive core survives: Loading, Empty, and Stale are genuinely absent. "loading"=0; "stale"'s only 2 hits (lines 67, 70) are about stale *documentation*, not data-freshness labelling; "unauthorized"'s single hit (line 924) is a pairing test criterion, not a UI state.

WHERE THE FINDING MISREADS ITS OWN EVIDENCE (severity-reducing, found by reading not grepping):
a. "No error-state or empty-state design exists for ANY new surface" is too absolute. Real degraded-state directives exist: 436-438 "Maintain an eviction sentinel. Missing critical state stops authorship and offers recovery import or peer resynchronization. Never silently create a new identity"; 439 "Explain that unreplicated local blobs disappear if browser storage is lost"; 431 "display storage estimates"; 294 forbids claiming "durable storage when unavailable"; 206 "truthful limited offline behavior".
b. The proposed fix's "At minimum specify what the user sees when the companion is not running" is ALREADY satisfied at 421-422: "Mark files, pipes, membership actions, and agents unavailable while the companion cannot be reached." The finding cites 421 as an undesigned condition when it is in fact the doc's answer to precisely the gap the fix demands.
c. Same self-undermining pattern on 3 of the 6 cited "conditions": 683 is "quota warnings and storage recovery" (doc names the warning), 946 is "offline shell and cached view open during origin outage" (doc specifies the behavior), and 761 is an SLO table row "| Relay regional failover | At most 2 minutes |" — an availability objective, not a UI condition at all.
d. "The only user-facing bullet in the slice is line 1011" is wrong on plain reading — bullets 1002-1013 (recovery kit, SAS pairing, cached room view, offline drafts) are all user-facing.
e. Of the five "new surfaces" at 837-841, only ui/src/pairing/ is clearly UI-bearing per the doc's own wording; ui/src/sw.ts and ui/src/runtime/ are non-UI infrastructure and are not "destinations" Decision 5 governs.
f. Convention counter-evidence: the repo's other two type:"Architecture" docs (agent-marketplace.md, security-threat-model.md) also do not reference room-workbench.md. Only Decision/Runbook/index/PRODUCT/DESIGN docs do. So non-linking is normal for this document type rather than an anomaly, and the doc is explicitly a "proposal" that (lines 22-25) "does not authorize a production deployment by itself" with statuses advancing independently — detail deferral is structurally expected.

NET: a legitimate MISSING finding — Loading/Empty/Stale are absent and the release-defining vertical slice at 998-1015 contains no state design — but it is one gap in a proposal with partial coverage already present, against a doc type that conventionally defers UI states. Critical is reserved for wrong/unsafe/deployment-breaking claims; this is none of those. Medium. The review should also drop citations 761/946/683 and the "only user-facing bullet" line, since those points collapse on inspection.
````

### R43

Candidate finding: The document sets 99.9%/99.95% availability objectives and a 2-minute relay failover target with no on-call rotation, alerting, escalation, or status page.

Verification reasoning:

````text
REFUTE (high): ACCURACY LENS VERDICT: the literal greps are all reproducible, but the finding's load-bearing inference is contradicted by the doc's own cited source, and its characterization of the doc depends on four sections the investigator never mentions. Refuted as written.

WHAT I VERIFIED AS ACCURATE (evidence is real and correctly read):
1. Lines 754-762 are quoted verbatim and correctly. Confirmed by reading: L756 "Static shell | 99.95 percent monthly availability", L757 "Relay authentication plus at least one relay | 99.9 percent monthly availability", L760 "Frontend rollback | At most 15 minutes", L761 "Relay regional failover | At most 2 minutes".
2. I re-ran the greps over /home/sekou/AGI/jeliya/docs/production-deployment.md and got byte-identical counts: alert 0, pager 0, oncall 0, escalation 0, burnout 0, staffing 0, "time zone" 0, vacation 0. Also confirmed my own additions: SLO 0, PagerDuty 0, uptime 0, threshold 0, "error budget" 0, 5xx 0.
3. "on-call" appears exactly once, L814: "but move availability and on-call cost to the team" — correctly characterized as a cost trade-off.
4. "status page" appears once, L67: "Some status pages contain stale contradictions: the top of capability status records fresh candidate direct and relay runs..." — correctly read as documentation status, not operational.
5. L810 "| Privacy-reviewed monitoring | $0 to $150 |" exists as stated.
So the residual gap is genuine: there is no alert definition, threshold, routing, on-call roster, escalation path, or public status page anywhere in 1087 lines.

WHY IT STILL FAILS THE ACCURACY LENS:

(a) The load-bearing inference is REFUTED. The finding asserts "A 2-minute failover objective is unattainable without paging." The doc's architecture is two-region STATELESS relays — L251-253 "Dedicated Iroh relays / at least two regions / stateless, not members", and L775 "Relays are stateless and need configuration recovery, not room-data backup." I fetched the doc's own cited reference at L1076 (https://docs.iroh.computer/add-a-relay), which states: "Iroh's relay architecture is uniquely suited to multi-relay deployments because relays are stateless" and "Clients automatically fail over between relays in your list, so adding capacity or surviving an outage is just a matter of running more relay processes" and "iroh clients try multiple relays automatically, so if one becomes unreachable they'll seamlessly fall back to another." Failover here is client-side and automatic; the 2-minute figure is a client re-homing budget, not a human response budget. No pager is required to meet it. The same applies to L757's "at least one relay" objective, which is satisfied by architecture rather than by human response.

(b) "Monitoring appears only as a cost row: line 810" is a keyword-grep artifact that misrepresents the document. The WORD "monitoring" appears once, but L713-748 is a 36-line section "## Privacy-safe observability" enumerating the exact allowed aggregate metrics (L717-723: build/runtime version, capability mode, "result/error code and latency histogram", byte-size buckets, "relay region and aggregate direct/relay ratio", storage/quota bucket) and the banned fields (L725-735). Claiming monitoring appears only as a cost row is not a correct reading.

(c) The MISSING classification omits four sections it depends on being absent: L639 "## CI/CD, smoke testing, rollback, and incident response"; L695-711 "### Incident runbooks" listing 8 exercised scenarios with named responses ("frontend rollback, browser-control-key revocation, relay-token rotation, component revocation, signing-key rotation..."); L859 the `docs/runbooks/` deliverable "Deployment, rollback, relay failure, key rotation, and incident procedures"; L932-940 "### Phase 3: production web and relay operations" delivering "privacy-safe metrics and incident runbooks", whose go/no-go gate at L948 is "a regional relay outage fails over within 2 minutes" — i.e. the 2-minute target is a tested launch gate, not an unbacked aspiration. L1014-1015 puts "observability, and incident response" in the launch slice.

(d) The "staffing" grep=0 implies no staffing consideration, but L197-198 states "Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time, and an independent security review." The doc explicitly staffs an operations role. (The investigator's "2-3 person team" comes from L45 "For a small team of two to three engineers" — grounded, not fabricated, but they cited L45's implication while missing L197-198's explicit ops headcount.)

(e) The proposed fix demands the doc flag objectives as ASPIRATIONAL. It already does, at L764-765: "These are launch objectives to measure during beta. They are not guarantees inherited from the current preview." That defuses the "cannot honestly offer 99.9%" charge — the doc offers 99.9% to no one; it sets an internal beta measurement target and disclaims guarantee status.

CORRECTED SEVERITY: low, not critical. What survives is narrow and real — the doc specifies what metrics are collected (713-748) and what to do once an incident is declared (695-711) but never the detection-and-notification layer joining them (no alert conditions, no routing, no on-call roster, no status page; "notification" at L710 is scoped USER notification during incident response, not operator paging). That is a legitimate completeness note worth one line in a proposal that sets a 99.9% objective. It is not critical: this is a pre-implementation architecture proposal that explicitly scopes ops to Phase 3, gates the failover number on a test, and disclaims the objectives as unmeasured. The finding must not appear as written — its central reasoning is wrong and its evidence misrepresents the document's monitoring and incident-response coverage.
````

### R44

Candidate finding: The viability of the companion model on iOS and Android is never assessed, and the document never states the resulting conclusion that mobile users get durable participation under neither mode.

Verification reasoning:

````text
UPHOLD (high): ACCURACY LENS: every piece of stated evidence is real and correctly read. I re-ran the greps myself over /home/sekou/AGI/jeliya/docs/production-deployment.md (1087 lines) and reproduced the investigator's numbers exactly: "app lifecycle"=0, APNs=0, "push notification"=0, "App Store"=0, "Play Store"=0, sideload=0, tablet=0, "background execution"=1. Line 294 is correctly characterized as a PROHIBITION, not an analysis — it is the "Prohibited responsibilities" cell of the Browser row: "Claim native direct P2P, durable background execution, or durable storage when unavailable". Lines 446-447 quote verbatim: "Browser peers are available while the application is active. Native or optional server peers provide durable availability." Lines 966-967 quote verbatim: "the latest two Chrome, Edge, Firefox, and Safari releases plus current iOS / Safari and Android Chrome pass the supported matrix". PRODUCT.md:13-15 quotes verbatim ("mobile is for checking in on rooms and agent runs on the go") — note it lives at repo root /home/sekou/AGI/jeliya/PRODUCT.md, not docs/, which matches the unprefixed citation. No misreading anywhere, so I cannot refute on this lens.

INDEPENDENT SWEEP CONFIRMS THE CORE GAP: rather than trusting the investigator's term list, I grepped for all mobile/iOS/Android/desktop/phone terms. Hits at 154-156, 313-314, 966-967, 1003, 1025, 1036, 1056, 1070. None assesses whether a background companion process is installable or durable on iOS or Android. The closest is line 211, a comparison-table cell reading "Companion can run subject to native OS policy" — a one-clause hand-wave in a table, not an assessment. Line 313-314 lists "Android Keystore" as a native secret-wrapping target, implying an Android companion is contemplated but never evaluated. Line 1003 scopes the companion to "supported desktop platforms". I also found a sharper unaddressed version the investigator missed: the doc has zero notification/push analysis ("web push"=0; "notification"=1, and that lone hit at line 710 is "scoped user notification" in incident response). Attention delivery is the load-bearing mechanism for the stated mobile use case and is entirely absent.

WHERE THE FINDING OVERREACHES (basis for downgrade, both undercut by the investigator's own cited lines): (1) "never states the resulting conclusion" is too strong. Line 1025 — cited but never quoted by the investigator — explicitly excludes "mobile background-availability claims" from the slice; line 1036 states "The first supported production matrix is desktop-focused and is narrowed in Phase 0"; line 1056 defers "Supported browser, desktop OS, and mobile matrix" to an ADR. This is a deliberate, stated deferral, not an unnoticed omission. The doc does not spell out the product consequence, but it is far from silent. (2) The PRODUCT.md commitment is weaker than the finding implies: "checking in on the go" describes transient active use, which line 446's "available while the application is active" satisfies. The claim that the plan "may not be able to keep" that commitment overreads PRODUCT.md.

VERDICT: survives as a genuine MISSING item — a plan whose durable-participation story rests entirely on a desktop-only companion, in a product with a stated mobile use case, should say in one sentence what mobile users actually get. But the explicit desktop-scoping (1003, 1036) and explicit mobile exclusion (1025) make this a "state the conclusion plainly / add a short mobile subsection" gap rather than a hole in the plan. Severity high -> medium.
````

### R45

Candidate finding: Export-control and app-store cryptography compliance obligations for distributing signed cryptographic binaries are never mentioned.

Verification reasoning:

````text
UPHOLD (high): The core evidence is real and correctly read, so the finding is not fully refuted — but both legal premises that carry its severity are misread, so it survives only in materially reduced form.

VERIFIED ACCURATE:
- docs/production-deployment.md:561-563 quotes verbatim as claimed ("Native signing: Apple Developer ID/notarization, HSM-backed Windows Authenticode such as Azure Trusted Signing, and signed Linux repository/checksum plus provenance"). The finding cited 560-563; line 560 is actually "Infrastructure code: OpenTofu under infra/" — trivial off-by-one at range start, quoted content is real.
- docs/production-deployment.md:856 is an exact match: "| `.github/workflows/companion-release.yml` | Signed/notarized native companion publication |".
- docs/production-deployment.md:247-249 does show "E2E Iroh control | E2E Iroh Rooms" and "TB2: relays see transport metadata", supporting the E2E-transport half.
- The grep evidence is real: `grep -c -i "export control"` = 0, and also 0 for export-control, ECCN, "encryption declaration", self-classification, BIS, word-boundary EAR, and even "compliance" (zero occurrences in the entire document). Repo-wide grep for export.control|export.classification|ECCN|CCATS|5D002|mass.market returned nothing. The gap genuinely exists.

MISREAD (refutes the severity, not the existence):
1. Line-cite error: ":38-40" is offered for "Ed25519 signing" but those lines read "browser storage, / signing, synchronization, and Iroh Rooms adapters pass independent gates" and "Dedicated relays route encrypted traffic but never join rooms." Ed25519 occurs only at :316, :461, :1084. Minor, but the investigator did not read what it cited.
2. LOAD-BEARING ERROR — the "app-store" half of the headline claim is inapplicable. The doc proposes no app-store distribution anywhere: grep -i for "app store", "Mac App", "TestFlight", "Play Store", "Google Play" = 0 hits. The only iOS/Android mentions are the browser support matrix (:966-967) and keystore status (:154-155, :314), not distribution channels. Line 561 names Developer ID/notarization, which is direct distribution explicitly outside App Store Connect. Apple's own export-compliance page (https://developer.apple.com/help/app-store-connect/manage-app-information/overview-of-export-compliance/) scopes the requirement to App Store Connect and attaches the approved documentation to a "beta build" (TestFlight) or "app version build"; ITSAppUsesNonExemptEncryption is an App Store Connect artifact. Notarization is an automated malware scan and has no encryption questionnaire. So "Apple distribution requires an encryption-compliance declaration" is false for the channel the doc actually chose.
3. SECOND PREMISE ERROR — "US EAR mass-market crypto self-classification is the standard posture for such binaries" is the posture for proprietary commercial crypto. This repo is public (gh repo view → "visibility":"PUBLIC") and dual-licensed MIT OR Apache-2.0 (Cargo.toml:7; LICENSE-MIT, LICENSE-APACHE present). Publicly available open-source encryption follows the much lighter EAR §742.15(b) notification path (email to BIS/NSA), not 5D002/§740.17(b)(1) mass-market self-classification with annual reports.

NET: a real residual obligation remains — a one-time BIS/NSA §742.15(b) notification for publicly released crypto source and binaries — and the doc's Phase 2 gate at :921 ("Go/no-go gate:") is a sensible place for it. But the finding as written overstates the obligation on two independent counts and invokes a distribution channel the plan does not use. It should appear only if rewritten to drop the app-store framing entirely and to name the publicly-available-source notification rather than mass-market self-classification. That is a low-severity housekeeping item for a 2-3 engineer team, not a medium architectural gap.
````

### R46

Candidate finding: The human cost of the 24-hour RC bake and 48-hour soak on a 2-3 person team is never acknowledged or staffed.

Verification reasoning:

````text
REFUTE (high): ACCURACY LENS VERDICT: the quotes are real but the inferences drawn from them are wrong, and the finding's headline assertion is directly contradicted by two doc lines the investigator did not cite.

WHAT I VERIFIED AS ACCURATE (quotes are verbatim):
- Line 666: "Release candidates bake for at least 24 hours." CONFIRMED verbatim.
- Line 929: "a 48-hour soak loses no committed event;" CONFIRMED verbatim.
- Line 670: "Use a protected production environment with manual approval." CONFIRMED verbatim.
- Line 760: "| Frontend rollback | At most 15 minutes |" and line 761: "| Relay regional failover | At most 2 minutes |" CONFIRMED verbatim.
- grep -c -i over /home/sekou/AGI/jeliya/docs/production-deployment.md: burnout=0, staffing=0, "time zone"=0, timezone=0, vacation=0, pager=0, headcount=0. CONFIRMED.
- "all 6 'rotation' hits are key or token rotation" CONFIRMED: lines 136, 307, 364, 709, 843, 859 are key rotation / key-epoch rotation / relay-token rotation / signing-key rotation. Not one is an on-call rotation.

WHY THE FINDING IS REFUTED:

1. "never ... staffed" is FALSE. Lines 197-198: "Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time, and an independent security review." The doc explicitly allocates a dedicated web/operations role. The investigator's own "2-3 person team" premise comes from doc line 45 ("For a small team of two to three engineers") — so the doc self-describes team size AND assigns an ops engineer against it. The staffing model the finding says is absent is stated 470 lines above the first cited line.

2. "never acknowledged" is FALSE. Line 814: "Self-hosted relays may reduce the direct infrastructure bill to roughly $50 to $200 per month plus egress, but move availability and on-call cost to the team." The doc names on-call cost as a real burden borne by this team and uses it as the decision input for preferring managed relays. The investigator's grep list omitted "on-call" — the single most on-point term — while including vacation, burnout, and timezone. `grep -c -i "on-call"` = 1. This is a selection error that manufactured the gap.

3. Line 929 is misread as a recurring release burden. It is a ONE-TIME gate: section header line 912 "### Phase 2: companion-backed vertical slice, 5 to 7 weeks", subheader line 921 "Go/no-go gate:". The proposed fix asks "how a 2-3 person team sustains that cadence across releases" — the 48h soak has no per-release cadence anywhere in the doc. Only the 24h bake (line 666) is per-release.

4. Line 929 is misread as human-attended. It sits in a list of automated harness assertions: line 924 "1,000 automated pairing/revocation cycles accept no unauthorized controller;" and line 906 "10,000 injected lost-response retries produce no duplicate message;". An assertion that a 48h run "loses no committed event" is a log-diff check by construction — it cannot be performed by a human watching a screen.

5. The 2-minute failover objective (line 761) is cited backwards. A 2-minute relay-failover RTO is infeasible for a human responder and is a standard marker of AUTOMATED failover. The doc reinforces this at line 775 ("Relays are stateless and need configuration recovery, not room-data backup") and in its own citation summary at line 1076 ("stateless failover"). This evidence argues against the "near-continuous human availability" reading it was offered to support.

6. Line 666 in context is a pipeline dwell, not a vigil. Steps 1-5 of "### Merge and staging" are automated pipeline stages ("3. Those exact bytes deploy automatically to staging. 4. Staging smoke and compatibility suites run against dedicated staging relays."). "Bake" is standard release-engineering vocabulary for an artifact soaking in staging under automated suites.

RESIDUAL KERNEL (why 'low', not 'drop'): two narrow observations survive. (a) `grep -c -i "alert"` = 0 — the doc has a full "## Privacy-safe observability" section (lines 712-748) defining allowed metrics and forbidden fields, but never specifies alerting or paging on those metrics. (b) The 15-minute frontend-rollback objective (line 760, restated line 688) genuinely does imply a human able to act on short notice, and the bake's watcher is unspecified. However, the doc pre-defers incident procedure to line 859 (`docs/runbooks/` — "Deployment, rollback, relay failure, key rotation, and incident procedures") and line 940 (Phase 3 deliverable: "privacy-safe metrics and incident runbooks"), so this is a deferral the doc makes explicitly rather than an omission it is blind to. A proposal at this altitude is not obligated to publish an on-call roster.

This should be rewritten, not merely downgraded. A defensible version is: "The doc defines observability metrics (712-748) but no alerting on them, and does not say whether the per-release 24h bake (666) is automated or attended." The claim as filed — that the human cost is "never acknowledged or staffed" — cannot ship, because lines 197-198 and 814 contradict it on the page.
````

### R47

Candidate finding: iOS Safari PWA storage behavior is listed as a high-risk unknown but is never bound to a phase, owner, or fallback, while a Phase 4 gate depends on it passing.

Verification reasoning:

````text
UPHOLD (high): ACCURACY lens: I independently read every cited line in /home/sekou/AGI/jeliya/docs/production-deployment.md. The evidence is real and quoted verbatim.

VERIFIED ACCURATE:
- Line 1070 is exactly "9. PWA storage behavior across real Safari/iOS and low-storage devices." Confirmed by `awk 'NR>=1058 && NR<=1071'`.
- Lines 966-967 are exactly "- the latest two Chrome, Edge, Firefox, and Safari releases plus current iOS / Safari and Android Chrome pass the supported matrix;" inside "### Phase 4: browser peer and multi-device identity, 10 to 14 weeks" (heading at line 954). Confirmed.
- Lines 436-438 are exactly "Maintain an eviction sentinel. Missing critical state stops authorship and / offers recovery import or peer resynchronization. Never silently create a new / identity." Confirmed.
- "The list at 1058-1071 assigns no owner, phase, or spike to any of the ten unknowns" — CONFIRMED. Lines 1058-1071 are a bare numbered list. `grep -n -i "owner|spike|decision date|assign"` over the whole doc returns only lines 175, 397, 789, 879 — none inside 1058-1071, and none of them an accountability assignment.

INDEPENDENT REINFORCEMENT I found: `grep -n -i "ios|safari"` returns only FOUR hits in the entire 1100-line doc: 155, 966, 967, 1070. So iOS genuinely appears nowhere between the risk statement and the gate that depends on it. Line 155 additionally says "iOS has no application scaffold", which sharpens the gap — the natural fallback (companion mode, lines 412-422, where "Keep the companion authoritative" and eviction is "a re-pair and resync event, not identity loss") has no iOS implementation today, and is never named as the iOS answer. Phase 0's gate (881-889) and Phase 3's gate (964-972 region / 939-945) contain no storage-durability measurement, so the empirical question is first answered at the Phase 4 gate — after a 10-to-14-week phase whose deliverable list already includes "browser recovery and eviction handling" (line 961). Building 10-14 weeks of work before measuring the unknown that gates it is the real defect.

TWO CORRECTIONS to the finding's wording (neither refutes it):
1. Off-by-one citation. "Request `navigator.storage.persist()`, display storage estimates" is at line 431, not 432. The bullet spans 431-433. Text quoted is verbatim; only the anchor is one line low.
2. Over-stated sub-claim: "no fallback is defined for the case where iOS Safari cannot hold a durable browser-peer identity." A generic fallback IS defined at 436-438 and is gated at line 970 ("clearing storage triggers recovery and never silent identity replacement"), and companion mode at 412-422 is an architectural mode that survives non-durable browser storage. The accurate form is the one the finding itself hedges to: the machinery exists but is never bound to the iOS risk, and no product-level decision is stated for "iOS browser peers cannot be durable at all" (as opposed to a one-off clearing event). The reviewer should adopt the hedged phrasing and drop "no fallback is defined."

Severity stays medium. It is a genuine structural inconsistency: a doc that is otherwise highly specific (explicit go/no-go gates, week-range estimates per phase) terminates in ten unowned, unphased, undated unknowns, one of which gates its longest phase. Not high — the doc does define eviction behavior and does gate it, so this is a sequencing/ownership gap rather than an unhandled failure mode. Not low — the fix is cheap and the exposure (10-14 weeks of Phase 4 work) is large.

Refinement to the proposed fix: "Phase 0 or Phase 3" should be Phase 3, not Phase 0. Phase 0 is 1-2 weeks with no browser storage surface; line 959 puts "service worker and encrypted companion-view cache" in Phase 3, which is the first phase with something to measure.
````

## Claims verified accurate

138 claims in the proposal were checked and found accurate. This section is the
only evidence of the review's coverage: silence about what was checked would make the finding
count impossible to interpret.

### A1

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 62-63: "The assessed HEAD is 14 commits and 142 changed files after that Jeliya commit."

Confirming evidence: ACCURATE, exactly. `git rev-list --count 55024a46b3e112796ba2acf1dc408dab26dbba2e..4d4621c929e6f9678b31b7e4a3ee1c8d751b545b` -> 14. `git diff --shortstat 55024a4 4d4621c9` -> "142 files changed, 12331 insertions(+), 2679 deletions(-)". Both figures match the doc verbatim.

### A2

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 64-66: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml."

Confirming evidence: ACCURATE. Cargo.toml:15: `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }`. Also confirmed resolved in the lockfile — Cargo.lock:2013-2015: `name = "iroh-rooms" / version = "0.1.0-rc.3" / source = "git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef4ce83631c94762ec68c2beef3d79#71fbb5007bef4ce83631c94762ec68c2beef3d79"`.

### A3

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim:

````text
Lines 58-61: "[Capability status](capability-status.md) names Jeliya commit `55024a46b3e112796ba2acf1dc408dab26dbba2e` and Iroh Rooms commit `71fbb5007bef4ce83631c94762ec68c2beef3d79` as the network-qualified `v0.6.0` pair."
````

Confirming evidence:

````text
ACCURATE. docs/capability-status.md:29: "| Network-qualified commit (`v0.6.0` candidate) | `55024a46b3e112796ba2acf1dc408dab26dbba2e` with `iroh-rooms` pin `71fbb500…` (published tag `v0.1.0-rc.3`) |". The full 40-hex iroh-rooms SHA is spelled out one row down at :31: "| Candidate `iroh-rooms` pin (`main`) | `71fbb5007bef4ce83631c94762ec68c2beef3d79` — published tag `v0.1.0-rc.3` |". Corroborated at docs/security-threat-model.md:32-33 and docs/verification-evidence.md:33-35.
````

### A4

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 67-69 (first half): capability status contradicts itself — the top records fresh candidate direct and relay runs while a lower row says no candidate run exists.

Confirming evidence:

````text
ACCURATE and reproduced exactly. TOP — docs/capability-status.md:19-22: "The `v0.5.0` evidence does not transfer to that pin, so fresh certifying direct and forced-relay runs were executed at the candidate on 2026-07-16; `v0.6.0` is qualified but not yet published." And :32: "| Candidate network verification | certified — signed direct and forced-relay runs bind `55024a4` + `71fbb500`." LOWER ROW — docs/capability-status.md:50: "Known `v0.5.0` limitation: its pin predates upstream's join-after-conversation fix, so an invite minted after non-admin chat cannot complete `room.join`; the rc.3 candidate on `main` fixes this and has no network run yet." The phrase "has no network run yet" is flatly negated by :19-22, :30, :32, :53 and :54 of the same file.
````

### A5

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim:

````text
Lines 70-71: "The [security threat model](security-threat-model.md) also retains a stale statement that the public lockfile has not been repinned."
````

Confirming evidence: ACCURATE, and the statement is genuinely stale. The statement is docs/security-threat-model.md:122-124 (in the "Synchronization invariant" section): "Because the public / Jeliya lockfile does not yet resolve that code, upstream publication and Jeliya / repinning are mandatory before release qualification." It IS stale: Cargo.lock:2015 resolves `iroh-rooms` to `git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef4ce83631c94762ec68c2beef3d79`, i.e. the repin already happened. The same file contradicts itself elsewhere — :20-24 "the public dependency pin now carries the room-scoped synchronization remediation", :43-45 "Publication, immutable safe repinning, the direct and relay reruns, and the fresh signed evidence — all now complete at this pin", and :80 "the room-scoped remediation is published and pinned at `71fbb500...`". Independently corroborated by docs/verification-evidence.md:299-301: "Jeliya's public `Cargo.toml` and `Cargo.lock` are repinned to that immutable public revision".

### A6

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim:

````text
Lines 105-107: "Signed evidence exists for direct and forced-relay behavior at exact recorded revisions, with the limits documented in [Verification evidence](verification-evidence.md)."
````

Confirming evidence: ACCURATE and supported by the artifacts, not just the prose. `ls docs/evidence/v0.6.0/` -> direct.json, direct.json.sig, relay.json, relay.json.sig. Parsing both manifests: `/schema = 2`, `/certifiable = True`, `/source/releaseable = True`, `/source/commit = '55024a46b3e112796ba2acf1dc408dab26dbba2e'`, run ids `20260716T201318Z-1ca39cfa` (direct) and `20260716T203450Z-cf28bc63` (relay). Ledger text at docs/verification-evidence.md:172-173 and :186-192 ("carries a detached Ed25519 signature (`.sig`) that verifies against the pinned release-evidence public SPKI"). The "limits" are documented as claimed: :72 ("NOT network-certified: both certifying manifests set `synchronization_isolation_claimed: false`" — confirmed in both manifests as `False`) and :254-265 ("Historical qualification limits").

### A7

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim:

````text
Lines 154-156: "Android lacks Keystore wrapping and remote-network evidence; iOS has no application scaffold. See [Platform matrix](platform-matrix.md)."
````

Confirming evidence:

````text
ACCURATE on all three sub-claims. Android/Keystore — docs/platform-matrix.md:57: "| Android identity storage | app-private no-backup storage with cloud and device-transfer exclusions | rules and validation pass | unreleased | included security control; not Keystore-backed |", corroborated by docs/security-threat-model.md:70-73 ("It does **not** wrap the identity with Android Keystore") and docs/verification-evidence.md:73 ("this is app-private no-backup storage, not Android Keystore wrapping"). Android/remote-network — docs/platform-matrix.md:56: "Android 13 local lifecycle/FFI smoke only; no cross-network, NAT, direct, or relay evidence", and :68: "| Android in-process engine | local device-smoke evidence | unverified | unverified |", corroborated by docs/verification-evidence.md:287-290 ("It did not communicate with a remote peer or measure a direct/relay path, so it is not Android real-network evidence"). iOS — docs/platform-matrix.md:58: "| iOS app | no scaffold or engine build | none | none | excluded |". Verified on the filesystem: `ls app/` -> analysis_options.yaml, android, build, jeliya_app.iml, l10n.yaml, lib, linux, macos, pubspec.lock, pubspec.yaml — no `ios` directory, and a `find . -maxdepth 3 -iname ios -type d` returned nothing.
````

### A8

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 77-78: "Rust core and daemon: 71 unit tests passed; one opt-in performance test was ignored."

Confirming evidence: ACCURATE, reproduced at current HEAD. `cargo test --locked -p jeliya-core -p jeliyad` -> "running 64 tests / test result: ok. 63 passed; 0 failed; 1 ignored" and "running 8 tests / test result: ok. 8 passed; 0 failed; 0 ignored" = 71 passed, 1 ignored. The ignored test is a performance test as described: `cargo test -p jeliya-core -- --ignored --list` -> "supervisor::tests::hot_reads_are_fast_on_a_room_with_real_history: test". Independently matches docs/verification-evidence.md:340 ("63/63 `jeliya-core` and 8/8 `jeliyad` tests pass").

### A9

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 80-83: "A full `cargo test --locked --workspace` could not build `jeliya-ffi` because the local environment lacked Dart SDK headers... This is an unverified local FFI prerequisite, not evidence of a product failure or success."

Confirming evidence: ACCURATE, reproduced. `env -u FLUTTER_ROOT -u DART_SDK_INCLUDE cargo test --locked --workspace --no-run` -> "error: failed to run custom build command for `jeliya-ffi` ... thread 'main' panicked at crates/jeliya-ffi/build.rs:63:5: jeliya-ffi build.rs: could not locate the Dart SDK include dir (needed to compile dart_api_dl.c for Dart NativePort posting)". crates/jeliya-ffi/build.rs:14 documents the intent: "A miss is a loud build failure". The doc's refusal to read this as a product signal is the correct call.

### A10

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Line 79: "Documentation, secret-storage, and release-contract checks passed."

Confirming evidence: ACCURATE at the assessed commit and still passing with the new page in place. At 4d4621c9 (detached worktree): `node scripts/check-docs.mjs` -> "docs-check: OK — profile, indexes, titles, and local links are valid." exit=0; `node scripts/check-secret-storage.mjs` -> "secret-storage: PASS — Android backup/transfer and local identity guards are explicit" exit=0; `node scripts/check-release.mjs` -> "release-integrity: source versions match v0.6.0" exit=0. All three also pass against the current working tree including the untracked docs/production-deployment.md and its docs/index.md entry.

### A11

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 84-85: "On 2026-07-17, `jeliya.ai` and `app.jeliya.ai` had no resolvable A, AAAA, or CNAME record from the assessment environment."

Confirming evidence:

````text
CORROBORATED today (I cannot re-observe 2026-07-17). `dig +short A|AAAA|CNAME` for both `jeliya.ai` and `app.jeliya.ai` returned empty for all six queries. `getent hosts` returned only `192.168.1.1 jeliya.ai.lan` / `192.168.1.1 app.jeliya.ai.lan`, which are local resolver search-domain artifacts (`.lan` suffix), not records for the real names. The doc's own hedge "from the assessment environment" is appropriately scoped.
````

### A12

- Source: track `T1-Repo`, investigator `commits-and-docs-consistency`

Claim: Lines 22-25 / 87-89: the page frames itself as a proposal whose statuses must advance independently, and cites external platform facts to a Citations section.

Confirming evidence:

````text
ACCURATE and internally consistent. Frontmatter (lines 7-10) sets `status: "proposal"`, `implementation_status: "planned"`, `verification_status: "partial"`, `release_status: "unreleased"`, matching the line 22-25 prose. The `[Citations](#citations)` anchor at line 89 resolves to the "## Citations" heading at line 1073. `node scripts/check-docs.mjs` validates "profile, indexes, titles, and local links" and passes with this file present, and docs/index.md:42 registers it under "## Proposals".
````

### A13

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 172-173: `crates/jeliyad/src/main.rs` "binds only `127.0.0.1` and exposes no flag for a non-loopback address". This is the doc's single most load-bearing security claim; I attacked it from every angle I could construct.

Confirming evidence:

````text
CONFIRMED, and it survives adversarial falsification. The bind is a compile-time constant with no data path from any input:

crates/jeliyad/src/main.rs:364-388 (`bind_loopback`, the ONLY bind site) — line 371: `let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, candidate));`. `Ipv4Addr::LOCALHOST` is a std const (127.0.0.1). The only variable is `candidate`, a `u16` port. Called once, at main.rs:170: `bind_loopback(args.port, 20)`.

Falsification attempts, all negative:
1. Any other listener in the workspace? `grep -rn "TcpListener::bind|UdpSocket::bind|::bind(" crates/ --include=*.rs` returns exactly ONE hit: `crates/jeliyad/src/main.rs:372`. No in-process host, no FFI path, binds anything.
2. Any other address literal? `grep -rn "0\.0\.0\.0|unspecified|UNSPECIFIED|Ipv6Addr" crates/jeliyad/` — zero hits. The daemon does not even bind `::1`.
3. Full CLI surface (main.rs:50-81, the complete `Args` struct): `--port` (u16), `--data-dir`, `--ui-dir`, `--no-open`, `--loopback`, `--supervised`, plus `--verification-relay-only-build` which exists only under `#[cfg(feature = "relay-only-test")]`. No `--host`, `--bind`, `--listen`, `--address`, or `--public`. Note `--loopback` (main.rs:68-70) is a decoy for a reviewer: it selects "the SDK's loopback/CI network mode" for the iroh peer network and is passed to `RoomSupervisor::new` (main.rs:145) — it has no effect on the HTTP bind.
4. Env var? `grep -rn "env::var|option_env|std::env" crates/jeliyad/` returns only three `env!("CARGO_PKG_VERSION")` compile-time hits (serve.rs:186, main.rs:195, main.rs:213). Further, clap is declared as `clap = { version = "4", features = ["derive"] }` in crates/jeliyad/Cargo.toml — the `env` feature is NOT enabled, so `#[arg(env = ...)]` is not even available in this build.
5. Config file? No config deserialization, no `--config` flag, nothing read from disk that feeds the bind.
6. Test hook? The only bind-related test is main.rs:504-512 `port_zero_reports_the_os_assigned_port`, which calls `bind_loopback(0, 20)` — still loopback. There is no `#[cfg(test)]` or feature-gated alternate bind.
7. Feature flags? `embed-ui` (UI assets) and `relay-only-test` (prints a marker and returns before networking, main.rs:107-115). Neither touches the bind. main.rs:430-443 has a test asserting the default binary REJECTS the hidden verifier flag.

The source's own module doc (main.rs:5-7) makes the identical claim, and the doc's roadmap row at line 845 ("never receive a public bind option") is consistent with it.
````

### A14

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 174-175: "It creates one per-process bearer token" — token generation, RNG, and entropy.

Confirming evidence:

````text
CONFIRMED and stronger than the doc states. crates/jeliyad/src/lifecycle.rs:64-68:
```
pub fn generate_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("OS CSPRNG unavailable: {e}"))?;
    Ok(hex::encode(bytes))
}
```
32 bytes = 256 bits from the OS CSPRNG via `getrandom` 0.4 (crates/jeliyad/Cargo.toml), hex-encoded to 64 chars. It FAILS CLOSED if the CSPRNG is unavailable (main.rs:153-159 exits 1). Called exactly once per process at main.rs:153, so "per-process" is precise.

The token is deliberately kept out of the stdout `ready` line — main.rs:227-229: "The token is NOT here — it lives in the 0600 portfile." Confirmed by inspecting the JSON at main.rs:230-243, which carries pid/port/http/ws/version/protocol/data_dir/portfile and no token. `grep -rn "info!|debug!|trace!|warn!" crates/jeliyad/src/serve.rs` returns zero hits, so the request-handling layer logs no URLs and therefore never logs a `?token=` query string.

Comparison is constant-time (serve.rs:295-310, `token_ok` → `constant_time_eq`), with a test at serve.rs:852-859.
````

### A15

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 177-178: "Its documented threat model explicitly excludes hostile same-user processes and shared multi-user service operation."

Confirming evidence:

````text
CONFIRMED verbatim. docs/PROTOCOL.md:90-96: "any process running as the same user can already read the 0600 portfile, so the token grants such a process nothing it could not otherwise [obtain]... hostile *web pages* in a real browser (which cannot forge those headers), **not** [non-browser processes]... a multi-user machine is therefore **out of scope**: a different local user who can reach `127.0.0.1` could obtain the token via `/api/session`."

The source repeats it at crates/jeliyad/src/serve.rs:206-209: "neither header is a boundary against a *non-browser* local process, which can forge both. On a single-user machine that process could read the 0600 portfile anyway; multi-user machines are out of scope."
````

### A16

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 179-180: Host and Origin checks are "not remote account authentication" — the judgment itself.

Confirming evidence:

````text
CONFIRMED. The Host gate is a header-shape check (crates/jeliyad/src/main.rs:394-409, `host_header_is_loopback`) and the Origin gate is likewise (main.rs:413-422, `is_local_origin`). Neither carries any notion of a user, account, or credential — the only credential in the system is the per-start token.

Both checks are implemented soundly for their stated purpose. `host_header_is_loopback` (main.rs:404-408) requires the host to PARSE as a loopback IP rather than merely look like one, with an explicit comment that `127.0.0.1.evil.example` must not slip through; there are four test cases covering exactly that at main.rs:445-502, including `"127.0.0.1.evil.example"`, `"localhost.evil.example"`, `"null"`, and the empty string. `is_local_origin` (main.rs:417-421) rejects the literal `"null"` opaque origin. Absent-or-unparsable Host is refused (serve.rs:272-278, `.unwrap_or(false)`).

The WebSocket upgrade specifically receives BOTH checks: Host at serve.rs:120 (the condition includes `path == "/ws"`) and Origin at serve.rs:360-368, followed by the token gate at serve.rs:369-375.
````

### A17

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 187, first half: "The current UI transports bearer material in WebSocket and upload query URLs" — the two transports the doc DOES name.

Confirming evidence:

````text
CONFIRMED for both. WebSocket: ui/src/lib/client.ts:249-258 in `openWithToken()` — `const withToken = new URL(this.url); withToken.searchParams.set('token', token); url = withToken.toString();` then `new WebSocket(url)`. Upload: ui/src/lib/client.ts:87-96 in `uploadFileToRoom()` — `const url = new URL('/api/files/share', daemonHttpBase()); ... if (token) url.searchParams.set('token', token);` then `fetch(url, { method: 'POST', ... })`.

The daemon accepts both forms (serve.rs:283-293, `presented_token`: `?token=` query param first, then `Authorization: Bearer`), so the `Authorization` header path already exists and the UI simply does not use it — which makes the doc's implied remediation cheap for at least the upload case. (See the separate finding: the enumeration omits a third transport.)
````

### A18

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 181-182: "The RPC surface includes identity creation, daemon shutdown, room history, native file operations, pipes, and agent projections."

Confirming evidence:

````text
CONFIRMED, all six categories present. `grep -oE '"[a-z]+\.[a-z_]+"' crates/jeliya-core/src/engine.rs | sort -u` yields the dispatch surface: `identity.create`; `daemon.shutdown`, `daemon.status`; `room.timeline`, `room.create/join/leave/open/close/list/members`; `file.share`, `file.fetch`, `file.list`; `pipe.expose`, `pipe.connect`, `pipe.close`, `pipe.list`; `agent.history`, `agents.fleet`; plus `invite.create`, `message.send`, `peers.status`, `status.post`.
````

### A19

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 303-304: `crates/jeliya-core/src/identity.rs` "creates one identity/root key and one event/device key".

Confirming evidence:

````text
CONFIRMED. crates/jeliya-core/src/identity.rs:119-120: `let identity_key = SigningKey::generate(); let device_key = SigningKey::generate();` — exactly two keys, recorded in the profile as `identity_id` (from `identity_key.identity_key()`) and `device_id` (from `device_key.device_key()`) at identity.rs:124-125. The doc-comment at identity.rs:47-50 confirms the roles: identity "Signs the device binding (authorizes `device_id` under `sender_id`)", device "Signs events; signatures verify under `device_id`".

Both keys come from the OS CSPRNG. Verified upstream at the exact pinned revision — /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50 (`git rev-parse HEAD` = 71fbb5007bef4ce83631c94762ec68c2beef3d79, matching the Cargo.toml pin), file crates/iroh-rooms-core/src/event/keys.rs:249-253:
```
    pub fn generate() -> Self {
        let mut seed = Zeroizing::new([0u8; PUBLIC_KEY_LEN]);
        getrandom::fill(seed.as_mut_slice()).expect("OS CSPRNG (getrandom) must be available");
        Self::from_seed(&seed)
    }
```
````

### A20

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 304-305: "It stores both seeds in a plaintext JSON secret" — the plaintext and both-seeds parts.

Confirming evidence:

````text
CONFIRMED, literally. crates/jeliya-core/src/identity.rs:236-248:
```
fn secret_file_contents(identity_key: &SigningKey, device_key: &SigningKey) -> String {
    let identity_seed = identity_key.to_seed();
    let device_seed = device_key.to_seed();
    let mut identity_hex = hex::encode(identity_seed.as_slice());
    let mut device_hex = hex::encode(device_seed.as_slice());
    let contents = format!(
        "{{\"version\":{PROFILE_VERSION},\"identity_secret\":\"{identity_hex}\",\
         \"device_secret\":\"{device_hex}\"}}\n"
    );
```
Raw 32-byte Ed25519 seeds, hex-encoded, both in one JSON object, no encryption or KDF anywhere in the path. Written to `identity.secret` (identity.rs:20, 133). The module doc at identity.rs:5-6 states the same: "Seeds are stored plaintext under owner-only permissions (the SDK MVP threat model)".

Credit where due: the implementation is careful about memory hygiene even though the disk form is plaintext — `SecretKeys` has no `Debug`/`Serialize` (identity.rs:44-51), the secret buffer is zeroized after write (identity.rs:135), the read buffer and parsed struct are zeroized (identity.rs:180, 185), and hex intermediates are zeroized (identity.rs:245-246).
````

### A21

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 306-307: "There is no export, recovery, rotation, device authorization, or same-identity cross-device flow." AND the implied claim that no keystore/encryption path exists that the doc missed.

Confirming evidence:

````text
CONFIRMED, and I actively hunted for a missed path.

(a) The entire public surface of identity.rs is four functions: `ensure_dir` (:69), `load_profile` (:86), `create` (:108), `SecretKeys::load` (:162). No export, no import, no rotate, no delete, no re-key.

(b) `grep -rniE "identity\.(export|recover|rotate|import)|device\.(authorize|revoke|authorized|revoked)|recovery|rotate_key|export_seed" --include=*.rs crates/` — ZERO hits across the entire Rust workspace (jeliya-core, jeliyad, jeliya-ffi).

(c) No hidden keystore/encryption path: `grep -rniE "keyring|keychain|dpapi|secret[-_ ]?service|android.*keystore|EncryptedSharedPreferences|flutter_secure_storage|argon2|scrypt|pbkdf2|chacha20|aes-gcm|aead"` across `--include=*.rs --include=*.toml --include=*.dart --include=*.ts --include=*.tsx --include=*.yaml`, excluding `docs/` and `node_modules` — ZERO hits. (I sanity-checked the grep by re-running it against `docs/` alone, which correctly returns docs/security-threat-model.md:71 and docs/known-gaps-roadmap.md:30,59,105 — so the pattern works and the zero result over source is real, not a broken query.) There is no encrypted-at-rest path, no OS keystore integration, and no password-hardening code anywhere in the repository. The doc missed nothing.

(d) The `Profile` struct (identity.rs:31-42) has a single `device_id` field — the on-disk format cannot represent multiple device bindings, which independently corroborates "no same-identity cross-device flow" and supports the doc's Phase 4 framing at lines 355-357 that this needs a protocol-v2 change.
````

### A22

- Source: track `T1-Repo`, investigator `daemon-and-identity-security`

Claim: Doc 305-306: "The current UI truthfully states that the identity is unrecoverable."

Confirming evidence:

````text
CONFIRMED — two distinct strings, in two different surfaces, both truthful given finding (c) above.

ui/src/l10n/en.ts:619 (`onboardingIdentityCopy2`, shown during identity creation): "There's no password reset and no recovery — if you lose this device or its data folder, this identity is gone for good."

ui/src/l10n/en.ts:282 (`settingsSelfLabelNote`, shown in Settings): "Your name is a local label — it never changes your cryptographic identity, which is unrecoverable if this device or its data folder is lost."

Also accurate in the same block, ui/src/l10n/en.ts:618: "A keypair generated and stored by your local daemon. No account, no server — the private key never leaves this machine." — consistent with identity.rs, which never transmits a seed.
````

### A23

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "Files are bounded to 100 MiB" (line 108)

Confirming evidence: Exactly 100 MiB. crates/jeliya-core/src/supervisor.rs:33 imports `MAX_SHARED_FILE_BYTES` from `iroh_rooms::events::constants`; at the pinned rev 71fbb5007bef4ce83631c94762ec68c2beef3d79, /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/event/constants.rs:37 reads `pub const MAX_SHARED_FILE_BYTES: u64 = 104_857_600;` (= 100 * 1024 * 1024). The bound is enforced at FOUR independent points, not one: (a) authoring — supervisor.rs:1712-1718 rejects `meta.len() > MAX_SHARED_FILE_BYTES`; (b) daemon HTTP upload preflight — crates/jeliyad/src/serve.rs:68 re-exports it as `FILE_UPLOAD_MAX_BYTES` and serve.rs:512-526 rejects an over-limit `Content-Length` with 413; (c) streaming upload body — serve.rs:553 `read_limited(req.into_body(), FILE_UPLOAD_MAX_BYTES)` counts bytes and aborts mid-stream (serve.rs:588-594), so a lying Content-Length does not help; (d) on the wire during fetch — upstream blob/fetch.rs:59-66 passes `MAX_SHARED_FILE_BYTES` as `max_bytes` to `fetch_blob_sized`, which "refuses to buffer more than `max_bytes`". Peers also reject a *declared* oversize in the signed event: upstream event/content.rs:713 `if size_bytes > MAX_SHARED_FILE_BYTES`.

### A24

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "BLAKE3-verified" (line 108)

Confirming evidence: Enforced on READ (fetch), derived on WRITE (share) — both correct by construction. On write: supervisor.rs:1741-1744 `session.node.blob_import(&import_path)`, then supervisor.rs:1770 embeds `iroh_rooms::files::HashRef::from_bytes(import.hash)` into the signed `file.shared` event, so the digest is computed by the store and signed — there is nothing to re-verify. On read: supervisor.rs:1926 `let declared = *shared.blob_hash.as_bytes();` is passed as BOTH the fetch hash and the declared hash at supervisor.rs:1931-1933, and supervisor.rs:1942-1951 maps `FetchOutcome::HashMismatch` to a hard `ErrorKind::HashMismatch` error ("integrity check FAILED: fetched bytes do not hash to the declared {}; refusing to save") — no partial write. The actual comparison is upstream: /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-net/src/blob/fetch.rs:116 `let actual = blake3::hash(&bytes);` → fetch.rs:120 `(FetchOutcome::HashMismatch, Some(bytes))`.

### A25

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "confined against arbitrary local-file sharing" (lines 108-109)

Confirming evidence: Concretely, `file.share` may only read files already inside the daemon data dir, and the confinement is SOUND against both path traversal and symlinks. supervisor.rs:288-321 `fn assert_shareable_path(&self, canonical: &Path)`: it canonicalizes the data dir (`std::fs::canonicalize(&self.data_dir)`, :292) and rejects unless `canonical.starts_with(&root)` (:294). Its doc comment (:290-293) states the exact threat: "Without this the daemon is an arbitrary-local-file read primitive: a hostile local (or cross-site-WebSocket) client could `file.share` a path like `~/.ssh/id_rsa`." Path traversal and symlinks are both defeated because the CALLER canonicalizes first — supervisor.rs:1719-1721 `let import_path = std::fs::canonicalize(path)...; self.assert_shareable_path(&import_path)?;` — and `std::fs::canonicalize` fully resolves `..` and every symlink component before the prefix test. The same canonical path (`import_path`) is what is later imported (supervisor.rs:1743), so no second resolution occurs. Two further exclusions inside the allowed region: the daemon's own blob store (:301-305, `canonical.starts_with(root.join(BLOBS_DIR))`) and the secret/state files (:307-320, matching `identity.json`, `identity.secret`, and any name prefixed `rooms.db` or `state.json` — the prefix match covers `rooms.db-wal`/`-shm`). I verified those files are always DIRECT children of the data dir, so the `canonical.parent() == Some(root)` guard at :307 is complete: crates/jeliya-core/src/identity.rs:110-111 `data_dir.join(IDENTITY_FILE)` / `data_dir.join(SECRET_FILE)`, crates/jeliya-core/src/localstate.rs:81,99 `data_dir.join(STATE_FILE)`. The browser upload path does not bypass any of this: serve.rs:557-568 stages the body into `state.data_dir.join("uploads")` and then calls the same `share_file`, which re-confines. A repo test covers it: supervisor.rs:4549 `async fn file_share_confined_to_the_data_dir()`.

### A26

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "and fetched from active peers. There is deliberately no central inbox or guaranteed offline delivery" (lines 109-110)

Confirming evidence:

````text
Accurate on both halves. Fetch requires the FETCHER to be an active member at request time (supervisor.rs:1873-1879, `if !snapshot.is_active(&self_id)` → `ErrorKind::FileUnauthorized`; the comment at :1869-1871 notes this is stricter than archive reads). The provider set excludes this device (supervisor.rs:1908-1911 `.filter(|id| *id != self_device)`) and an empty provider set is a hard error whose message is verbatim the doc's claim: supervisor.rs:1918-1923 "...has no other provider to fetch from; there is no central inbox and no guaranteed offline delivery". `file.list` reports `available` only when a provider is a currently-CONNECTED peer (supervisor.rs:1826-1832, `s.node.peer_state(id) == Some(PeerConnState::Connected)`), and the comment at :1783-1791 explicitly refuses to claim availability for a self-only file.
````

### A27

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "Pipes are restricted to numeric loopback targets and an authorized peer" (line 111)

Confirming evidence: Fully accurate; I verified the parser empirically rather than by inspection. "Numeric" is real: supervisor.rs:2056-2060 parses with `SocketAddr::from_str(target_str.trim())` and errors "(expected ip:port)". I compiled and ran a probe (rustc, /tmp/.../scratchpad/ip.rs) against the same std parser: `localhost:80` → PARSE ERROR, `2130706433:80` (decimal-encoded) → PARSE ERROR, `0177.0.0.1:80` (octal) → PARSE ERROR, `127.1:80` (short form) → PARSE ERROR. Loopback filtering: supervisor.rs:2061-2067 `if !is_loopback_target(&target)` → `ErrorKind::PipeDenied` with hint "pipes may only forward to 127.0.0.0/8 or ::1"; upstream crates/iroh-rooms-net/src/pipe/registry.rs:27-31 is `addr.ip().is_loopback()`. Probe results: `0.0.0.0:80` → is_loopback=false (REJECTED), `[::]:80` → false (REJECTED), `[fe80::1]:80` → false (REJECTED), `[::ffff:127.0.0.1]:80` → false (REJECTED — Rust's Ipv6Addr::is_loopback matches only `::1`, so the IPv4-mapped bypass fails closed), `127.0.0.1:80` and `[::1]:80` → true, `127.0.0.2:80` → true (correctly allowed: it is within 127.0.0.0/8, matching the code's own stated rule).

### A28

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "...and an authorized peer" — the authorization half of line 111

Confirming evidence: Enforced per-connection AND re-enforced continuously, which is stronger than the doc says. jeliya passes exactly one identity: supervisor.rs:2068-2070 parses `peer_identity` as an `IdentityKey`, and supervisor.rs:2093 passes `&[peer]` as `allowed_members`. Upstream refuses an empty allowlist outright (crates/iroh-rooms-net/src/node.rs:804 `if allowed_members.is_empty()`; pipe/error.rs:19 "no default-all"). The gate runs at stream-accept: crates/iroh-rooms-net/src/pipe/handler.rs:130 `let verdict = gate::evaluate(&state.query, &state.registry, &device, pipe_id, now_ms()).await;`. It ALSO runs per-tick on every live session: pipe/watcher.rs:45 same call. pipe/gate.rs:12-15 states this explicitly: "The same function backs both the accept handler (one new stream) and the teardown watcher (each live session, each tick), so a session is judged by exactly the rule a fresh connect would face." gate.rs:39-67 fails closed at every lookup (missing target / closed pipe / missing announcement / unreachable snapshot all → `Reject(PipeDenyCause::Closed)`) and composes `pipe_connect_allowed` (identity → Active → allowed_members → owner-Active → expiry) against the CURRENT snapshot, never an ancestor view.

### A29

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "The shared engine implements identity creation, rooms, invites, membership, messaging, files, pipes, peer status, and agent projections" (lines 98-100)

Confirming evidence: All nine present in the crates/jeliya-core/src/engine.rs dispatch table (775 lines total): identity creation → `identity.create` (:206); rooms → `room.create` (:215), `room.list` (:220), `room.open` (:221), `room.close` (:226), `room.timeline` (:236); invites → `invite.create` (:244); membership → `room.join` (:252), `room.leave` (:231), `room.members` (:240); messaging → `message.send` (:265), `status.post` (:270); files → `file.share` (:285), `file.list` (:290), `file.fetch` (:294); pipes → `pipe.expose` (:301), `pipe.list` (:306), `pipe.connect` (:310), `pipe.close` (:315); peer status → `peers.status` (:328); agent projections → `agents.fleet` (:321), `agent.history` (:322). Also present and consistent with the doc's §"Why the loopback daemon must not be public" line 183: `daemon.status` (:195) and `daemon.shutdown` (:196).

### A30

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "CI pins third-party actions" (line 113)

Confirming evidence: Every single one, in both files. `grep -n "uses:" .github/workflows/ci.yml` returns 24 lines and all 24 are 40-hex commit SHAs with a version comment, e.g. ci.yml:31 `actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0`, ci.yml:170 `subosito/flutter-action@1a449444c387b1966244ae4d4f8c696479add0b2 # v2.23.0`, ci.yml:579 `taiki-e/install-action@43aecc8d72668fbcfe75c31400bc4f890f1c5853 # v2.83.2`. Even a floating channel is pinned: ci.yml:245 `dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable` and ci.yml:431 `...@ad7910d95e317a4b12a9a3dfad520f4b409b3ec0 # 1.91.0`. release.yml is identical (18 `uses:` lines, all SHA-pinned); its only unpinned entries are the two local reusable-workflow calls release.yml:27,31 `uses: ./.github/workflows/ci.yml`, which are first-party.

### A31

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "[CI] includes Rust, TypeScript, Dart/Flutter, platform, dependency-audit, release-sealing, and smoke-test jobs" (lines 113-116)

Confirming evidence: Every named category maps to a real job. The actual ci.yml job list is exactly eight: `docs-ui` (:25, named "docs + TypeScript + release contracts"), `ui-e2e` (:102), `flutter` (:148), `linux-flutter` (:217), `rust-runtime` (:321, named "Rust + Dart + smoke + E2E + protocol conformance"), `msrv` (:420), `windows-installer` (:452), `dependency-security` (:563). Mapping: Rust → `rust-runtime` + `msrv`; TypeScript → `docs-ui` (:88 `npm run test:unit`, :91 `npm run build`) + `ui-e2e`; Dart/Flutter → `flutter` + `linux-flutter`; platform → `windows-installer` + `linux-flutter`; dependency-audit → `dependency-security`; release-sealing → `docs-ui` step "Verify release and installer contracts" (:54-60, runs `check-release.mjs`, `release-receipt.test.mjs`, `finalize-release.test.mjs`) plus release.yml jobs `validate-release` (:255) and `publish` (:354); smoke-test → ci.yml:402-403 "Daemon smoke test" `node scripts/smoke.mjs target/debug/jeliyad`, ci.yml:293 `node scripts/smoke.mjs "$bundle/jeliyad"`, and release.yml job `smoke-release` (:323). One wording nuance only: in ci.yml smoke is a STEP inside `rust-runtime`, not a standalone job; the standalone smoke JOB lives in release.yml, which the doc also cites.

### A32

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "The existing agent runner is an intentional, unsandboxed local code-execution surface" (lines 159-160)

Confirming evidence: Confirmed, and the runner's own header says so more bluntly than the doc. It is scripts/jeliya-agent.mjs (named at docs/agent-orchestration.md:22 "JS runner (`scripts/jeliya-agent.mjs`, `scripts/jeliya-fleet.mjs`)"). Execution site: scripts/jeliya-agent.mjs:281-294 `async function claudeWorker(task, ctx) { const child = spawn("claude", ["-p", task, "--output-format", "stream-json", "--verbose", "--max-turns", String(cfg.maxTurns), "--permission-mode", "acceptEdits"], { cwd: ctx.workspace, stdio: ["ignore", "pipe", "pipe"], detached: true })`. Header trust model, jeliya-agent.mjs:27-31: "This is room-driven code execution. A chat message that starts with the trigger phrase becomes either a deterministic echo (--worker echo) or a PROMPT TO THE `claude` CLI running with --permission-mode acceptEdits (--worker claude): an allowed sender effectively gets arbitrary-code / file-write access inside the per-task workspace on this machine." A runtime warning is printed at :188-192. Mitigations exist but are policy, not a sandbox: the default worker is the inert `echo` (:130-134, "Real host execution (`--worker claude`) is arbitrary code/file execution for any allowlisted sender, so it must be [opt-in]"), the allowlist defaults to exactly the room-creator identity (:35-38), non-allowed triggers are silently ignored with no oracle (:39-41), pre-start messages are stale-rejected fail-closed (:43-46), and there is a 15-minute wall-clock cap (:96 `TASK_HARD_CAP_MS = 15 * 60_000`). No OS-level isolation of any kind is applied.

### A33

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: Corollary supporting line 160, "It must remain unavailable to the hosted browser product"

Confirming evidence:

````text
The runner is already architecturally unreachable from the daemon and from any client, which strengthens the doc's position — worth stating rather than leaving implicit. There is no process-spawn RPC anywhere in the Rust workspace: the engine dispatch table (crates/jeliya-core/src/engine.rs:195-328) has no agent-spawn method, and `grep -rn "Command::new\|process::Command" crates/` returns ZERO hits across all three crates. `agents.fleet`/`agent.history` are read-only derivations — crates/jeliya-core/src/fleet.rs:5-11: "Liveness is derived at read time, never stored... Nothing here fabricates a count or a heartbeat: every input is a stored event or a `PeerConnState`." docs/agent-orchestration.md:404-405 states the rule: "The clients never spawn runner processes; the daemon gets no 'spawn agent' RPC. Executing the command on the target machine is deliberately a human step."
````

### A34

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "The existing UI has an install manifest but no service worker or browser room runtime" (line 406)

Confirming evidence:

````text
Both halves confirmed. Manifest exists at ui/public/site.webmanifest with `name`, `short_name`, `icons` (192/512, `"purpose": "any maskable"`), `theme_color`, `background_color`, `"display": "standalone"`, `"start_url": "/"` — which also substantiates the change-map row at doc line 836 (no `id`, no `scope`, no `shortcuts`). No service worker: `grep -rn "serviceWorker|workbox|vite-plugin-pwa|sw.js|sw.ts" ui/ --include=*.ts --include=*.tsx --include=*.html --include=*.json --exclude-dir=node_modules` returns NO matches. No browser room runtime: `grep -rn "indexedDB|IndexedDB|navigator.storage" ui/src ui/public` returns NO matches.
````

### A35

- Source: track `T1-Repo`, investigator `engine-limits-and-capabilities`

Claim: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml" (lines 64-66)

Confirming evidence: Independently re-confirmed at the exact line and cross-checked against the on-disk source I audited. Cargo.toml:15 `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }`, with Cargo.toml:11-14 explaining it is the published v0.1.0-rc.3 tag commit. `git -C /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50 rev-parse HEAD` → `71fbb5007bef4ce83631c94762ec68c2beef3d79`, so every upstream file:line I cite above is from the pinned revision, not a drifted local tree.

### A36

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: Line 77-78: "Rust core and daemon: 71 unit tests passed; one opt-in performance test was ignored."

Confirming evidence: ACCURATE, and still exact at current HEAD 7248fb0. `cargo test -p jeliya-core -p jeliyad` output: jeliya-core `test result: ok. 63 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 4.52s`; jeliyad `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. 63 + 8 = 71 passed. The single ignored test is named in the output: `supervisor::tests::hot_reads_are_fast_on_a_room_with_real_history ... ignored, perf: authors ~1000 events; run explicitly with --ignored` — which matches "one opt-in performance test" precisely, including the opt-in characterization.

### A37

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: Line 80-83: "A full `cargo test --locked --workspace` could not build `jeliya-ffi` because the local environment lacked Dart SDK headers."

Confirming evidence: ACCURATE and reproduced verbatim. `cargo test --locked --workspace` fails: `error: failed to run custom build command for 'jeliya-ffi v0.1.0'` / `thread 'main' panicked at crates/jeliya-ffi/build.rs:63:5: jeliya-ffi build.rs: could not locate the Dart SDK include dir (needed to compile dart_api_dl.c for Dart NativePort posting)`. The build script emits `cargo:rerun-if-env-changed=DART_SDK_INCLUDE` and `FLUTTER_ROOT`. The doc's follow-on characterization — "This is an unverified local FFI prerequisite, not evidence of a product failure or success" — is exactly right: the failure is a missing toolchain, not a code defect.

### A38

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: Line 76 (second clause): "the Vite production build succeeded."

Confirming evidence: ACCURATE at current HEAD. `cd ui && npm run build` (which runs `tsc --noEmit && vite build`) exits 0: `✓ 72 modules transformed.` / `dist/assets/index-Co2Qlrho.js   343.58 kB │ gzip: 103.22 kB` / `✓ built in 533ms`. Typecheck passed too, since `tsc --noEmit` gates the build.

### A39

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: All relative markdown links in the document resolve to real files.

Confirming evidence: ACCURATE — zero broken links. Programmatically extracted and resolved all 22 local link targets: PROFILE.md, capability-status.md, security-threat-model.md, verification-evidence.md, platform-matrix.md, signing-notarization.md, PROTOCOL.md, ../Cargo.toml, ../crates/jeliya-core/src/engine.rs, ../crates/jeliya-core/src/supervisor.rs, ../crates/jeliya-core/src/identity.rs (x2, lines 303 and 843), ../crates/jeliyad/src/main.rs, ../ui/public/site.webmanifest (x2, lines 409 and 836), ../ui/package.json, ../ui/src/lib/client.ts, ../ui/src/main.tsx, ../.github/workflows/ci.yml, ../.github/workflows/release.yml, ../crates/jeliya-core, ../crates/jeliyad — every one reports EXISTS. No link carries a heading fragment, so no cross-file fragment could break.

### A40

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim:

````text
The in-page anchor `[Citations](#citations)` at line 89 resolves.
````

Confirming evidence: ACCURATE. Slugified all headings in the document; `citations` is present, produced by `## Citations` at line 1073. Independently corroborated by `node scripts/check-docs.mjs` passing, since the profile (PROFILE.md:223) states "Local paths and heading fragments must resolve" and the gate enforces it.

### A41

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: All 13 external citation URLs (lines 1075-1087) are live.

Confirming evidence: ACCURATE. `curl -sL -o /dev/null -w '%{http_code}'` against each returned HTTP 200 for all 13: the three iroh.computer pages (wasm-browser, add-a-relay, services/hosting), nine developer.mozilla.org pages, and component-model.bytecodealliance.org/design/worlds.html. Note this validates resolution only, not that each page supports the claim attributed to it.

### A42

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: Repository change map (lines 830-859) distinguishes existing from proposed-new paths.

Confirming evidence: ACCURATE and perfectly self-consistent — no path is falsely presented as existing. The table uses a clean convention: linked entries exist, unlinked code-span entries are proposed-new, and the column header literally reads "Existing or new area". Verified every row with a filesystem test. All 7 linked paths EXIST: ui/src/lib/client.ts, ui/src/main.tsx, ui/public/site.webmanifest, crates/jeliya-core, crates/jeliya-core/src/identity.rs, crates/jeliya-core/src/supervisor.rs, crates/jeliyad. All 19 unlinked paths are MISSING, i.e. correctly proposed-new: ui/src/sw.ts, ui/src/runtime/, ui/src/storage/, ui/src/pairing/, ui/src/invites/, crates/jeliya-protocol/, crates/jeliya-runtime/, crates/jeliya-platform-native/, crates/jeliya-web/, crates/jeliya-control/, crates/jeliya-companion/, crates/jeliya-components/, crates/jeliya-server-peer/, .github/workflows/web-ci.yml, .github/workflows/web-deploy.yml, .github/workflows/companion-release.yml, infra/, docs/adr/, docs/runbooks/.

### A43

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: The frontmatter (lines 1-12) satisfies every required field in docs/PROFILE.md.

Confirming evidence: ACCURATE. scripts/check-docs.mjs:21-32 defines requiredFields as exactly [type, title, description, tags, timestamp, status, implementation_status, verification_status, release_status, audience]. All 10 are present, in that order, no unknown fields (PROFILE.md:103 rejects unknown fields). YAML subset conforms: unquoted ASCII keys, double-quoted string values, flow-style non-empty double-quoted arrays for tags and audience, no nested mappings/anchors/aliases/block scalars.

### A44

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: Every frontmatter enum VALUE is legal per the profile — including `type: "Architecture"` and `status: "proposal"`.

Confirming evidence: ACCURATE, all five controlled values check out against the enums in scripts/check-docs.mjs:33-63. `type: "Architecture"` is the first entry of PROFILE.types (line 34) and PROFILE.md:133 scopes it to "System boundaries, component responsibilities, and proposed technical designs" — an exact fit. `status: "proposal"` is in PROFILE.statuses (line 44); PROFILE.md:174-175 explicitly endorses this case: "A proposed architecture remains `proposal` even if parts of its current-state analysis are verified." `implementation_status: "planned"` is in implementationStatuses (line 47). `verification_status: "partial"` is in verificationStatuses (line 54). `release_status: "unreleased"` is in releaseStatuses (line 60). `audience` has NO controlled vocabulary in either the profile or the checker, so "operators", "product", "release-engineers", and "security-reviewers" are all legal free-form lowercase tokens.

### A45

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: title matches the single H1, per PROFILE.md:116 ("It must match the document's single level-one heading").

Confirming evidence: ACCURATE. `grep -n '^# '` returns exactly one match: line 14 `# Production deployment architecture`, identical to the frontmatter title on line 3. The H1 sits immediately after the closing `---` on line 12, satisfying PROFILE.md:230-231, and all 61 remaining headings start at `##` or deeper.

### A46

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: The documentation gate passes for this file.

Confirming evidence: ACCURATE. `node /home/sekou/AGI/jeliya/scripts/check-docs.mjs` from the repo root prints exactly `docs-check: OK — profile, indexes, titles, and local links are valid.` and exits 0. I also ran the COMMITTED version of the checker (`git show HEAD:scripts/check-docs.mjs`) against the working tree via --root: it also prints OK and exits 0. So the doc passes both the committed gate and the modified one — it does not depend on the uncommitted checker changes.

### A47

- Source: track `T1-Repo`, investigator `tests-links-profile`

Claim: docs/index.md needs an entry for this page, and the checker enforces that.

Confirming evidence: ACCURATE on both halves, and the requirement is already satisfied. PROFILE.md:214 states "Every concept must be reachable from docs/index.md through local Markdown links. Orphaned documents fail CI." The entry exists at docs/index.md:42 under the `## Proposals` group — added by the uncommitted index.md modification (`git diff docs/index.md` shows exactly one added line). I confirmed the rule is NOT vacuous with a negative test: copying docs/ to a scratch root, stripping that one line, and re-running the gate yields `docs/production-deployment.md:1 [document-orphan] document is not reachable from docs/index.md` with exit 1. The index entry's wording also differs from the frontmatter description, which PROFILE.md:68 expressly permits ("descriptive task-oriented entries that need not copy frontmatter verbatim").

### A48

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 800-802: the arithmetic behind "$389 per 30-day month" for two relays at $0.27/hr.

Confirming evidence: Computed: 0.27 * 24 * 30 * 2 = 388.8. Rounds to $389. The stated figure is arithmetically correct for the stated 30-day basis. (Single relay 30-day = $194.40.)

### A49

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 800: "Current Iroh managed relay pricing starts at $0.27 per hour."

Confirming evidence: https://www.iroh.computer/services/hosting: "$0.27/hour and up". "starts at" is a faithful paraphrase of "and up". Rate is current as of this check (2026-07-18).

### A50

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 800-801, implicit: that the $0.27/hr rate is PER RELAY, licensing the x2 multiplication.

Confirming evidence: https://www.iroh.computer/pricing states the unit explicitly: "$0.27/relay/hour", with worked example "$197.10/mo" for 1 relay x 730 hours. The per-relay assumption is correct (though sourced from a page the doc does not cite — see findings).

### A51

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 1077: the hosting page supports the "public-service limitations" attribution.

Confirming evidence: https://www.iroh.computer/services/hosting, public/free tier: "Free to use", "No setup required", "Great for development & testing", limitations "Rate-limited traffic", "No uptime guarantees". Attribution accurate.

### A52

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 508-509 / 152: browser connections are relay-only, because browsers cannot do UDP hole-punching from the sandbox.

Confirming evidence: https://docs.iroh.computer/languages/wasm-browser: "All connections from browsers to somewhere else need to flow via a relay server. This is because we can't port our hole-punching logic in iroh to browsers: They don't support sending UDP packets to IP addresses from inside the browser sandbox." The doc's stated reason matches upstream's actual stated reason and is technically accurate for the UDP path. (See finding re: WebRTC/WebTransport nuance.)

### A53

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 510-511: "Iroh also requires default features to be disabled."

Confirming evidence: https://docs.iroh.computer/languages/wasm-browser: "You need to disable iroh's default features for the Wasm build to succeed" and "To do so, depend on iroh via `iroh = { version = \"1\", default-features = false }`." Confirmed as a requirement, not a recommendation. Side effect noted upstream: this removes metrics in the browser build.

### A54

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 511-512: Iroh "recommends an application-specific `wasm-bindgen` wrapper rather than an off-the-shelf npm package."

Confirming evidence: https://docs.iroh.computer/languages/wasm-browser: "Should you need javascript APIs, we recommend that you write an application-specific rust wrapper crate that depends on iroh and exposes whatever the javascript side needs via wasm-bindgen." Line 511-512's phrasing ("recommends") is correct; line 153's "require" is not.

### A55

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 510 / 547: "The connections remain end-to-end encrypted" over the relay; relays cannot read room content.

Confirming evidence: Confirmed explicitly and in two places. https://docs.iroh.computer/languages/wasm-browser: "Keep in mind that *connections are end-to-end encrypted*, as always with iroh. So even though traffic from browsers is always relayed, it can't be decrypted by the relay." https://docs.iroh.computer/concepts/relays: "Relay servers do not have access to the data being transmitted, as it's encrypted end-to-end" and "All relay traffic is end-to-end encrypted regardless." The doc's line 546-547 caveat that source IPs, routing, timing and volumes remain sensitive metadata is also well-founded — the relay authorization discussion (github.com/n0-computer/iroh/discussions/3168) has maintainers acknowledging the relay learns that "Node ID X talks to Node ID Y".

### A56

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 536-537: "Start with two dedicated managed relays, one in North America and one in Europe."

Confirming evidence: Near-verbatim match with upstream. https://docs.iroh.computer/add-a-relay: "For production, run at least two relays in different geographic regions, for example one in North America and one in Europe." Note upstream says "at least two" — the doc's "start with two" is a faithful floor.

### A57

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 538 / relay design: relays are stateless and retain no room history.

Confirming evidence: https://docs.iroh.computer/add-a-relay: "Iroh's relay architecture is uniquely suited to multi-relay deployments because relays are stateless." https://docs.iroh.computer/concepts/relays: "Unlike traditional servers, relay servers are stateless. They don't store your application data; they just facilitate connections."

### A58

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Line 1076: failover behavior is as the doc's citation describes.

Confirming evidence: https://docs.iroh.computer/add-a-relay: "Clients automatically fail over between relays in your list, so adding capacity or surviving an outage is just a matter of running more relay processes." https://docs.iroh.computer/concepts/relays: "Iroh can attempt to connect to multiple relays automatically; as long as one is reachable, your peers find each other." Automatic client-side failover confirmed.

### A59

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Lines 539-543 / assumption #3 (1040), core half: that endpoint-bound, short-lived relay credentials exist at all.

Confirming evidence: CONFIRMED as a real capability today, which downgrades this from 'possible fantasy' to 'integration-shape risk'. https://docs.iroh.computer/add-a-relay: "it mints a short-lived access token scoped to your endpoint's key". https://docs.iroh.computer/concepts/relays: "the SDK mints a short-lived, signed access token scoped to that endpoint's identity", "Your endpoint presents a token, not the key itself", plus live revocation: "Delete an API key and its access is withdrawn from your relays right away, including connections that are already open." Endpoint-binding is meaningfully enforced (the token is bound to an endpoint key the iroh handshake authenticates), so a leaked token is not usable by an arbitrary bearer. The unsupported part is delegated issuance only — see findings.

### A60

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Risk #2 (line 1062) is correctly classified as a risk rather than a known blocker.

Confirming evidence: Adjudicated per the task's load-bearing question. The token primitive is documented and shipping (quotes above), so this is NOT a known blocker. But it is a sharper risk than the doc conveys: zero documentation exists for third-party/delegated issuance, and https://docs.iroh.computer/concepts/relays states "Your API key never leaves your application" — the opposite topology from a browser fetching a credential from relay-auth.jeliya.ai. Verdict: genuine risk, understated, needs a vendor-confirmation gate before Phase 3.

### A61

- Source: track `T2-External`, investigator `iroh-pricing-and-relay`

Claim: Citation URLs at lines 1075-1077 resolve and say what the doc attributes to them.

Confirming evidence: All three Iroh URLs fetched successfully on 2026-07-18: docs.iroh.computer/languages/wasm-browser (browser build instructions, relay-only, E2E encryption, feature flags, wrapper guidance — all four attributions verified); docs.iroh.computer/add-a-relay (production guidance, authentication, stateless failover, two-region recommendation — all four attributions verified); www.iroh.computer/services/hosting (public-service limitations and "$0.27/hour and up" — both attributions verified, though the page does not state the per-relay unit the doc relies on).

### A62

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Line 163-164: the substance of issue #121 — "live fanout visible to an unproven provisional dialer during an open join window" — accurately paraphrases the upstream issue.

Confirming evidence: Issue #121 body: "a provisional accept reaching `Connected` calls `engine.on_connect` (node.rs:1385), which inserts the unproven dialer into the engine's peer set — so `store_and_fanout` pushes every newly accepted event (including live chat published during the `--accept-joins` window) to it" and "the exact #112 threat actor — an uninvited dialer knowing only the room id and the admin's address — no longer gets room *history*, but still receives all chat *published while it stays connected* during an open join window." The doc's one-liner is a faithful compression of this for the pinned revision.

### A63

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Line 164-165: the substance of issue #119 — "some store holes incompletely healable" — accurately paraphrases the upstream issue's residual, including the "some" qualifier.

Confirming evidence: Issue #119 title: "[SYNC] store_and_fanout swallows store.insert errors after fold acceptance, leaving a permanent store hole". Body, Residuals section: "A hole in a region no peer serves (pure chat outside every window and outside the membership closure) never heals." The doc's "some … incompletely healable" correctly reflects that the general case self-heals from peers post-#118 while a specific class does not.

### A64

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Line 166: "the latter needs repair or a fail-loud integrity response" correctly names the remediation options for #119.

Confirming evidence: Issue #119, Fix direction: "Either make fold-accept transactional with the insert (ingest → insert → only then commit fold state), or track failed inserts for bounded retry …, or at minimum surface a CRITICAL trust decision so the operator knows the store is degraded." The upstream fix PR #132 is titled "fix(sync): retry failed store inserts and surface store degradation (#119)" — i.e. it implemented exactly the repair-plus-fail-loud pair the doc names.

### A65

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Line 64-66: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml".

Confirming evidence: /home/sekou/AGI/jeliya/Cargo.toml:15 → `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }`. Also matches orchestrator ground truth.

### A66

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Implicit premise throughout: the pinned rev is a reviewed, published upstream revision (doc line 532, "Every release pins and qualifies an exact upstream revision").

Confirming evidence: `gh api repos/kortiene/iroh-room/git/refs/tags` → refs/tags/v0.1.0-rc.1, rc.2, rc.3; `gh api .../tags` → v0.1.0-rc.3 = 71fbb5007bef4ce83631c94762ec68c2beef3d79. The pinned rev is exactly the v0.1.0-rc.3 tag commit ("chore(release): bump shipping crates to 0.1.0-rc.3", 2026-07-16T05:16:35Z). The repo is public (`gh api repos/kortiene/iroh-room` → "private":false, "visibility":"public", fork:false).

### A67

- Source: track `T2-External`, investigator `upstream-issues`

Claim: Line 887-888: "production work does not continue with upstream issue #121 exploitable and unmitigated" is a valid gate as written.

Confirming evidence: The gate is phrased in terms of exploitability rather than issue state, so it survives the issue's closure — it is satisfied by repinning past 58aca4ba. Flagged only as incomplete (see the MISSING finding on #126), not as wrong.

### A68

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: CORRECTION TO ORCHESTRATOR HYPOTHESIS — doc lines 316-317 + 966-967: Ed25519 in SubtleCrypto IS shipped across every browser in the doc's own Phase 4 support matrix. The Phase 4 gate is NOT unsatisfiable on Ed25519 grounds, and I found no evidence for a high-severity finding there.

Confirming evidence: https://caniuse.com/mdn-api_subtlecrypto_sign_ed25519 gives Chrome 137+, Edge 137+, Firefox 129+, Safari 17.0+, Safari on iOS 17.0+, Chrome for Android 150, Opera 121+ (Samsung Internet is the sole holdout — reported separately). Firefox shipped in 129 (Aug 2024, Bugzilla 1804788), Safari in 17.0 (Sept 2023), Chrome in M137: per https://blogs.igalia.com/jfernandez/2025/08/25/ed25519-support-lands-in-chrome-what-it-means-for-developers-and-the-web/ , "Chrome M137 is the first stable version shipping the Ed25519 feature enabled by default" (May 2025), "joining Safari and Firefox in their support". Against a July 2026 assessment date, the "latest two releases" of Chrome, Edge, Firefox, and Safari, plus current iOS Safari and Android Chrome, all exceed these floors with more than a year of margin. MDN https://developer.mozilla.org/en-US/docs/Web/API/SubtleCrypto/sign documents Ed25519 as one of five supported sign algorithms with a dedicated section. The doc's hedge "when browser compatibility ... pass" is therefore satisfied today; only the second half of the hedge ("exact wire interoperability") remains live, and it is live for a reason the doc does not state — see the verification-strictness finding.

### A69

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc lines 374-375: "URI fragments are processed by the browser and are not included in the HTTP request or Referer header."

Confirming evidence: Both halves confirmed at the two MDN pages cited at 1082-1083. https://developer.mozilla.org/en-US/docs/Web/URI/Reference/Fragment : "The fragment is not sent to the server when the URI is requested; it is processed by the client (e.g., the browser) after the resource is retrieved." https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Referer : "The Referer header can contain an origin, path, and querystring, and may not contain URL fragments (i.e., #section) or username:password information", and "URL fragments (i.e., #section) and user info ... are not included. Origin, path, and query string may be included, depending on the referrer policy." The transport-level claim is accurate as written; my finding against this section concerns client-side retention, not this sentence.

### A70

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc lines 616-617: "Do not depend on hosted-page access to loopback addresses; the relevant browser policy is still experimental and platform-dependent."

Confirming evidence: Accurate. https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Permissions-Policy/loopback-network carries the banner verbatim: "Experimental: This is an experimental technology. Check the Browser compatibility table carefully before using this in production." The page confirms the directive "controls whether the current document is allowed to make network requests to loopback addresses", with default allowlist `self`. The doc's operational conclusion — do not build a dependency on it — is the correct posture for an experimental, single-engine policy.

### A71

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc lines 432-433: "Persistent storage reduces automatic eviction but does not prevent user deletion."

Confirming evidence: Accurate, and appropriately hedged. https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria : persistent data "is only evicted, or deleted, if the user chooses to, by using their browser's settings", and the quota-pressure eviction mechanism "skips over origins that have been granted data persistence by using navigator.storage.persist()". The doc's weaker phrasing ("reduces") is in fact better calibrated than MDN's absolute phrasing, because WebKit's ITP eviction is a separate mechanism from quota eviction. The grant rules the doc leaves implicit: "In Firefox, when a site chooses to use persistent storage, the user is notified with a UI popup that their permission is requested", whereas "Safari and most Chromium-based browsers, such as Chrome or Edge, automatically approve or deny the request based on the user's history of interaction with the site and do not show any prompts to the user."

### A72

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc lines 443-447: a service worker "is not a permanent room peer or an agent host", "browsers terminate long-running service-worker work", and "Browser peers are available while the application is active."

Confirming evidence: Accurate and well-supported by the page cited at 1080. https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Offline_and_background_operation : "browsers may stop service workers when they think it is appropriate. For example, if a service worker has been inactive for a while, it will be stopped." Concrete Chrome limits given there: the service worker is likely closed if "It has been idle for 30 seconds", "It has been running synchronous JavaScript for 30 seconds", or "The promise passed to waitUntil() has taken more than 5 minutes to settle". The same page independently supports the doc's activity constraint: "issuing a background sync request may only be made while the main app is open". The doc's product-copy gate at line 974 ("product copy makes no durable background-availability claim") follows correctly from this.

### A73

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc line 428 / citation 1079: OPFS is a viable, broadly available storage tier.

Confirming evidence: https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system : "Baseline Widely available. This feature is well established and works across many devices and browser versions. It's been available across browsers since March 2023", available only in secure contexts. Availability supports the doc's use of OPFS; my separate finding concerns only the worker-only constraint on synchronous access handles, not availability.

### A74

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: All MDN citation URLs at doc lines 1078-1087 resolve and support what is attributed to them (one weak fit noted).

Confirming evidence: I fetched each of the six MDN URLs in my track plus the two additional MDN links, and all returned live content matching the attributed topic: 1078 storage quotas/eviction (supports "best-effort and persistent ... eviction boundaries", and additionally documents the Safari 7-day rule the doc omits); 1079 OPFS (supports "availability, worker support"); 1080 offline/background operation (supports "service-worker execution boundaries"); 1081 Background Synchronization API (supports "limited browser availability" — MDN's own banner reads "Limited availability"); 1082 URI fragment (supports "fragment processing in the browser rather than the HTTP request"); 1083 Referer (supports "exclusion of URL fragments from Referer values"); 1084 SubtleCrypto sign (supports "browser Ed25519 signing support"); 1086 loopback-network Permissions Policy (supports "experimental"). The one weak fit is 1085 SubtleCrypto unwrapKey, attributed "wrapped and nonextractable browser-key behavior": the page resolves and does define the `extractable` flag, but it does not discuss nonextractable-key behavior, and it does not support the mechanism the doc attaches it to — reported as a separate finding.

### A75

- Source: track `T2-External`, investigator `browser-platform-facts`

Claim: Doc line 393 and 396-397: preserving identity-bound tickets and the two-step onboarding flow, rather than a generic bearer invitation.

Confirming evidence: Independently corroborated as the right call by RFC 9700 §2.1.2, which recommends against bearer credentials returned in the authorization response because they "are vulnerable to access token leakage and access token replay" and lack "sender-constraining mechanisms". Identity binding is precisely a sender-constraining mechanism, so the doc's insistence at line 399-400 that "a generic holder-bearer invitation is a different capability model and must not replace identity binding implicitly" is the security-correct position and is the main thing limiting the blast radius of the fragment-retention vectors I report separately.

### A76

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: style-src 'self' with NO 'unsafe-inline' (line 586) does NOT break the current React app. This was the review brief's top suspicion; it is not a problem.

Confirming evidence:

````text
Four independent checks. (1) No CSS-in-JS: ui/package.json dependencies are exactly `react` ^18.3.1 and `react-dom` ^18.3.1; `grep -rn -E "styled-components|@emotion|stitches|goober|linaria|vanilla-extract|styled-jsx" ui/` (excluding node_modules) returns zero matches. (2) The 22 React inline styles that do exist (e.g. ui/src/components/Timeline.tsx:861 `style={{ width: '32%' }}`, ui/src/components/Sidebar.tsx:160, ui/src/components/FleetDashboard.tsx:233) are applied via the CSSOM, which CSP does not intercept — node_modules/react-dom/cjs/react-dom.development.js:2804-2829, `function setValueForStyles(node, styles) { var style = node.style; ... style.setProperty(styleName, styleValue); } else { style[styleName] = styleValue; }`. Only `<style>` elements and `style` attributes parsed from markup are gated by style-src. (3) `grep -rn "<style" ui/src ui/index.html` returns zero matches. (4) The production build emits an external stylesheet, not an inline block: ui/dist/index.html line 27 `<link rel="stylesheet" crossorigin href="/assets/index-DA1iuxYh.css">`.
````

### A77

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: script-src 'self' 'wasm-unsafe-eval' (line 585) is spelled and used correctly, and 'wasm-unsafe-eval' does what the doc needs.

Confirming evidence:

````text
CSP3 §4.5.1 gates WebAssembly byte compilation on a source expression that is "an ASCII case-insensitive match for the string 'wasm-unsafe-eval'"; it permits Wasm compilation without permitting JS `eval`/`new Function`, which is precisely the trade this app wants. `'self'` with no nonce or hash is sufficient for the current build: ui/dist/index.html contains exactly one script, `<script type="module" crossorigin src="/assets/index-Co2Qlrho.js"></script>` — no inline script, no Vite modulepreload polyfill inline block.
````

### A78

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: worker-src 'self' (line 590) IS sufficient for service worker registration; the SW script does not additionally need a script-src entry.

Confirming evidence: MDN, Content-Security-Policy/worker-src: the directive "specifies valid sources for Worker, SharedWorker, or ServiceWorker", with the worked example showing `navigator.serviceWorker.register("https://not-example.com/sw.js")` blocked by worker-src. Fallback order is worker-src → child-src → script-src → default-src; because worker-src is explicitly present at line 590, script-src is never consulted for the SW script URL.

### A79

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: base-uri 'none' (line 593) is valid, and is a better choice than 'self' here.

Confirming evidence:

````text
CSP3 §6.3.1 defines base-uri as taking a source list, and the source-list grammar permits `'none'`. base-uri does not fall back to default-src (MDN default-src page lists it among directives default-src "does not influence"), so listing it explicitly is required, not optional. `'none'` blocks every `<base>` element; ui/index.html and ui/dist/index.html contain no `<base>` tag, so nothing is lost and base-tag injection is fully closed rather than merely same-origin-restricted.
````

### A80

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: form-action 'none' (line 596) is valid, and frame-ancestors 'none' (line 595) is correctly present.

Confirming evidence:

````text
CSP3 §6.4.1 (form-action) and §6.4.2 (frame-ancestors) both take source lists, and the grammar permits `'none'`. Both are in MDN's list of directives that do NOT fall back to default-src, so both had to be stated explicitly — the doc got this right. There are no `<form>` elements requiring submission in the current app.
````

### A81

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: frame-src 'none' (line 594) and object-src 'none' (line 592) are redundant under default-src 'none', but not errors.

Confirming evidence: MDN default-src lists both frame-src and object-src among the directives that fall back to default-src; CSP3 notes frame-src "continues to defer to child-src if not present (which defers to default-src in turn)". Both therefore already resolve to 'none'. Keeping them is standard defensive practice and, for frame-src specifically, gives the doc's note at lines 600-602 ("add only the reviewed isolated component origin to `frame-src`") a concrete line to edit. No change needed.

### A82

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: The COEP guidance at lines 615-616 ("Add COEP only if Wasm threading requires cross-origin isolation") is CORRECT. Cross-origin isolation is not mandatory for this design.

Confirming evidence: OPFS synchronous access handles do NOT require cross-origin isolation — MDN FileSystemFileHandle.createSyncAccessHandle lists exactly two requirements, "available only in secure contexts (HTTPS)" and "This feature is only available in Dedicated Web Workers"; there is no COOP/COEP or SharedArrayBuffer requirement. Only shared memory needs isolation — MDN SharedArrayBuffer: "the constructor on the global object is hidden, unless the two headers mentioned above are set" (Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy), and "When creating a WebAssembly.Memory with shared: true, the same COOP/COEP headers are required to share it between workers." The doc's Iroh browser build is relay-only single-threaded (lines 508-520) with no threading requirement stated. So COEP is correctly optional, and the doc's conditional framing is the right one.

### A83

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: Cross-Origin-Opener-Policy: same-origin (line 610) is safe for this app — it breaks no existing flow.

Confirming evidence: COOP same-origin severs opener relationships with cross-origin popups. The only popup in the app already opts out: ui/src/App.tsx:920 `window.open(\`${ISSUE_URL}?${params.toString()}\`, '_blank', 'noopener,noreferrer');`. The remaining external links (ui/src/components/ui.tsx:402, ui.tsx:493, ui/src/components/RightPanel.tsx:826) are `target="_blank"` anchors, which get implicit noopener in modern browsers. No `postMessage` or `opener` usage exists in ui/src. It is also half of the future COEP flip, which is a sensible thing to have already paid for.

### A84

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: Omitting X-Frame-Options alongside frame-ancestors 'none' (line 595) is the correct call, not an oversight.

Confirming evidence: X-Frame-Options is only load-bearing for browsers that do not support frame-ancestors — essentially IE11 and legacy Edge. This app cannot run in those browsers at all: it requires WebAssembly, service workers, OPFS, and (per line 597) require-trusted-types-for, which MDN marks "Baseline 2026 — newly available since February 2026". Every browser that can execute the app enforces frame-ancestors. Adding XFO would be dead config with a small chance of conflicting-header confusion. I recommend against adding it.

### A85

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: font-src 'self' (line 588) and manifest-src 'self' (line 591) are correct and sufficient.

Confirming evidence:

````text
font-src: `grep -o "@font-face" ui/dist/assets/*.css` returns 0; the only font declaration in the built CSS is `font-family:var(--mono)`, i.e. system font stacks. No web font is loaded, so 'self' is sufficient and no `data:` grant is needed on font-src. manifest-src: ui/index.html line 15 `<link rel="manifest" href="/site.webmanifest" />` is same-origin, and ui/public/site.webmanifest references only same-origin icons (`/icon-192.png`, `/icon-512.png`).
````

### A86

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: Referrer-Policy: no-referrer (line 609) is correct and consistent with the doc's own privacy requirement.

Confirming evidence: Line 381 requires that invite material never appear in "reports, query strings, or URL paths". no-referrer is the strictest option and prevents app.jeliya.ai URLs leaking to the external destinations the app does navigate to (ui/src/l10n/tokens.ts:94 `export const ISSUE_URL = 'https://github.com/kortiene/jeliya/issues/new';`, opened at App.tsx:920). No same-origin analytics depends on Referer.

### A87

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: The cache policy at lines 621-625 is correct as far as it goes.

Confirming evidence: `no-cache` on index.html, the service worker, and public env config is the right directive — it means "store but revalidate", not "do not store", so clients still get a fast conditional request while never running a stale shell. `public, max-age=31536000, immutable` on content-hashed assets is the conventional one-year maximum and is safe precisely because the filenames are hashed (ui/dist/assets/index-Co2Qlrho.js, index-DA1iuxYh.css). "Keep N and N-1 assets available through rollout and rollback" (line 625) is not optional garnish — a client still running the previous index.html will request the previous chunk hashes, and deleting them turns a deploy into an outage for anyone mid-session. Getting this stated is a genuine strength of the block.

### A88

- Source: track `T2-External`, investigator `csp-and-headers`

Claim: Cross-Origin-Resource-Policy: same-origin (line 611) is the correct default for this app.

Confirming evidence: The app exposes no resource intended for cross-origin embedding — every asset in ui/dist is consumed by app.jeliya.ai itself. same-origin is the strictest CORP value and blocks other sites from loading these responses, closing off cross-site read side channels. It is also forward-compatible with the conditional COEP at line 615.

### A89

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 62-63: "The assessed HEAD is 14 commits and 142 changed files after that Jeliya commit."

Confirming evidence: Accepted per orchestrator ground truth (verified exact at 4d4621c9). Not re-derived. Note for the record: current repo HEAD is 7248fb067b3b9096070d2add484911e1c04d203e (git rev-parse HEAD), so the figure is stale relative to today's tree but correct as of the stated assessment boundary.

### A90

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 64-66: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml."

Confirming evidence: Cargo.toml:15 `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }`. Corroborated in Cargo.lock: iroh-rooms 0.1.0-rc.3 `source = "git+https://github.com/kortiene/iroh-room?rev=71fbb5007bef4ce83631c94762ec68c2beef3d79#71fbb5007bef4ce83631c94762ec68c2beef3d79"`.

### A91

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Lines 46-48 and 214: the effort estimates are internally consistent with the phase table (as calendar weeks).

Confirming evidence: Phases 0+1+2+3 = (1+3+5+2) to (2+5+7+3) = 11 to 17, matching lines 46 and 214. Phases 0+1+3+4 = (1+3+2+10) to (2+5+3+14) = 16 to 24, matching line 214's static-PWA cell. The arithmetic is sound; only the UNIT LABEL is wrong (see critical finding). Worth noting the static-PWA figure is if anything conservative, since that path would not need Phase 1's "companion pairing/control protocol" (898) or `crates/jeliya-control/` (850).

### A92

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Lines 221-223: "the repository does not yet contain its storage or network runtime" for a browser peer.

Confirming evidence: ACCURATE. crates/jeliya-core/src/ contains only engine.rs, error.rs, fleet.rs, identity.rs, localstate.rs, materializer.rs, supervisor.rs, lib.rs — no wasm, IndexedDB, or OPFS code. No service worker exists: `ls ui/src/sw.ts ui/public/sw.js` returns "No such file or directory" for both, confirming the change map's line 837 (`ui/src/sw.ts` listed as new) and line 405-406 ("install manifest but no service worker or browser room runtime").

### A93

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Change map line 834: ui/src/lib/client.ts has a "production same-origin `/ws` assumption" that must be replaced with transport interfaces.

Confirming evidence: ACCURATE. ui/src/lib/client.ts:22 `export const DEFAULT_DAEMON_URL = 'ws://127.0.0.1:7420/ws';` and :40 `return `${scheme}://${window.location.host}/ws`;` with the comment at :33-34 "the control socket is same-origin: derive it from the page host." The identified change is real and correctly scoped.

### A94

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 112: "The React UI works against the loopback daemon and has mock browser coverage."

Confirming evidence: ACCURATE and consistent with the transport code above. ui/src contains 16,135 lines of TS/TSX including ui/src/lib/conformance/{harness.ts, conformance.mock.test.ts}, matching the "mock browser coverage" claim.

### A95

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 1036 planning assumption: "The first supported production matrix is desktop-focused."

Confirming evidence:

````text
ACCURATE and corroborated. docs/platform-matrix.md:58 "| iOS app | no scaffold or engine build | none | none | excluded |" and :56 Android Flutter app "excluded" with "no cross-network, NAT, direct, or relay evidence." (This assumption is correct — but it is what falsifies the line 205 browser-compatibility cell, per that finding.)
````

### A96

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Lines 861-862: "Portable Iroh Rooms storage, network, and blob interfaces should preferably land upstream. A long-lived private fork is a security and maintenance liability."

Confirming evidence: ACCURATE and, in my assessment, the single most important sentence in the change map. Corroborated by Cargo.lock: iroh-rooms, iroh-rooms-core, and iroh-rooms-net are all pinned to one git rev, and iroh-rooms-core owns the rusqlite dependency — a fork would mean maintaining the store layer of a networking SDK. The doc is right about this; it simply fails to schedule it (see the MISSING finding).

### A97

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 866 as applied to Phases 2, 3, and 5: "No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase."

Confirming evidence: HOLDS for these phases; I checked each gate against earlier deliverables and found no inversion. Phase 2's gate (921-930) is evaluable from Phase 0-2 outputs — signing ownership is confirmed in Phase 0 ("confirm DNS, CDN, relay, and signing ownership", 879), the pairing protocol comes from Phase 1 (898), and forced-relay runs are already demonstrated capability per lines 104-107. Phase 3's gate (942-950) is evaluable from its own deliverables (936-940). Phase 5's gate (987-996) is evaluable from its own deliverables (981-985). The two inversions I found are in Phase 0 and Phase 1 and are reported as findings.

### A98

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Lines 168-193: "Why the loopback daemon must not be public" and the prohibition on giving jeliyad a public-listen flag (change map line 845).

Confirming evidence: ACCURATE and well-founded. crates/jeliyad is described in its own manifest as "Jeliya daemon: local-only WebSocket server implementing docs/PROTOCOL.md over jeliya-core" (crates/jeliyad/Cargo.toml:7). The architectural conclusion — that a reverse proxy adds no security model and invalidates the Host/Origin assumptions — is sound and is the strongest reasoning in the document. Nothing in my review disputes it.

### A99

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Line 202 hybrid Security cell: "highest design complexity", and line 212: "Medium-high" operational complexity.

Confirming evidence: ACCURATE self-assessment, and I credit the doc for stating it. My disagreement is not that the doc hides the complexity — it discloses it — but that it never weighs it against the alternatives it rejects, so the admission has no effect on the decision.

### A100

- Source: track `T3-Judgment`, investigator `architecture-decision`

Claim: Lines 1058-1071: the ten highest-risk unknowns.

Confirming evidence: ACCURATE and well-chosen as a list. Unknown #1 (upstream traits), #3 (multi-device vs existing membership history), #4 (web-controller authority), #6 (upstream #121/#119), #7 (signing timing), and #9 (Safari/iOS storage) each independently threaten a phase. My criticism is structural, not about the list's content: several of these unknowns are contradicted by confident assertions in the decision table at lines 202-214, and none of them is assigned to a phase or an owner.

### A101

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 30, 170-173: "`jeliyad` must never be exposed through a public listener" and "binds only `127.0.0.1` and exposes no flag for a non-loopback address."

Confirming evidence: ACCURATE. crates/jeliyad/src/main.rs:5-7: "Local-only by construction: the listener binds `127.0.0.1` and nothing else — there is no flag to bind another interface." Verified in code: `bind_loopback` (main.rs:364-388) constructs `SocketAddr::from((Ipv4Addr::LOCALHOST, candidate))` and is the only bind path (called at main.rs:170). The Args struct (main.rs:50-81) has `--port` but no address/interface flag.

### A102

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 176-178: "`/api/session` gives the token only to the expected loopback browser shape. Its documented threat model explicitly excludes hostile same-user processes and shared multi-user service operation."

Confirming evidence:

````text
ACCURATE on both halves. crates/jeliyad/src/serve.rs:210-228 implements the shape check (loopback `Origin`, or absent `Origin` plus `sec-fetch-site: same-origin|none`), returning 403 otherwise. docs/PROTOCOL.md:89-95: "The trust boundary is a single-user machine: any process running as the same user can already read the 0600 portfile... The `Origin` / `Sec-Fetch-Site` checks on `/api/session` defend against hostile web pages in a real browser... not against a local non-browser process (`curl` can set any header). A shared multi-user machine is therefore out of scope." Also restated in serve.rs:204-209.
````

### A103

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Line 187: "The current UI transports bearer material in WebSocket and upload query URLs."

Confirming evidence: ACCURATE. ui/src/lib/client.ts:95 (`uploadFileToRoom`): `if (token) url.searchParams.set('token', token);` against `/api/files/share`; ui/src/lib/client.ts:255 (WebSocket connect): `withToken.searchParams.set('token', token);`. The daemon accepts both forms — crates/jeliyad/src/serve.rs:283-292 reads the token from the `token` query param first, then falls back to `Authorization: Bearer`.

### A104

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 64-66: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml."

Confirming evidence: ACCURATE. Cargo.toml:15: `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }` (matching the orchestrator's ground truth). The comment at Cargo.toml:10-14 identifies it as the published v0.1.0-rc.3 tag commit.

### A105

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 69-72: "The security threat model also retains a stale statement that the public lockfile has not been repinned."

Confirming evidence: ACCURATE — I located the exact contradiction. docs/security-threat-model.md:122-124: "Because the public Jeliya lockfile does not yet resolve that code, upstream publication and Jeliya repinning are mandatory before release qualification." That is contradicted within the same file by line 32 (the rc.3 pin row) and line 80: "the room-scoped remediation is published and pinned at `71fbb500...` (iroh-room tag v0.1.0-rc.3)", and by Cargo.toml:15.

### A106

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 108-110: "Files are bounded to 100 MiB, BLAKE3-verified, confined against arbitrary local-file sharing... There is deliberately no central inbox or guaranteed offline delivery."

Confirming evidence: ACCURATE for the SHARE direction. crates/jeliya-core/src/supervisor.rs:291-323 (`assert_shareable_path`) rejects any canonical path outside the data dir, plus the blob store and the identity/DB/state files. The size cap is enforced at supervisor.rs:1713-1719 against `MAX_SHARED_FILE_BYTES` (docs/PROTOCOL.md gives 104_857_600). The no-inbox statement is verbatim in the error at supervisor.rs:1968-1976: "there is no central inbox and no guaranteed offline delivery". Hash verification: supervisor.rs:1943-1951 returns a hard `HashMismatch` error. (See the separate finding: the FETCH destination `save_dir` is not confined.)

### A107

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Line 111: "Pipes are restricted to numeric loopback targets and an authorized peer."

Confirming evidence: ACCURATE. crates/jeliya-core/src/supervisor.rs:2056-2070: the target is parsed with `SocketAddr::from_str` (numeric only — a hostname fails to parse), then `if !is_loopback_target(&target)` returns `PipeDenied` with the hint "pipes may only forward to 127.0.0.0/8 or ::1". The authorized peer is a single parsed `IdentityKey` passed as `&[peer]` to `node.pipe_expose` (supervisor.rs:2069-2093).

### A108

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 160, 452: "The existing agent runner is an intentional, unsandboxed local code-execution surface. It must remain unavailable to the hosted browser product" / "remains disabled in hosted mode."

Confirming evidence: ACCURATE, and stronger than the doc states. docs/security-threat-model.md:144-150: "The runner is a deliberate local code-execution surface. The daemon and browser do not enable it automatically." docs/agent-guide.md:28: "This is room-driven code execution." Structurally, the runner is an external Node script — docs/agent-orchestration.md:22: "JS runner (`scripts/jeliya-agent.mjs`, `scripts/jeliya-fleet.mjs`)" — that spawns its own daemon; it is not reachable from the protocol. The engine's only agent methods are read-only projections: `agents.fleet` and `agent.history` (crates/jeliya-core/src/engine.rs:320-325). Worth noting for line 336: there is no agent-invocation RPC to grant or withhold scope over.

### A109

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 137-138 and 388-389: invite "cancellation and safer default expiry" are missing today; the plan adds `invite.cancel`.

Confirming evidence:

````text
ACCURATE. `grep -rn "invite.cancel|invite_cancel" crates/ docs/PROTOCOL.md` returns nothing. docs/PROTOCOL.md (invite.create row) confirms the unsafe default: "`expiry` accepts a duration string... or a number of seconds; omitted ⇒ single-use, not time-boxed." The engine's `expiry_spec` (crates/jeliya-core/src/engine.rs:636-648) maps a missing expiry to `None`.
````

### A110

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 275-277 (TB1): "CSP reduces injection risk but cannot make a deliberately malicious first-party build trustworthy." And line 285 (TB4): "Signatures prevent forgery; they do not prevent an authorized peer from copying content."

Confirming evidence: ACCURATE and correctly reasoned — these are the two most important honest statements in the document. TB1's framing is right: CSP is same-origin-scoped policy served by the compromised party, and it cannot restrict top-level navigation exfiltration in any case (the `navigate-to` directive was removed from CSP). TB4 matches docs/security-threat-model.md:209: "An authorized room member can copy data already shared with that member; removal cannot recall it." My findings above extend these rather than contradict them.

### A111

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 407-410: "The existing UI has an install manifest but no service worker or browser room runtime." Line 836: the manifest needs "a stable app ID, scope".

Confirming evidence: ACCURATE. `find ui -name "sw*" -not -path "*/node_modules/*"` returns nothing, and `ls ui/src/` shows only App.tsx, components, l10n, lib, main.tsx, styles.css, vite-env.d.ts. ui/public/site.webmanifest contains name, short_name, description, icons, theme_color, background_color, display, start_url — and no `id` and no `scope`, exactly as line 836 claims.

### A112

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Line 835: `ui/src/main.tsx` must "run secure invite cleanup and capability bootstrap before React mounts" (i.e. no such bootstrap exists today).

Confirming evidence: ACCURATE. ui/src/main.tsx has no fragment handling of any kind; it calls `createClient()` at module top level (line 8) before `createRoot(...).render(...)`. Note for implementation: because `createClient()` runs at import time, an invite bootstrap must precede this module in the import graph, not merely precede the `render` call.

### A113

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 800-802: "Current Iroh managed relay pricing starts at $0.27 per hour. Two continuously running relays therefore start near $389 per 30-day month before bandwidth or SLA charges."

Confirming evidence: ACCURATE on both the price and the arithmetic. https://www.iroh.computer/services/hosting lists the Cloud plan at "$0.27/hour and up", and the free public tier as "Rate-limited traffic" with "No uptime guarantees", positioned for "development & testing". Arithmetic: 0.27 x 24 x 30 x 2 = $388.80. The "before bandwidth" qualifier is also correct — the pricing page states no separate egress rate, which is itself why the finding about uncapped minting matters.

### A114

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 152-153, 208, 508-512: "Browser Iroh is supported, but current browser connections are relay-only and require an application-specific `wasm-bindgen` wrapper."

Confirming evidence: ACCURATE per the cited source. https://docs.iroh.computer/languages/wasm-browser is cited at line 1075 for exactly these properties, and the corroborating architectural fact (relays forward by EndpointId and cannot decrypt) is confirmed by https://docs.iroh.computer/about/faq: "They accept encrypted traffic for iroh endpoints which are connected to them, forwarding it to the correct destination based on the EndpointId only." The end-to-end-encryption claim at line 510 is likewise supported.

### A115

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Lines 373 and 1082-1083: "URI fragments are processed by the browser and are not included in the HTTP request or Referer header."

Confirming evidence: ACCURATE as stated about the wire. The Fetch standard (https://fetch.spec.whatwg.org/) serializes URLs "with exclude fragment set to true" for reporting, and fragments have never been transmitted at the HTTP protocol level. This is precisely why my service-worker finding is separate and non-obvious: the fragment is absent from the network request and present in the in-process `Request` object the service worker sees.

### A116

- Source: track `T3-Judgment`, investigator `security-attack`

Claim: Line 184: "There is no tenant, account, authorization-domain, quota, or public audit model."

Confirming evidence: ACCURATE, and load-bearing for the relay-auth finding. crates/jeliya-core/src/identity.rs:104-154 creates exactly one identity/root key and one device key per data dir, with no account, registration, or external authority. crates/jeliya-core/src/engine.rs:206-212 exposes `identity.create` with no parameters and no admission control. This is what makes "proof of possession" an empty admission predicate at relay-auth.jeliya.ai.

### A117

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: Phases 0+1+2+3 sum to the headline "11 to 17 engineering weeks" (line 46).

Confirming evidence: Phase headings: line 869 `Phase 0: freeze the claim boundary, 1 to 2 weeks`; line 890 `Phase 1: production identity and protocol primitives, 3 to 5 weeks`; line 912 `Phase 2: companion-backed vertical slice, 5 to 7 weeks`; line 932 `Phase 3: production web and relay operations, 2 to 3 weeks`. Lows: 1+3+5+2 = 11. Highs: 2+5+7+3 = 17. Line 46 says `11 to 17 engineering weeks`. ARITHMETIC CORRECT — and line 952 confirms Phase 3 is the boundary: `This is the first production launch gate.` The unit label is the problem, not the sum.

### A118

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: Phase 4's 10-14 weeks matches the headline browser-peer figure.

Confirming evidence: Line 954 `### Phase 4: browser peer and multi-device identity, 10 to 14 weeks` vs lines 46-47 `A robust browser-only peer adds approximately **10 to 14 weeks**`. CONSISTENT. The word "adds" also correctly conveys that it is incremental on top of Phases 0-3 rather than standalone.

### A119

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The comparison table's 16-24 (PWA) and 10-14 (Phase 4) are not arithmetically contradictory.

Confirming evidence: 16-24 from scratch (line 214) minus 10-14 for the browser peer on an existing foundation (line 954) implies the Phase 0-3 foundation contributes ~6-10 weeks of reusable work toward a browser peer while costing 11-17 total. That is internally coherent, because the majority of Phases 1-3 is companion-specific and non-transferable: `companion pairing/control protocol` (line 895), `jeliya-companion and PWA companion transport` (line 916), `signed macOS and Windows packages` (line 918). The defect is presentational (undisclosed 21-31 total), not arithmetic.

### A120

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The rollback and failover gates match the service-objective table exactly.

Confirming evidence:

````text
Line 947 `N-to-N-1 rollback completes within 15 minutes` matches line 760 `| Frontend rollback | At most 15 minutes |` and line 688 `The CDN deployment pointer returns to immutable N-1 within 15 minutes`. Line 948 `a regional relay outage fails over within 2 minutes` matches line 761 `| Relay regional failover | At most 2 minutes |`. Three-way internal consistency, and both are objectively testable.
````

### A121

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The monthly cost model's arithmetic is internally consistent.

Confirming evidence: Line 800 `Current Iroh managed relay pricing starts at $0.27 per hour. Two continuously running relays therefore start near $389 per 30-day month`. Check: 0.27 x 24 x 30 = 194.40 per relay; x2 = 388.80, which rounds to $389. CORRECT. Table sum check (lines 805-811): lows 0+389+0+0+0 = 389; highs 25+389+25+10+150 = 599; stated `Approximately $400 to $600 plus relay bandwidth`. CORRECT to the stated precision.

### A122

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: Several Phase 1-2 gates are genuinely OBJECTIVE and well-constructed.

Confirming evidence: Line 906 `10,000 injected lost-response retries produce no duplicate message` — a script decides it, the sample size is stated, the predicate is exact. Line 924 `1,000 automated pairing/revocation cycles accept no unauthorized controller` — same structure. Line 907 `cursor resync matches full-log materialization` — a differential test. Line 929 `a 48-hour soak loses no committed event` — bounded duration, exact predicate. Line 923 `the companion has no non-loopback TCP or HTTP control listener` — decidable by binding-table inspection plus a test. These set the standard the weaker gates should be rewritten to.

### A123

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The Phase 5 component-capability gate is correctly constructed as a decidable negative, unlike its Phase 2 counterpart.

Confirming evidence: Lines 990-991 `a component cannot access a secret, file, room, network, process, or pipe without the corresponding import and grant` enumerates a CLOSED capability set and ties each item to a structural precondition (the missing WIT import), which is exactly the mechanism described at lines 473-474 `A missing import means the component cannot ask the host for that facility.` That makes the universal negative decidable by construction. Line 928 attempts the same shape without either property.

### A124

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: Line 950 is the only external-review gate with a stated severity threshold.

Confirming evidence: Line 950 `an external penetration review has no unresolved critical or high finding` — a threshold exists, unlike line 910 (`approves`), line 944 (`passes`), line 989 (`passes`), and line 975 (`receives security qualification`). Residual weaknesses: "unresolved" is not defined (fixed vs accepted-risk), no severity taxonomy is named (CVSS or the vendor's own), and no scope statement exists — but this bullet is the best of the five and the right template for the others.

### A125

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The repository change map's scope counts are accurate as stated.

Confirming evidence: 8 new crates, counted from rows at lines 846-853: jeliya-protocol, jeliya-runtime, jeliya-platform-native, jeliya-web, jeliya-control, jeliya-companion, jeliya-components, jeliya-server-peer. 3 new workflows, lines 854-856: web-ci.yml, web-deploy.yml, companion-release.yml. Plus infra/ (857), docs/adr/ (858), docs/runbooks/ (859), and the jeliya-core split (842). Current baseline verified: `ls crates/` returns exactly jeliya-core, jeliyad, jeliya-ffi; `ls .github/workflows/` returns exactly ci.yml, release.yml. Note the new UI paths at lines 837-841 number 5 (sw.ts, runtime/, storage/, pairing/, invites/), not 6, with 3 further existing files reworked at lines 834-836.

### A126

- Source: track `T3-Judgment`, investigator `gates-and-estimates`

Claim: The document is honest about the status of its own numbers.

Confirming evidence: Line 47-48 `These are planning estimates, not release commitments.` Line 197 `Planning estimates assume two core/full-stack engineers, one web/operations engineer at least part-time, and an independent security review.` Line 764-765 `These are launch objectives to measure during beta. They are not guarantees inherited from the current preview.` Line 866-867 `No phase starts implementation work that depends on an unresolved go/no-go gate from the previous phase.` The framing discipline is good; the defects are in the numbers' units, their bottom-up support, and the falsifiability of specific gate bullets — not in overclaiming.

### A127

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 30 and 168-193: `jeliyad` is local-only by construction — it binds only 127.0.0.1 and exposes no flag for a non-loopback address.

Confirming evidence: crates/jeliyad/src/main.rs:5-7 module doc: "Local-only by construction: the listener binds `127.0.0.1` and nothing else — there is no flag to bind another interface, so the protocol's 'MUST refuse to bind non-loopback interfaces' holds trivially." Confirmed in code at main.rs:364-374, `async fn bind_loopback(...)` constructing `SocketAddr::from((Ipv4Addr::LOCALHOST, candidate))`. The only address-related CLI flag is `--port` (main.rs:51).

### A128

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 64-66: "The Iroh Rooms dependency remains pinned to `71fbb500...` in Cargo.toml."

Confirming evidence: Cargo.toml:15: `iroh-rooms = { git = "https://github.com/kortiene/iroh-room", rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79", features = ["experimental"] }`. Matches the orchestrator's ground truth.

### A129

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 303-307: identity.rs creates one identity/root key and one event/device key and "stores both seeds in a plaintext JSON secret protected by filesystem permissions."

Confirming evidence: crates/jeliya-core/src/identity.rs:5 module doc: "Seeds are stored plaintext under owner-only permissions (the SDK MVP threat...)"; identity.rs:19 "Secret seed file name (the ONLY file holding secrets)"; identity.rs:75 `std::fs::set_permissions(dir, ...Permissions::from_mode(0o700))`; identity.rs:133-134 `write_new_owner_only(&secret_path, secret_json.as_bytes())`; identity.rs:214-215 loads `identity` and `device` signing keys from `file.identity_secret` / `file.device_secret`.

### A130

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 406-410: "The existing UI has an install manifest but no service worker or browser room runtime."

Confirming evidence: `ls ui/public/` returns site.webmanifest plus icons only. `find ui/src ui/public -iname "*sw*" -o -iname "*service-worker*"` returns nothing — no service worker file exists.

### A131

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 68-72: "The security threat model also retains a stale statement that the public lockfile has not been repinned."

Confirming evidence: docs/security-threat-model.md:122-124 still reads "Because the public Jeliya lockfile does not yet resolve that code, upstream publication and Jeliya repinning are mandatory before release qualification" — contradicting the same file's own line 32, which records the pin as `71fbb5007bef...` and "qualified for `v0.6.0`", and Cargo.toml:15 which carries that rev. The stale-contradiction claim is accurate.

### A132

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Lines 154-156: "Android lacks Keystore wrapping and remote-network evidence; iOS has no application scaffold."

Confirming evidence:

````text
docs/platform-matrix.md:57 "Android identity storage | app-private no-backup storage... | not Keystore-backed"; :58 "iOS app | no scaffold or engine build | none | none | excluded"; :56 Android "Android 13 local lifecycle/FFI smoke only; no cross-network, NAT, direct, or relay evidence". Corroborated by docs/security-threat-model.md:70-73: "It does **not** wrap the identity with Android Keystore."
````

### A133

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Lines 163-166: upstream issue #121 leaves live fanout visible to an unproven provisional dialer during an open join window; upstream issue #119 leaves some store holes incompletely healable.

Confirming evidence: docs/known-gaps-roadmap.md:70-73: "while a room is accepting joins, an unproven provisionally-admitted dialer no longer receives history but still receives live event fan-out until it disconnects (upstream issue #121); a store hole left by a swallowed insert error heals only from peers that re-serve the region (upstream issue #119)". Corroborated by docs/security-threat-model.md:81-82.

### A134

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 108: "Files are bounded to 100 MiB."

Confirming evidence: docs/PROTOCOL.md:189-192 defines the client-side code `file_too_large` for a "picked file exceeds the 100 MiB share cap".

### A135

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Lines 800-802 and 807: the relay cost arithmetic is correct as stated for two relays.

Confirming evidence: Doc: "$0.27 per hour. Two continuously running relays therefore start near $389 per 30-day month". Arithmetic check: 0.27 x 24 x 30 = $194.40 per relay; x 2 = $388.80, which rounds to the stated $389. The stated table row at line 807 is internally consistent with line 800-801. (Separately reported: staging and test relays are omitted from the same table.)

### A136

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: The phase durations sum exactly to the headline slice estimate.

Confirming evidence: Phase 0 (line 869) 1-2 + Phase 1 (line 890) 3-5 + Phase 2 (line 912) 5-7 + Phase 3 (line 932) 2-3 = 11 to 17, matching line 47 and line 214. Phase 4 (line 954) "10 to 14 weeks" matches line 48's "A robust browser-only peer adds approximately 10 to 14 weeks." The arithmetic is self-consistent; the unit label is not (reported separately as INTERNALLY-INCONSISTENT).

### A137

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 159-160: "The existing agent runner is an intentional, unsandboxed local code-execution surface. It must remain unavailable to the hosted browser product."

Confirming evidence: docs/security-threat-model.md:142-150 ("Agent boundary"): "The runner is a deliberate local code-execution surface. The daemon and browser do not enable it automatically... The sender allowlist limits who can trigger work; it does not sandbox an allowed sender's task." Corroborated by SECURITY.md:34-37.

### A138

- Source: track `T3-Judgment`, investigator `missing-surfaces`

Claim: Line 62-63 assessment-boundary claim (14 commits / 142 changed files after 55024a46) and the branch/HEAD facts.

Confirming evidence: Accepted as established by the orchestrator and not re-derived. `git log --oneline -3` confirms current HEAD is 7248fb0 on main, so the doc's assessment HEAD 4d4621c9 is not current — the doc's own line 52-54 correctly scopes its claim to that earlier commit rather than to HEAD.

## Limitations

Each investigator recorded its own method, scope, and the things it could not check. Those
statements are reproduced here in full. They are the boundary of this review: anything an
investigator declared out of scope has not been reviewed by anyone.

### Track `T1-Repo`, investigator `commits-and-docs-consistency`

````text
METHOD. All work was read-only against the repo. To measure the assessed commit without touching the working tree I created a detached `git worktree` at 4d4621c9 in the session scratchpad, symlinked `ui/node_modules` from the main checkout (safe: `git diff --stat 4d4621c9 HEAD -- ui/package.json ui/package-lock.json` shows only additive drift, +3 lines / +24 lines), ran the JS suites there, then ran `git worktree remove --force` + `git worktree prune`. Final `git status --short` matches the starting snapshot exactly (M docs/PROFILE.md, M docs/index.md, M scripts/check-docs.mjs, M scripts/check-docs.test.mjs, ?? docs/production-deployment.md). The Rust suites were run in the main checkout, which only writes to `target/`.

ONE NUMBER I MEASURED DIFFERENTLY FROM THE BRIEF — not a contradiction, a framing difference. The brief says the assessed HEAD is "12 commits behind main". That is the delta-of-deltas (26 commits from 55024a4 to HEAD minus 14 commits from 55024a4 to 4d4621c9). Direct git measurement gives different figures because 4d4621c9 is not an ancestor of main: `git rev-list --count 4d4621c9..HEAD` -> 16, `git rev-list --count HEAD..4d4621c9` -> 4, and `git rev-list --count 77501d7..HEAD` -> 13 counting from the squash-merge that actually landed the work. I used 13 in the findings because it is the reproducible, on-main figure. None of these contradicts the brief's substantive point; all of them are larger than zero and the doc states none of them.

WHAT I COULD NOT CHECK, AND WHY.
1. Whether 4d4621c9 was the branch TIP at merge time, or a mid-branch commit. The branch is deleted from origin so no reflog or remote ref survives. Circumstantial evidence says it was effectively final: the branch work (`0277577f..4d4621c9`) is 24 files / +1534 / -58 and the landed squash (`3059d51..77501d7`) is 24 files / +1533 / -225, with identical file counts. The delta in deletions is consistent with a rebase onto 3059d51 (which had already added ui/src/lib/roomList.ts) rather than with additional branch commits. I did not treat this as a finding.
2. Whether "87 Vitest tests passed" was literally true in the author's environment. Depends on whether `target/debug/jeliyad` existed there; I can show 81/6/87 in mine and cannot show theirs. Filed as UNVERIFIABLE rather than WRONG, deliberately.
3. The Ed25519 signature files docs/evidence/v0.6.0/{direct,relay}.json.sig exist (89 bytes each) and the manifests self-report certifiable/releaseable, but I did not run the signature verification against the committed public SPKI — no verification script was invoked. The doc's claim at lines 105-107 is about existence and recorded limits, both of which I did confirm from the manifest JSON itself.
4. The DNS observation is dated 2026-07-17; I could only observe 2026-07-18.

SCOPE NOTE. Track 1a is doc lines 50-89 plus the two cross-references at 105-107 and 154-156. I did not evaluate the deployment-model comparison, cost model, roadmap estimates, or the Citations URLs — the citation list at lines 1075-1088 was not fetched.

STRONGEST SIGNAL FOR THE PARENT AGENT. The doc's two accusations against other documents both hold up under literal inspection: capability-status.md:50 and security-threat-model.md:122-124 are real, quotable, stale contradictions. That is the doc at its best and it earns credibility there. The damage is entirely self-inflicted at lines 52-54: a page whose central thesis is "evidence must bind an exact revision" anchors itself to a SHA that exists on no published ref, and stamps itself 17h44m after that anchor was merged away and 17 minutes after main moved again. Fixing lines 52-54 and adding the forward-gap disclosure would restore the page's authority without touching any of its architecture content.
````

### Track `T1-Repo`, investigator `daemon-and-identity-security`

````text
SCOPE: Track 1b only — doc lines 168-193 and 301-307. I did not evaluate the deployment-model comparison, cost model, roadmap estimates, citations, or the assessment-boundary section (lines 50-89); the orchestrator's ground truth on HEAD/commit-count/Cargo pin was taken as given and I found nothing contradicting it (I independently confirmed the pinned iroh-rooms checkout at ~/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50 has `git rev-parse HEAD` == 71fbb5007bef4ce83631c94762ec68c2beef3d79).

CALIBRATION ON THE "UNDERSTATED = CRITICAL" RULE: You asked me to treat an understated security claim as CRITICAL. I found one understatement (the doc's line-187 enumeration omits the token-in-`<a href>` download link) and rated it **high**, not critical, deliberately. Reasoning: the doc already identifies the correct defect class ("bearer material in query URLs"), already gives the correct rationale ("query strings land in logs"), and already prescribes the fix in its change map. What is wrong is the completeness of the list, not the security judgment — a reader acting on line 187 would fix the WS and upload cases and ship the worst one. That is a real and actionable gap, but calling it critical would put it on par with a claim that is affirmatively false in a way that changes the architecture decision, and nothing here does that. Flagging this so you can override if your severity scale differs from mine.

NET ASSESSMENT: The doc's security claims about jeliyad and identity are unusually well-grounded. The single most load-bearing claim — the loopback-only bind — is not just true but structurally true (a `const` address, one bind site in the whole workspace, no env/config/flag/test path, and clap compiled without the `env` feature so the attribute isn't even available). I could not construct any falsification. Every finding I raised is a precision or completeness gap, not a wrong judgment. The pattern across all five findings is the same: the doc is slightly LESS precise than the source it cites, and in three cases the source's own comments are more honest than the doc (lifecycle.rs:26 and PROTOCOL.md:59 both carry the "on Unix" qualifier the doc drops; serve.rs:206-209 states the non-browser-forgery limit the doc compresses).

TWO SOURCE-HARDENING NITS — not doc findings, no doc line, reported so they are not lost:

1. crates/jeliyad/src/lifecycle.rs:78 uses `opts.write(true).create(true).truncate(true)` for the portfile temp file, whereas crates/jeliya-core/src/identity.rs:253 correctly uses `create_new(true)` for the secret file. Unix `mode()` applies only at file CREATION, so if `daemon.json.tmp` already exists with wider permissions the 0600 is silently not applied, and `fs::rename` (lifecycle.rs:89) carries those permissions onto the real `daemon.json` containing the token. Not exploitable today — the data dir is 0700 by the time the portfile is written (main.rs:145 → RoomSupervisor::new → crates/jeliya-core/src/supervisor.rs:193 `identity::ensure_dir`), so an attacker who could pre-create the tmp file is already the same user and can read the portfile anyway. Still worth matching identity.rs's stricter pattern.

2. `--ui-dir` (main.rs:63-64) lets the daemon serve an arbitrary local directory from its own loopback origin, which is the origin that `/api/session` hands the token to. A malicious `--ui-dir` is therefore script-in-the-token-holding-origin. Requires control of the command line, so it is inside the documented same-user exclusion, but it is a sharper edge than the flag's description suggests and is worth a line in the threat model rather than only the deployment doc.

WHAT I COULD NOT CHECK: I did not run the daemon or exercise any endpoint at runtime — every claim above is from reading source, and I have marked nothing as verified-by-execution that was not. I did not audit the Flutter/Dart client's token handling (out of Track 1b scope; the grep for keystore patterns covered `*.dart` and found nothing, but I did not read those files). I did not verify Windows ACL inheritance behavior empirically — my Windows claims are strictly "the code sets no ACL", which is a read of `#[cfg(unix)]` gating, not a claim about what Windows actually grants by default.
````

### Track `T1-Repo`, investigator `engine-limits-and-capabilities`

````text
SCOPE: Track 1c only — doc lines 93-116, 159-160, 406-407, plus the Cargo.toml pin re-check. I did not evaluate the deployment-model table, cost model, roadmap phases, or citations.

I did NOT edit any file. The only file I wrote was a throwaway probe at /tmp/claude-1000/-home-sekou-AGI-jeliya/5e158fd5-cc0e-4772-b191-e38f4c1316ec/scratchpad/ip.rs (compiled with rustc) to empirically test `SocketAddr::from_str` + `is_loopback()` rather than assert std behavior from memory.

FOUR SUB-QUESTIONS I COULD NOT FULLY CLOSE:

1. Symlink TOCTOU in `file.share` — NOT reported as a finding, deliberately. There is a window between `std::fs::canonicalize(path)` (supervisor.rs:1719) and `blob_import(&import_path)` (supervisor.rs:1743) in which a directory component inside the data dir could be swapped for a symlink pointing outside, defeating the prefix test. I did not report it because exploiting it requires write access INSIDE the daemon data dir, and the doc's own threat model (line 177) already "explicitly excludes hostile same-user processes". It would become a real finding if the companion in Phase 2 ever widens the shareable region beyond the data dir — worth a line in the Phase 2 gate, but it is not a defect in the doc's line 108-110 claim as written.

2. `assert_shareable_path`'s reserved-file list is an ALLOW-region-with-denylist, not a pure allowlist. It blocks `identity.json`, `identity.secret`, `rooms.db*`, `state.json*` (supervisor.rs:307-320) and the `blobs/` tree (:301). Any FUTURE daemon-private file added under the data dir would be shareable by default unless the list is updated. That is a latent maintenance hazard, not a current inaccuracy — the three known secret paths are all covered, which I verified against identity.rs:110-111 and localstate.rs:81,99.

3. I could not run the test suites to confirm the doc's line 76-78 numbers ("87 Vitest tests", "71 unit tests") — outside Track 1c and would require a full cargo/npm build. Anyone checking those should note the memory-recorded caveat that CI historically ran only 1 of 13 vitest files; ci.yml:74-87 now carries a comment saying that was fixed under issue #76 and `npm run test:unit` runs the whole suite except the live-daemon oracle. I did not execute it, so the specific count 87 is UNVERIFIED by me.

4. The doc's line 112 claim ("The React UI works against the loopback daemon and has mock browser coverage") was not in my track and I did not verify it.

ONE PIECE OF CONTEXT THE FINDINGS DON'T CARRY: the engine-limits claims in this section are, on the whole, unusually well-calibrated — the file, pipe, and confinement claims all survived adversarial checking, and in two cases (per-connection pipe authorization, four-layer 100 MiB enforcement) the code is STRONGER than the doc claims. The two real defects are both of the same shape: a summary sentence that says "all" or "limited to" where the underlying reality has a documented, deliberate exception (jeliya-ffi's unsafe opt-out) or an incomplete enumeration (four unlisted localStorage keys). Both are fixable with a clause, not a redesign.
````

### Track `T1-Repo`, investigator `tests-links-profile`

````text
SCOPE: Track 1d only — test reproduction, link validation, profile compliance, checker. I did not evaluate the doc's architectural judgments, cost model, effort estimates, or its characterizations of iroh/browser platform behavior.

NO FILES WERE EDITED. I created a temporary git worktree at 4d4621c9 to reproduce the historical test count, then removed it with `git worktree remove --force && git worktree prune`. Final `git status --porcelain` is byte-identical to the starting state (M docs/PROFILE.md, M docs/index.md, M scripts/check-docs.mjs, M scripts/check-docs.test.mjs, ?? docs/production-deployment.md). The `ui/dist/` build output from `npm run build` is gitignored and did not appear in status.

HEADLINE: This document is in unusually good shape on every axis I was assigned. Zero broken links (22 local, 13 external, 1 anchor), zero profile violations, gate passes clean, and the change map's existing-vs-proposed distinction is flawless. My two findings are both low severity and neither is a substantive error. I want to be explicit that this is a genuine result and not incuriosity: I ran every test suite, resolved every link individually rather than trusting the gate, curl-tested all 13 external URLs, filesystem-tested all 26 change-map paths, and ran a negative test to prove the orphan rule actually fires.

ON THE 87 vs 219 VITEST GAP — the important characterization: this is EXPECTED DRIFT, NOT A FALSE CLAIM, and the orchestrator's framing was correct. I confirmed this rather than assuming it. At the doc's own assessed commit 4d4621c9, vitest collects exactly 87 tests. That the doc's number reproduces to the digit at the commit it names is strong evidence the assessment was actually performed as described. The growth to 219 at HEAD is attributable to the localization work in commits 5695e61/7248fb0 (#111/#112), which added l10n.test.ts and expanded several suites. Anyone reading only the raw numbers would wrongly conclude the doc overstated its evidence; the opposite is true.

The one real nuance is the passed/skipped distinction (finding 1). The 6 daemon-conformance tests skip unless `target/debug/jeliyad` is built, so "87 passed" is environment-dependent. In my main-repo run at HEAD the daemon binary was present and those 6 ran; in the fresh worktree it was not and they skipped. I could not determine which state the original assessor's environment was in, so I have NOT called the claim wrong — only imprecise relative to the rigor the doc itself applies to the Rust ignored-test one line later.

NOT CHECKED / OUT OF SCOPE, stated so silence is not mistaken for coverage:
- Per instructions I did not re-derive the orchestrator's ground truth (HEAD, the 14-commit/142-file delta at 4d4621c9, the Cargo.toml:15 iroh-rooms pin). I did incidentally observe Cargo.toml:15 carries `rev = "71fbb5007bef4ce83631c94762ec68c2beef3d79"` and that workspace members are exactly ["crates/jeliya-core", "crates/jeliyad", "crates/jeliya-ffi"] — both consistent with the ground truth given.
- I validated that the 13 external URLs RESOLVE (HTTP 200). I did NOT verify that each page substantiates the claim attributed to it — notably the $0.27/hour managed-relay price on line 800 that drives the entire cost table. That needs a content check by whichever track owns the cost model.
- Line 84-85's DNS claim (no A/AAAA/CNAME for jeliya.ai / app.jeliya.ai on 2026-07-17) is UNVERIFIABLE from here as stated: it is explicitly scoped to a past date and a different environment, so a DNS query today could neither confirm nor refute it. I did not run one.
- Lines 52-54 name branch `feat/69-fleet-attention-projection`, which the orchestrator confirmed is deleted. Worth flagging to whichever track owns the assessment boundary: a reader cannot check out that branch by name, though the commit is still reachable (my worktree at 4d4621c9 succeeded). That is a reproducibility wrinkle in the doc's evidence trail, not a factual error, and I left it to the owning track rather than double-reporting it.
- I ran the unit/integration suites only. The e2e suite (`npm run test:e2e`, Playwright) and the Dart/Flutter gates were not run; the doc makes no specific numeric claim about them.

MERGE RISK worth surfacing: the doc's gate-passing status depends on the uncommitted one-line docs/index.md addition traveling with it. If production-deployment.md is committed alone, CI fails with document-orphan (proven by my negative test). The uncommitted scripts/check-docs.mjs and docs/PROFILE.md changes are NOT required — the committed checker passes this file as-is.
````

### Track `T2-External`, investigator `iroh-pricing-and-relay`

````text
SCOPE: Track 2a only — Iroh pricing, relay architecture, browser-Wasm. All checks run live on 2026-07-18. No files edited.

CORRECTION TO THE TASK BRIEF: the brief asks me to check "all four Iroh citation URLs (lines 1075-1077)". There are only THREE Iroh URLs in the doc, at lines 1075, 1076, and 1077; lines 1078-1084 are all MDN. All three Iroh URLs resolve and support their attributions. Separately, a fourth Iroh page that the doc *should* cite but does not — https://www.iroh.computer/pricing — is where the load-bearing "$0.27/relay/hour" unit and the plan tiers actually live.

HEADLINE: the two most consequential findings are not in the pricing arithmetic (which is fine) but in the credential design. (1) Delegated issuance — relay-auth.jeliya.ai minting an endpoint-scoped token for a browser after proof of possession — is undocumented upstream; every documented mint happens in-process from the API key, and Iroh's own text says "Your API key never leaves your application." (2) The self-hosted fallback at line 544-545 is not credential-equivalent: upstream iroh-relay offers only static shared secrets with no revocation short of a restart. Together these mean the browser relay-auth story has no confirmed implementation path on either the managed OR the self-hosted side, which is more exposure than assumption #3 + risk #2 convey. Recommend a Phase 0 vendor-confirmation gate.

COST MODEL, net: the $389 figure is arithmetically sound but the model is incomplete in the direction of underestimating. Missing: the $19/mo Pro base fee, concurrent-endpoint overage at $0.50/100 beyond 100 (this scales per active user and may dominate), and metrics overage at $1.49/1K DPM beyond 10K. Vendor normalizes on 730h not 720h. Bandwidth is "negotiated", so line 822's egress term has no public rate to consume, and an SLA appears to require Enterprise rather than a surcharge. A corrected floor is roughly $408/mo on the doc's own rows, before any usage scaling.

WHAT I COULD NOT CHECK: (a) the $50-$200 self-hosted compute estimate — the doc names no provider or instance class, so there is nothing to price against; reported UNVERIFIABLE rather than guessed. (b) Actual negotiated bandwidth rates or any Enterprise quote — not public. (c) Whether iroh_services exposes an undocumented delegated-minting API — absence of documentation is not proof of absence, which is exactly why I marked the credential finding UNVERIFIABLE rather than WRONG; a direct question to n0 would settle it. (d) crates.io/crates/iroh-relay returned only a page shell via WebFetch; I sourced the self-hosted auth quotes from the GitHub iroh-relay README instead, which returned real content. (e) I read GitHub discussion #3168 but it concerns application-level peer authorization, not relay credentials, so I used it only for the metadata-leak corroboration and did not stretch it further.

EVIDENCE HYGIENE: one intermediate claim about self-hosted shared secrets first surfaced in a WebSearch result summary; I did not report it until I confirmed it with a direct fetch of the iroh-relay README. Every quote in the findings above comes from a page I actually fetched, not from search snippets.

Sources: [Iroh hosting](https://www.iroh.computer/services/hosting), [Iroh pricing](https://www.iroh.computer/pricing), [Iroh Wasm/browser docs](https://docs.iroh.computer/languages/wasm-browser), [Iroh add-a-relay](https://docs.iroh.computer/add-a-relay), [Iroh relay concepts](https://docs.iroh.computer/concepts/relays), [iroh-relay README](https://github.com/n0-computer/iroh/tree/main/iroh-relay), [iroh discussion #3168](https://github.com/n0-computer/iroh/discussions/3168)
````

### Track `T2-External`, investigator `upstream-issues`

````text
Repository access: fully reachable. `gh` authenticated as user `kortiene`; kortiene/iroh-room is public and not a fork. No WebFetch fallback was needed. Nothing in this track is UNVERIFIABLE.

Timeline summary (all evidence via `gh`, 2026-07-18):
- Pin 71fbb5007bef = tag v0.1.0-rc.3, committed 2026-07-16T05:16:35Z. Newest tag in the repo; `main` is 26 commits ahead, 0 behind.
- #121 CLOSED/COMPLETED 2026-07-16T14:05:35Z, commit 58aca4ba (PR #125) — 8 commits after pin.
- #126 CLOSED/COMPLETED 2026-07-16T15:10:31Z, commit d0dde879 (PR #130) + 85a3aedb (PR #131) — 9 and 21 commits after pin.
- #119 CLOSED/COMPLETED 2026-07-16T19:30:18Z, commit a5d98b70 (PR #132) — 24 commits after pin.
So the answer to the orchestrator's decisive question is unambiguous: BOTH issues are fixed strictly AFTER the pinned rev. The doc is not raising resolved-at-pin issues; it is describing real defects in the deployed dependency while omitting that the remedy already exists upstream and is a repin.

Other open upstream issues (question 4): only four are open — #100, #101, #102, #103, all `area/community` process/cohort issues, none security-relevant to a deployment. There are zero open PRs and zero GitHub security advisories (`gh api repos/kortiene/iroh-room/security-advisories` → empty). The only genuinely missing threat is the CLOSED-but-post-pin #126, reported above. I read #102's body ("plaintext local storage on trusted machines only; invite tickets are password-grade capabilities; no native ticket-specific revocation…") and confirmed the doc already covers those themes at lines 162, 305, 337, 351-364, 681, 1013 — no gap there.

v0.5.0 (question 5): there is NO v0.5.0 in kortiene/iroh-room — its only releases are v0.1.0-rc.1/rc.2/rc.3. The v0.5.0 in question is Jeliya's own release (2026-07-14T23:11:15Z), pinned to iroh-rooms d0ceb0b (pre-PR #111), and it does carry the join-after-conversation deadlock. Reported as a low-severity MISSING item plus a medium mixed-version-skew item, because the hosted product builds from the rc.3 pin where the deadlock is already fixed — the residual risk is fleet skew, not the hosted build.

Minor observation not raised as a finding: the doc's #121 one-liner covers fan-out but drops the second half of the issue's own title — the unproven peer is also advertised to via `on_tick` (\"advertises the admin tip and heads to it\"). A one-line compression; noting for completeness only.

Cross-document context (outside my review scope but explains the staleness): docs/security-threat-model.md:82 asserts \"upstream issue #119 remains open\", which is also now false, and docs/known-gaps-roadmap.md:70-71 carries the same framing. The production-deployment doc appears to have inherited its stale upstream state from those sibling pages rather than from a fresh upstream check — which is notable given the doc itself flags stale sibling status pages at lines 68-72 as \"a release gate, not editorial cleanup.\"

No files were edited.
````

### Track `T2-External`, investigator `browser-platform-facts`

````text
SCOPE: Track 2c only — browser platform claims at doc lines 316-320, 374-375, 400-402, 424-447, 616-617, 966-967, 1078-1087. I did not evaluate the commit-count, relay-cost, workspace-crate, or Rust-architecture claims. Per instructions I edited nothing; all repo access was read-only.

CALIBRATION NOTE ON THE ED25519 HYPOTHESIS. The task framing anticipated that Ed25519 would not be shipped across the doc's Phase 4 matrix, making the gate "unsatisfiable as written — that is a high-severity finding." The evidence does not support that. Ed25519 has been in all four major engines since Chrome M137 (May 2025), the most recent arrival, and against a July 2026 assessment every browser in the doc's matrix clears its floor by over a year. I am reporting this as a confirmed claim rather than a finding. The Ed25519 sentence does contain a real high-severity problem, but it is the *other* half of the hedge — "exact wire interoperability" — and it bites on verification, not on availability or on signature bytes. I would rather hand back the corrected shape than manufacture the predicted finding.

The interop finding is the one I would act on first. It is not theoretical: I confirmed the native verifier by reading the pinned dependency, not by inference. /home/sekou/.cargo/git/checkouts/iroh-room-5518ef183531c90f/71fbb50/crates/iroh-rooms-core/src/event/keys.rs:153 calls `verify_strict` and self-describes as "the **only** event-signature verification entry point", against a WebCrypto spec that mandates a strictly weaker check and carries an open issue admitting implementations diverge further. In a system where both peer classes fold the same signed log, an acceptance-set mismatch is a partition, and the doc's current gate would pass while the mismatch persists.

WHAT I COULD NOT VERIFY, explicitly flagged rather than asserted:
1. Whether Chrome Sync, Firefox Sync, or iCloud upload history entries with fragments intact, and whether session restore persists them to disk. Plausible and widely assumed, but I found no authoritative vendor documentation either way. My fragment finding rests on RFC 9700 §4.3.2's explicit browser-history statement, which I did verify; the sync/session-restore extension is labeled unverified inside that finding.
2. Whether Chrome Enhanced Safe Browsing transmits fragments. Google's support pages (support.google.com/chrome/answer/13844634 and /9890866) confirm full URLs are sent in real time under Enhanced protection but do not state whether the fragment is stripped. Not claimed.
3. Whether `PerformanceNavigationTiming.name` includes the fragment. The Navigation Timing spec defers to "the document's URL" without specifying fragment handling, and I could not resolve it through the Resource Timing/HTML chain. Not claimed.
4. Whether `Client.url` retains a fragment and whether replaceState clears it. MDN notes "the url property is not updated unless a new page is actually loaded ... it will not be updated if the user navigates within the same page using a URL fragment", which is suggestive that a replaceState would not update it either — but that is inference about a different operation, so I did not turn it into a finding. It is worth a targeted experiment during Phase 2, since the doc's control at line 378-379 (register the service worker only after replaceState) depends on the answer.
5. Whether Safari's ITP 7-day eviction exempts origins granted `navigator.storage.persist()`. WebKit's blog does not address the interaction, and MDN documents the two mechanisms separately. The home-screen-PWA exemption is documented and is the mitigation I recommend; the persist() interaction is not, so I recommended the documented path rather than the uncertain one.

I did not re-derive any of the orchestrator's stated ground truth, and nothing I found contradicts it.
````

### Track `T2-External`, investigator `csp-and-headers`

````text
SCOPE: Track 2d only — CSP block (583-598), headers block (606-613), COEP note (615-617), cache policy (619-625), plus lines 576-577 (HSTS preload) and 745 (report scrubbing) as directed. I did not audit the rest of the document, and I did not re-derive the orchestrator's ground truth about commit counts or the iroh-rooms pin. I edited no files.

CORRECTIONS TO THE REVIEW BRIEF (the brief asked me to consider several directives as potentially missing; four of them are not):
- child-src is NOT missing. Its only consumers are worker-src and frame-src, and both are explicitly present (lines 590, 594), so child-src would never be consulted. MDN fallback chains: worker-src → child-src → script-src → default-src; frame-src → child-src → default-src. Adding it would be dead config.
- prefetch-src is NOT missing. CSP3 does not define it — my fetch of w3.org/TR/CSP3 returned "The spec doesn't define prefetch-src—it's absent" and it is not in the §6.1 fetch-directive list. MDN still shows it in the default-src fallback list, but it is non-standard and was removed from Chrome. Do not add it.
- script-src-elem, script-src-attr, style-src-elem, style-src-attr are NOT missing. Each falls back to script-src/style-src, both of which are set (585, 586). Splitting them is only worth it if you want an asymmetric policy (e.g. allowing hashed inline styles via style-src-elem while keeping style-src-attr locked). Not needed today.
- media-src IS worth adding, but note it currently falls back to default-src 'none' — i.e. it fails in the SAFE direction. It is a roadmap gap (file preview), not an open hole. Reported as such.
- interest-cohort should NOT be recommended: it is absent from MDN's 49-directive Permissions-Policy list (FLoC was withdrawn). browsing-topics is the live token.

The brief also suggested the connect-src `<relay-hosts>` placeholder might need a wildcard for relay failover. I could not resolve this: no relay hostnames exist anywhere in the repo, and the doc says only "two Iroh managed dedicated relays" (line 557). If Iroh's managed relays have stable per-region hostnames, enumerate them; if they are allocated dynamically, a wildcard like `wss://*.relay.iroh.network` would be needed, which weakens the policy from an exact allowlist to a delegation of trust to whoever controls that DNS zone. Marking UNVERIFIABLE rather than guessing — but the doc should resolve the placeholder before Phase 0 rather than leaving `<relay-hosts>` to be filled in by whoever writes the OpenTofu config, because "wildcard because it was easier" is exactly how this decision gets made by default.

WHAT I COULD NOT CHECK:
- No CSP is deployed anywhere for app.jeliya.ai, so I could not test the policy against a running build — everything about "does this break the app" is derived from static analysis of ui/src, ui/dist, and node_modules/react-dom. The strongest way to settle it is the Playwright securitypolicyviolation gate I recommend in the last finding.
- The Trusted Types spec's exact "Should Trusted Type policy creation be blocked by CSP?" algorithm — both my fetch of w3c.github.io/trusted-types and the CSP3 §4.2.6 fetch came back truncated. My conclusion that policy creation is unrestricted without a `trusted-types` directive rests on MDN's createPolicy page ("Thrown if policy names are restricted by the ... trusted-types directive") plus MDN's trusted-types page, not on the primary algorithm text. The conclusion is standard and I am confident, but flagging the sourcing honestly.
- The service-worker CSP inheritance rule IS primary-sourced (MDN Content-Security-Policy header page, quoted verbatim in that finding). The connect-src-inside-SW behavior is sourced to a blog (qubyte.codes) with a direct quote — I could not find it stated in MDN or CSP3. Treat that sub-claim as well-attested but secondary; it is worth confirming empirically when ui/src/sw.ts is written, since it changes what the SW is allowed to cache.
- Whether Cloudflare Pages serves .wasm as application/wasm by default (it generally does) — I did not verify, which is why the finding is framed as "the doc must state the requirement and smoke-test it" rather than "this host is broken".

OVERALL CALIBRATION: this is a well-constructed CSP. It correctly uses default-src 'none' as a deny-by-default base, correctly enumerates the three directives that do not fall back (base-uri, form-action, frame-ancestors), and its COEP reasoning is right for the wrong-seeming reason people usually get wrong (OPFS does not need cross-origin isolation; only shared memory does). The two findings I would not ship without are the missing `trusted-types` directive — because `require-trusted-types-for` without it is bypassable and gives false assurance — and the missing reporting configuration, which is both a real gap and a live contradiction with line 745. The loopback/connect-src finding is high severity not because the policy is wrong but because the currently-shipping client code cannot function under it, and no line in the document says so.
````

### Track `T3-Judgment`, investigator `architecture-decision`

````text
SCOPE: Track 3a only — the decision (195-224), phasing (864-996), vertical slice (998-1030), and change map (830-859). I read the full 1087-line document for cross-references but did not systematically audit sections outside those ranges. No files were edited.

WHAT I COULD NOT CHECK (all UNVERIFIABLE by me in this pass, presumably other tracks' scope): I did not fetch any of the 14 citation URLs (1075-1087) — no network verification was performed. So every external-platform claim is unchecked by me: browser Iroh being relay-only (508-512), the $0.27/hr managed relay price and the ~$389/month derivation (800-807), Background Sync availability (445), WebCrypto Ed25519 support (1084), OPFS behavior (1079), and fragment exclusion from Referer (1082-1083). I also did not run cargo build/test, did not verify the 87 Vitest / 71 Rust test counts at lines 76-78, and did not check DNS for jeliya.ai (84-85).

THE VERDICT ON THE CENTRAL QUESTION, stated plainly since the schema fragments it:

The hybrid END STATE is right, and "companion first" is right — but the document argues for it incorrectly, and one component of slice 1 does not survive scrutiny.

Why companion-first is right, for a reason the doc never gives: Phase 4 is gated on a third party. Cargo.lock shows rusqlite is a dependency of iroh-rooms-core, not of jeliya-core, so the native-store assumption blocking the browser peer lives in a pinned upstream crate. The team cannot schedule its removal. That — not "the repository does not yet contain its storage or network runtime" (222-223), which is merely an effort claim — is the decisive argument. Effort arguments lose here: by the doc's own arithmetic the hybrid costs 21-31 weeks to reach zero-install versus 16-24 going straight, so on effort alone the rejected option wins.

Why it is not simply "the most complex option chosen to defer the hard decision": that critique lands on the TABLE but not on the SEQUENCING. The table is rationalized — the hybrid column is the union of two options scored as the best of both, and it cannot lose by construction. But shipping the controllable thing first while an uncontrollable dependency resolves is correct sequencing, not avoidance.

Where the plan genuinely fails: (1) the browser shell as a REQUIRED slice-1 surface. The user installs native software anyway (1003), so zero-install value is nil, while the shell adds TB1, a pairing protocol, a control plane, relay-auth, and a CDN dependency. Its only real value is cross-device browser access — which the document never states as a requirement, making the decision unfalsifiable. (2) The claim that Phase 2 is a foundation for Phase 4. It is not. jeliya-companion is a native service consuming the same platform adapters jeliyad already consumes, so it exercises the portability seam with a second consumer of one implementation — the reliable way to get an abstraction wrong. Phases 1 and 3 are largely reusable by Phase 4 (idempotency, cursors, invite lifecycle, protocol negotiation, DNS/TLS/CSP/CDN/relay/relay-auth/smoke/rollback/observability all transfer). Phase 2 is not: pairing transcript, SAS UI, scope/grant model, revocation UI, three signed installers, and the encrypted view cache are companion-only, plus permanent signing and support cost. The 5-7 week detour is real; the "necessary foundation" defense is not.

The surgical fix that preserves the good judgment and removes the bad: keep companion-first, cut the hosted shell from slice 1 unless cross-device browser access is written down as a launch requirement, and put the upstream trait negotiation in Phase 0 with an owner. That is a smaller, honest slice-1 whose terminal risk is being actively worked rather than deferred fifteen weeks.
````

### Track `T3-Judgment`, investigator `security-attack`

````text
SCOPE: Track 3b only — adversarial attack on the proposed security design. I did not re-derive the orchestrator's ground truth (HEAD, commit distance, workspace crate list); I did independently re-confirm the Cargo.toml:15 pin as a coverage check and it matched.

WHAT I READ IN THE REPO (so you can judge coverage): crates/jeliya-core/src/engine.rs (full), identity.rs (full), fleet.rs (partial), supervisor.rs (targeted: require_public_room_access, assert_shareable_path, share_file, fetch_file, local_file, pipe_expose, pipe_list, sanitize_name, save_atomic), crates/jeliyad/src/main.rs (full), serve.rs (targeted: routing, /api/session, token presentation, WS gate), ui/src/main.tsx (full), ui/src/lib/client.ts (targeted: token handling), ui/public/site.webmanifest, docs/PROTOCOL.md (targeted), docs/security-threat-model.md (full), docs/agent-orchestration.md and agent-guide.md (grep-level), Cargo.toml.

WHAT I COULD NOT VERIFY, AND WHY:
1. Upstream issues #121 and #119 (doc lines 163-166, 390, 1067). I did not fetch the kortiene/iroh-room repository, so I took the doc's and docs/security-threat-model.md:81-82's characterization at face value. My finding about room.join scope leans on #121 only as an aggravator, not as its basis.
2. The service-worker fragment behavior is established from the spec-change record (w3c/ServiceWorker#854 via GoogleChrome/workbox#488) and the stated Firefox 52 / Chrome 59 alignment. I did NOT run a live browser test. Given the doc is a proposal, I judged spec evidence sufficient to require a design change, but a 20-minute empirical check in Chrome, Firefox, and Safari would be worth doing before Phase 3 — Safari's behavior in particular I have no citation for.
3. The browser-history claim (finding on lines 377-379/401-402) rests on Mozilla bug 753264 for "replaceState does not remove the URL from global history". I did NOT independently verify that Firefox/Chrome store the FRAGMENT in places.sqlite / the History DB, nor that history sync uploads it. That portion is inference from how fragment-bearing URLs appear in omnibox autocomplete. Treat the finding as high-confidence on "replaceState is not an erasure primitive" and medium-confidence on the specific fragment-persistence channel; verify empirically before writing product copy about it.
4. The relay-auth analysis is based on https://docs.iroh.computer/add-a-relay, which describes preset()-based minting with a project API key. I could not see any Jeliya-side Worker (it does not exist yet), and iroh may expose an admission API not documented on that page. If it does, the finding softens from "open oracle by construction" to "admission policy unspecified" — but the doc still has to state one.
5. Cost impact of relay abuse is qualitative. I confirmed the $0.27/hr price and the $389 arithmetic but iroh's hosting page states no egress rate, so I could not put a dollar figure on the abuse ceiling.

CALIBRATION NOTES:
- I deliberately did NOT report missing SRI as a gap, even though the task flagged it for checking and it is genuinely absent from the document. SRI is the wrong control for a first-party CDN: the attacker who rewrites the bundle also rewrites the integrity attribute in the no-cache index.html. I folded this into the signed-manifest finding and recommended the doc say so explicitly, so a future reviewer does not re-propose SRI as the fix.
- The doc is unusually honest in several places that a hostile reviewer would normally attack — TB1's "cannot make a deliberately malicious first-party build trustworthy", TB4's copy-vs-forgery distinction, the explicit refusal to claim content-blind servers (lines 42-44, 297, 783), and the "these are planning estimates, not release commitments" hedge. Those are confirmed above rather than passed over in silence.
- Two findings partly overlap by design and should be fixed together: the room_id-keyed scope model falling open on parameter-free methods, and the unconfined save_dir. The first is the mechanism, the second is the highest-severity payload.
- The single highest-value architectural change I would push for: the companion must verify WHICH web build is talking to it (signed build manifest in the control handshake). The CI already emits signed provenance (line 662) and then drops it; carrying it to a verifier is comparatively cheap and is the only control in this design that meaningfully bounds TB1 rather than merely cleaning up after it.
````

### Track `T3-Judgment`, investigator `gates-and-estimates`

````text
FULL GATE CLASSIFICATION (all 40 gate bullets, Phases 0-5). O = OBJECTIVE (machine/script decides), J = JUDGMENT (human decides, criteria stated), A = ASPIRATIONAL (signable with no real evidence).

PHASE 0 (883-888): 883 no contradictory release claim remains — A. 884 complete CI passes twice on one immutable SHA — O. 885 direct and forced-relay evidence signed and bound to that SHA — O. 886 a browser reaches a native test endpoint through an authenticated relay — O. 887-888 #121 not exploitable and unmitigated — A (as written; a J/O version exists at lines 390-391 but is not referenced).

PHASE 1 (904-910): 904 recovery succeeds from fresh install on every supported OS — O (conditional on the OS matrix, which ADR #8 at line 1056 has not yet fixed). 905 native production mode no longer leaves the root secret plaintext — O. 906 10,000 retries, no duplicate — O. 907 cursor resync matches full-log materialization — O. 908 expired and cancelled tickets fail on every transport — O. 909 replay/wrong-SAS/expired-key/revoked-key pairing tests fail closed — O. 910 independent security review approves wire formats and key lifecycle — A.

PHASE 2 (923-930): 923 no non-loopback TCP/HTTP control listener — O. 924 1,000 pairing/revocation cycles — O. 925-926 two NAT-separated users full flow — O. 927 direct and forced-relay runs pass — O. 928 malicious controller cannot invoke files/pipes/agents/identity reset — A. 929 48-hour soak loses no committed event — O. 930 supported installers verify signatures and reject tampering — O.

PHASE 3 (944-950): 944 external TLS/header/CSP assessment passes — J with no threshold (borderline A). 945 invitations in no CDN/Worker/relay/client log — O (canary-token grep; CI precedent at lines 653-654). 946 offline shell and cached view open during origin outage — O. 947 N-to-N-1 rollback within 15 minutes — O. 948 regional relay failover within 2 minutes — O. 949 load tests inside resource and cost ceilings — A (ceilings undefined; "ceiling" occurs once in the whole document, in this bullet). 950 external pen review, no unresolved critical or high — J.

PHASE 4 (966-975): 966-967 latest two Chrome/Edge/Firefox/Safari + iOS Safari + Android Chrome pass the matrix — O (a deliberately moving target; needs a re-qualification cadence). 968-969 byte-compatible signatures and membership folds — O (conformance corpus, line 531). 970 forced-relay chat and file tests across browser/native combinations — O. 971 clearing storage triggers recovery, never silent identity replacement — O. 972 active browser peer works offline and converges — O. 973 revoked device cannot author an accepted future event — O. 974 product copy makes no durable background-availability claim — A. 975 exact revision receives security qualification — A (repairable by reference to docs/capability-status.md).

PHASE 5 (989-996): 989 sandbox escape and confused-deputy review passes — A. 990-991 component cannot access secret/file/room/network/process/pipe without import and grant — O (closed enumeration + structural precondition). 992 quota violation terminates cleanly without corrupting host state — J ("cleanly" and "corrupting" undefined; becomes O with a stated post-termination invariant). 993 rollback preserves prior component state and signed room history — O. 994 server-peer UI states precisely whether the server can read content — J (and must hold in FR too). 995-996 no blind-backup privacy claim before interop tests pass — half O (tests), half A (claim policing).

TALLY: 26 OBJECTIVE, 5 JUDGMENT, 9 ASPIRATIONAL. The aspirational nine are lines 883, 887-888, 910, 928, 949, 974, 975, 989, and the copy half of 995-996. Three of them (910, 928, 949) sit on the critical path to the first production launch gate at line 952, which is the material risk: a launch can be declared with the security review, the controller-authority negative, and the cost ceiling all satisfied by assertion.

STRUCTURAL OBSERVATION. The document's gate quality is bimodal. Where it is testing its own code it is excellent and often better than typical (10,000-retry idempotency, 1,000 pairing cycles, 48-hour soak, byte-compatible signature conformance). Where the gate depends on a person outside the codebase — a reviewer, a copywriter, a load-test ceiling someone must set — it degrades to a verb with no object. Every one of the nine aspirational gates is repairable, and six of the nine (883, 887-888, 949, 974, 975, 995-996) are repairable into fully OBJECTIVE form with work the document already scopes elsewhere.

WHAT I COULD NOT CHECK, AND WHY.
1. I did not attempt to validate the 16-24 estimate for the PWA-first path against any external benchmark; I only checked its internal coherence against Phase 4's 10-14. Whether either absolute number is right is outside what the repository can tell me.
2. My bottom-up estimate (the 49-90 person-week finding) is engineering judgment, filed as DISAGREE rather than WRONG. I decomposed the change map into tasks and sized them; I did not calibrate against this team's historical velocity. The repo's recent milestones (UX 0-3) could supply that calibration and would make the argument much stronger — I did not have the per-milestone effort data to do it.
3. Azure Trusted Signing's reported three-year-organization-history eligibility requirement came from a search summary, not a primary-source quote. I verified and quoted only the geographic restriction and the "can't be expedited" statement from https://learn.microsoft.com/en-us/azure/artifact-signing/faq, and the 1-7 business day figure from a Microsoft Q&A page rather than the product docs. Treat the three-year requirement as UNVERIFIED; it is worth confirming directly, because if it applies it is a hard eligibility stop rather than a delay, and it would change the Phase 2 plan.
4. I did not verify Iroh's $0.27/hour managed relay price against the vendor page (line 800 / citation at line 1077); I checked only that the arithmetic derived from it is correct.
5. Phase 5 (8-16 weeks) received less scrutiny than Phases 0-4 because it falls past the first production launch gate and outside the headline estimates.
6. The SLO-versus-telemetry analysis at line 758 assumed the "Allowed aggregate metrics are:" list at lines 717-723 is exhaustive rather than illustrative. If it is meant as illustrative, finding 9 (the OS/browser dimension) softens considerably, but finding 8 does not — that one turns on the explicit prohibitions at lines 726-735, not on the allow-list.
````

### Track `T3-Judgment`, investigator `missing-surfaces`

````text
SCOPE: This is Track 3d (what is MISSING). I did not systematically re-verify the doc's positive technical claims except where needed to calibrate coverage — other tracks own that.

WHAT I COULD NOT CHECK, AND WHY:
- I did not fetch any external URL. No WebFetch/WebSearch was run, so every one of the doc's 14 citations (lines 1075-1087) is UNCHECKED by me, including the Iroh managed-relay price of $0.27/hr at line 800 (I verified only that the doc's own arithmetic from that number is right) and the MDN claims about OPFS, Background Sync, and storage eviction. Anything I say about iOS/Android background-execution policy or iOS Safari's storage eviction is therefore framed as "the document never assesses this", which is verifiable from the document, rather than as a platform claim I proved.
- The $15k-$50k figure the orchestrator supplied for an external penetration review is NOT independently verified by me. My finding rests on the verifiable fact that the doc gives no figure at all for a hard launch gate, not on that range.
- I did not re-check DNS for jeliya.ai / app.jeliya.ai (doc lines 84-85). Out of track.
- I did not read docs/capability-status.md, verification-evidence.md, signing-notarization.md, agent-orchestration.md, or DESIGN.md in full. I read known-gaps-roadmap.md, security-threat-model.md, platform-matrix.md, accessibility-checklist.md, i18n.md, PROTOCOL.md (compat sections), room-workbench.md (decisions 3 and 5), agent-marketplace.md (grep-level), SECURITY.md, and PRODUCT.md in full or in the relevant sections.

METHOD NOTE ON ABSENCE EVIDENCE: absence findings are backed by `grep -c -i` counts over docs/production-deployment.md, and in every case where a term returned a non-zero count I read the hits to confirm they were a different sense (e.g. "controller" = browser controller, "processor" = social-preview processor, "ban" = substring of bandwidth, "phone" = substring of microphone, "status page" = documentation status pages at line 67, "on-call" = a single cost aside at line 814). I did not report an absence without doing that check.

TWO CROSS-CUTTING OBSERVATIONS THAT DID NOT FIT A SINGLE FINDING:

1. LINK GRAPH. The doc cites seven internal docs (capability-status, platform-matrix, PROFILE, PROTOCOL, security-threat-model, signing-notarization, verification-evidence). It never links: known-gaps-roadmap.md, accessibility-checklist.md, i18n.md, room-workbench.md, agent-guide.md, agent-marketplace.md, glossary-fr.md, design-tokens.md, SECURITY.md, PRODUCT.md, or DESIGN.md. The omitted set is almost exactly the product/UX/a11y/i18n/operations half of the repo's documentation — which predicts, and explains, the shape of the gaps above. The doc reads as written by someone with deep knowledge of the security and release-engineering docs and no exposure to the product and design records.

2. GATE ASYMMETRY. The five phase gates (881-888, 902-910, 922-930, 942-950, 964-975, 987-996) contain roughly 40 criteria. Every one is a security, protocol, network, or infrastructure criterion. Not one is a usability, accessibility, localization, legal, or support-readiness criterion. Line 952 declares Phase 3 "the first production launch gate" — so under this plan a product can launch having proven that a malicious controller cannot invoke pipes, and having proven nothing about whether a screen-reader user can complete pairing, whether a French user sees French, or whether a privacy policy exists. That asymmetry is the single highest-level finding of this track and is the reason the individual omissions were invisible: the document's quality bar is genuinely high, but it is high along one axis only.

STRONGEST INDIVIDUAL FINDING: the mixed-version revocation gap (v1 peer ignores device.revoked per the normative PROTOCOL.md:236-239 MUST-ignore-unknown-kinds rule, defeating the security property the Phase 4 gate at line 972 claims to test). It is the only omission I found that is both a security defect and mechanically provable from two documents in the repo.
````

## Citations

- [Production deployment architecture](production-deployment.md) - The reviewed proposal.
- [Documentation profile](PROFILE.md) - Metadata, lifecycle, linking, and CI rules this page conforms to.
- [Capability status](capability-status.md) - The qualification boundary several findings depend on.
- [Security and threat model](security-threat-model.md) - The threat model several findings compare against.
- [Verification evidence](verification-evidence.md) - The revision-bound evidence ledger.
- [Known gaps and roadmap](known-gaps-roadmap.md) - Records the mixed-fleet upgrade failure one finding cites.
- [Internationalization](i18n.md) - The localization contract the accessibility and localization finding depends on.
- [Accessibility release checklist](accessibility-checklist.md) - The accessibility gates that finding depends on.
