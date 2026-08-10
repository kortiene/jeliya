---
type: "Decision"
title: "How the embedded UI artifact is compressed on the wire"
description: "Decision record for how the embedded UI artifact is delivered on the wire: the #183 manifest seals Brotli and gzip variants of each compressible asset served by static content negotiation, the canonical content-address stays the uncompressed bytes, a corrupt or missing variant fails closed, and Embedded and --ui-dir sources behave identically."
tags: ["release", "web", "dioxus", "compression", "artifact", "clean-slate", "security"]
timestamp: "2026-08-10T18:00:00Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "release-engineers", "security-reviewers"]
---

# How the embedded UI artifact is compressed on the wire

**Nothing in this record is built.** It is a canonical decision about unwritten
code, exactly like [the native update policy](native-update-policy.md) and
[the first-release distribution boundary](first-release-distribution.md). It
satisfies issue #207 and answers the open finding
[the architecture](dioxus-architecture.md) carries from the #158 spike: whether
the daemon should negotiate content encoding for the embedded UI artifact or ship
pre-compressed bytes sealed alongside it. It changes no production code. The
serving and manifest code this decision constrains is specified in
[Deferred implementation contract](#deferred-implementation-contract) so it is
executable, but it lands under #183 (manifest and build) and a `serve.rs`
serving-and-verification slice, not here.

Today `jeliyad` serves the static UI artifact **uncompressed and without
negotiation**: `crates/jeliyad/src/serve.rs`'s `UiSource::load` returns only the
bytes and a content type, `asset()` sets only `Content-Type`, and a request
carrying `Accept-Encoding: gzip, br` is answered with the full canonical bytes
and no `Content-Encoding`. Measured against a real `jeliyad` 0.6.1 serving the
#158 spike's embedded assets
([spike results](../spikes/dioxus-web/README.md)), the wasm alone is
**542,111 B on the wire; `gzip -9` takes it to 211,557 B — a 61% reduction** —
and Brotli would beat that. That saving is currently unclaimed. It costs nothing
*today* only because every path that exercises the daemon is same-host loopback
([#113 ships no hosted origin and no CDN](first-release-distribution.md)).

## The decision

Seal a **Brotli** and a **gzip** variant of each compressible asset inside the
#183 artifact manifest, each with its own digest and shared provenance, and have
the daemon serve them by **static content negotiation**: it reads the client's
`Accept-Encoding`, chooses the best *sealed* variant the client accepts (Brotli
preferred over gzip when both are offered and sealed), serves those bytes, and
sets `Content-Encoding` and `Vary: Accept-Encoding`. **The daemon never runs a
compressor at request time.** When the client offers no encoding whose variant is
sealed, the daemon serves the canonical uncompressed bytes; this is ordinary
negotiation, not degradation.

Why sealing rather than on-the-fly, which is the crux the issue names: #183
requires the artifact to be **content-addressed and byte-identical across every
target**. Compressing at request time makes the served bytes a function of the
daemon's Brotli/zlib library version rather than of the sealed artifact — it
cannot be covered by the identical-bytes assertion, and it puts a compressor (CPU
per cold load, and a decompression-bomb-adjacent surface) in the hot serving
path. Sealing keeps the serving path trivial, keeps compression reproducible, and
lets the manifest cover every byte that leaves the daemon.

Why record it **now**, before #183 builds, even though the realized wire benefit
is loopback-negligible today (see
[Rejected and deferred alternatives](#rejected-and-deferred-alternatives)): #183
is *Planned* and about to fix the manifest format. If compressed variants are not
folded into that schema at design time, adding them later re-opens the
sealed-artifact format and re-seals a shipped artifact. The decision is cheap to
make now and expensive to retrofit.

## Compressible set and canonical identity

- **Compress** the text-like and wasm assets `guess_mime` already enumerates:
  `html`, `js`/`mjs`, `css`, `wasm`, `json`/`map`, `webmanifest`, `svg`, and
  `txt`. These are first-party, content-addressed, and contain no secret and no
  attacker-controlled input, so compressing them is safe (see
  [Security boundary](#security-boundary)).
- **Do not bother compressing** already-compressed binary assets (`png`, `jpg`,
  `webp`, `woff2`, `gif`, `ico`): the manifest may omit their variants, and the
  daemon serves them canonical. The build should skip a variant whose bytes are
  not smaller than the canonical bytes; a variant larger than the original is a
  build-time waste, not a correctness problem, and the daemon must treat "no
  sealed variant" as "serve canonical".
- **Canonical identity is uncompressed.** The digest #183 content-addresses, and
  the digest the exact-version rejection matches, is the SHA-256 of the
  **canonical uncompressed** byte set. Every variant is an additional sealed
  artifact keyed to that identity, never a replacement for it. This satisfies the
  issue's constraint that if encoding is negotiated the digest must continue to
  cover the canonical uncompressed bytes.

## Embedded and --ui-dir sources behave identically

Both `UiSource::Embedded` and `UiSource::Dir` resolve their bytes through the
**same manifest-driven negotiation and verification code**. The only difference
is where the manifest and the variant bytes come from — compiled-in for
`Embedded`, read from the `--ui-dir` for `Dir`. A `--ui-dir` daemon pointed at a
built artifact directory (the one `dx`/#183 produces, manifest included) behaves
**byte-for-byte identically** to an `embed-ui` daemon. This is the property that
keeps a development build from masking a packaging bug: the negotiation, the
`Content-Encoding`, and the fail-closed hash check are exercised on every source,
not just the packaged one.

A `--ui-dir` pointed at a bare directory that carries **no manifest** is a
distinct, dev-only state: it has no sealed variants and no sealed digests, so the
daemon serves canonical uncompressed bytes with no `Content-Encoding`. That is
not a silent degrade of a sealed variant — there is nothing sealed to degrade; it
is the honest "nothing was sealed" answer.

## Security boundary

- **Compress only the static, first-party UI artifact** served by
  `serve_static`. These bytes are content-addressed, contain no daemon token (the
  browser fetches the token separately at `/api/session`), and reflect no
  attacker-supplied input, so no CRIME/BREACH-class oracle exists.
- **Never compress a response whose body is influenced by peer-supplied
  content.** The fetched-file path `local_file` deliberately serves inert
  attachments (`Content-Disposition: attachment`, `X-Content-Type-Options:
  nosniff`, a locked-down CSP, and a `safe_download_mime` type). It,
  `share_upload`, and all `/api/*` and `/ws` responses **must remain
  uncompressed**. This is a hard rule; the verification matrix includes a
  regression guard that `local_file` ignores `Accept-Encoding`.
- **Compression must not weaken existing header guarantees.** Serving a variant
  changes only `Content-Encoding`, the `Content-Length` (which becomes the encoded
  length — `hyper`'s in-memory-bytes body sets it from the served bytes), and adds
  `Vary: Accept-Encoding`. The asset's `Content-Type` stays the canonical type of
  the *decoded* asset. This decision adds no `nosniff`/CSP to the static path and
  removes none; those remain owned by the WebView security matrix
  ([#189/#196](security-threat-model.md)).
- **`Vary: Accept-Encoding` is required** on any asset that has sealed variants,
  so an HTTP cache never hands a Brotli body to a client that did not accept it.

## Fail-closed integrity

- When the daemon selects a sealed variant, the served bytes **must hash to the
  variant digest the manifest seals**. For `Embedded`, the bytes are compiled in
  and sealed at build; for `Dir`, the variant file is read from disk and a
  developer could have edited it, so the read bytes are hashed against the
  manifest **before** serving.
- A requested-and-sealed encoding whose variant is **missing, or whose bytes do
  not match the sealed digest**, is a fail-closed error (a 5xx with an
  `internal`-class body) — **not** a silent fall-through to the uncompressed bytes
  and **not** a fall-through to a different encoding. This is the same posture
  [#113 already requires](first-release-distribution.md) of a legacy artifact
  ("fails closed; it does not fall back, and it does not serve what it has"),
  applied to the variant.
- The distinction drawn sharply: *no offered encoding is sealed* → serve canonical
  (normal negotiation). *An offered encoding is sealed but its bytes are
  corrupt/absent* → fail closed (tamper/packaging detection).

## Measured sizes for the bundle budget

The **only** measured evidence today is the #158 spike
([spike results](../spikes/dioxus-web/README.md)), against `jeliyad` 0.6.1, a
**v1** slice with **no `wasm-opt`**. These are explicit **upper bounds**, not
budgets, and Brotli was not measured. The authoritative raw/`br`/`gzip` numbers
for #198 must be **re-measured against the real #183 artifact** (post-`wasm-opt`,
protocol-v2 client) once #183 lands; this doc is their citable home. Compression
is **not** a substitute for the budget — #198 still sets it.

| Asset | Raw | `gzip -9` | Brotli | Reduction (gzip) |
|---|---|---|---|---|
| `..._bg.wasm` | 542,111 B (529 KiB) | 211,557 B (207 KiB) | to be measured (expected below gzip) | 61% |
| `...web.js` | 83,570 B | 12,352 B | to be measured (expected below gzip) | 85% |
| `styles.css` (verbatim, unminified) | 95,524 B | 23,539 B | to be measured (expected below gzip) | 75% |
| root document (`index.html`) | to be measured | to be measured | to be measured | — |
| dist total | 747,884 B (730 KiB) | to be measured | to be measured | — |

The root document was not separately measured in the spike (the dist total
includes it); its raw and encoded sizes are re-measured with the rest against the
#183 artifact. Record every row raw and encoded so #198 reads them from here.

## Rejected and deferred alternatives

- **Compress at request time (on-the-fly gzip/Brotli).** *Rejected.* Served bytes
  become a function of the daemon's compression-library version, so they cannot be
  covered by #183's identical-bytes assertion; it adds a compressor to the hot
  path (CPU per cold load, and a decompression-bomb-adjacent surface); and it buys
  nothing sealing does not. If encoding were ever negotiated this way, the digest
  would still have to cover the canonical uncompressed bytes — which is exactly
  what sealing does without the downsides.
- **Serve uncompressed for the first release, defer any compression.**
  *Considered and rejected as the recorded strategy, stated honestly.* Its one
  true premise is that all delivery today is loopback (#113), where the wire
  saving is not measurable and the #158 cold load is already fast (72 ms FCP,
  263 ms interactive). But (a) #183 is fixing the manifest **now**, and
  retrofitting variants later re-seals a shipped artifact; (b) the mechanism
  chosen here is static and compressor-free, so it costs the serving path
  essentially nothing; (c) the packaged desktop and Android WebViews still fetch
  this artifact, and a slower device benefits from roughly 330 KiB less to move
  and parse. The **realized** wire benefit is deferred to any future non-loopback
  path (which itself requires a new decision record per #113) while the
  **mechanism** is decided now. This keeps the P2 priority honest: it is worth
  deciding, not worth a hot-path compressor.
- **Embed the variants but keep the daemon serving only canonical.** *Rejected.*
  It pays the binary-size cost of the variants (they are compiled in) and claims
  none of the benefit, and it never exercises the negotiation/fail-closed path, so
  it cannot satisfy the issue's "receives encoded bytes" and "behave identically"
  criteria.

## Deferred implementation contract

This is specified so it is executable, but it is **not** part of #207's doc
deliverable. #207 records the decision; the code and manifest land under #183
(manifest/build) and a `serve.rs` serving-and-verification slice, which may be a
#207 follow-up or folded into #183/#199. The decision doc stays
`implementation_status: planned` accordingly.

### Manifest schema #183 must produce

Extend the #183 artifact manifest so each asset entry carries its identity and
its sealed variants, and the manifest carries shared provenance:

- **Manifest-level provenance** (already #183's remit; the variants join it):
  renderer, source SHA, toolchain versions (`rustc`, `wasm-bindgen`, `dx` /
  `wasm-opt`, **and the exact compressor and version** used for each coding — for
  example the `brotli` CLI/library version and quality, and the `gzip`/zopfli
  level), and a digest over the whole manifest.
- **Per-asset entry:**
  - `path` (request-relative, matching `safe_rel` output),
  - `content_type` (the canonical decoded type),
  - `identity`: the content-address, `{ digest: sha256(canonical bytes), bytes }`,
  - `encodings`: a list of `{ coding, digest: sha256(variant bytes), bytes,
    params }` where `coding` is `br` or `gzip`, omitted or empty when no variant
    is smaller than canonical.
- **Determinism requirement:** Brotli/gzip output is **not** guaranteed identical
  across tool versions or platforms. The build must pin the exact compressor and
  settings (sealed in `params`), and #183's cross-target identical-bytes gate must
  cover **every** sealed byte set — canonical *and* each variant — so a `br`
  produced on macOS equals the `br` produced on Linux. Prefer a deterministic
  compressor invocation (for example Brotli quality 11, and a reproducible gzip
  such as zopfli or `gzip -n -9`) documented in `params`.

### Daemon serving changes in serve.rs

- Give `UiSource` access to the parsed manifest for both variants — compiled-in
  for `Embedded`, loaded from the `--ui-dir` for `Dir`.
- Change the load/serve path so `serve_static` negotiates: parse `Accept-Encoding`
  (honour `q=0` to exclude a coding; ignore unknown codings), pick Brotli, then
  gzip, then identity among **sealed** codings, hash the chosen variant against
  the sealed digest, and build the response with `Content-Type` (decoded type),
  `Content-Encoding` (when not identity), the encoded `Content-Length` (set
  automatically from the served in-memory bytes), and `Vary: Accept-Encoding`.
- Keep the SPA fallback (`index.html` for extension-less routes) on the same
  negotiated path.
- **Leave `local_file`, `share_upload`, `session`, `health`, `preflight`,
  `gate_refusal`, and all JSON responses untouched** — they must never gain
  `Content-Encoding`.

## Verification matrix

The serving slice implements this matrix as tests. For **each** source — an
`embed-ui` daemon, and a `--ui-dir` daemon pointed at the built artifact
directory — request the **root document, the wasm, the JS, and the stylesheet**:

- `Accept-Encoding: br, gzip` → assert `Content-Encoding: br`, `Content-Length`
  equals the sealed `br` bytes, `Vary: Accept-Encoding` present, and
  `brotli_decode(body)` SHA-256 equals the sealed `identity.digest`.
- `Accept-Encoding: gzip` → assert `Content-Encoding: gzip`, `Content-Length`
  equals the sealed `gzip` bytes, and `gunzip(body)` SHA-256 equals
  `identity.digest`.
- No `Accept-Encoding` → assert **no** `Content-Encoding`, body equals the
  canonical bytes, SHA-256 equals `identity.digest`.
- `Accept-Encoding: br;q=0, gzip` → assert gzip chosen, not br.
- **Fail-closed:** with a variant deliberately corrupted or removed from the
  source, a request offering that sealed encoding returns a **5xx**, never a
  `200` with uncompressed bytes and never a different encoding.
- **Security regression:** `GET /api/files/local?...` with `Accept-Encoding: br,
  gzip` returns **no** `Content-Encoding` and the inert-attachment headers are
  unchanged.
- **Identical sources:** the negotiated encoding, `Content-Length`, and decoded
  digest for each asset match between the `Embedded` and `Dir` daemons.
- **Cross-target identical bytes:** #183's gate diffs every sealed variant across
  daemon targets; this decision only adds the variants to that set.
- **Evidence:** record raw and encoded sizes per asset and as a dist total into
  the [size table](#measured-sizes-for-the-bundle-budget) and the release evidence
  #198 reads.

## Records this decision amends

Three existing pages currently point at this as an open question and are updated
in the same change so the wiki stays consistent and reachable:

- **[the architecture](dioxus-architecture.md)** — the #158-findings paragraph is
  given a pointer here; Decision 5 ("one embedded artifact") and the Decision 7
  artifact row gain a clause that the sealed manifest also carries the compressed
  variants and their digests. #183 stays the owner of the schema.
- **[the first-release distribution boundary](first-release-distribution.md)** —
  "The embedded artifact" section gains a bullet that the sealed manifest includes
  pre-compressed `br`/`gzip` variants served by static negotiation, canonical
  digest unchanged.
- **[the documentation index](index.md)** — the new page is listed so every
  concept remains reachable from the root index.

## Non-goals

Reproduced so no reviewer re-adds them:

- **No compression of the control surface or the file endpoints.** This decision
  is about static UI assets only. `/ws`, `/api/session`, `/api/health`,
  `/api/files/*`, and every JSON envelope stay uncompressed.
- **No hosted origin and no CDN.** The artifact is still served only by the
  trusted local daemon path (#113). This decision introduces no network hop.
- **Compression is not a bundle budget.** #198 still owns the budget; a smaller
  wire size never licenses a larger artifact.
- **No renderer rollback artifact and no legacy `ui/dist` compatibility.** This
  targets only the new #183 artifact; no fallback to an uncompressed legacy
  artifact is retained (clean-slate cutover).
- **This decision adds or removes no `nosniff`/CSP/navigation policy.** Those are
  owned by the WebView security matrix (#189/#196); compression is orthogonal to
  them.

## Risks and mitigations

- **Cross-platform variant non-determinism.** Brotli/gzip bytes can differ by tool
  version, which would false-fail the identical-bytes gate. *Mitigation:* pin the
  compressor and settings, seal them in `params`, and make #183's gate cover the
  variants. This risk is why on-the-fly compression is rejected — it makes the
  problem unfixable.
- **Binary-size inflation for `Embedded`.** The variants are compiled in, so the
  daemon binary grows by the compressed sizes; on loopback the wire benefit is
  small. *Mitigation:* skip variants that are not smaller than canonical, and keep
  the P2 scope (static, no hot-path compressor).
- **Scope creep into the file/download or WS paths.** *Mitigation:* the hard rule
  in [Security boundary](#security-boundary) and the `local_file` regression guard
  in the [verification matrix](#verification-matrix).
- **Deciding ahead of #183.** The manifest does not exist yet, so the
  hash/fail-closed criteria cannot be *executed* by #207 alone. *Mitigation:*
  record the decision with `implementation_status: planned`; the serving slice
  lands with or after #183. This is the intended use of the status axes, not a
  gap.

## What this record does not decide

- **The artifact's build, provenance format, and cross-target identical-bytes
  gate** — #183. This decision only adds the compressed variants to the set that
  gate covers.
- **The first-release bundle budget** — #198, which consumes the sizes recorded
  here.
- **Any future networked delivery of the artifact** — that requires its own
  decision record per #113; this record fixes the *encoding mechanism*, not a new
  transport.
- **The `nosniff`/CSP/navigation policy of the packaged WebView** — #189/#196.

## Citations

- [Dioxus clean-slate architecture](dioxus-architecture.md) — Decision 5, the
  Decision 7 artifact boundary, the #183 manifest requirement, and the #158
  finding this record answers.
- [First-release distribution boundary](first-release-distribution.md) — the
  artifact-origin trust boundary and the exact-digest fail-closed rule this
  decision must not weaken.
- [Security and threat model](security-threat-model.md) — the artifact-origin and
  daemon-token trust boundaries, and the inert served-file rationale that forbids
  compressing peer-supplied content.
- [Dioxus web spike results](../spikes/dioxus-web/README.md) — the only recorded
  measurements, explicitly v1 upper bounds with no `wasm-opt`.
- [Documentation profile](PROFILE.md) — the authoring and CI contract this page
  follows.
