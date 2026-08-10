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

# Native update, signing, and anti-rollback policy

**Nothing in this record is built.** It is a canonical decision about unwritten
code, exactly like [the clean-slate architecture](dioxus-architecture.md) and
[the first-release distribution boundary](first-release-distribution.md). It
satisfies issue #121 as reframed by
[first-release-distribution.md](first-release-distribution.md#amendment-disposition)
("Retained, reframed around native update-channel trust, signing, and
anti-rollback — with no compatibility window, mixed-version period, or N/N-1
behavior"). It changes no production code, and it is the record that
[the architecture](dioxus-architecture.md#what-this-record-does-not-decide)
points to when it defers native update, signing, and anti-rollback policy to
#121.

For every published native package this record decides four things:

1. whether the channel has an updater, or an explicit **no-updater** decision;
2. how update metadata and artifacts are authenticated — signing roots, trusted
   metadata, rotation, revocation, and channel separation;
3. the **anti-rollback** policy and version monotonicity;
4. **actionable failure UX** for unsupported, revoked, too-old, and can't-update
   states, plus offline and interrupted recovery, operator and support commands,
   and privacy-bounded version diagnostics.

The original issue assumed web-origin ↔ companion version skew. #113 removed
that architecture. Exact protocol and storage-generation mismatch rejection is
owned elsewhere (#157, #161, #164, #170, #172, #199) and is **referenced, not
re-decided** here. The downstream per-platform packaging (#186–#190) and
qualification (#199) consume this policy but do **not** block it.

## Channels and the update decision

### Channel inventory

This is the authoritative list of published native channels, derived from
[`../packaging/README.md`](../packaging/README.md) and the
[platform matrix](platform-matrix.md#decided-dioxus-targets). Every channel is
enumerated and every channel is decided.

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

Channels **not** on this list — distribution-specific repositories, Flatpak,
AppImage, WinGet, the Microsoft Store, the App Store, a Sparkle appcast, and iOS
— are **not published channels** in the first release and therefore carry **no
update claim**. Adding a new channel requires an added row in this table and its
own evidence (see [Evidence contract](#evidence-contract-and-downstream-integration))
before it may ship.

### Updater vs. no-updater, per channel

**Decision: no in-app or bundled auto-updater on any native channel for the
first release.** Updates delegate to each channel's own authenticated
mechanism. "No in-app updater" is a *decision*, not an omission. The rationale:

- it honors the non-goals — no claimed automatic update where the channel gives
  none, and no updater that bypasses platform signing authority;
- the single-generation clean slate removes the compatibility machinery a
  self-updater usually justifies (see
  [Anti-rollback](#anti-rollback-and-version-monotonicity));
- a bundled updater is a new privileged network-plus-write surface that the
  WebView boundary review (#196) and
  [the threat model](security-threat-model.md#trust-boundaries) have not
  covered, so it is out of scope until separately reviewed.

| Channel | Decision | Update mechanism that IS authoritative |
|---|---|---|
| Homebrew formula/cask | **no in-app updater** | `brew upgrade`; the digest pinned in the reviewed tap commit is the authenticity root |
| `install.sh` / `install.ps1` | **no in-app updater** | operator re-runs the installer; the `.sha256` sidecar is verified **before extraction**, fail-closed |
| macOS DMG / browser download | **no in-app updater** (no Sparkle) | Gatekeeper + stapled notarization ticket; operator re-downloads |
| Windows installer / browser download (if Windows ships) | **no in-app updater** (no Squirrel) | Authenticode + SmartScreen reputation; operator re-downloads |
| Google Play (AAB) | **platform updater** — Play's own auto-update *is* the updater | Play App Signing + Play delivery |
| Sideload APK | **no updater**, and re-sign **breaks signature continuity** | operator manually installs a same-key APK; a key change forces uninstall/reinstall (data loss), warned below |

Google Play is the **only** channel where an automatic updater is claimed —
because it is the platform's updater, not Jeliya's. The sideload APK row carries
a required warning: Android refuses to upgrade an installed app in place when the
signing key changes, so any upload-key change forces an uninstall and reinstall,
which discards app-private state; operators must be told this before they change
keys.

**Windows is conditional.** [The architecture](dioxus-architecture.md) leaves
Windows an *undecided* first-release target: #188 must explicitly include or
formally defer Windows. The two Windows rows above therefore state the policy
that applies **if Windows ships**; they assert no committed Windows channel.

## Signing roots, metadata trust, rotation, revocation, channel separation

### Signing roots (supplied by #1/#2, referenced not owned)

| Platform | Signing root | State today |
|---|---|---|
| macOS | Developer ID Application certificate + notarization (`notarytool`) | planned; see [signing and notarization](signing-notarization.md) — #1 |
| Windows | Authenticode (EV preferred for SmartScreen reputation) | planned; see [Windows Authenticode signing](signing-notarization.md#windows-authenticode-signing) — #2 |
| Android | Play App Signing (distribution key held by Google); the local keystore is the **upload** key | planned; #190 / #194 |
| Linux | **no publisher signature today** | the daemon archives are unsigned; the checksum sidecar is integrity-only |

### Metadata trust — integrity is not authenticity

This is the load-bearing honesty point of the record. A `.sha256` sidecar
**fetched from the same origin as the artifact proves integrity, not
authenticity**: it detects a corrupted or truncated download, but it does not
defend against a compromised origin, because an attacker who can replace the
artifact can replace the sidecar beside it. Authenticity comes only from:

- **(a)** platform code signatures — macOS Developer ID + notarization, Windows
  Authenticode (#1/#2); or
- **(b)** the Homebrew formula/cask digest committed in a **reviewed tap
  change**, whose authenticity root is the reviewed Git history, not the
  download origin; or
- **(c)** Play App Signing.

This record must never imply the same-origin sidecar defends a compromised
origin. Where a channel has only the sidecar (the Linux `curl | sh` and source
tarball paths today), the record states plainly that it provides integrity, not
publisher authenticity, and that adding a publisher signature is owed by #187.

### Rotation

Signing keys and certificates live in GitHub Actions repository or environment
secrets (per [signing and notarization](signing-notarization.md)) and are
**never committed**. The role that rotates each root is the **release
maintainer**, coordinating with the platform authority (Apple, the Windows CA,
Google Play). Rotation must not silently invalidate installed builds: when a
rotation changes what a user's platform will trust, the failure must surface as
[actionable UX](#fail-closed-states-and-actionable-ux) that points to the
current download, not as an opaque launch failure. This makes concrete the
[roadmap NEXT commitment](known-gaps-roadmap.md) to "operate signing,
notarization, and evidence keys with documented custody, rotation, and incident
response" for the update dimension.

### Revocation — two layers

1. **Platform-level, OS-honored:** Apple certificate/notarization revocation,
   Authenticode CRL/OCSP, and Play takedown or key rotation. Jeliya does not
   reimplement these; it relies on the operating system to honor them.
2. **Application-level, network-independent:** a compiled **minimum-supported
   build floor** in `jeliyad` and the app. A build below the floor **fails
   closed with actionable UX, without a network call and without executing
   untrusted bytes.** An optional build-time-embedded revocation list of
   known-bad build digests is a documented extension; the recommendation for the
   first release is **floor-only**, with the digest list available if a specific
   bad build must be denied before the floor advances.

### Channel separation / anti-substitution

One reserved application or bundle identifier and one signing identity **per
packaged target** (per the `package identity` rule in
[the architecture](dioxus-architecture.md), Decision 3). This ensures a
foreign- or legacy-channel install cannot upgrade in place into the new
generation, and a cross-channel artifact swap fails the platform's own identity
check. The reserved identifier must be one no retiring client already ships, so a
legacy install cannot masquerade as the new generation.

## Anti-rollback and version monotonicity

Two distinct cases; they are kept separate.

- **Cross-generation downgrade** — a build of a *different* protocol/storage
  generation. This already **fails closed before mutation** at the v2 handshake
  and the namespaced-storage gate (#161, #164, #170, #173, #178, #185). It is
  referenced, not re-specified here. There is no data-preserving rollback (see
  [the architecture](dioxus-architecture.md), Decision 2).
- **Same-generation downgrade** — an *older build* of the *same* generation.
  This is this record's own decision. The new namespaced storage generation
  records a **monotonic on-disk marker**: a `written_by` build number and a
  `min_reader` floor. A build refuses to open state written by a strictly newer
  build, **failing closed with the actionable reset path shown, never migrating
  and never silently downgrading**. This is anti-rollback that fits the clean
  slate: it is local, it prevents a downgraded binary from operating on newer
  state, and it retains no legacy compatibility.

The concrete on-disk key or field name is owned by #185 (desktop), #178
(browser), and #173 (Android); this record references those and deliberately
invents no key name, because a replacement key must be a name no retiring client
ever wrote.

The invariant, stated plainly: **version is monotonic per generation; a rollback
that would reinterpret newer state fails closed.**

## Fail-closed states and actionable UX

For each fail-closed state the record fixes the user-facing message shape and the
recovery action. The wording below is the canonical intent; exact EN/FR strings
follow [internationalization](i18n.md) and are finalized with the UI
localization work. In every case the reset path is **shown, not taken** — no
unverified directory is deleted on the user's behalf.

| State | Fail-closed behavior | Actionable UX |
|---|---|---|
| unsupported generation / old client | reject at handshake, before mutation | "This build is too old for your data. Reset to continue: `<reset path>`." |
| below minimum-supported floor / revoked build | refuse to start; no network required | "This version is no longer supported. Download the current version from `<channel>`." |
| tampered artifact (bad checksum/signature) | installer refuses **before extraction** | "Download failed verification and was not installed. Retry from `<channel>`." |
| newer state, older binary (downgrade) | refuse to open state | "This install is older than your data. Update, or reset: `<reset path>`." |
| offline | the prior verified install keeps running; no partial artifact is executed | "You're offline; the installed version is unchanged." |
| interrupted download | checksum mismatch → fail closed → safe retry | "Update didn't complete. Your current version is intact; retry." |

## Offline and interrupted recovery

Recovery must **never execute untrusted bytes.** The prior verified install stays
runnable until a replacement verifies; **verification precedes extraction**; and
where an updater exists (Google Play only), the OS owns download atomicity and
rollback of a failed install.

- **Offline.** No channel here performs a background self-update, so an offline
  machine simply keeps running its installed, already-verified build. Nothing
  partial is fetched or executed.
- **Interrupted download.** The installer scripts verify the `.sha256` sidecar
  before extraction and are idempotent with atomic replace, so a partial or
  interrupted download fails the checksum, is discarded, and leaves the current
  install intact; the operator re-runs the installer. Homebrew and Play recover
  through their own resumable, verified delivery.

Because only the Google Play channel has an updater, it is the only channel with
an updater-owned interrupted-install recovery; every other channel recovers by
re-running an operator-initiated, verify-before-extract install.

## Operator commands and privacy-bounded diagnostics

The record specifies a diagnostics surface — recommended as an extension of
`jeliyad --version` / a `jeliya version` command — that reports, per platform:

- package/app version, artifact **digest**, channel, signing identity /
  notarization state, storage **generation**, and (device evidence only) the
  system WebView version;
- a **verify/support command** that re-checks the installed artifact's digest
  against the published sidecar and reports signature validity, so support can
  confirm a build's identity without transmitting anything sensitive.

**Privacy bound.** Diagnostics must contain **no** identity keys, daemon tokens,
invite tickets, IP addresses, room or member data, or file contents — consistent
with the evidence-sanitization rule in
[verification evidence](verification-evidence.md#evidence-schema-2-source-build-contract)
and the secrets rule in [the documentation profile](PROFILE.md). This bound is
stated as a requirement, not a recommendation.

## Evidence contract and downstream integration

This record is the **authority** for the update-case evidence schema. The
applicable rows are carried by the per-platform packaging issues #186–#190 and
per-platform qualification #199; those downstream implementations do not block
this policy.

**Case matrix, per claimed channel:** current, older, newer, revoked, tampered,
offline, interrupted, downgrade.

**Retained per case** (sanitized and revision-bound per
[verification evidence](verification-evidence.md#evidence-schema-2-source-build-contract),
carrying nothing secret-bearing):

- package **digest**,
- **signing identity**,
- **metadata/version**,
- the **command run**, and
- the **result**.

**Where rows live.** This document defines the schema; the evidence rows are
carried by #186–#190 and #199. In the docs bundle, the
[platform matrix](platform-matrix.md#decided-dioxus-targets) "Decided Dioxus
targets" rows cross-reference this policy. These results are **enforced evidence,
not certification**, and **a missing per-channel update gate blocks only that
channel's publication row** — there is no all-platform barrier, consistent with
#199.

"Tested," for the anti-rollback and fail-closed criteria, means this record
**defines** the case matrix and the retained-evidence schema. Executing the
matrix on real packages is downstream qualification work (#186–#190, #199), not
part of authoring this policy.

<!-- Editing the GitHub issue bodies of #186–#190 and #199 is the
orchestrator's/maintainer's job, not this document's. The documentation side of
the evidence criterion is satisfied in-repo by defining the schema here and
cross-referencing it from the platform matrix; see the Open questions note. -->

## Non-goals

Reproduced so no reviewer re-adds them:

- No N/N-1 compatibility windows, grace periods, migrations, dual protocol
  support, mixed-version windows, canaries, or legacy rollback artifacts. The
  single-generation clean slate has none (see
  [the architecture](dioxus-architecture.md), Decision 2 and "Rejected and
  deferred alternatives").
- No hosted-web/companion skew flow.
- Do **not** claim automatic updates where the channel provides none.
- Do **not** let any updater bypass platform signing authority.
- Do **not** re-decide exact protocol/storage-generation mismatch rejection
  (#157, #161, #164, #170, #172, #199) — reference it.
- This record does not decide legal/compliance publication gating (#118) or the
  build/provenance format of the embedded artifact (#183).

**No compatibility window, migration, or legacy rollback requirement remains.**

## What this record does not decide

- **Signing and notarization authority** — #1 and #2. This record references the
  roots; it does not own or generate them.
- **The embedded artifact's build, provenance format, and qualification
  evidence** — #183 and #199 (see
  [the embedded artifact](first-release-distribution.md#the-embedded-artifact)).
- **Exact protocol and storage-generation mismatch rejection** — #157, #161,
  #164, #170, #172, #173, #178, #185.
- **The concrete on-disk generation marker key or field name** — #185 (desktop),
  #178 (browser), #173 (Android).
- **Legal, privacy, and compliance gates for public publication** — #118.
- **Whether Windows ships in the first release** — #188; this record's Windows
  rows are conditional on that answer.
- **The exact EN/FR wording** of the fail-closed messages — finalized with the
  UI localization work under [internationalization](i18n.md).

## Citations

- [Dioxus clean-slate architecture](dioxus-architecture.md) — the
  single-generation policy, `package identity` rule, per-platform WebView floors,
  and the #121 pointer this record satisfies.
- [First-release distribution boundary](first-release-distribution.md) — the
  artifact-origin trust boundary, exact-digest fail-closed rule, "no renderer
  rollback artifact," and the "#121 retained, reframed" disposition.
- [Signing and notarization](signing-notarization.md) — the macOS Developer ID +
  notarization and Windows Authenticode plans and their secret custody.
- [Security and threat model](security-threat-model.md) — the trust boundaries
  this policy extends.
- [Platform matrix](platform-matrix.md) — the "Decided Dioxus targets" rows this
  policy annotates.
- [Known gaps and roadmap](known-gaps-roadmap.md) — the NEXT key-custody
  commitment this policy makes concrete for updates.
- [Verification evidence](verification-evidence.md) — the sanitized,
  revision-bound evidence-ledger contract the update-case rows follow.
- [Internationalization](i18n.md) — the EN/FR rule the fail-closed messages
  follow.
- [Packaging and distribution](../packaging/README.md) — the actual current
  channels and the checksum-sidecar behavior.
