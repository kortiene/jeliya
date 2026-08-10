# Spec: Native update channels, signing trust, and anti-rollback policy (#121)

> **This is an implementation plan, not the deliverable.** The deliverable of
> issue #121 is a single canonical **Decision** document in `docs/`. This spec
> tells another engineer/agent exactly what that document must decide, how to
> author it against the repository's documentation contract, how it links to the
> existing records, and how it is verified. It changes no production code.

## 1. Summary

Issue #121 (reframed by [`docs/first-release-distribution.md`](../docs/first-release-distribution.md)
§"Amendment disposition") asks Jeliya to **define**, for every published native
package, four things:

1. whether the channel has an updater or an explicit **no-updater** decision;
2. how update metadata and artifacts are authenticated (signing roots, trusted
   metadata, rotation, revocation, channel separation);
3. the **downgrade / anti-rollback** policy and version monotonicity;
4. **actionable failure UX** for unsupported / revoked / too-old / can't-update
   states, plus offline/interrupted recovery, operator/support commands, and
   privacy-bounded version diagnostics.

The original issue assumed web-origin ↔ companion version skew. #113 removed that
architecture. Exact protocol/storage-generation mismatch rejection is already
owned elsewhere (#157/#161/#164/#170/#172/#199) and is **referenced, not
re-decided** here. The enduring, still-unowned question is the *native package*
update/trust/rollback model. This is a **policy definition**; the downstream
per-platform packaging (#186–#190) and qualification (#199) consume it but do
**not** block it.

**Recommended deliverable:** `docs/native-update-policy.md`, `type: "Decision"`,
`status: "canonical"`, `implementation_status: "planned"`,
`verification_status: "unverified"`, `release_status: "unreleased"`. It is a
canonical decision about **unwritten code**, exactly like
[`docs/dioxus-architecture.md`](../docs/dioxus-architecture.md) and
[`docs/first-release-distribution.md`](../docs/first-release-distribution.md).

## 2. Owning surface and where this fits

- **Primary artifact:** a new page in the `docs/` OKF wiki. `docs/` is
  CI-validated by `scripts/check-docs.mjs` (the
  [documentation profile](../docs/PROFILE.md) is the contract).
- **Inputs it must cite, not restate:**
  - [`dioxus-architecture.md`](../docs/dioxus-architecture.md) — the
    single-generation clean-slate policy (Decision 2), per-platform system
    WebView floors (Decision 6), `package identity` / bundle-identifier rule
    (Decision 3), and the `#121` pointer in "What this record does not decide"
    (this new doc *satisfies* that pointer). Architecture/version inputs
    #157/#161.
  - [`first-release-distribution.md`](../docs/first-release-distribution.md) —
    the artifact-origin trust boundary, exact-digest fail-closed, "no renderer
    rollback artifact," and the "#121 retained, reframed" disposition.
  - [`signing-notarization.md`](../docs/signing-notarization.md) — macOS
    Developer ID + notarization and Windows Authenticode plans; signing roots
    are supplied by #1/#2.
  - [`security-threat-model.md`](../docs/security-threat-model.md) — the trust
    boundaries this policy extends (artifact origin, storage generation, daemon
    token custody, release/CI supply chain), and the release boundary.
  - [`platform-matrix.md`](../docs/platform-matrix.md) — the "Decided Dioxus
    targets" rows for macOS(#186)/Linux(#187)/Windows(#188)/Android(#160,#194),
    which this policy annotates.
  - [`known-gaps-roadmap.md`](../docs/known-gaps-roadmap.md) — the NEXT bullet
    "operate signing, notarization, and evidence keys with documented custody,
    rotation, and incident response," which this policy makes concrete for the
    update dimension.
  - [`verification-evidence.md`](../docs/verification-evidence.md) — the
    sanitized, revision-bound evidence-ledger contract the update-case evidence
    rows must follow.
  - [`../packaging/README.md`](../packaging/README.md) — the *actual* current
    channels (`install.sh`, `install.ps1`, `jeliya.rb`, `jeliya-app.rb`,
    `package-linux.mjs`, Android AAB/APK) and the checksum-sidecar behavior.
- **Downstream consumers (do not block this policy):** #186–#190 packaging and
  #199 per-platform qualification carry the applicable evidence rows.

## 3. Non-goals (carry verbatim into the doc)

Reproduce these so no reviewer re-adds them:

- No N/N-1 compatibility windows, grace periods, migrations, dual protocol
  support, mixed-version windows, canaries, or legacy rollback artifacts (the
  single-generation clean slate has none — see `dioxus-architecture.md`
  Decision 2 and "Rejected and deferred alternatives").
- No hosted-web/companion skew flow.
- Do **not** claim automatic updates where the channel provides none.
- Do **not** let any updater bypass platform signing authority.
- Do not re-decide exact protocol/storage-generation mismatch rejection
  (#157/#161/#164/#170/#172/#199) — reference it.
- This doc does not decide legal/compliance publication gating (#118) or the
  build/provenance format of the embedded artifact (#183).

## 4. Decisions the document must record

The doc must be **decisive**. Where the repository has not yet chosen a value
(e.g. macOS/Android WebView floors, Windows scope), record the *policy* and mark
the value **open with an owner**, never waive it. The following are the
recommended positions; the author confirms them with maintainers and records the
rationale.

### 4.1 Channel inventory (authoritative list)

The doc must enumerate **every published native channel** and give each an
explicit decision. Recommended inventory, derived from `packaging/README.md`,
`platform-matrix.md`, and the Dioxus targets:

| Platform | Channel | Artifact today | Clean-slate successor / owner |
|---|---|---|---|
| macOS | `curl \| sh` (`install.sh`) | `jeliyad` `.tar.gz` + `.sha256` | #186 |
| macOS | Homebrew formula (`jeliya.rb`, tap) | `jeliyad` | #186 |
| macOS | Homebrew cask (`jeliya-app.rb`) | unpublished future DMG app | #186 |
| macOS | direct browser download (DMG/archive) | future signed/notarized DMG | #186 / #1 |
| Windows | `install.ps1` (PowerShell) | `jeliyad` `.zip` + `.sha256` | #188 (scope undecided) |
| Windows | direct browser download | future signed installer/zip | #188 / #2 |
| Linux | `curl \| sh` (`install.sh`) | `jeliyad` `.tar.gz` + `.sha256` | #187 |
| Linux | Homebrew (Linuxbrew) | `jeliyad` | #187 |
| Linux | source tarball (`package-linux.mjs`) | unsigned dev package | #187 |
| Android | Google Play (AAB) | none published | #190 / #194 / #160 |
| Android | sideload APK (per-ABI) | dev build only | #190 / #194 / #160 |

The doc must state that channels not on this list (distro repos, Flatpak,
AppImage, WinGet, Microsoft Store, App Store, Sparkle appcast, iOS) are **not
published channels** in the first release and therefore carry no update claim; a
new channel requires an added row and its own evidence before it may ship.

### 4.2 Updater vs. no-updater, per channel (AC-1)

Recommended baseline — **no in-app / bundled auto-updater on any native channel
for the first release**; updates delegate to the channel's own authenticated
mechanism. Rationale to record:

- it honors the non-goals ("no claimed automatic update where the channel gives
  none"; "no updater bypasses platform signing authority");
- the single-generation clean slate removes the compatibility machinery a
  self-updater usually justifies;
- a bundled updater is a new privileged network-plus-write surface that the
  WebView boundary review (#196) and the threat model have not covered, so it is
  out of scope until separately reviewed.

Per-channel decision the doc must state explicitly:

| Channel | Decision | Update mechanism that IS authoritative |
|---|---|---|
| Homebrew formula/cask | **no in-app updater** | `brew upgrade`; digest pinned in the reviewed tap commit is the authenticity root |
| `install.sh` / `install.ps1` | **no in-app updater** | operator re-runs the installer; `.sha256` sidecar verified **before extraction**, fail-closed |
| macOS DMG / browser download | **no in-app updater** (no Sparkle) | Gatekeeper + stapled notarization ticket; operator re-downloads |
| Windows installer / browser download | **no in-app updater** (no Squirrel) | Authenticode + SmartScreen reputation; operator re-downloads |
| Google Play (AAB) | **platform updater** — Play's own auto-update *is* the updater | Play App Signing + Play delivery |
| Sideload APK | **no updater**, and re-sign **breaks signature continuity** | operator manually installs a same-key APK; a key change forces uninstall/reinstall (data loss), which the doc must warn about |

The doc must be explicit that "no in-app updater" is a *decision*, not an
omission, and that the Play row is the **only** channel where an automatic
updater is claimed — because it is the platform's, not Jeliya's.

### 4.3 Signing roots, trusted metadata, rotation, revocation, channel
separation (AC-2)

The doc must document, per platform:

- **Signing roots (supplied by #1/#2, referenced not owned):** macOS Developer
  ID Application certificate + notarization (`notarytool`), Windows Authenticode
  (EV preferred), Android Play App Signing (Google-held) with the local keystore
  as *upload* key. Linux daemon archives have **no publisher signature today** —
  the doc must say so and name the checksum sidecar as integrity-only.
- **Metadata trust — and the integrity-vs-authenticity distinction (critical
  honesty point):** a `.sha256` sidecar fetched from the same origin as the
  artifact proves **integrity, not authenticity** — it does not defend against a
  compromised origin. Authenticity comes from (a) platform code signatures
  (#1/#2), (b) the Homebrew formula/cask digest committed in a reviewed tap
  change, or (c) Play App Signing. The doc must not overclaim the sidecar.
- **Rotation:** where keys/certs live (GitHub Actions secrets per
  `signing-notarization.md`; never committed), who rotates them, and that
  rotation must not silently invalidate installed builds without an actionable
  message.
- **Revocation — two layers:**
  1. *Platform-level:* Apple certificate/notarization revocation, Authenticode
     CRL/OCSP, Play takedown/key rotation — honored by the OS.
  2. *Application-level, network-independent:* a compiled **minimum-supported
     build floor** in `jeliyad`/the app, plus an optional build-time-embedded
     revocation list of known-bad build digests. A build below the floor or on
     the list **fails closed** with actionable UX **without a network call and
     without executing untrusted bytes**.
- **Channel separation / anti-substitution:** one reserved application/bundle
  identifier and one signing identity **per packaged target** (per
  `dioxus-architecture.md` Decision 3 `package identity` row), so a foreign- or
  legacy-channel install cannot upgrade in place into the new generation and a
  cross-channel artifact swap fails the platform's own identity check.

### 4.4 Downgrade / anti-rollback and version monotonicity (AC-3)

Two distinct cases; keep them separate in the doc:

- **Cross-generation downgrade** (a build of a *different* protocol/storage
  generation): already fails closed **before mutation** at the v2 handshake and
  the namespaced-storage gate (#161/#164/#170/#173/#178/#185). Reference it;
  do not re-specify. There is no data-preserving rollback (`dioxus-architecture.md`
  Decision 2).
- **Same-generation downgrade** (an *older build* of the *same* generation) —
  this doc's own decision. Recommended: the new namespaced storage generation
  records a **monotonic on-disk marker** (a `written_by` build number and a
  `min_reader` floor). A build refuses to open state written by a strictly newer
  build, **failing closed with the actionable reset path shown, never migrating
  or silently downgrading**. This is anti-rollback that fits the clean slate: it
  prevents a downgraded binary from operating on newer state, it is local, and
  it retains no legacy compatibility. The concrete key/name is owned by #185
  (desktop), #178 (browser), #173 (Android); the doc references those and must
  not invent a key name any retiring client wrote.

The doc must state the invariant plainly: **version is monotonic per generation;
a rollback that would reinterpret newer state fails closed.**

### 4.5 Actionable failure UX (AC-4) and offline/interrupted recovery (AC-5)

For each fail-closed state, the doc must specify a user-facing message shape and
the recovery action (short, EN/FR-ready per [`i18n.md`](../docs/i18n.md)):

| State | Fail-closed behavior | Actionable UX |
|---|---|---|
| unsupported generation / old client | reject at handshake, before mutation | "This build is too old for your data. Reset to continue: `<reset path>`." (path shown, not taken — per clean-slate) |
| below minimum-supported floor / revoked build | refuse to start; no network required | "This version is no longer supported. Download the current version from `<channel>`." |
| tampered artifact (bad checksum/signature) | installer refuses **before extraction** | "Download failed verification and was not installed. Retry from `<channel>`." |
| newer state, older binary (downgrade) | refuse to open state | "This install is older than your data. Update, or reset: `<reset path>`." |
| offline | prior verified install keeps running; no partial artifact executed | "You're offline; the installed version is unchanged." |
| interrupted download | checksum mismatch → fail closed → safe retry (installers are idempotent, atomic replace) | "Update didn't complete. Your current version is intact; retry." |

**Recovery must never execute untrusted bytes.** The prior verified install
stays runnable until the replacement verifies; verification precedes extraction;
where an updater exists (Play), the OS owns atomicity. This is the
"recovery path that does not require executing untrusted bytes" requirement.

### 4.6 Operator/support commands and privacy-bounded diagnostics (AC scope)

The doc must define a diagnostics surface (recommend extending `jeliyad
--version` / a `jeliya version` output) reporting, per platform:

- package/app version, artifact **digest**, channel, signing identity /
  notarization state, storage **generation**, and (device evidence only) the
  system WebView version.
- a verify/support command that re-checks the installed artifact digest against
  the published sidecar and reports signature validity.

**Privacy bound:** diagnostics must contain **no** identity keys, daemon tokens,
invite tickets, IP addresses, room/member data, or file contents — consistent
with the evidence-sanitization rule in `verification-evidence.md` and the
secrets rule in `PROFILE.md` §"Markdown and citations." State this bound
explicitly.

### 4.7 Evidence contract and downstream integration (AC-6)

The doc must define the **update-case evidence schema** and where rows land:

- **Case matrix per claimed channel:** current, older, newer, revoked, tampered,
  offline, interrupted, downgrade.
- **Retained per case (sanitized, revision-bound per `verification-evidence.md`):**
  package **digest**, **signing identity**, **metadata/version**, **command
  run**, and **result** — and nothing secret-bearing.
- **Where rows live:** the doc is the *authority*; the applicable evidence rows
  are carried by the per-platform packaging issues #186–#190 and per-platform
  qualification #199. In the docs bundle, the author must (a) add a
  cross-reference from each `platform-matrix.md` "Decided Dioxus targets" row to
  this policy, and (b) note that these results are **enforced evidence, not
  certification**, and that **a missing per-channel update gate blocks only that
  channel's publication row** (no all-platform barrier) — consistent with #199.

> **Phase boundary note:** editing the GitHub issue bodies of #186–#190/#199 is
> the orchestrator's/maintainer's job, not this doc's. The doc satisfies AC-6 on
> the documentation side by defining the schema and adding the cross-references
> in-repo; see Open Questions for the issue-body follow-up.

## 5. Authoring constraints (the docs CI contract)

The new page must pass `node scripts/check-docs.mjs`. Enforced constraints:

1. **Exactly the 10 required frontmatter fields**, in the restricted YAML subset
   (double-quoted JSON-compatible strings; `tags`/`audience` non-empty flow
   arrays of unique lowercase-hyphenated tokens; no nesting/anchors/implicit
   scalars; no unknown fields). Recommended block:

   ```yaml
   ---
   type: "Decision"
   title: "Native update, signing, and anti-rollback policy"
   description: "Decision record for whether each published native Jeliya channel has an updater, how update artifacts and metadata are authenticated, the single-generation anti-rollback and version-monotonicity rule, and the fail-closed UX, recovery, diagnostics, and evidence contract for unsupported, revoked, tampered, offline, and downgrade states."
   tags: ["release", "security", "signing", "update", "clean-slate", "dioxus"]
   timestamp: "2026-08-10T00:00:00Z"
   status: "canonical"
   implementation_status: "planned"
   verification_status: "unverified"
   release_status: "unreleased"
   audience: ["contributors", "maintainers", "release-engineers", "security-reviewers"]
   ---
   ```

   (Use a real UTC instant for `timestamp`. `description` must be one sentence.
   `type` = `"Decision"`; do not invent a new type — the two-document rule
   forbids it. Keep `implementation_status: "planned"` and
   `verification_status: "unverified"` because nothing is built or tested.)

2. **Exactly one `#` H1, immediately after the frontmatter, byte-identical to
   `title`.** Start internal structure at `##`.
3. **File-relative links only** (`dioxus-architecture.md`,
   `signing-notarization.md#...`, `../packaging/README.md`); never a leading
   slash. Local paths and heading fragments must resolve. External links, if
   any, must be credential-free `https://` URLs in a final `## Citations`
   section.
4. **Reachability:** add one descriptive link from `docs/index.md` (recommended
   under "Operations and release evidence," adjacent to
   `signing-notarization.md`), or CI fails on an orphan.
5. No raw HTML (comments allowed). GitHub-style tables/task lists are fine.
6. Keep secrets/tokens/tickets/keys out of every example.

## 6. Step-by-step implementation

1. **Create `specs/`** (this file) — done by this plan.
2. **Confirm channel inventory and the updater decisions** in §4.1–4.2 with the
   release maintainer; record any deviation and its rationale in the doc.
3. **Write `docs/native-update-policy.md`** with sections mapping 1:1 to §4:
   - `## Channels and the update decision` (AC-1, table from §4.1–4.2)
   - `## Signing roots, metadata trust, rotation, revocation, channel
     separation` (AC-2, §4.3)
   - `## Anti-rollback and version monotonicity` (AC-3, §4.4)
   - `## Fail-closed states and actionable UX` (AC-4, §4.5)
   - `## Offline and interrupted recovery` (AC-5, §4.5)
   - `## Operator commands and privacy-bounded diagnostics` (§4.6)
   - `## Evidence contract and downstream integration` (AC-6, §4.7)
   - `## Non-goals` (§3; AC-7 lives here — "no compatibility window, migration,
     or legacy rollback requirement remains")
   - `## What this record does not decide` (defer to #1/#2/#118/#183/#161 etc.)
   - `## Citations` (repo docs inline; external platform docs here if cited)
4. **Add the index link** in `docs/index.md` (§5.4).
5. **Cross-reference the policy** from:
   - `dioxus-architecture.md` "What this record does not decide" — change the
     `#121` line to link the new doc (it now *is* decided). **Bump that file's
     `timestamp`** because its meaning changes.
   - each `platform-matrix.md` "Decided Dioxus targets" row (add a policy link).
     Bump its `timestamp`.
   - optionally `signing-notarization.md` and `known-gaps-roadmap.md` NEXT.
     Bump `timestamp` on any file whose meaning changes.
6. **Run the gate:** `node scripts/check-docs.mjs` (from repo root); fix any
   frontmatter/link/reachability failures.
7. **Self-review round** (see memory: review rounds are load-bearing). Re-read
   against each AC in §7; verify every internal link and heading fragment
   resolves; verify no non-goal was silently re-added; verify the doc claims
   nothing built or verified.

## 7. Acceptance criteria → where satisfied

| # | AC | Satisfied by |
|---|---|---|
| 1 | Every published native channel has an explicit updater / no-updater decision | §4.1–4.2 tables (all channels enumerated; each decided) |
| 2 | Signing roots, trusted metadata, rotation, revocation, channel boundaries documented | §4.3 |
| 3 | Downgrade/rollback policy explicit and tested | §4.4 policy + §4.7 case matrix (`downgrade`, `older`, `newer`) |
| 4 | Unsupported/tampered/revoked fail closed with actionable UX | §4.5 table |
| 5 | Offline/interrupted recovery defined where an updater exists | §4.5 (offline/interrupted rows) + §4.2 (only Play has an updater) |
| 6 | #186–#190 and #199 contain the applicable evidence rows | §4.7 schema + platform-matrix cross-references; issue-body edits flagged as orchestrator follow-up |
| 7 | No compatibility window, migration, or legacy rollback requirement remains | §3 non-goals reproduced in the doc's `## Non-goals` |

"Tested" for AC-3/AC-4/AC-5 means the doc **defines** the case matrix and the
retained-evidence schema; executing the matrix on real packages is downstream
qualification work (#186–#190/#199), not part of authoring the policy.

## 8. Test / verification strategy

- **Machine gate:** `node scripts/check-docs.mjs` must pass (frontmatter, four
  status axes, single-H1, link/fragment resolution, index reachability,
  UTF-8/symlink boundaries).
- **Review obligations the gate does not cover** (per `PROFILE.md` §"CI
  contract"): one-sentence `description`, meaningful `timestamp`, citation
  completeness, and — specific to this doc — that every channel row has a
  decision, that non-goals are intact, and that no built/verified claim is made.
- **No production code, tests, or CI workflows change.** If any cross-referenced
  file's meaning changes, bump only its `timestamp` and re-run the gate.

## 9. Risks

- **Overclaiming authenticity.** Treating a same-origin `.sha256` sidecar as
  authenticity is the most likely error; §4.3 forces the integrity-vs-authenticity
  distinction. A reviewer must confirm the doc never implies the sidecar defends
  a compromised origin.
- **Accidentally re-introducing a non-goal.** N/N-1 windows, migrations, or a
  rollback artifact can slip in via the "recovery" or "downgrade" sections.
  §3/§4.4/§4.5 are written to fail closed instead; AC-7 review guards it.
- **Scope creep into #1/#2/#183/#161.** The doc must reference, not re-own,
  signing roots, artifact build/provenance, and protocol-generation rejection.
- **Stale cross-references.** Editing `dioxus-architecture.md` /
  `platform-matrix.md` without bumping `timestamp` or fixing links breaks the
  gate; step 5/6 covers it. (Memory: never trust a stated count in this repo —
  recompute the channel list from `packaging/README.md` at author time.)
- **AC-6 boundary.** The documentation side is in-repo; the GitHub issue-body
  evidence rows are an orchestrator/maintainer follow-up (Open Questions).

## 10. Assumptions

- The `docs/` OKF wiki is the correct home; #121 is a `documentation`-labeled
  policy, and `dioxus-architecture.md` already routes the `#121` decision to a
  doc record. (This spec lives in `specs/` only because the ADW workflow asks
  for a plan artifact; it is not part of the docs bundle and must not be linked
  from `docs/index.md`.)
- The recommended "no in-app updater" baseline is subject to release-maintainer
  confirmation; the spec records it as the recommended position with rationale.
- `#186–#190`, `#199`, `#1`, `#2`, `#113`, `#157`, `#161` retain the meanings
  recorded in the cited docs as of 2026-08-10.

## 11. Open questions

1. **Windows scope.** `dioxus-architecture.md` Decision 6 leaves Windows as an
   *undecided* first-release target (#188 must include or formally defer it).
   The policy should give Windows a **conditional** decision ("if Windows ships,
   then …") rather than assert a committed channel. Confirm with #188's owner.
2. **Application-level revocation list.** Is a build-time-embedded bad-digest
   list in scope for the first release, or is the minimum-supported-build floor
   sufficient? (§4.3 offers both; recommend floor-only for the first release,
   list as a documented option.)
3. **AC-6 issue bodies.** Adding the evidence rows to the GitHub issues
   #186–#190/#199 is outside this phase's git/gh boundary. Confirm the
   orchestrator/maintainer will mirror the §4.7 schema into those issues, or
   whether a `platform-matrix.md` table is the accepted in-repo substitute.
4. **Diagnostics surface owner.** Does the `jeliyad --version` / `jeliya
   version` diagnostics extension belong to this policy as a requirement, or is
   it a separate small implementation issue the policy merely *specifies*?
   (Recommend: policy specifies the required fields and privacy bound; a
   follow-up issue implements it.)
5. **French UX strings.** The §4.5 messages need EN/FR per `i18n.md`; confirm
   whether the policy fixes canonical wording or defers exact strings to the UI
   localization work.
```
