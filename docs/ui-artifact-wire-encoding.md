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
#183 artifact manifest, each with its own digest and shared provenance (per
asset, a variant whose bytes are not smaller than canonical is omitted — refined
in [Compressible set and canonical identity](#compressible-set-and-canonical-identity);
that per-asset omission is distinct from dropping a *coding* build-wide, which
stays rejected), and have
the daemon serve them by **static content negotiation**: it reads the client's
`Accept-Encoding`, chooses the best *sealed* variant the client accepts (Brotli
preferred over gzip when both are offered and sealed), serves those bytes, and
sets `Content-Encoding` and `Vary: Accept-Encoding`. **The daemon never runs a
compressor at request time.** When the client offers no encoding whose variant is
sealed, the daemon serves the canonical uncompressed bytes; this is ordinary
negotiation, not degradation.

Two negotiation corners are decided deliberately, so neither reads as an
oversight:

- **Nonzero quality weights do not reorder the daemon's preference.** `q=0`
  excludes a coding; among the codings that remain acceptable, the daemon's own
  order — Brotli, then gzip, then identity — decides, even for a request such as
  `Accept-Encoding: gzip;q=1, br;q=0.1`. RFC 9110 §12.1 permits an origin server
  to serve against the client's stated preference, this is the established
  behavior of pre-compressed static serving (nginx `gzip_static`/`ngx_brotli`),
  and weight-ranking would be underspecified anyway: canonical bytes are usually
  *unlisted* and acceptable only by default, so they carry no weight to rank
  against.
- **A request that excludes identity is still served canonical.** For
  `Accept-Encoding: identity;q=0` or `*;q=0` with no sealed coding acceptable,
  the daemon deliberately exercises RFC 9110 §12.4.1's option to **disregard the
  header** rather than send 406. A 406 branch would serve no shipped client (no
  browser or WebView emits this shape), would fork behavior between a sealed
  daemon and the manifest-less dev directory (which has nothing sealed and would
  406 on everything), and would blur the line this record draws: a negotiation
  shortfall serves canonical; only a sealed-but-corrupt variant refuses.

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
  `html`/`htm`, `js`/`mjs`, `css`, `wasm`, `json`/`map`, `webmanifest`, `svg`,
  and `txt`. These are first-party, content-addressed, and contain no secret and no
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
not just the packaged one. (What the hash check *proves* differs by source — see
[Fail-closed integrity](#fail-closed-integrity).)

A `--ui-dir` pointed at a bare directory that carries **no manifest** is a
distinct, dev-only state: it has no sealed variants and no sealed digests, so the
daemon serves canonical uncompressed bytes with no `Content-Encoding`. That is
not a silent degrade of a sealed variant — there is nothing sealed to degrade; it
is the honest "nothing was sealed" answer. The unsealed state is entered **only
on a definite not-found** of the manifest file: a manifest that is present but
unreadable, unparsable, or schema-invalid is a **fail-closed error**, never a
collapse into the unsealed state. The same rule binds `Embedded` — a compiled-in
manifest that fails to parse is a packaging defect and fails the same way;
binary integrity proves the shipped bytes are intact, not that they parse, and a
deterministically malformed manifest passes #183's cross-target diff on every
target.

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

- When a manifest is present, **every representation the daemon serves must
  hash to its sealed digest before serving** — a chosen variant to the variant
  digest the manifest seals, and bytes served as identity (by negotiation
  shortfall, or because the asset has no sealed variants, e.g. `png`/`woff2`)
  to `identity.digest`. For `Embedded`, the bytes are compiled in and sealed at
  build, so this holds transitively; for `Dir`, the file is read from disk and
  a developer could have edited it, so the read bytes are hashed against the
  manifest **before** serving. Canonical-corrupt must fail exactly as
  variant-corrupt does: a contract that refuses a bad variant but serves a bad
  canonical with `200` is not fail-closed. The manifest-less bare directory
  remains the one exempt state — nothing is sealed there.
- **A valid manifest is an allow-list.** A request path that, after SPA-fallback
  resolution, matches no manifest entry is answered **404** — never read from
  the directory or the embed and served unverified. An extra, stale, or
  hand-added file in a `--ui-dir` (or a leftover compiled-in asset) is not
  servable content; only the manifest-less bare directory serves unlisted
  files.
- **What that check proves differs by source, stated rather than implied.** For
  `Embedded`, the manifest is authenticated transitively by the integrity of the
  daemon binary, so a mismatch is genuine tamper or packaging evidence. For
  `Dir`, the manifest is read from the same operator-supplied directory as the
  variants, so the check detects **corruption and manifest-file skew** (a partial
  copy, a stale variant, a hand-edited file) — not adversarial tampering: a
  writer who can edit the directory can rewrite a variant *and* its manifest
  entry together, exactly as they could rewrite the canonical bytes themselves.
  `--ui-dir` is an operator-trusted source. If a deployment ever requires an
  *authenticated* directory artifact, the manifest itself must first be verified
  against an externally pinned digest — a future decision under #183's
  provenance remit, not a property this record claims.
- A requested-and-sealed encoding whose variant is **missing, or whose bytes do
  not match the sealed digest**, is a fail-closed error (a 5xx with an
  `internal`-class body) — **not** a silent fall-through to the uncompressed bytes
  and **not** a fall-through to a different encoding. This borrows the posture
  [#113 already requires](first-release-distribution.md) of a legacy artifact
  ("fails closed; it does not fall back, and it does not serve what it has");
  the trust root behind the check is build-pinned for `Embedded` and
  operator-supplied for `Dir`, per the previous bullet.
- The distinction drawn sharply: *manifest definitively absent* → the unsealed
  dev-only state (serve canonical, nothing to verify). *Manifest present but
  unreadable, unparsable, or schema-invalid* → fail closed; collapsing a load
  error into the unsealed state would let one truncated file silently disable
  every digest check while wearing the honest "nothing was sealed" answer.
  With a valid manifest: *no offered encoding is sealed* → serve (verified)
  canonical — normal negotiation; *any served representation's bytes are
  corrupt/absent* → fail closed (tamper/packaging detection for `Embedded`,
  corruption/skew detection for `Dir`).

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
  level), and a manifest digest — **detached** (computed over the complete
  serialized manifest and carried alongside it) or stored in a field excluded
  from a canonical serialization; the exact scheme is #183's to fix. A digest
  stored inside the very bytes it covers is a fixed point no producer can
  compute.
- **Per-asset entry:**
  - `path` (request-relative, matching `safe_rel` output),
  - `content_type` (the canonical decoded type),
  - `identity`: the content-address, `{ digest: sha256(canonical bytes), bytes }`,
  - `encodings`: a list of `{ coding, path, digest: sha256(variant bytes), bytes,
    params }` where `coding` is `br` or `gzip`, omitted or empty when no variant
    is smaller than canonical. `path` is the artifact-relative location of the
    sealed variant bytes, validated by the same containment rules as the asset
    `path` and forbidden from colliding with any canonical asset `path`; for
    `Embedded` the same key addresses the compiled-in variant lookup. The
    concrete layout convention (for example `<path>.br` / `<path>.gz` suffixing)
    remains #183's choice — the contract requires only that the choice is
    sealed in the manifest, never assumed by convention.
- **Canonical artifact digest** — the single identity the exact-version
  rejection pins, derived **only** from the canonical byte set: the manifest's
  entries sorted bytewise by `path`, each contributing `path` and
  `identity.digest` to a deterministic serialization (recommended: `path`, a
  NUL separator, the lowercase-hex `identity.digest`, a newline, per entry)
  whose SHA-256 is the artifact digest. It excludes `encodings`,
  `content_type`, and provenance, so sealing, dropping, or re-compressing a
  variant — or a compressor version bump — can never change the artifact's
  canonical identity; the whole-manifest digest above continues to cover
  everything. #183 may seal a different serialization only by recording it in
  the manifest itself; the invariants — canonical-only inputs, bytewise `path`
  order, deterministic — are contractual.
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
- Change the load/serve path so `serve_static` negotiates: resolve the SPA
  fallback first (an extension-less route resolves to `index.html`, exactly as
  today — a deep link such as `/rooms/r-99` must keep reloading), then resolve
  the **resulting asset path** against the manifest's entry set (with a valid
  manifest an unlisted resolved path is a **404**, never a filesystem or embed
  fallback), parse `Accept-Encoding`
  (`q=0` excludes a coding; unknown codings are ignored; `*` matches every
  coding not explicitly listed — so a bare `*` accepts the sealed codings and
  `*;q=0` excludes them, RFC 9110 §12.5.3; nonzero weights do **not** reorder),
  pick Brotli, then gzip, then identity among **sealed** codings the client
  accepts — an empty acceptable set serves canonical, never 406 — locate
  variant bytes solely by the manifest-sealed variant `path` (never a derived
  filename), hash the chosen representation — variant or canonical — against
  its sealed digest (the variant digest or `identity.digest`), and build the
  response with `Content-Type` (decoded type),
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
directory — request the **root document, the wasm, the JS, and the stylesheet**.
Every row against an asset that has sealed variants additionally asserts
`Vary: Accept-Encoding` is present — including the gzip, no-header, weighted,
and `identity;q=0` rows — per the per-asset requirement in
[Security boundary](#security-boundary); an implementation that adds `Vary`
only on the Brotli branch must fail these tests. Every row — identity and
encoded alike — also asserts the response `Content-Type` equals the manifest's
sealed `content_type` (the canonical decoded type: `application/wasm` for the
wasm asset, never `application/octet-stream` and never a type derived from a
variant `path`). The rows:

- `Accept-Encoding: br, gzip` → assert `Content-Encoding: br`, `Content-Length`
  equals the sealed `br` bytes, `Vary: Accept-Encoding` present, and
  `brotli_decode(body)` SHA-256 equals the sealed `identity.digest`.
- `Accept-Encoding: gzip` → assert `Content-Encoding: gzip`, `Content-Length`
  equals the sealed `gzip` bytes, and `gunzip(body)` SHA-256 equals
  `identity.digest`.
- No `Accept-Encoding` → assert **no** `Content-Encoding`, body equals the
  canonical bytes, SHA-256 equals `identity.digest`.
- `Accept-Encoding: br;q=0, gzip` → assert gzip chosen, not br.
- `Accept-Encoding: gzip;q=1, br;q=0.5` → assert **br** chosen: nonzero weights
  do not reorder the daemon's preference (the recorded RFC 9110 §12.1 stance).
- `Accept-Encoding: identity;q=0` (no sealed coding offered) → assert `200`,
  canonical bytes, **no** `Content-Encoding`: the recorded RFC 9110 §12.4.1
  disregard-the-header stance, pinned so an implementer does not "helpfully"
  add a 406.
- `Accept-Encoding: *` → assert `Content-Encoding: br`: `*` matches available
  codings not explicitly listed (RFC 9110 §12.5.3), so the daemon's own order
  picks Brotli — pinned so an implementer does not lump `*` in with unknown
  codings and serve canonical.
- `Accept-Encoding: identity;q=1, *;q=0` → assert canonical bytes, **no**
  `Content-Encoding`: the wildcard at `q=0` excludes the unlisted sealed
  codings (the real-world Safari range-request shape).
- **Every entry, every coding:** iterate the manifest's **own entry list** (never
  a hard-coded path set) and, for each asset entry and each sealed coding **plus
  identity**, on **both** sources, request the asset over HTTP offering exactly
  that coding, assert the response's `Content-Encoding` equals the requested
  coding (absent for identity) — proving the sealed variant, not canonical
  identity, answered — assert the **encoded** body's SHA-256 equals that
  coding's sealed variant digest (byte-exact: a different valid gzip/Brotli
  stream with the same decoded content must fail — the cross-target gate
  checks stored artifacts, this row checks what the serving path actually
  returned), and assert the
  decoded body's SHA-256 equals `identity.digest`. The four named types above
  stay the behavioral subset (headers, weights, fail-closed); this row proves the
  universal claim that every sealed variant decodes to its canonical identity —
  which neither the served-bytes-vs-variant-digest check (self-consistent even
  when the sealed variant is wrong) nor #183's cross-target diff (a deterministic
  generation defect passes identically everywhere) proves on its own.
- **Fail-closed:** with a variant deliberately corrupted or removed from the
  source, a request offering that sealed encoding returns a **5xx**, never a
  `200` with uncompressed bytes and never a different encoding.
- **Fail-closed, canonical:** with the canonical file corrupted in the `Dir`
  source, a request selecting identity (no `Accept-Encoding`, and a request
  for an asset with no sealed variants such as a `png`) returns a **5xx**,
  never a `200` whose bytes do not match `identity.digest`.
- **Fail-closed, manifest:** a `--ui-dir` whose manifest is present but
  truncated or unparsable fails closed (**5xx**/refusal), never serves
  canonical bytes with no `Content-Encoding` as if no manifest existed.
- **Allow-list:** with an extra file (for example `debug.html`) placed in the
  `Dir` source but absent from the manifest, requesting it returns **404** —
  never a `200` with unverified bytes; the manifest-less bare directory
  serving that same file stays the dev-only contrast. In the same run, a deep
  link such as `/rooms/r-99` still resolves through the SPA fallback to the
  sealed `index.html` and serves it negotiated as usual — the allow-list
  applies to the resolved asset path, never to the raw route.
- **Security regression:** `GET /api/files/local?...` with `Accept-Encoding: br,
  gzip` returns **no** `Content-Encoding` and the inert-attachment headers are
  unchanged.
- **Identical sources:** the negotiated encoding, `Content-Type`,
  `Content-Length`, and decoded digest for each asset match between the
  `Embedded` and `Dir` daemons.
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
