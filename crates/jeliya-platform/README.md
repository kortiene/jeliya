# jeliya-platform

The **injectable `PlatformServices` boundary** for the Jeliya clean-slate stack
(#174): one cloneable, renderer-agnostic facade carrying object-safe capability
traits for **files, persistence, lifecycle, URLs, clipboard/share, navigation,
and window actions**, with a closed outcome taxonomy, safe path/URL types, and
deterministic browser/desktop/Android fakes.

It is the third separately injected input to the shared UI, beside `jeliya-api`
view models and the `jeliya-client` `ClientHandle` seam. The authoritative
record of the decision is `docs/dioxus-architecture.md` §"Decision 4"; where
this crate and that record (or `docs/product-behavior-contract.md` /
`docs/protocol-v2.md`) disagree, **the record is right and this crate has a
bug** — say which in the PR.

## What it delivers

- **Capability traits whose types distinguish platform object kinds.** A
  browser blob, a desktop path, and an Android `content://` URI are different
  object kinds behind one opaque `PickedSource`; a shared component reaches only
  a `FileObjectKind` discriminant, never a path.
- **A closed outcome taxonomy that never collapses** (`CapabilityError`):
  `Unavailable`, `Denied`, `Cancelled`, and typed `Failed` stay apart, so a
  cancellation can never read as success.
- **An allowlisted external-URL launcher** (`SafeExternalUrl`) that fails closed
  on any scheme outside `https` / `mailto` / `tel`.
- **Honest storage**: preferences, secret custody, and protected-directory facts
  are separate concerns; durability (`SessionScoped` vs `Persistent`) is a
  queryable fact, and a non-durable write is reported, not swallowed.
- **Representable lifecycle events** on a bounded, loss-visible subscription;
  control intents (`BackRequested`, `ProcessRestored`, terminal window events)
  are never silently lost: a saturated mailbox run-length-encodes a Back burst
  (every Back still delivered) and absorbs a restated close/restore into its
  still-undelivered twin, keeping the mailbox hard-bounded.
- **A deterministic in-process fake for every service** (behind the `fake`
  feature), shaped as browser / desktop / Android fixtures and scriptable for
  denied / unavailable / cancelled outcomes.

## Boundaries held by construction (`tests/boundaries.rs`)

- The library graph (default and `wasm32`) pulls no Iroh, `jeliya-core`,
  `jeliyad`, `jeliya-ffi`, WebSocket crate, native transport,
  `quinn`/`rustls`/`tokio`, `wry`/`tao`, `openssl-sys`, `native-tls`, or Dioxus.
- No `serde_json::Value` in any public signature.
- No platform `cfg` fork in the contract surface (target selection happens at
  the crate root and in the fakes, never in a shared component).

## Scope

This crate ships the **contract and the fakes only**. Target implementations
(browser web-sys, desktop file dialogs, Android SAF/JNI) are out of scope and
land in M3–M5 behind the unchanged facade.

## Local gate

```sh
cargo test -p jeliya-platform --features fake
cargo build -p jeliya-platform --example shared_component --features example
cargo build -p jeliya-platform --example shared_component --features example \
    --target wasm32-unknown-unknown
```
