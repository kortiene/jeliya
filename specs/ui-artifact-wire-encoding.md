# Spec: How the embedded UI artifact is compressed on the wire (#207)

> **This is an implementation plan, not the deliverable.** The deliverable of
> issue #207 is a single canonical **Decision** document in `docs/` that records
> the wire-encoding strategy for the embedded UI artifact, plus the small pointer
> edits that make the existing architecture records defer to it, plus the
> measured sizes #198 consumes. This spec tells another engineer/agent exactly
> what that document must decide, how to author it against the repository's
> documentation contract, which existing records it amends, and how the decision
> is later verified in code. **It changes no production code.**
>
> The daemon-serving and manifest work this decision *constrains* is specified in
> §7 so it is executable, but it is **deferred to the named issues** (#183 for the
> manifest and build, a serving + verification slice for `serve.rs`). Recording
> the decision does not wait on that code — that is exactly what the four
> documentation status axes exist for.

## 1. Summary

Today `jeliyad` serves the static UI artifact **uncompressed and without
negotiation**: `crates/jeliyad/src/serve.rs`'s `UiSource::load` (lines 79–89)
returns only `(Bytes, content-type)`, `asset()` (line 1928) sets only
`Content-Type`, and a request carrying `Accept-Encoding: gzip, br` is answered
with the full canonical bytes and no `Content-Encoding`. Measured against a real
`jeliyad` 0.6.1 serving the #158 spike's embedded assets, the wasm alone is
**542,111 B on the wire; `gzip -9` takes it to 211,557 B — a 61% reduction** —
and Brotli would beat that. That saving is currently unclaimed. It costs nothing
*today* only because every path that exercises the daemon is same-host loopback
(#113 ships no hosted origin and no CDN).

Issue #207 asks the program to **decide, and record**, one of two shapes:

1. the daemon **negotiates** content encoding at request time, or
2. the artifact **ships pre-compressed variants sealed** alongside it.

**Recommended decision (this spec argues for it decisively in §4):** seal a
**Brotli** and a **gzip** variant of each compressible asset inside the #183
artifact manifest, each with its own digest and shared provenance, and have the
daemon serve them by **static content negotiation** — it selects the best sealed
variant the client offered and **never compresses at request time**. The
content-address that #183 seals remains the **canonical uncompressed** bytes; a
requested-and-sealed variant whose bytes do not hash to the sealed digest, or is
absent, **fails closed** rather than silently serving the uncompressed bytes. The
same manifest-driven path serves both `UiSource::Embedded` and `UiSource::Dir`,
so a development build cannot mask a packaging bug.

**Recommended deliverable:** a new page `docs/ui-artifact-wire-encoding.md`,
`type: "Decision"`, `status: "canonical"`, `implementation_status: "planned"`,
`verification_status: "unverified"`, `release_status: "unreleased"`. It is a
canonical decision about **unwritten code**, exactly like
[`docs/native-update-policy.md`](../docs/native-update-policy.md) and
[`docs/first-release-distribution.md`](../docs/first-release-distribution.md),
and it resolves the "open question this spike did not answer" that
[`docs/dioxus-architecture.md`](../docs/dioxus-architecture.md) currently records
at its #158-findings section (see §6).

## 2. Owning surface and where this fits

- **Primary artifact:** a new page in the `docs/` OKF wiki. `docs/` is
  CI-validated by `scripts/check-docs.mjs`; the
  [documentation profile](../docs/PROFILE.md) is the contract (exactly ten
  required frontmatter fields, restricted YAML subset, single H1 matching
  `title`, file-relative links, and reachability from `docs/index.md` —
  orphaned docs fail CI).
- **Inputs the doc must cite, not restate:**
  - [`dioxus-architecture.md`](../docs/dioxus-architecture.md) — **Decision 5**
    ("one embedded artifact", line 208), the **Decision 7** artifact boundary row
    (line 266), the artifact-manifest requirement it hands to **#183**, and the
    **#158 spike finding** (lines 339–343) that names this decision an open
    question. This new doc *answers* that open question.
  - [`first-release-distribution.md`](../docs/first-release-distribution.md) —
    the **Artifact origin** trust-boundary row (line 69) and the
    **"The embedded artifact"** section (line 134): one artifact, identical bytes
    in every target, content-addressed with recorded provenance, exact-digest
    fail-closed, cache-forever-by-hash. This decision must not weaken any of
    those.
  - [`security-threat-model.md`](../docs/security-threat-model.md) — the artifact
    origin and daemon-token trust boundaries; the rationale that peer-supplied
    content on the file-download path is served inert (the boundary that forbids
    compressing it — see §4.4).
  - [`spikes/dioxus-web/README.md`](../spikes/dioxus-web/README.md) — the only
    recorded measurements (§5), explicitly upper bounds (v1 slice, no
    `wasm-opt`).
  - [`docs/PROFILE.md`](../docs/PROFILE.md) — the authoring/CI contract.
- **Downstream consumers (do not block this decision):**
  - **#183** — the content-addressed artifact and its manifest. This decision
    *extends the manifest schema* it must produce (§7.1). #183 stays the owner of
    the build, the provenance format, and the cross-target identical-bytes gate;
    this decision only adds the compressed variants to the set that gate covers.
  - **#198** — the first-release bundle budget. It consumes the raw/encoded
    numbers this decision records (§5). Compression is **not** a substitute for
    the budget; #198 still sets it.
  - **#199** — per-platform qualification carries the applicable evidence rows.
- **Dependency:** program **#156**. **Relates to** the artifact decision in
  **#183** and the first-release distribution decision in **#113**; **feeds**
  **#198**.

## 3. Non-goals (carry verbatim into the doc)

Reproduce these so no reviewer re-adds them:

- **No compression of the control surface or the file endpoints.** This decision
  is about static UI assets only. `/ws`, `/api/session`, `/api/health`,
  `/api/files/*`, and every JSON envelope stay uncompressed.
- **No hosted origin and no CDN.** The artifact is still served only by the
  trusted local daemon path (#113). This decision introduces no network hop; it
  makes a claim about how the *loopback* (and future, separately-decided
  networked) delivery of a first-party artifact is encoded.
- **Compression is not a bundle budget.** #198 still owns the budget; a smaller
  wire size never licenses a larger artifact.
- **No renderer rollback artifact and no legacy `ui/dist` compatibility.** This
  targets only the new #183 artifact. No fallback to an uncompressed legacy
  artifact is retained (clean-slate cutover).
- **This decision adds or removes no `nosniff`/CSP/navigation policy.** Those are
  owned by the WebView security matrix (#189/#196). Compression must be
  orthogonal to them (§4.4).

## 4. The decision the document must record

The document must be **decisive**. The recommended positions below are what the
author confirms with maintainers and records with rationale.

### 4.1 Strategy — seal variants, negotiate statically

**Decision:** the #183 artifact manifest seals, for each compressible asset, the
canonical uncompressed bytes **and** a `br` (Brotli) and a `gzip` variant. The
daemon performs **static content negotiation**: it reads the client's
`Accept-Encoding`, chooses the best *sealed* variant the client accepts (Brotli
preferred over gzip when both are offered and sealed), serves those bytes, and
sets `Content-Encoding` and `Vary: Accept-Encoding`. **The daemon never runs a
compressor at request time.** When the client offers no encoding whose variant is
sealed, the daemon serves the canonical uncompressed bytes (this is ordinary
negotiation, not degradation).

Why sealing rather than on-the-fly (this is the crux the issue names): #183
requires the artifact to be **content-addressed and byte-identical across every
target**. Compressing at request time makes the served bytes a function of the
daemon's Brotli/zlib library version rather than of the sealed artifact — it
cannot be covered by the identical-bytes assertion, and it puts a compressor (CPU
and a decompression-bomb-adjacent surface) in the hot serving path. Sealing keeps
the serving path trivial, keeps compression reproducible, and lets the manifest
cover every byte that leaves the daemon. The issue's implementation direction
reaches the same conclusion.

Why record it **now**, before #183 builds, even though the realized wire benefit
is loopback-negligible today (§4.6): #183 is *Planned* and about to fix the
manifest format. If compressed variants are not folded into that schema at design
time, adding them later re-opens the sealed-artifact format and re-seals a shipped
artifact. The decision is cheap to make now and expensive to retrofit.

### 4.2 Compressible set and canonical identity

- **Compress:** `html`/`htm`, `js`/`mjs`, `css`, `wasm`, `json`/`map`,
  `webmanifest`, `svg`, and `txt` — the text-like and wasm assets `guess_mime`
  already enumerates (`serve.rs` line 1906). These are first-party, content-addressed,
  and contain no secret and no attacker-controlled input, so compressing them is
  safe (§4.4).
- **Do not bother compressing** already-compressed binary assets (`png`, `jpg`,
  `webp`, `woff2`, `gif`, `ico`): the manifest may omit their variants, and the
  daemon serves them canonical. Sealing a variant that is larger than the
  original is a build-time waste, not a correctness problem; the build should skip
  a variant whose bytes are not smaller than the canonical bytes, and the daemon
  must treat "no sealed variant" as "serve canonical".
- **Canonical identity is uncompressed.** The digest #183 content-addresses, and
  the digest the exact-version rejection matches, is the SHA-256 of the
  **canonical uncompressed** byte set. Every variant is an additional sealed
  artifact keyed to that identity, never a replacement for it. This satisfies the
  issue's constraint that "if encoding is negotiated instead, the digest must
  continue to cover the canonical uncompressed bytes."

### 4.3 Applies identically to `Embedded` and `Dir`

Both `UiSource::Embedded` and `UiSource::Dir` resolve their bytes through the
**same manifest-driven negotiation + verification code**. The only difference is
where the manifest and the variant bytes come from (compiled-in vs. read from the
`--ui-dir`). A `--ui-dir` daemon pointed at a built artifact directory (the one
`dx`/#183 produces, manifest included) behaves **byte-for-byte identically** to
an `embed-ui` daemon. This is the property that keeps a development build from
masking a packaging bug: the negotiation, the `Content-Encoding`, and the
fail-closed hash check are exercised on every source, not just the packaged one.

A `--ui-dir` pointed at a bare directory that carries **no manifest** is a
distinct, dev-only state: it has no sealed variants and no sealed digests, so the
daemon serves canonical uncompressed bytes with no `Content-Encoding`. That is
not a silent degrade of a sealed variant (there is nothing sealed to degrade); it
is the honest "nothing was sealed" answer, and the doc must say so explicitly.
*Review amendment:* the unsealed state is entered **only on a definite
not-found** of the manifest; a manifest that is present but unreadable,
unparsable, or schema-invalid **fails closed on both sources** — a compiled-in
manifest that fails to parse is a packaging defect (binary integrity proves the
shipped bytes are intact, not that they parse).

### 4.4 Security boundary — what may and may not be compressed

- **Compress only the static, first-party UI artifact** served by `serve_static`
  (`serve.rs` line 895). These bytes are content-addressed, contain no daemon
  token (the browser fetches the token separately at `/api/session`), and reflect
  no attacker-supplied input, so no CRIME/BREACH-class oracle exists.
- **Never compress a response whose body is influenced by peer-supplied
  content.** The fetched-file path `local_file` (`serve.rs` line 545)
  deliberately serves inert attachments (`Content-Disposition: attachment`,
  `X-Content-Type-Options: nosniff`, a locked-down CSP, and a `safe_download_mime`
  type). It, `share_upload`, and all `/api/*`/`/ws` responses **must remain
  uncompressed**. The doc must state this as a hard rule and the test matrix
  (§8) must include a regression guard that `local_file` ignores
  `Accept-Encoding`.
- **Compression must not weaken existing header guarantees.** Serving a variant
  changes only `Content-Encoding`, the `Content-Length` (which becomes the
  encoded length — `hyper`'s `Full<Bytes>` body sets it from the served bytes),
  and adds `Vary: Accept-Encoding`. The asset's `Content-Type` stays the
  canonical type of the *decoded* asset. This decision adds no `nosniff`/CSP to
  the static path and removes none; those remain owned by #189/#196.
- **`Vary: Accept-Encoding` is required** on any asset that has sealed variants,
  so an HTTP cache never hands a Brotli body to a client that did not accept it.

### 4.5 Fail-closed integrity

- When a manifest is present, **every representation the daemon serves must
  hash to its sealed digest before serving** — a chosen variant to the sealed
  variant digest, and bytes served as identity (negotiation shortfall, or an
  asset with no sealed variants such as `png`/`woff2`) to `identity.digest`.
  For `Embedded` this holds transitively via binary integrity; for `Dir` the
  read bytes are hashed **before** serving. Canonical-corrupt fails exactly as
  variant-corrupt.
- **A valid manifest is an allow-list.** A request path that, after resolution
  (entry lookup first, then the route-like `index.html` fallback on a miss),
  matches no manifest entry is a **404** — never read from the
  directory or the embed and served unverified. An extra, stale, or hand-added
  file in a `--ui-dir` (or a leftover compiled-in asset) is not servable
  content; only the manifest-less bare directory serves unlisted files.
- A representation whose bytes are **missing or do not match the sealed digest**
  is a fail-closed error (a 5xx, e.g. a `500`/`503` with an `internal`-class
  body), **not** a silent fall-through to the uncompressed bytes and **not** a
  fall-through to a different encoding. This borrows the posture #113 already
  requires of a legacy artifact ("fails closed; it does not fall back, and it
  does not serve what it has"); the trust root is build-pinned for `Embedded`
  and operator-supplied for `Dir`.
- The distinction the doc must draw sharply: *manifest definitively absent* →
  the unsealed dev-only state. *Manifest present but invalid* → fail closed.
  With a valid manifest: *no offered encoding sealed* → serve (verified)
  canonical; *any served representation corrupt/absent* → fail closed. What the
  check proves differs by source: tamper/packaging detection for `Embedded`
  (its manifest is authenticated transitively by binary integrity);
  **corruption and manifest-file skew** for `Dir` — the manifest comes from the
  same operator-supplied directory it vouches for, so dir-local hashing is
  *not* adversarial tamper detection unless the manifest is authenticated
  against an external trust root (a future #183-provenance decision).

### 4.6 Rejected and deferred alternatives (record in the doc)

- **Compress at request time (on-the-fly gzip/Brotli).** *Rejected.* Served bytes
  become a function of the daemon's compression-library version, so they cannot be
  covered by #183's identical-bytes assertion; it adds a compressor to the hot
  path (CPU per cold load, and a decompression-bomb-adjacent surface); and it
  buys nothing sealing does not. If encoding were ever negotiated this way, the
  digest would still have to cover the canonical uncompressed bytes — which is
  exactly what sealing does without the downsides.
- **Serve uncompressed for the first release, defer any compression.**
  *Considered and rejected as the recorded strategy, honestly.* Its one true
  premise is that all delivery today is loopback (#113), where the wire saving is
  not measurable and the #158 cold load is already fast (72 ms FCP, 263 ms
  interactive). But (a) #183 is fixing the manifest **now**, and retrofitting
  variants later re-seals a shipped artifact; (b) the mechanism chosen here is
  static and compressor-free, so it costs the serving path essentially nothing;
  (c) the packaged desktop and Android WebViews still fetch this artifact, and a
  slower device benefits from ~330 KiB less to move and parse. The doc records
  this alternative and the loopback caveat plainly, and records that the
  **realized** wire benefit is deferred to any future non-loopback path (which
  itself requires a new decision record per #113) while the **mechanism** is
  decided now. This keeps the P2 priority honest: it is worth deciding, not worth
  a hot-path compressor.
- **Embed the variants but keep the daemon serving only canonical.** *Rejected.*
  It pays the binary-size cost of the variants (they are compiled in) and claims
  none of the benefit, and it never exercises the negotiation/fail-closed path,
  so it cannot satisfy the issue's "receives encoded bytes" and
  "behave identically" criteria.

## 5. Numbers to record (for #198)

The **only** measured evidence today is the #158 spike
([`spikes/dioxus-web/README.md`](../spikes/dioxus-web/README.md)), against
`jeliyad` 0.6.1, a **v1** slice with **no `wasm-opt`** — these are explicit
**upper bounds**, not budgets, and Brotli was not measured:

| Asset | Raw | `gzip -9` | Reduction |
|---|---|---|---|
| `…_bg.wasm` | 542,111 B (529 KiB) | 211,557 B (207 KiB) | 61% |
| `…web.js` | 83,570 B | 12,352 B | 85% |
| `styles.css` (verbatim, unminified) | 95,524 B | 23,539 B | 75% |
| dist total | 747,884 B (730 KiB) | — | — |

The decision doc must reproduce this table **with the spike caveat attached**,
add a Brotli column marked *to be measured (expected < gzip)*, and state that the
authoritative raw/`br`/`gzip` numbers for #198 must be **re-measured against the
real #183 artifact** (post-`wasm-opt`, protocol-v2 client) once #183 lands. The
doc is the citable home for those numbers; #198 consumes them from there. Record
them per asset (root document, wasm, JS, stylesheet) and as a dist total, raw and
encoded.

## 6. Records this decision amends

The decision doc is new, but three existing pages currently point at this as an
open question and must be updated in the **same change** so the wiki stays
consistent and reachable:

1. **`docs/dioxus-architecture.md`**
   - The #158-findings paragraph (lines 339–343) says "Whether the daemon should
     negotiate encoding, or ship pre-compressed bytes with the sealed manifest,
     is an open question this spike did not answer." Replace the "open question"
     clause with a one-line pointer:
     the question is now decided in
     [`ui-artifact-wire-encoding.md`](ui-artifact-wire-encoding.md) — seal
     Brotli+gzip variants, negotiate statically, canonical digest stays
     uncompressed.
   - **Decision 5** (line 208) and the **Decision 7** artifact row (line 266):
     add a short clause that the sealed manifest also carries the compressed
     variants and their digests, cross-linking the new doc. Keep #183 the owner;
     do not restate the schema here.
   - Bump `dioxus-architecture.md`'s `timestamp` (its meaning changed).
2. **`docs/first-release-distribution.md`** — "The embedded artifact" section
   (line 134): add a bullet that the artifact's sealed manifest includes
   pre-compressed `br`/`gzip` variants served by static negotiation, with the
   canonical digest unchanged, cross-linking the new doc. Bump its `timestamp`.
3. **`docs/index.md`** — add the new page under "Architecture and protocols" (or
   "Current status and evidence"), because every concept must be reachable from
   the root index or CI fails. Index files carry no frontmatter and need no
   timestamp.

Optionally note the decision against the **#183 backlog row** in
`dioxus-architecture.md` (line 417) if the maintainer wants the coupling visible
there; not required for CI.

## 7. Deferred implementation contract (executed under #183 + a serving slice)

This is specified so it is executable, but it is **not** part of #207's
doc deliverable. #207 records the decision; the code and manifest land under #183
(manifest/build) and a `serve.rs` serving + verification slice (which may be
#207's own follow-up or folded into #183/#199). Mark the decision doc
`implementation_status: planned` accordingly. This contract was amended during
PR review (commits `957f84a`, `f7c2b06`) after this plan was first authored;
this plan mirrors the amendments, and where the two ever diverge,
`docs/ui-artifact-wire-encoding.md` governs.

### 7.1 Manifest schema #183 must produce

Extend the #183 artifact manifest so each asset entry carries its identity and
its sealed variants, and the manifest carries shared provenance:

- **Manifest-level provenance** (already #183's remit; the variants join it):
  renderer, source SHA, toolchain versions (`rustc`, `wasm-bindgen`, `dx` /
  `wasm-opt`, **and the exact compressor and version** used for each coding —
  e.g. `brotli` CLI/lib version and quality, `gzip`/zopfli level), and a
  manifest digest — **detached** (computed over the complete serialized
  manifest, carried alongside) or stored in a field excluded from a canonical
  serialization; the exact scheme is #183's. A digest stored inside the bytes
  it covers is an uncomputable fixed point.
- **Per-asset entry:**
  - `path` (request-relative, matching `safe_rel` output; **unique** across
    the manifest — a duplicate asset `path` fails validation, the serving
    lookup could not deterministically select between them),
  - `content_type` (the canonical decoded type),
  - `identity`: `{ digest: sha256(canonical bytes), bytes }` — the
    content-address,
  - `encodings`: `[ { coding: "br" | "gzip", path, digest: sha256(variant
    bytes), bytes, params } ]` — at most **one** entry per coding (a second
    `br`/`gzip` for the same asset fails validation) — omitted or empty when
    no variant is smaller
    than canonical. `path` is the artifact-relative sealed location of the
    variant bytes (same containment rules as the asset `path`; unique across
    the **entire artifact namespace** — no collision with any canonical path,
    any other variant path, or the manifest and its detached-digest sidecar,
    and no sealed path may be an **ancestor**/directory prefix of another: a
    `Dir` source cannot materialize a path as both file and directory, and
    `Embedded`/`Dir` parity forbids sealing what a directory cannot
    represent — so materializing the directory can never overwrite one
    representation with another); the daemon locates variant bytes **solely** by
    it, never by a derived filename. The concrete layout convention stays
    #183's choice — sealed in the manifest, never assumed.
- **Canonical artifact digest** — the single identity the exact-version
  rejection pins, derived **only** from the canonical byte set: entries sorted
  bytewise by `path`, each contributing `path` and `identity.digest` to a
  deterministic serialization (recommended: `path`, a NUL separator, the
  lowercase-hex `identity.digest`, a newline, per entry), SHA-256 over that.
  Excludes `encodings`, `content_type`, and provenance — sealing, dropping, or
  re-compressing a variant, or a compressor bump, can never change canonical
  identity; the whole-manifest digest continues to cover everything. #183 may
  seal a different serialization only by recording it in the manifest; the
  invariants (canonical-only inputs, bytewise `path` order, deterministic) are
  contractual.
- **Determinism requirement:** Brotli/gzip output is **not** guaranteed identical
  across tool versions or platforms. The build must pin the exact compressor and
  settings (sealed in `params`), and #183's cross-target identical-bytes gate must
  cover **every** sealed byte set — canonical *and* each variant — so a `br`
  produced on macOS equals the `br` produced on Linux. Prefer a deterministic
  compressor invocation (e.g. Brotli quality 11, and a reproducible gzip such as
  zopfli or `gzip -n -9`) documented in `params`.

### 7.2 Daemon serving changes in `serve.rs`

- Give `UiSource` access to the parsed manifest for both variants (compiled-in
  for `Embedded`, loaded from the `--ui-dir` for `Dir`).
- Change the load/serve path so `serve_static` (line 895) negotiates: look the
  requested path up against the manifest's entry set **first** (a sealed
  extension-less asset such as a `LICENSE` entry stays reachable, never
  shadowed by the fallback); only when it matches no entry **and** is
  route-like (extension-less) fall back to `index.html` — the baseline's
  lookup-then-fallback order (lines 912-921), so deep links keep reloading;
  then enforce the allow-list on the resulting target (with a valid manifest
  an unlisted non-route-like path is a **404**, never a filesystem or embed
  fallback), parse
  `Accept-Encoding` (coding tokens compare **case-insensitively** — `GZIP` ≡
  `gzip`, RFC 9110 §8.4.1; `q=0` excludes a coding; unknown codings are ignored; `*`
  matches every coding not explicitly listed — bare `*` accepts the sealed
  codings, `*;q=0` excludes them, RFC 9110 §12.5.3; nonzero weights do not
  reorder), pick Brotli > gzip > identity among **sealed** codings the client
  accepts (an empty acceptable set serves canonical, never 406), hash the
  chosen representation — variant or canonical — against its sealed digest
  (§4.5), and build the response with `Content-Type`
  (decoded type), `Content-Encoding` (when not identity), the encoded
  `Content-Length` (auto from `Full<Bytes>`), and `Vary: Accept-Encoding`.
- Keep the SPA fallback (`index.html` for extension-less routes, line 916) on the
  same negotiated path.
- **Leave `local_file`, `share_upload`, `session`, `health`, `preflight`,
  `gate_refusal`, and all JSON responses untouched** — they must never gain
  `Content-Encoding`.

### 7.3 What #207's own deliverable is, concretely

1. `docs/ui-artifact-wire-encoding.md` authored per §4/§5 and the frontmatter in
   §9.
2. The three amendment edits in §6 (dioxus-architecture, first-release-distribution,
   index) with timestamps bumped.
3. `node scripts/check-docs.mjs` green.

## 8. Verification (specified for the later serving slice; the doc records the matrix)

The decision doc states this matrix; the serving slice implements it as tests.
For **each** source — an `embed-ui` daemon, and a `--ui-dir` daemon pointed at
the built artifact directory — request the **root document, the wasm, the JS,
and the stylesheet**. Every row against a variant-bearing asset also asserts
`Vary: Accept-Encoding` is present (not only the Brotli row). Every row —
identity and encoded alike — also asserts `Content-Type` == the manifest's
sealed `content_type` (the canonical decoded type, never
`application/octet-stream` for the wasm asset and never a type derived from a
variant `path`). Because response and manifest could agree on a wrong value,
the matrix additionally validates every entry's sealed `content_type`
**independently** against the canonical extension-to-MIME mapping
(`guess_mime`'s characterized table) — a mislabeled manifest must fail even
when the daemon serves its value faithfully. The rows:

- `Accept-Encoding: br, gzip` → assert `Content-Encoding: br`,
  `Content-Length` == sealed `br` bytes, `Vary: Accept-Encoding` present, and
  `brotli_decode(body)` SHA-256 == the sealed `identity.digest`.
- `Accept-Encoding: gzip` → assert `Content-Encoding: gzip`, `Content-Length` ==
  sealed `gzip` bytes, and `gunzip(body)` SHA-256 == `identity.digest`.
- `Accept-Encoding: GZIP` → assert `Content-Encoding: gzip` — content-coding
  tokens are case-insensitive (RFC 9110 §8.4.1); a literal token comparison
  must fail this row.
- No `Accept-Encoding` → assert **no** `Content-Encoding`, body == canonical
  bytes, SHA-256 == `identity.digest`.
- `Accept-Encoding: br;q=0, gzip` → assert gzip chosen, not br.
- Stance rows (review amendments, mirrored from the decision doc):
  `gzip;q=1, br;q=0.5` → br (weights do not reorder); `identity;q=0` → `200`
  canonical (the RFC 9110 §12.4.1 disregard stance); bare `*` → br (§12.5.3
  wildcard); `identity;q=1, *;q=0` → canonical, no `Content-Encoding`.
- **Every entry, every coding:** iterate the manifest's **own entry list** —
  every asset, every sealed coding **plus identity**, on both sources —
  request offering exactly that coding, assert `Content-Encoding` == the
  requested coding (absent for identity), proving the sealed variant rather
  than canonical identity answered, and assert served bytes against sealed
  digests branched by representation: an **encoded** body's SHA-256 == its
  encoding entry's digest (byte-exact: a different valid stream with the
  same decoded content must fail — this row checks what the serving path
  returned, not the stored artifact) and its decoded body == `identity.digest`;
  the **identity** body — no encoding entry exists for it — ==
  `identity.digest` directly.
- **Fail-closed:** with a variant deliberately corrupted or removed from the
  source, a request offering that sealed encoding returns a **5xx**, never a
  `200` with uncompressed bytes and never a different encoding.
- **Fail-closed, canonical:** a corrupted canonical file in the `Dir` source →
  **5xx** on an identity-selecting request (and for a no-variant asset such as
  a `png`), never `200` with bytes not matching `identity.digest`.
- **Fail-closed, manifest:** a present-but-truncated/unparsable manifest →
  fail closed, never a collapse into the "no manifest" dev state.
- **Allow-list:** an extra file (for example `debug.html`) in the `Dir` source
  but absent from the manifest → **404**, never a `200` with unverified bytes;
  the manifest-less bare directory serving that same file stays the dev-only
  contrast. In the same run, a deep link such as `/rooms/r-99` still resolves
  through the SPA fallback to the sealed `index.html`, served negotiated as
  usual — the allow-list applies to the resolved asset path, never the raw
  route. A sealed extension-less asset (for example a `LICENSE` entry) is
  served, not shadowed: entry lookup precedes the route-like fallback.
- **Security regression:** `GET /api/files/local?...` with `Accept-Encoding: br,
  gzip` returns **no** `Content-Encoding` and the inert-attachment headers are
  unchanged.
- **Identical sources:** the negotiated encoding, `Content-Type`,
  `Content-Length`, and decoded digest for each asset match between the
  `Embedded` and `Dir` daemons.
- **Cross-target identical bytes:** #183's gate diffs every sealed variant across
  daemon targets (this decision only adds the variants to that set).
- **Evidence:** record raw and encoded sizes per asset and as a dist total into
  the decision doc's table and the release evidence #198 reads.

## 9. Acceptance criteria

Mapping the issue's ACs to this plan:

- [ ] **Strategy decided and recorded in the architecture record** — satisfied by
  `docs/ui-artifact-wire-encoding.md` plus the §6 pointer edits into
  `dioxus-architecture.md`/`first-release-distribution.md` (§4.1).
- [ ] **A client offering `gzip`/`br` receives encoded bytes (or uncompressed is
  stated with rationale)** — the decision *is* to serve encoded bytes via static
  negotiation; rationale and the honest loopback caveat recorded (§4.1, §4.6).
- [ ] **Decoded bytes hash to the digest the manifest seals** — canonical digest
  stays uncompressed; verification asserts decoded == `identity.digest`
  (§4.2, §4.5, §8).
- [ ] **Embedded and `--ui-dir` sources behave identically** — one manifest-driven
  path for both (§4.3, §8).
- [ ] **A mismatched or missing pre-compressed variant fails closed** — 5xx, never
  a silent uncompressed fallback (§4.5, §8).
- [ ] **Raw and encoded sizes recorded for #198** — the §5 table in the doc,
  spike caveat attached, re-measured against the #183 artifact (§5, §8).

Documentation-contract criteria (`scripts/check-docs.mjs`):

- [ ] Exactly the ten required frontmatter fields, restricted YAML subset, one H1
  matching `title`, file-relative links only, reachable from `docs/index.md`.
- [ ] Amended pages' `timestamp` fields bumped; no new document type introduced.

## 10. Risks and mitigations

- **Cross-platform variant non-determinism.** Brotli/gzip bytes can differ by
  tool version → the identical-bytes gate would false-fail. *Mitigation:* pin the
  compressor and settings, seal them in `params`, and make #183's gate cover the
  variants (§7.1). This risk is why on-the-fly compression is rejected — it makes
  the problem unfixable.
- **Binary-size inflation for `Embedded`.** The variants are compiled in, so the
  daemon binary grows by the compressed sizes; on loopback the wire benefit is
  small. *Mitigation:* record this trade-off honestly in §4.6; skip variants that
  are not smaller than canonical; keep the P2 scope (static, no hot-path
  compressor).
- **Scope creep into the file/download or WS paths.** *Mitigation:* the hard rule
  in §4.4 and the `local_file` regression guard in §8.
- **Deciding ahead of #183.** The manifest does not exist yet, so the hash/
  fail-closed ACs cannot be *executed* by #207 alone. *Mitigation:* record the
  decision with `implementation_status: planned`; the serving slice lands with
  or after #183. This is the intended use of the status axes, not a gap.
- **Reviewer reads "uncompressed is acceptable" and pushes to defer entirely.**
  *Mitigation:* §4.6 records that alternative and refutes it on the "retrofitting
  re-seals a shipped artifact" and "mechanism is compressor-free" grounds; the
  loopback caveat is stated rather than hidden.

## 11. Assumptions

- The `docs/` documentation contract in `PROFILE.md` is current and
  `scripts/check-docs.mjs` is the gate (confirmed by reading both).
- #183 is still `Planned` (per `dioxus-architecture.md` backlog line 417), so its
  manifest format is open to this extension rather than already sealed.
- All first-release delivery of this artifact is loopback (#113 forbids a hosted
  origin/CDN), and any future networked delivery requires its own decision
  record — so this decision fixes the *encoding mechanism*, not a new transport.
- The #158 spike numbers are the only measured evidence and are v1/no-`wasm-opt`
  upper bounds; final #198 numbers come from the #183 artifact.
- Brotli is available to the build toolchain as a pinnable, deterministic
  compressor. The decision as shipped seals **both** `br` and `gzip`
  unconditionally at the coding level (the per-asset omission of a variant
  that is not smaller than canonical, §4.4, is orthogonal to this) and
  records **no** gzip-only fallback; if a deterministic
  Brotli genuinely cannot be pinned, that is a blocker to resolve — or a formal
  amendment to the canonical decision record — before #183 ships, not a silent
  gzip-only build.

## 12. Open questions (resolve with maintainers while authoring)

- **Standalone doc vs. amend Decision 5 in place.** This spec recommends a
  standalone `Decision` doc so #183/#198 have a single citable target and
  `dioxus-architecture.md` stays stable (mirroring how `native-update-policy.md`
  satisfies the #121 pointer). Confirm the maintainer prefers this over folding
  the decision into Decision 5 directly.
- **Brotli vs. gzip-only for v1.** *Resolved by the decision as authored:* seal
  both, prefer Brotli, no fallback recorded. Shipping gzip-only would first
  require amending the canonical decision record; the determinism risk is
  handled by pinning the compressor (§7.1, §10), not by dropping a variant.
- **Where the serving code lands.** Whether the `serve.rs` negotiation + tests
  are a #207 follow-up slice, folded into #183, or gated at #199. This spec
  specifies the contract regardless of which issue executes it.
- **Compressor determinism target.** Confirm the exact pinned invocations
  (Brotli quality, gzip/zopfli) to seal in `params` so the cross-target gate is
  reproducible.
