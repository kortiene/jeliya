# Spec: protocol-v2 byte-stream framing for `file.share` / `file.read` (#233)

- **Issue:** #233 — [Protocol][Rust] Specify and implement protocol-v2 byte-stream framing
- **Labels:** enhancement, rust, priority:p0, cross-client, dioxus, protocol, clean-slate
- **Type:** planning / implementation specification (no production code changes in this document)
- **Normative source of truth:** `docs/protocol-v2.md` § *Byte-stream framing* (currently lines ~404–797) plus the `file.share` / `file.read` / `transfer.cancel` operation schemas (~1631–1760), the served *limits object* (~108–139), the WebSocket close-code table (~190–207), and the `stream_abort_reason` / `byte_total` / `outcome` shared value types (~968–969).
- **Dependencies:** protocol authority #161; prior JSON codec slice #164; typed host cutover #165/#166; first-release size decision #92; later resumability #209. Upstream carries U1/U2/U3 (`iroh-rooms` ConnEvent generation, streaming/progress fetch, size-distinguishable fetch outcome).

---

## 1. Summary

Issue #233 has two slices: a **normative decision** (docs-only) and a **codec/runtime/harness implementation**. The normative decision is already merged and marked `canonical` in `docs/protocol-v2.md`; the deferral to #164 that this issue calls out is gone and the framing is fixed in-record. A substantial implementation of the codec, daemon runtime, core staging, conformance harness executor, and file fixtures is also present in this tree.

This spec therefore serves three purposes:

1. **Specify** the byte-stream framing implementation in enough detail that an engineer could build it from scratch or independently verify the present tree against it.
2. **Map** every issue Scope item and Acceptance Criterion onto concrete artifacts (`file:function`, tests, fixtures, scripts) so status is auditable rather than asserted.
3. **Define the residual work and verification plan** — the parts that are genuinely not yet complete: the download-path executable corpus fixture, the harness fault-injection controls (raw-record / credit-pause / client-ABORT / transport-drop), the full-corpus A/B measurement, and the live smoke/sidecar/agent-E2E gate.

> **Honesty note.** The #233 issue body describes a pre-implementation world ("`jeliya-codec` … has no byte-stream record type", "`file.share` returns `not_ready`"). That description is stale relative to this tree. Do **not** re-implement what exists; the plan below is written so each step is a *build-or-verify* step. Per repo convention, **recompute every count and status against the tree before acting** — the numbers in this document (line ranges, occurrence counts, corpus size) are snapshots and drift.

---

## 2. Current state (verified against this tree)

| Layer | Artifact | State |
|---|---|---|
| Normative decision | `docs/protocol-v2.md` § *Byte-stream framing* | **Present & canonical.** Distinguishes RFC frames / Text·Binary messages / JSON frames / byte-stream records; fixes the 48-byte `JBS2` header, the closed kind byte (`0x01`–`0x06`), ABORT reason vocabulary, admission/OPEN, cumulative CREDIT + one-byte probe, END, ABORT/ACK, zero-byte streams, deadlines/stall, size precedence, malformed-record handling, disconnect/replay, and the harness `stream` execution contract. |
| Codec | `crates/jeliya-codec/src/byte_stream.rs` | **Present.** `StreamIdentity`, `StreamRecord`/`StreamRecordView`, `StreamRecordBody(View)`, `StreamRecordKind`, `BinaryAbortReason`, staged decoders `decode_stream_identity` → `decode_stream_kind` → `decode_stream_record_view`/`decode_stream_record`, `encode_stream_record`, `max_stream_data_bytes`, full `StreamCodecError` taxonomy. Tests: `crates/jeliya-codec/tests/{byte_stream,fuzz,golden}.rs`. |
| Core staging | `crates/jeliya-core/src/protocol_upload.rs` | **Present.** `PreparedFileShare` → `OpenFileShareSink` → `FileShareFinalizer`: one unpredictable exclusive stage object under `protocol-v2-stream-staging`, forward-only contiguous writes, blake3 digest, unlink-on-drop before END, no path/seek/random-access surface. |
| Daemon upload | `crates/jeliyad/src/file_share.rs` | **Present.** `UploadIngress` state machine (Opening → Active → DaemonAbort*/ClientAbort*/Finalizing → AckPending → Retired), byte- and message-bounded `UploadIngressBudget`, CREDIT/probe/sentinel accounting, ordered terminal admission, `UploadCancellationRegistry`, deadline/stall interrupts. |
| Daemon download | `crates/jeliyad/src/file_read.rs` | **Present.** Download producer + shared `StreamRegistry`, `UploadStreamBinding`, `ConnectionCloser`, `RequestPermit`, `BinaryRoute`. |
| Daemon transfer pool | `crates/jeliyad/src/transfer.rs` | **Present.** `RuntimeLimits`, `TransferPool` (count + inflight-byte reservation), `StreamIdGenerator` (unpredictable nonzero, connection-unique). |
| Daemon wiring | `crates/jeliyad/src/serve.rs` | **Present.** `file.share` (~1739) and `file.read` (~1782) dispatch, `StreamRegistry` install, Binary routing, integration tests (~2059+). |
| Harness | `conformance/v2/harness/stream.mjs` + `session.mjs` | **Present.** JBS2 encode/decode, `runUpload`/`runDownload`, real receiver-accepted `bytes_streamed`, stall gauge, inflight-budget check, ABORT/ACK correlation, pre-OPEN fabrication guards. Session layer preserves the WebSocket Text/Binary bit instead of `JSON.parse`-ing every message. |
| Fixtures | `conformance/v2/files.json`, `conformance/v2/manifest.json` | **Present.** `stream: {send_bytes|receive_bytes}` and `observe: bytes_streamed` keys transcribed; corpus DSL for `stream`/`observe` documented in `conformance/v2/README.md`. |
| CI scripts | `scripts/{check-docs,check-v2-corpus,smoke,sidecar-check,agent-e2e}.mjs` | **Present.** |

### 2.1 Known residual gaps (from `conformance/v2/README.md`, executor status row)

These are the parts that are **not** complete and are the true forward work of #233:

- **R-A — Download has no *executable corpus* fixture.** `file.read` requires a locally *fetched* file (`resource:fetched_file`) and `link:*` provider preconditions. The corpus replay harness is **single-subject** (a `member:b` runs on the primary daemon), so a genuinely fetched-from-a-peer file cannot be staged by the corpus alone. The download producer is exercised by Rust integration tests in `serve.rs`, but the corpus download cases remain declarative. See [[jeliya-ci-local-dev-gotchas]] (single-subject harness) in maintainer memory.
- **R-B — Harness fault controls unimplemented.** Raw-record injection, credit-pause/release, client-originated ABORT/ACK, and transport-drop are not yet wired into the harness driver, so malformed / backpressure / crossed-terminal / cancellation / disconnect **stream** cases are declarative in the corpus rather than executed.
- **R-C — Full-corpus A/B not measured here.** CI live-gates only a slice of the corpus. A full run (all cases) with the byte-stream executor must be run and its pass/fail/error/blocked counts recorded as evidence, not assumed.
- **R-D — Upstream U1/U2/U3 still block their conformance cases** (declared `blocked_on_upstream`; they fail, they do not skip). Not reopened by #233.

---

## 3. Normative decision (Scope §"Normative decision") — content contract

The docs-only PR must fix **all** of the following in `docs/protocol-v2.md`, remove the stale #164 deferral, and pass `node scripts/check-docs.mjs`. This section is the checklist the decision text must satisfy (it is satisfied in the current tree; treat it as the audit list for AC-1).

1. **Message taxonomy.** RFC 6455 *frames* (transport fragments, no application meaning) vs complete *Text/Binary messages* vs *JSON frames* vs *byte-stream records*. Fragmentation must not create records, terminate a stream, or reset the stall timer. Every hello/request/reply/push is exactly one UTF-8 Text message; every byte-stream record is exactly one Binary message; JSON-in-Binary and record-in-Text are malformed. No permessage-deflate.
2. **Binary record + header.** One record per Binary message; a record never spans messages. Fixed 48-byte big-endian header: magic `JBS2`, kind byte, 3 reserved zero bytes, request `id` (8), `stream_id` (16), `offset` (8), `value` (8), payload only on DATA. `(request id, stream_id)` is the stream identity; neither binds alone; `stream_id` is unpredictable, nonzero, connection-unique, never reused.
3. **Record kinds (closed).** `0x01 OPEN` (daemon; value=total), `0x02 DATA` (producer; payload `1..=min(65 536, max_frame_bytes-48)`), `0x03 CREDIT` (receiver; offset=`accepted_through`, value=`send_through`), `0x04 END` (producer; offset=total), `0x05 ABORT` (either; offset=high-water, value=reason), `0x06 ACK` (ABORT recipient; value=`0x05`). Non-DATA records are exactly 48 bytes; empty DATA is malformed; offset arithmetic is checked and non-wrapping.
4. **ABORT reasons (closed).** `0x01 cancelled`, `0x02 source_failed`, `0x03 sink_failed`, `0x04 protocol_error`, `0x05 operation_error` (daemon-only; authoritative typed error in the terminal reply). `transport_lost` has no wire value (synthesized locally).
5. **Binding & lifecycle.** OPEN is an admission record, not a second reply; sent only after validation order passes, source/sink open, and both transfer limits reserved. Exactly one OPEN and one terminal reply per fresh execution. The two legal success sequences for upload and download (request → OPEN → CREDIT → DATA*/CREDIT* → END → terminal reply, with the correct directions). JSON/other-stream interleaving allowed between any two messages; ordering exists only within one stream; a scheduler must service JSON/CREDIT/ABORT between DATA.
6. **Credit & bounded backpressure.** CREDIT is cumulative, monotonic, `accepted_through <= send_through`; idempotent repeat; regression/over-ack/producer-credit is malformed. Inbound storage, source read-ahead, and outbound queues are **byte-bounded**; message-count-only queues are insufficient. Mandatory one-byte probe to `min(declared_bytes, max_shared_file_bytes)+1` for upload; upload `send_through <= max_shared_file_bytes+1`; download credit `<=` OPEN total.
7. **Deadlines.** Absolute budget `transfer_connect_allowance_ms + ceil(total*8*1000 / transfer_floor_bits_per_second)`; stall on `transfer_stall_ms` of no accepted-progress; active transfer counts as connection activity (no idle-timeout race). ACTIVE tie order: explicit cancel > deadline > stall; FINALIZING governed only by its sequenced result.
8. **END / ABORT / cancellation.** Receiving declared bytes is not EOF; END only after all sent DATA is CREDIT-acknowledged; zero-byte stream = OPEN·CREDIT·END at offset 0. FINALIZING is uncancellable. The race table (client END wins, daemon cancel wins, ABORT×ABORT, client ABORT wins, `transfer.cancel` wins, cancel/ABORT in FINALIZING). `transferred_bytes` counts only provably receiver-accepted bytes.
9. **Disconnect & retry.** No stream survives its connection. Pre-END disconnect → abort, remove staging residue, release reservation; with `op_id` records `stream_aborted{transport_lost}` (replay returns recorded failure, retry needs a new `op_id` from offset zero). Post-END finalization may complete; lost-final-reply replays through the `op_id` ledger with no second effect. No resume; restart from zero. Daemon restart resumes no stream; startup cleanup removes abandoned staging.
10. **Size & record precedence.** `declared_bytes > max_shared_file_bytes` → `file_too_large@stage_declared` before OPEN. Per-DATA order: `candidate > max_shared_file_bytes` → `file_too_large@stage_stream`; else `candidate > declared_bytes` → `declared_size_mismatch{observed_bytes}`; else enforce CREDIT + sink capacity. `frame_too_large` (over `max_frame_bytes`) → close `4005` unparsed, aborting still-active pre-END streams as `transport_lost`; a DATA payload over the DATA bound but within `max_frame_bytes` → correlated `malformed_frame`, not `4005`. `4007` closes only when a record cannot be safely bound.
11. **Connection-local vs stream-local refusal.** Client→daemon record with a bound active stream but a request-local fault → abort only that stream + correlated `malformed_frame`. Malformed daemon OPEN/END/ABORT is connection-fatal `4007`. `1006` is never sent.
12. **Harness execution of `stream`.** The `stream` fixture key is executed incrementally (byte at offset `i` = `i mod 251`), routed by `(id, stream_id)`, with `observe: bytes_streamed` meaning receiver-accepted payload bytes; a pre-OPEN terminal records zero. Fixture corrections are transcribed from the record, never inferred from the implementation.

---

## 4. Implementation plan

Each phase lists **goal → design → concrete artifacts → build-or-verify steps → done-when**. Phases B–F are independent enough to review as separate slices; the issue requires the docs PR (Phase A) to land first.

### Phase A — Normative decision (docs-only)

- **Goal:** Land §3's content contract; remove the #164 deferral.
- **Artifacts:** `docs/protocol-v2.md` only. No codec code in this PR (Constraint).
- **Steps:**
  1. Verify every §3 item is stated normatively and unambiguously; ensure the ten framing/lifecycle families each have a definite rule (no "TBD", no "see #164").
  2. Keep the docs profile contract intact: exactly the required frontmatter fields, `status: canonical`, no "superseded" status, index reachability. See [[jeliya-docs-profile-contract]].
  3. Run `node scripts/check-docs.mjs` (and `scripts/check-docs.test.mjs`).
- **Done-when:** AC-1 green; adversarial review (§Phase G / AC-2) has a recorded pass over ordering, termination, cancellation, disconnect, size precedence, malformed records, and resource bounds.

### Phase B — Codec typed records (`jeliya-codec`)

- **Goal:** Represent every decided JSON and binary record with **no domain logic and no unbounded parsing** (AC-3).
- **Design:** A pure structural codec. It validates only what is decidable without transport state: size ≤ `max_frame_bytes`, header length/magic, reserved zero, closed kind, browser-safe request id, nonzero stream id, per-kind fixed-field zeroing, DATA payload bound `min(65 536, max_frame_bytes-48)`, non-empty DATA, checked offset+len. Active-pair lookup, direction, state, and cumulative-credit semantics stay in the runtime.
- **Artifacts:** `crates/jeliya-codec/src/byte_stream.rs` (present) — keep the three-stage decode (`decode_stream_identity` → `decode_stream_kind` → `decode_stream_record_view`) so a runtime can bind before any payload is copied; keep `StreamRecordView<'a>` borrowing DATA (zero input-dependent allocation) and `decode_stream_record` allocating at most one 64 KiB payload. Export via `crates/jeliya-codec/src/lib.rs`.
- **Build-or-verify:**
  1. Confirm the kind and ABORT-reason enums are closed and reject unknown bytes (`UnknownKind`, `InvalidAbortReason`).
  2. Confirm `max_stream_data_bytes` performs the checked subtraction and rejects `max_frame_bytes <= 48` (`FrameLimitTooSmall`).
  3. Confirm ACK encodes/decodes `value == 0x05` and OPEN/END/CREDIT/ABORT reject stray payloads (`UnexpectedPayload`).
  4. Confirm encode round-trips every body and enforces the frame and DATA bounds symmetrically.
- **Tests:** `crates/jeliya-codec/tests/{byte_stream,golden,fuzz}.rs` — golden byte vectors for each kind; a fuzz target proving no panic / no unbounded allocation on arbitrary input.
- **Done-when:** AC-3 satisfied; `cargo test -p jeliya-codec` green.

### Phase C — Core bounded staging + source handles (`jeliya-core`)

- **Goal:** A typed, path-free staging sink (upload) and source (download) that the runtime drives, keeping domain logic (import, event authorship, digest, size limits) in core, not in the codec.
- **Design:** Upload: `PreparedFileShare::open_sink` only after admission; `OpenFileShareSink` accepts only contiguous complete records, hashes with blake3, and **synchronously unlinks on drop before exact END**; exact END yields a single-use `FileShareFinalizer` that imports staged bytes and authors exactly one `file_shared` event, or drops (authoring nothing, removing the stage). Download: a bounded source handle the producer reads from into DATA records without loading the whole file.
- **Artifacts:** `crates/jeliya-core/src/protocol_upload.rs` (present); source-side handles in `crates/jeliya-core/src/{engine,typed}.rs`; `FILE_UPLOAD_MAX_BYTES` = `max_shared_file_bytes`.
- **Build-or-verify:**
  1. Staging directory is protocol-only (`protocol-v2-stream-staging`); cleanup never traverses HTTP `uploads` or durable `blobs`.
  2. Drop-before-END removes the stage (no visible partial file) — assert with a test hook (`before_import`, `fail_cleanup`).
  3. Finalize is single-use and authors exactly one event; a lost-reply replay returns the committed result via the `op_id` ledger with no second import.
- **Done-when:** AC-4 (no visible partial file on any failure path) and AC-6 (replay-after-lost-reply) hold at the core boundary.

### Phase D — Daemon runtime wiring (`jeliyad`)

- **Goal:** Wire `file.share` and `file.read` through the typed core with byte-bounded queues and observable abort/disconnect cleanup (Scope §"Separate implementation slice"; AC-4/5/6).
- **Design:**
  - **Upload consumer** (`file_share.rs`): `UploadIngress` phase machine; separate **byte** and **message** bounded ingress budgets; the mandatory one-byte probe and the `max_shared_file_bytes+1` sentinel; ordered terminal admission so a deadline/disconnect/sink-accept cannot overtake an already-received terminal; per-DATA precedence (`file_too_large@stage_stream` > `declared_size_mismatch` > credit/sink); `UploadCancellationRegistry` keyed by `(principal, op_id)` for `transfer.cancel`.
  - **Download producer** (`file_read.rs`): the shared `StreamRegistry`, `UploadStreamBinding`, `ConnectionCloser`, `RequestPermit`; bounded read-ahead; producer END only after final CREDIT; in-band ABORT (no `transfer.cancel` target — `file.read` is connection-scoped).
  - **Transfer pool** (`transfer.rs`): reserve `max_concurrent_transfers` and `max_transfer_bytes_inflight` before OPEN; `StreamIdGenerator` yields unpredictable, nonzero, connection-unique ids; absolute-deadline + stall clocks.
  - **Dispatch** (`serve.rs`): `file.share`/`file.read` cases, `StreamRegistry` install per connection, Binary-message routing to `route_bound`, connection-invalidation on disconnect.
- **Build-or-verify:**
  1. `resource_exhausted` is decided pre-OPEN from the declared (upload) / verified-local (download) total; no OPEN, no CREDIT, no byte on refusal.
  2. Disconnect before valid upload END aborts, removes staging residue, releases the reservation, authors no event; with `op_id`, records `stream_aborted{transport_lost}`.
  3. `frame_too_large` closes `4005` unparsed and aborts active pre-END streams as `transport_lost`; a request-local malformed record aborts only its stream with `malformed_frame`.
  4. Reservations released at the local terminal decision, not held awaiting ACK; FINALIZING is uncancellable.
- **Tests:** `serve.rs` integration tests (at-limit, zero-byte, over-limit, short, long, malformed, aborted, stalled, disconnected; released-capacity readmission).
- **Done-when:** AC-4, AC-5, AC-6 pass at the daemon boundary; abort/disconnect cleanup is asserted, not narrated.

### Phase E — Harness executor + session Text/Binary preservation

- **Goal:** Execute both stream directions incrementally, with byte counters that cannot pass without observing bytes (AC-7).
- **Design:** `session.mjs` preserves the WebSocket Text/Binary bit (no blanket `JSON.parse`). `stream.mjs` drives `runUpload`/`runDownload`: deterministic `i mod 251` generator; per-call trackers with separate generated / socket-sent / received / **receiver-accepted** counters; `observe: bytes_streamed` = receiver-accepted only; pre-OPEN terminal records zero and forbids stream-only error codes; CREDIT monotonicity, record-boundary, OPEN-total, and sentinel bounds all asserted; ABORT/ACK correlation to the typed terminal reply; stall gauge; inflight-budget check.
- **Artifacts:** `conformance/v2/harness/{stream,session,runner,values,assert}.mjs`.
- **Build-or-verify (present for upload; residual for the rest):**
  1. Upload (`send_bytes`) executes end-to-end; short/long/over-limit expressible because `send_bytes` is independent of `in.declared_bytes`.
  2. **R-A:** provide an executable path for download (`receive_bytes`) fixtures — either a harness affordance to stage a `fetched_file` on the single subject, or an explicit multi-subject harness capability. Until then, download corpus cases stay declarative and the download producer is covered only by Rust integration tests.
  3. **R-B:** implement the fault controls the executor status row names — raw-record send, credit pause/release, client ABORT/ACK, transport-drop — so malformed / backpressure / crossed-terminal / cancellation / disconnect stream cases become executed rather than declarative.
- **Done-when:** AC-7 holds for both directions in the corpus (not only in Rust tests); `bytes_streamed` provably reflects real receiver-accepted bytes.

### Phase F — Fixture transcription (`conformance/v2`)

- **Goal:** Correct `files.json` **from the merged record**, never from implementation behavior (Non-goal: deriving fixtures from the implementation).
- **Artifacts:** `conformance/v2/files.json`, `conformance/v2/manifest.json`; DSL in `conformance/v2/README.md` (`stream`, `observe: bytes_streamed`).
- **Steps:**
  1. For every `file.share`/`file.read` case, transcribe `stream: {send_bytes|receive_bytes}` and expected terminal reply straight from §3 and the operation schemas.
  2. Keep `stream` and `defer` mutually exclusive on a step; `stream` takes exactly one operation-fixed key.
  3. Mark cases needing U1/U2/U3 `blocked_on_upstream` (they fail, not skip).
  4. Run `node scripts/check-v2-corpus.mjs` for structural/shape validation.
- **Done-when:** AC-8 — corrected fixtures transcribed from the record and passing corpus validation.

### Phase G — Adversarial review + full verification

- **Goal:** AC-2 (adversarial review before implementation) and AC-8's final bullet (Rust build/lint/tests + live smoke, sidecar, agent E2E).
- **Steps:**
  1. **Adversarial review** of the decision text for ambiguity in ordering, termination, cancellation, disconnect, size precedence, malformed records, and resource bounds; record findings and their resolution. Budget a fix round and refute before acting. See [[jeliya-review-rounds-are-load-bearing]].
  2. **Full-corpus A/B** (R-C): run the entire corpus with the byte-stream executor and record pass / fail / error / blocked counts as evidence. CI live-gates only a slice, so a maintainer must run the full set locally.
  3. **Rust gate:** `cargo build`, `cargo clippy` (lint), `cargo test` across `jeliya-codec`, `jeliya-core`, `jeliyad`.
  4. **Live gate:** `node scripts/smoke.mjs`, `node scripts/sidecar-check.mjs`, `node scripts/agent-e2e.mjs`. (Per maintainer memory, these v1-era checks may be relics for pure daemon-protocol changes; if so, state that explicitly and cite the v2-corpus A/B as the real proof — do not silently skip. See [[jeliya-ci-local-dev-gotchas]].)
- **Done-when:** all AC met with recorded evidence.

---

## 5. Reference data structures (for a from-scratch builder)

### 5.1 `JBS2` header (48 bytes, big-endian unsigned)

| Offset | Width | Field |
|---:|---:|---|
| 0 | 4 | magic `JBS2` = `4a 42 53 32` |
| 4 | 1 | record kind (`0x01`–`0x06`) |
| 5 | 3 | reserved, MUST be zero |
| 8 | 8 | request envelope `id` (`0..=2^53-1`) |
| 16 | 16 | `stream_id` (nonzero, connection-unique) |
| 32 | 8 | `offset` (per-kind meaning) |
| 40 | 8 | `value` (per-kind meaning) |
| 48 | rem | payload (DATA only, `1..=min(65 536, max_frame_bytes-48)`) |

### 5.2 Kinds and field meanings

| Kind | Name | Sender | offset | value | payload |
|---:|---|---|---|---|---|
| 0x01 | OPEN | daemon | 0 | expected total | none |
| 0x02 | DATA | producer | first byte offset | 0 | 1..=bound |
| 0x03 | CREDIT | receiver | accepted_through | send_through | none |
| 0x04 | END | producer | total sent | 0 | none |
| 0x05 | ABORT | either | accepted high-water | reason 0x01–0x05 | none |
| 0x06 | ACK | ABORT recipient | final accepted | 0x05 | none |

### 5.3 Success sequences

```
file.share (upload):   Text req  → <OPEN <CREDIT  >DATA* <CREDIT*  >END  <Text reply
file.read  (download): Text req  → <OPEN >CREDIT  <DATA* >CREDIT*  <END  <Text reply
```
`>` client→daemon, `<` daemon→client. Other JSON/streams/controls may interleave between any two messages; per-stream order is enforced by offsets.

### 5.4 Per-DATA upload precedence (checked arithmetic, before copying payload)

1. `candidate = offset + len`; overflow → `malformed_frame` (protocol).
2. `candidate > max_shared_file_bytes` → `file_too_large @ stage_stream`.
3. else `candidate > declared_bytes` → `declared_size_mismatch { observed_bytes: candidate }`.
4. else enforce contiguity (`offset == received_through`), CREDIT (`candidate <= send_through`), and bounded sink capacity; accept the whole record atomically.

END below declaration → `declared_size_mismatch` with the END offset; exact equality (including 0 and `max_shared_file_bytes`) succeeds.

---

## 6. Acceptance criteria (issue AC → verification → status)

| # | Issue AC | Verification | Status in tree |
|---|---|---|---|
| AC-1 | Docs normatively fix every framing/lifecycle item; remove #164 deferral; pass `check-docs.mjs` | §3 audit list + `node scripts/check-docs.mjs` | **Met** (record is `canonical`, deferral gone) |
| AC-2 | Adversarial review for ambiguity before implementation | Recorded review over the 7 named axes (Phase G.1) | **Verify** — ensure the review is evidenced |
| AC-3 | Codec represents every JSON+binary record, no domain logic / no unbounded parsing | `cargo test -p jeliya-codec`; fuzz target | **Met** (`byte_stream.rs` + tests) |
| AC-4 | At-limit & zero-byte complete; over-limit/short/long/malformed/aborted/stalled/disconnected clean up with no visible partial file | `serve.rs` integration tests + core drop-unlink tests | **Met for upload**; **R-A** for download corpus coverage |
| AC-5 | `frame_too_large` = unparsed connection close; recoverable stream-local violations terminate only their bound request | Codec `FrameTooLarge` → `4005`; runtime `malformed_frame` path | **Met** (verify with a `4005`-vs-`malformed_frame` test) |
| AC-6 | Dropped pre-terminator stream cannot resume (retry at zero); lost final reply replays via `op_id` with no second effect | `transport_lost` path + dedup-ledger replay test | **Met** (verify replay test) |
| AC-7 | Harness executes both directions; byte counters cannot pass without observing bytes | `stream.mjs` receiver-accepted `bytes_streamed`; corpus run | **Upload met**; **R-A/R-B** for download + fault cases |
| AC-8a | Corrected file fixtures transcribed from the record, pass corpus validation | `node scripts/check-v2-corpus.mjs` | **Met** (verify transcription source = record) |
| AC-8b | Rust build/lint/unit/integration + live smoke, sidecar, agent E2E green | Phase G.3–G.4 | **Verify** (run and record; **R-C** full A/B) |

---

## 7. Test strategy

- **Codec unit + golden + fuzz** — round-trip and rejection for every kind and error variant; no panic / no unbounded allocation.
- **Core staging** — drop-before-END unlink; single-use finalize; single-event authorship; op_id replay returns the recorded result.
- **Daemon integration** (`serve.rs`) — the AC-4 matrix (at-limit, zero, over, short, long, malformed, aborted, stalled, disconnected), `resource_exhausted` pre-OPEN, released-capacity readmission, `4005` vs `malformed_frame`, cancellation races (client END wins, daemon cancel wins, ABORT×ABORT, client ABORT wins, `transfer.cancel` wins, FINALIZING immunity).
- **Conformance corpus** — shape validation (`check-v2-corpus.mjs`) plus live execution of `stream` cases; **full A/B** run recorded (R-C).
- **Live** — `smoke`, `sidecar-check`, `agent-e2e` (AC-8b); if these are v1 relics for this change, say so and cite the v2 corpus A/B as the real proof.

---

## 8. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Concurrency correctness in `UploadIngress` (deadline/disconnect/sink-accept vs terminal ordering) | A partial file becomes visible, or a terminal is lost | Ordered terminal admission installed before awaiting bounded capacity; assert every race in integration tests; keep the race table in §3.8 authoritative. |
| Unbounded memory under adversarial CREDIT/DATA | DoS | Byte-bounded ingress, source read-ahead, and outbound queues; codec caps DATA at 64 KiB; sentinel is one byte, never staged. |
| Fixtures drift from the record | Corpus asserts implementation, not contract | Transcribe from §3 only; forbid deriving fixtures from behavior; `check-v2-corpus.mjs` is shape-only, so pair it with the executor A/B. |
| Single-subject harness cannot stage a fetched file | Download corpus cases stay declarative (R-A) | Add a harness staging affordance or multi-subject capability; until then cover download with Rust integration tests and mark corpus cases honestly. |
| Declarative fault cases give false confidence | Malformed/backpressure/cancel/disconnect not actually executed (R-B) | Implement raw-record/credit-pause/client-ABORT/transport-drop controls; treat the executor status row in the README as the ledger of what is real. |
| Live smoke/sidecar/agent-e2e are v1 relics | AC-8b passes without proving v2 framing | Run them, but designate the full v2 corpus A/B as the load-bearing evidence; do not silently skip. |
| Upstream U1/U2/U3 unmet | Some cases cannot pass | Keep them `blocked_on_upstream` (fail, not skip); out of #233 scope. |

---

## 9. Rollout / rollback

- **Rollout:** docs PR first (Phase A), then independent codec / core / daemon / harness / fixture slices. v2 is a clean-slate generation with no dual-support and no migration; the released `v0.6.x` line keeps speaking v1 until retired, so shipping v2 framing does not touch the released product.
- **Rollback:** revert the offending slice. The codec is pure and side-effect free; the daemon runtime is gated behind the v2 generation gate, so a revert cannot corrupt v1 clients. Staging is transient (unlinked on drop / startup cleanup), so no durable artifact needs cleanup on rollback.

---

## 10. Open questions

1. **Download corpus executability (R-A).** Do we add a single-subject harness affordance to stage a `fetched_file`, or introduce a genuine multi-subject harness capability? The latter also unblocks the 73 multi-actor cases noted in maintainer memory but is a larger change.
2. **Fault-control surface (R-B).** What is the minimal, auditable API for raw-record injection / credit-pause / client-ABORT / transport-drop that keeps the harness deterministic and its watchdog (ERROR, not verdict) intact?
3. **Live-check relevance (AC-8b).** Confirm whether `smoke`/`sidecar-check`/`agent-e2e` meaningfully exercise v2 byte-stream framing, or whether they are v1-protocol relics for this change; document the decision so the gate is honest.
4. **Full-corpus baseline (R-C).** Record the current full-run pass/fail/error/blocked counts so regressions are measurable rather than asserted.

---

## 11. References

- `docs/protocol-v2.md` — § Byte-stream framing; `file.share`/`file.read`/`transfer.cancel` schemas; limits object; close codes; shared value types.
- `crates/jeliya-codec/src/byte_stream.rs` (+ `tests/`).
- `crates/jeliya-core/src/protocol_upload.rs`, `engine.rs`, `typed.rs`.
- `crates/jeliyad/src/{file_share,file_read,transfer,serve}.rs`.
- `conformance/v2/{README.md,files.json,manifest.json}`; `conformance/v2/harness/{stream,session,runner,values,assert}.mjs`.
- `scripts/{check-docs,check-v2-corpus,smoke,sidecar-check,agent-e2e}.mjs`.
- Issues: #233 (this), #161, #164, #165, #166, #92, #209.
