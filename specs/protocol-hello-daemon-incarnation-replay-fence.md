# Spec — `hello` carries a daemon incarnation identity so replay can fence across restarts (#270)

- **Issue:** kortiene/jeliya#270 — `[Protocol][Client]: hello should carry a daemon incarnation identity so replay can fence across restarts`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters), with a small protocol/daemon change in `jeliya-api`/`jeliyad`.
- **Records/derives its decision from:** `docs/protocol-v2.md` §"Layer 2 — `hello`" and §"Request deduplication lives in the envelope"; `specs/rust-client-bounded-kernel.md` §K5 (replay policy) and §K6 (never-sent vs may-have-executed); the engine's in-memory dedup ledger keyed `(session principal, op_id)` (`crates/jeliya-core/src/engine.rs`).
- **Found by:** #268's review (round 19) — the dedup ledger is deliberately in-memory and its principal key includes the daemon credential, but the storage-generation gate passes across a daemon restart, so a stable-`client_id` client that auto-replays a keyed mutation after a lost reply can re-execute it against the new incarnation's empty ledger.
- **Depends on (landed):** #163/#164 (`jeliya-api`, `jeliya-codec`), #168 (the bounded client kernel, present with the `stable_principal` gate and the `replay_hold` set), the v2 conformance corpus and its harness.
- **Blocks / unblocks:** #171 `WsWeb` and #172 `WsNative` — once `hello` carries the incarnation and the kernel fences on it, a socket adapter that supplies a stable `client_id` may honestly set `KernelConfig::stable_principal = true`. #173 `DirectClient` already certifies (in-process: daemon restart = client restart) and is unaffected in behaviour.
- **Owner role:** protocol/core maintainer (the change touches the normative `docs/protocol-v2.md`, the shared `jeliya-api` `Hello` type, the daemon, and the transport-independent client kernel).
- **Status of this document:** implemented (#270, 2026-08-16). `Incarnation` domain and `Hello.incarnation`, engine `mint_incarnation`, `Input::Connected { incarnation }`, `Core.last_incarnation`, and the `on_connected` drop-on-change branch all landed. Conformance corpus updated (two runnable cases + one U4-exempt cross-restart case). Socket adapters (#171/#172) defer `hello.incarnation` extraction and forwarding as stated in §3. Where this spec and `docs/protocol-v2.md` disagree, the protocol record is authoritative.

> This document is a **planning artifact**. It records decisions made during the design phase; the canonical reference for the wire contract is `docs/protocol-v2.md`.

---

## 1. Outcome

Make the client's opt-in replay of keyed mutations safe across a **daemon restart**, not only across a **reconnect**. Concretely:

1. The protocol-v2 `hello` frame carries a **daemon incarnation identity**: a per-process nonce, freshly minted at each daemon start, identical for every connection of one running process, and different (with overwhelming probability) after any restart.
2. The client kernel remembers the incarnation it last connected under and, on the next `Connected`, **drops every replay-held call** — settling `Disconnected { Unknown }` — when the incarnation changed, instead of re-sending it against the new incarnation's empty dedup ledger.
3. `KernelConfig::stable_principal` narrows to mean exactly the **static** fact it can honestly assert — the adapter supplies a stable session principal (a stable `client_id`) — because the **dynamic** "same daemon incarnation" half of dedup-scope continuity now lives in the kernel's runtime fence. A socket adapter (#171/#172) that supplies a stable `client_id` and forwards the incarnation from each `hello` may then set `stable_principal = true` truthfully.

The result closes the review-19 hole: a lost-reply keyed mutation either replays under a proven-continuous dedup scope (same principal **and** same incarnation, so the ledger returns the original result and performs no second effect) or is settled honestly as `Disconnected { Unknown }` — never silently re-executed against an empty ledger.

## 2. Background — the exact defect

The protocol's idempotency ledger (`docs/protocol-v2.md` §"Request deduplication lives in the envelope"; `crates/jeliya-core/src/engine.rs`) is keyed `(session principal, op_id)` and is **deliberately in-memory** (`DedupLedger`, engine.rs — "the v2 harness has no daemon restart, and a durable ledger is a persistence concern the clean-slate milestone does not take on"). The `principal_key` is the authenticated session principal rendered as `credential + client_id`.

The client kernel's §K5 replay gate holds a keyed mutation across a reconnect and re-sends it under the same `op_id` on the next connection, relying on the ledger to return the original result. It gates this on `stable_principal` (default `false`), whose doc today claims to certify the **complete** dedup scope: "a stable session principal ... AND the same daemon incarnation".

The bug: the storage-generation gate (`docs/protocol-v2.md` §Layer 1, step 4) compares the client's declared `sg` against `jeliya_core::engine::STORAGE_GENERATION`, a **compile-time constant tied to the on-disk data schema**. A daemon restart does not change it. So across a restart the gate still passes, the client reconnects under a **stable `client_id`** (stable principal), and the kernel replays a held mutation — but the new process's in-memory ledger is **empty**, so the replay **re-executes** the mutation rather than returning the original result. `stable_principal` cannot honestly be set by a socket adapter today precisely because it can verify the principal half but not the incarnation half.

`storage_generation` and the incarnation are **orthogonal identities**:

| Identity | Scope | Survives restart? | Purpose |
|---|---|---|---|
| `storage_generation` | the on-disk data schema/version | **yes** (persistent) | fail-closed gate: refuse a client carrying state from another generation before it can write |
| daemon incarnation | one running daemon process | **no** (fresh per start) | fence in-memory, process-scoped state (the dedup ledger) so a client does not treat a restarted daemon as continuous |

Same `storage_generation` + different incarnation = **the daemon restarted with the same data schema, and its in-memory ledger is empty.** That is exactly the state the kernel must detect to withhold replay.

## 3. Scope — what this issue is, and is not

**In scope:**

- `jeliya-api`: a new opaque `Incarnation` domain and a required `incarnation` field on `Hello`.
- `docs/protocol-v2.md`: document the field in §Layer 2, add it to the types table, and explain its orthogonality to `storage_generation`; add conformance cases.
- `jeliyad`: mint one incarnation per process start and serve it in every `hello`.
- `crates/jeliya-client` kernel: thread the incarnation into `Input::Connected`, remember the last one, and drop replay-held calls on change (settling `Disconnected { Unknown }`); rewrite the `stable_principal` doc and the `replay.rs` doc to reflect the split; extend the deterministic controller and fault suite.
- `conformance/v2/`: cases proving the incarnation is present, opaque, and stable across two connections to the same daemon; a cross-restart case declared as an exemption until the harness can restart a daemon.

**Not in scope (explicitly deferred):**

- **The socket adapters themselves (#171/#172).** This issue makes it *possible* for them to certify `stable_principal` by (a) carrying the incarnation on the wire and (b) fencing on it in the kernel. Writing the adapters — including the driver code that extracts `hello.incarnation` and forwards it to `Input::Connected` — is #171/#172. This spec defines the seam obligation they inherit.
- **A durable dedup ledger.** The incarnation *fence* is the clean-slate answer to a restart; persisting the ledger across restarts is a separate, larger persistence decision the milestone does not take on. The incarnation makes the in-memory ledger *safe*, not durable.
- **Any change to `storage_generation`, the Layer-1 gate order, or the shared `{protocol, min_protocol, storage_generation, limits}` discovery object** (Layer-0 health, portfile, ready line). The incarnation is a `hello`-only field (§5, D3).
- **Stream lifecycle hooks (#269)** and the byte-stream executor (#233) — untouched.

## 4. Owning modules and files

```
crates/jeliya-api/
  src/ids.rs            # + opaque_id!(Incarnation, …)
  src/push.rs           # Hello gains `incarnation: Incarnation`
  tests/contract.rs     # hello construction + a round-trip assertion on the field
crates/jeliyad/
  src/main.rs           # mint one incarnation at startup; store on AppState
  src/serve.rs          # include it in the `hello` builder
crates/jeliya-core/
  src/engine.rs         # (recommended home) Engine::new mints it; Engine::incarnation()
crates/jeliya-client/
  src/kernel/transport.rs  # Input::Connected carries `incarnation` (via the seam’s Connected shape)
  src/kernel/core.rs       # last_incarnation + the on_connected drop-on-change branch
  src/kernel/replay.rs     # doc: the incarnation half is now a runtime fence, not a static assumption
  src/kernel/mod.rs        # KernelConfig::stable_principal doc rewrite; controller connect_with_incarnation
  tests/…                  # fault cases (may live in core.rs unit tests + the in-memory controller tests)
docs/protocol-v2.md         # §Layer 2 field + prose; types table row; orthogonality note
conformance/v2/handshake.json + manifest.json  # presence, stability, cross-restart (exempt)
```

## 5. Design decisions

### D1 — The incarnation is a fresh per-process nonce, not a repurposed field

The incarnation MUST change on every daemon start and MUST be identical for every connection of one process. Candidates that fail:

- **`pid`** — recycled by the OS; a restart can reuse the same pid, so equal pids do not prove continuity.
- **`started_at_ms`** (already on the portfile) — coarse (two restarts within one millisecond collide) and derived from a clock that can move backwards; a monotonic-looking value that can repeat is a false continuity signal.
- **`storage_generation`** — persistent by design (§2); it is exactly the identity that *does not* change across a restart.

The honest choice is a **random 128-bit value from the OS CSPRNG, minted once at startup**, hex-encoded, carried as an opaque string. This mirrors how the daemon already mints its per-start WS auth token (`lifecycle::generate_token` — `getrandom::fill(&mut [u8; 32])` + `hex::encode`). `jeliya-core` already depends on `getrandom = "0.4"`, so the engine can mint it with no new dependency.

The incarnation is **not a secret** — it is disclosed to every connected (post-auth) client and carries no capability. It is not compared in constant time and grants nothing. Its only consumer is the client's equality check across two `hello`s.

### D2 — `Incarnation` is an opaque string domain in `jeliya-api`, defined once

Add `opaque_id!(Incarnation, "Opaque daemon-incarnation identity (`<incarnation>`); a per-process nonce, fresh at each daemon start.")` in `crates/jeliya-api/src/ids.rs`, matching the existing `RoomId`/`OpId`/… pattern (a `#[serde(transparent)]` `String` newtype with `new`/`as_str`/`Display`/`From<String>`/`AsRef<str>`, deriving `Clone, PartialEq, Eq, Hash, PartialOrd, Ord`). One definition is shared by the daemon's `Hello`, the codec round-trip, and the kernel's `Connected` input — the same single-source-of-truth discipline the record demands of the served limits and the version object.

Opaque (no format validation) is correct: protocol v2 guarantees no representation for any id, and the client only ever compares two incarnations for equality — it never parses, orders, or interprets the value.

### D3 — The incarnation lives on `hello` only, not in the Layer-0 discovery object

The shared `{protocol, min_protocol, storage_generation, limits}` object is defined **once** and carried identically by Layer-0 health, the portfile, and the ready line — a load-bearing "three producers, one definition" invariant. The incarnation is **deliberately kept out of it**:

- Its only use is a **same-value-across-two-`hello`s** comparison, which requires being connected. A client cannot act on it before its first connection, so publishing it at unauthenticated Layer 0 (or in the portfile) adds surface with no consumer.
- Layer 0 is unauthenticated and deliberately minimal; every field there is a potential fingerprint. The incarnation belongs behind the gate, in the daemon's first post-upgrade frame, where the client that needs it already is.

Therefore: extend `Hello` (`jeliya-api`), not `VersionInfo`. The portfile, health endpoint, ready line, and their conformance cases are untouched.

### D4 — The kernel fences on the incarnation in the sans-IO core, driver supplies it verbatim

Per §K1, **all correctness lives in the pure core**, so the incarnation comparison is a core decision, not a driver one. The driver's only new obligation is to lift `hello.incarnation` and hand it to the core on `Connected`:

- `Input::Connected` carries `incarnation: Incarnation` (non-optional — see D5).
- `Core` gains `last_incarnation: Option<Incarnation>`.
- In `on_connected`, after the dial-token fence passes and **before** the replay-hold is requeued:
  - if `last_incarnation == Some(prev)` and `prev != incarnation` → **drop** every `replay_hold` entry (D6);
  - else → requeue the held calls exactly as today.
  - then set `last_incarnation = Some(incarnation)`.

The comparison being in the core makes the whole behaviour a deterministic sequence of `step` calls — the fault suite exercises "reconnect, same incarnation → replay" and "reconnect, changed incarnation → drop" as ordinary unit inputs, identical on wasm and native, with no wall clock.

### D5 — `Connected` carries a non-optional incarnation; `DirectClient` supplies a stable constant

Making the field non-optional keeps the core free of `Option` branching and states the invariant plainly: every live connection has an incarnation.

- **Socket adapters (#171/#172):** forward `hello.incarnation` verbatim. A daemon restart yields a new value; the fence fires.
- **`DirectClient` (#173):** there is no `hello`; the in-process engine is the same object for the client's whole life (a daemon restart *is* a client restart, which rebuilds the kernel). It supplies a **per-instance constant** incarnation (minted once when the `DirectClient` is built). Across its multiple `Connected` reports (resume without a fabricated reconnect, §K10), the value is unchanged, so the fence never fires and replay proceeds exactly as before. `DirectClient` behaviour is unchanged.

### D6 — Dropping a replay-held call on incarnation change settles `Disconnected { Unknown }`

A held call was **sent** on the prior incarnation, so it may have executed there; the client cannot confirm and must not replay it against the new (empty) ledger. `Disconnected { Unknown }` is the honest classification — it withholds a provable-negative and never invites an unguarded caller retry (the same "weakest honest claim" posture as the send/close race, §K14).

Precise cleanup for each dropped held call (mirroring the queued-cancel accounting, since a held call is in `Phase::Queued` with its **full** charge restored at interrupt — `charge_count_unchecked()` re-added the count slot while `holds_charge` kept the bytes):

```
for call_id in sorted(drain(replay_hold)):
    if let Some(entry) = ledger.take(call_id):
        if entry.holds_charge:
            admission.release(entry.payload_bytes)   // release BOTH count and bytes
        actions.push(CancelTimer(entry.deadline_timer))
        actions.push(Settle(call_id, Err(Disconnected { execution: Unknown })))
```

Drain-and-sort by `CallId` so the action stream is deterministic (§K13), exactly as `on_interrupted` sorts its buckets. `replay_hold` ends empty; `in_flight_count` was already `0` (reset at interrupt); the dropped entries are fully removed from the ledger. **Never-sent queued calls are untouched** — they were never put on any incarnation's wire, so they flush normally on the new connection (safe: they provably did not execute anywhere).

### D7 — `stable_principal` narrows to the static half; the incarnation is the dynamic half

Before #270, `stable_principal` conflated two facts a socket adapter cannot both assert statically:

1. **Stable principal** (static): the adapter supplies a stable `client_id`, so `(principal, op_id)` survives a reconnect. Knowable at construction.
2. **Same daemon incarnation** (dynamic): the daemon has not restarted between the send and the replay. Knowable only at runtime.

#270 moves fact 2 into the kernel's runtime fence (D4–D6). `stable_principal` therefore means **only fact 1** and its doc is rewritten to say so. Consequences:

- `replay.rs` is unchanged in logic (`ReplayPolicy::derive` still gates admission-time eligibility on `stable_principal`); only its doc comment changes — the incarnation half is a *later* fence at reconnect, not an admission-time assumption.
- A socket adapter that supplies a stable `client_id` and forwards each `hello.incarnation` may now set `stable_principal = true` **honestly** — discharging AC-3. The kernel guarantees that even a replay-eligible call is dropped, not re-sent, if the incarnation changed.
- The field name `stable_principal` is retained because it now names exactly fact 1. Renaming is an Open Question (§13), not a requirement.

### D8 — The incarnation is not rendered in kernel diagnostics

By §K15 the kernel never renders payloads or correlating identifiers in diagnostics. The incarnation is not a secret, but it correlates a daemon's connections, so the kernel treats it like `op_id`: stored and compared, **never** logged or printed. Any `Debug` the kernel adds around `Connected`/`last_incarnation` redacts it (reuse the `Redacted` wrapper in `kernel/diag.rs`). The `jeliya-api` `Incarnation` may keep the domain's standard `Debug` (used by the daemon and tests); the redaction obligation is on the *kernel's* diagnostics, not the type.

## 6. Wire and API changes

### 6.1 `jeliya-api`

- `ids.rs`: `opaque_id!(Incarnation, …)`.
- `push.rs`: add to `Hello` a required field:

```rust
pub struct Hello {
    pub protocol: u64,
    pub storage_generation: u64,
    /// The daemon incarnation identity: a per-process nonce, fresh at each
    /// daemon start and identical for every connection of one process. A
    /// client compares it across reconnects to fence replay of keyed
    /// mutations — the dedup ledger is in-memory, so a changed incarnation
    /// means a restarted daemon with an empty ledger (orthogonal to
    /// `storage_generation`, which is persistent).
    pub incarnation: Incarnation,
    pub limits: Limits,
    pub subject: SubjectState,
    pub resume: Resume,
}
```

- `tests/contract.rs`: update the existing `hello_carries_t_discriminator` construction and add an assertion that `incarnation` round-trips (serialize → deserialize → equal) and appears as a top-level string key.

Adding a **required** field is safe within v2: `min_protocol == protocol == 2` (one generation at a time), so there is no in-band skew between a daemon that emits the field and a client that reads it. A `hello` lacking the field is malformed and rejected at the driver boundary (the field is not `Option`).

### 6.2 `jeliyad` daemon

- Mint one `Incarnation` per process start. **Recommended home:** `Engine::new` (`crates/jeliya-core/src/engine.rs`), stored beside the `DedupLedger` it identifies, exposed as `Engine::incarnation(&self) -> Incarnation` (clone of a stored value). Rationale: the incarnation *is* the lifetime-identity of that in-memory ledger; co-locating them makes the coupling structural and guarantees "one per process" because the engine is built once. `jeliya-core` already depends on `getrandom`.
  - Alternative: mint in `main.rs` via a `lifecycle::generate_incarnation()` mirroring `generate_token()` and store on `AppState`. Acceptable but decouples the incarnation from the ledger it fences (see §13 OQ-1).
- `serve.rs` `hello` builder: set `incarnation: state.engine.incarnation()`.

### 6.3 `jeliya-client` kernel

- `transport.rs` / `core.rs`: `Input::Connected { token }` → `Input::Connected { token, incarnation: Incarnation }`.
- `core.rs`: add `last_incarnation: Option<Incarnation>`; implement D4/D6 in `on_connected`.
- `mod.rs` (in-memory controller): `connect_with_incarnation(&self, incarnation: Incarnation) -> u64`, and `connect()` delegates with a fixed default constant (so existing reconnect-replay tests keep passing with a stable incarnation). `connect_at_token`/any other path that drives `Input::Connected` gains the incarnation likewise.
- Doc-only: rewrite `KernelConfig::stable_principal` (D7) and the `replay.rs` module/`derive` docs (the incarnation half is a runtime fence).

## 7. `docs/protocol-v2.md` changes

1. **§Layer 2 — `hello`:** add `incarnation` to the example JSON and note it is a required opaque string:

```json
{ "t": "hello",
  "protocol": 2,
  "storage_generation": 1,
  "incarnation": "<incarnation>",
  "limits": { "...": "as above" },
  "subject": { "state": "present", "subject_id": "<subject_id>", "device_id": "<device_id>" },
  "resume": { "state": "fresh" } }
```

   Prose: "`incarnation` is a per-process nonce, freshly minted at each daemon start, identical for every connection of one running process. It is **orthogonal to `storage_generation`**: the storage generation is a persistent property of the on-disk data and survives a restart; the incarnation identifies the running process and does not. A client that replays a deduplicated mutation across a reconnect (see [`op_id`](#request-deduplication-lives-in-the-envelope)) MUST treat a changed incarnation as a restarted daemon with an empty in-memory dedup ledger and MUST NOT replay against it — it settles the held work as a disconnect of unknown execution instead. `incarnation` is disclosed only after the gate, in `hello`; it is not part of the Layer-0 discovery object, is not a credential, and grants nothing."

2. **Types table** (§the row list near `subject`/`resume`): add `| incarnation | opaque string — `hello` only | ` and register it in the `<incarnation>` opaque-string family alongside `<op_id>` et al.

3. **§"Request deduplication lives in the envelope":** add a sentence noting the ledger is in-memory and process-scoped, and that the `hello` incarnation is how a client detects a restart so it does not re-execute a keyed mutation against a fresh ledger.

Docs-gate discipline (per `docs/PROFILE.md` and the docs gate): the placeholder `<incarnation>` in **prose** must be backticked; in JSON code fences it is a bare string literal as the other placeholders are. Frontmatter and index reachability are unchanged (this edits an existing page).

## 8. Conformance corpus changes (`conformance/v2/`)

Following the corpus rules — hand-authored, independent of the implementation, every exemption fails not skips, `name` corpus-wide unique:

1. **`hello_carries_a_daemon_incarnation_identity`** (`handshake`, runnable): complete the positive gate, `await` `hello`, `save` `frame.incarnation`, `assert` it is a non-empty `<string>`. Catches a daemon that omits the field.
2. **`the_daemon_incarnation_is_stable_across_connections`** (`handshake`, runnable): open two connections to the **same** running daemon; save `incarnation` from each `hello`; assert they are **equal**. Catches a daemon that mints the nonce per connection instead of per process (which would make the client's fence fire spuriously on every reconnect and defeat replay entirely).
3. **`the_daemon_incarnation_changes_across_a_restart`** (`handshake`, **exempt**): save the incarnation, restart the daemon, reconnect, assert the incarnation **differs** while `storage_generation` is **unchanged**. This requires a daemon-restart capability the current harness lacks (engine.rs: "the v2 harness has no daemon restart"). Declare it in `manifest.json` as an exemption with `reason` = "harness cannot restart a daemon within a case" and an unblocking issue (a harness-capability follow-up). Per §"Exemptions fail" it **fails until unblocked** — a skipped case reads as coverage; a failing case reads as work.

`manifest.json`: register the three case names; the exemption entry names its reason and unblocking issue. Existing `await`-frame cases that assert only `{t, protocol}` are partial matches and are unaffected by the new field.

## 9. Test strategy

**Kernel (deterministic, sans-IO — the correctness heart):**

| Behaviour | Test |
|---|---|
| First connect records the incarnation, drops nothing | dispatch nothing held; `Connected(inc_A)`; assert no settle, `last_incarnation == inc_A` |
| Reconnect, **same** incarnation → held keyed mutation replays (regression guard for today's behaviour) | send `message.send`(op_id) under `inc_A`; `Interrupted`; backoff+`Connected(inc_A)`; assert the held call re-sends (a new `Send` frame) and `replay_hold` drains to the queue — the existing `generation_fencing_drops_a_stale_reply_and_replays_the_held_call` shape with an explicit equal incarnation |
| Reconnect, **changed** incarnation → held call **dropped** `Disconnected { Unknown }`, not re-sent | send `message.send`(op_id) under `inc_A`; `Interrupted`; `Connected(inc_B)`; assert one `Settle(Disconnected{Unknown})`, **no** `Send` for it, `replay_hold_len() == 0`, ledger entry gone, admission fully released |
| Changed incarnation with a **mix**: never-sent queued + replay-held | `in_flight = 1` so a second call queues behind a sent keyed mutation; `Interrupted`; `Connected(inc_B)`; assert the held (previously-sent) call is dropped `Unknown` while the never-sent queued call **flushes** on `inc_B` |
| Admission/bounds intact after a drop | after a changed-incarnation drop, `outstanding`/`queued`/`in_flight`/`replay_held` are consistent and no collection exceeds its bound (§K12) |
| `stable_principal == false` → no held work, incarnation change is a no-op | with replay disabled, a changed incarnation across reconnect still settles the (already-`Never`) call honestly and never panics on the drop path |

The existing core unit tests that construct `Input::Connected { token }` are updated to pass a default incarnation (a small test helper), and the in-memory controller's `connect()` default keeps a stable incarnation so no unrelated test changes behaviour.

**API/daemon:** `jeliya-api` contract round-trip (§6.1); a daemon-level assertion (unit or the existing serve tests) that two `hello`s from one process carry the **same** incarnation and that it is present and non-empty.

**Conformance:** the three cases in §8 (two runnable, one exempt-failing).

**Boundaries:** `jeliya-client/tests/boundaries.rs` still passes — the kernel adds no new runtime dependency (the `Incarnation` type is already reachable via the existing `jeliya-api` dep), no `std::time`/RNG in the core.

## 10. Security, privacy, observability

- **Privacy/secrets:** the incarnation carries no capability and is disclosed only post-gate in `hello`; it is deliberately kept out of the unauthenticated Layer-0 response and the portfile (D3). The kernel never renders it in diagnostics (D8).
- **No new oracle:** because it is post-gate, an unauthenticated caller learns nothing new (contrast the capacity-check ordering rationale in §Layer 1). It reveals only "this is one running process", to a caller already authenticated to it.
- **Fail-safe direction:** the fence is conservative — on a changed incarnation it **withholds** replay (settles `Unknown`), never fabricates a `DefinitelyNot` that would invite an unguarded retry. A daemon that (wrongly) mints per-connection nonces degrades to "never replays" (safe, just less resilient), caught by conformance case 2.
- **Observability:** a client may surface "daemon restarted, N in-flight mutations settled with unknown outcome" from the `Disconnected { Unknown }` settlements; no new event type is required (the `Execution` classification already carries it through the seam).

## 11. Acceptance-criteria mapping

| Issue AC | Mechanism | Evidence |
|---|---|---|
| protocol-v2 `hello` carries a daemon incarnation identity, documented with conformance cases | `Incarnation` domain + `Hello.incarnation` (§6.1); `docs/protocol-v2.md` §Layer 2 + types table (§7); conformance cases (§8) | contract round-trip; conformance cases 1–3 |
| the kernel drops replay-held calls (settling `Disconnected { Unknown }`) when the incarnation changes across a reconnect | `last_incarnation` + `on_connected` drop branch (D4/D6) | kernel "changed incarnation → dropped" and "mixed" tests (§9) |
| the socket adapters may then certify `stable_principal` when they supply a stable `client_id` | `stable_principal` narrowed to the static half; incarnation is the runtime fence (D7); `Connected` carries the incarnation for adapters to forward (§6.3) | rewritten `stable_principal`/`replay.rs` docs; the seam obligation documented for #171/#172 |

## 12. Risks and mitigations

- **A weak incarnation source gives false continuity.** Reusing `pid` or `started_at_ms` could repeat across a restart and defeat the fence silently. *Mitigation:* D1 mandates a CSPRNG 128-bit nonce; conformance case 2 pins per-process stability, and the cross-restart case (once unblocked) pins per-restart change.
- **Per-connection nonce defeats replay entirely.** A daemon that mints the incarnation per connection would make every reconnect look like a restart, dropping all held work. *Mitigation:* conformance case 2 asserts stability across two connections; the daemon home is `Engine::new` (once per process), not the per-connection `hello` path.
- **Scope creep into the adapters.** Implementing the driver-side extraction/forwarding here would trespass on #171/#172. *Mitigation:* this issue defines the `Connected` shape and the seam obligation only; the in-memory controller is the sole driver it ships (as #168 did).
- **Making `incarnation` required breaks in-tree `Hello` constructions/fixtures.** *Mitigation:* only two Rust constructions exist (`serve.rs`, `tests/contract.rs`) plus the in-memory controller's `connect`; conformance `await`-frame cases are partial matches and unaffected. All are enumerated in §4/§6.
- **Cross-restart conformance cannot run today.** *Mitigation:* declared as a failing exemption (§8) per the corpus's "exemptions fail" rule, with an explicit unblocking issue — it reads as work, not coverage.
- **`stable_principal` doc drift.** If the narrowed meaning is not documented precisely, an adapter author could still under- or over-certify. *Mitigation:* D7 rewrites both the config doc and the `replay.rs` doc; the split is stated as the crux of AC-3.

## 13. Open questions

1. **Incarnation home: `Engine` vs `AppState`.** Recommend `Engine::new` (co-located with the ledger it identifies, `getrandom` already available). `AppState` + a `lifecycle::generate_incarnation()` is the alternative if the maintainer prefers all per-start nonces minted together in `jeliyad`. Either satisfies "one per process".
2. **Rename `stable_principal`?** It now names only the static stable-principal fact. A name like `principal_is_stable` or `dedup_scope_certified_static` could reduce confusion, but a rename is a public-surface churn (`KernelConfig` is re-exported). Recommend **keeping** the name and fixing the doc; revisit under the #175 parity suite if adapter authors find it misleading.
3. **Should `DirectClient` mint a random per-instance incarnation or a fixed sentinel?** Either works (it never changes within an instance). A random per-instance value is marginally more honest (two DirectClient instances differ) and costs nothing; recommend that, decided in #173.
4. **Harness restart capability.** The cross-restart conformance case needs a `daemon:restart` step the harness lacks. Track the harness follow-up as the case's unblocking issue; confirm the manifest exemption schema accepts a harness-capability reason (vs the existing `blocked_on_upstream` reasons).
5. **Client surfacing of a detected restart.** The kernel settles `Disconnected { Unknown }`; does any UI flow need a distinct "daemon restarted" signal beyond the existing `Execution::Unknown`? Recommend **no new type** for #270 (honest and sufficient); revisit if a product flow needs to distinguish restart from ordinary loss.

## 14. Assumptions

- `crates/jeliya-client` (#168) is landed at its current shape: the `stable_principal` config, the `replay_hold` set, the `on_connected`/`on_interrupted` accounting, and the deterministic in-memory controller behind `test-transport` are present as read here.
- `jeliya-api`'s `opaque_id!` macro and the `Hello` type are stable; adding an opaque domain and a required `Hello` field is the intended extension path.
- The engine's dedup ledger stays **in-memory** and process-scoped for this milestone; the incarnation makes it *safe across restarts*, not durable. If a durable ledger later lands, the incarnation fence remains correct (it becomes a belt-and-braces conservative check).
- `min_protocol == protocol == 2`, so a required `hello` field introduces no in-band version skew.
- The conformance harness supports two concurrent connections to one daemon (for case 2) but **not** an in-case daemon restart (case 3 is exempt).
- The orchestrator performs all git/gh/PR actions; this document is the only artifact the planning phase produces, and no production code is written for #270 by the planning phase.
