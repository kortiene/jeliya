# Spec — Injectable `PlatformServices` boundary for files, storage, lifecycle, and native actions (#174)

- **Issue:** kortiene/jeliya#174 — `[Rust][Platform]: Define injectable PlatformServices for files, storage, lifecycle, and native actions`
- **Program:** #156 (Dioxus clean-slate). **Milestone:** M2 (client runtime and platform adapters); target implementations follow in M3–M5.
- **Records the decision in:** `docs/dioxus-architecture.md` §"Decision 4 — one seam, four adapters, one platform boundary", the layering table in §"Decision 3", and §"Decision 7"; the product rules that bind it are `docs/product-behavior-contract.md` §"PlatformServices" and §"Preferences and device-local persistence".
- **Depends on (both landed):** #157 (architecture record) and #162 (behavior inventory — `docs/product-behavior-contract.md`).
- **Adjacent, landed:** #167 (`crates/jeliya-client`, the `ClientHandle` seam) and #176 (`crates/jeliya-ui`, which carries the **provisional** `PlatformServices` seam this issue makes canonical).
- **Owner role:** cross-platform maintainers (per the architecture layering table).
- **Status of this document:** planning/spec only. **No production code is to be written for this issue by the planning phase.**

> Where this spec and `docs/dioxus-architecture.md`, `docs/product-behavior-contract.md`, or `docs/protocol-v2.md` disagree, those records are authoritative and this spec has a bug — say which in the PR, exactly as the architecture record requires of every slice that tests against it.

---

## 1. Outcome

Deliver the **single injectable service boundary** that keeps platform authority out of shared RSX components. `PlatformServices` covers seven capability families — **files, persistence, lifecycle, URLs, clipboard/share, navigation, and window actions** — with:

1. **capability-oriented methods** whose types **distinguish platform object types** (a browser blob, a desktop filesystem path, and an Android `content://` URI are different Rust types and cannot be confused);
2. a **closed outcome taxonomy** that separates `Unavailable`, `Denied`, `Cancelled`, and operation failures, so **a cancellation can never become a success**;
3. **safe path/URL types** and an **allowlisted external-URL launcher**;
4. **explicit ownership** of preferences, secret custody, and protected/private directories, with **honest durability** (a browser tab persists nothing across reload; a silently failed write is reported, not swallowed);
5. **representable lifecycle events** — app resume, process restoration, back, navigation, and window events; and
6. a **deterministic in-process test implementation for every service**, shaped as browser / desktop / Android fakes, scriptable for denied / unavailable / cancelled outcomes.

Verification is a shared Dioxus component compiled against all three deterministic fakes for **both** `wasm32-unknown-unknown` and a native target, with **no per-component `cfg` logic**, plus per-capability behavior tests for the denied / unavailable / cancelled paths.

This becomes the sole platform-authority contract for the new stack. Target implementations (browser web-sys, desktop file dialogs, Android SAF/JNI) are **out of scope for this issue** and land in M3–M5; the clean-slate cutover applies (each target reads only new-format services and preferences; old helpers and stored values are references only).

## 2. What this issue is, and what it is not

`PlatformServices` is the **third separately injected input** to the shared UI, beside `jeliya_api` view models and the `jeliya_client::ClientHandle` seam. This issue owns:

- the **traits** (one per capability family) and the cloneable **`PlatformServices` facade** that carries them;
- the **safe value types** the methods traffic in (`PickedSource`, `ShareableBlob`, `ExportTarget`, `LocalFileRef`, `SafeExternalUrl`, `PreferenceKey`, the lifecycle event model);
- the **outcome taxonomy** (`CapabilityError` with its `Unavailable` / `Denied` / `Cancelled` / typed-failure arms, mirroring how `jeliya-client::CallError` refuses to collapse outcomes); and
- the **deterministic fakes** — the only implementations this issue ships.

This issue does **not** own (explicit non-goals, from the issue and the architecture record):

- **Implementing any real target service.** No web-sys, no `rfd`/native file dialog, no Android JNI/SAF, no `wry`/`tao` window control. Those are M3 (web), M4 (desktop), M5 (Android). This issue ships the contract and the fakes only.
- **Treating a local file path and a `content://` URI as interchangeable.** They are distinct types; a content URI is never renderable as a filesystem path (`docs/product-behavior-contract.md`).
- **Moving daemon filesystem authority into UI code.** The daemon still owns `file.share` / `file.read` and the anti-exfiltration invariant (`file.share` refuses any path outside the daemon data dir). The staging service produces a *daemon-shareable* handle; it never reads arbitrary daemon files and never mints a daemon path the component can forge.
- **Scattering target `cfg` blocks through components.** Target selection happens once, at the crate root (`compose.rs` + the per-target `bin`). The trait/type surface is `cfg`-free.

## 3. Owning crate and layout

Add two new workspace crates and add them to the single `members` line in the root `Cargo.toml` (the lane convention: every new-crate issue edits that one line): **`crates/jeliya-platform`**, the contract itself, and **`crates/jeliya-platform-implementation`**, a re-export-only door crate that is the *only* manifest permitted to enable the contract's `implementation` feature (see K4 — a Cargo feature unifies across a build graph and so cannot be a boundary there; a dependency edge can). This mirrors the architecture layering table, which lists `PlatformServices` as its own row distinct from `jeliya-ui`, and satisfies #176's recorded expectation that "when #174 lands, `jeliya-ui` adopts the canonical trait by **replacing the local seam with a re-export** — a mechanical change".

**Why a dedicated crate, not an expansion of `jeliya-ui/src/services.rs`.** The contract must compile on `wasm32-unknown-unknown` and on the workspace MSRV host job with **no renderer and no OpenSSL**, and the target implementations that follow (M3–M5) each pull platform-specific dependencies (`web-sys`, a native file-dialog crate, Android JNI) that must **never** enter the shared browser graph. A standalone trait-and-types crate keeps every target's dependencies out of `jeliya-ui`'s graph by construction and lets each target impl be its own crate later. Keeping the contract inside the renderer crate would couple it to Dioxus and is why #176 called its seam "provisional".

```
crates/jeliya-platform/
  Cargo.toml
  src/
    lib.rs          # crate docs, re-exports, boundary invariants (mirror jeliya-api/src/lib.rs)
    services.rs     # PlatformServices facade (cloneable, Arc-backed, injected separately from ClientHandle)
    error.rs        # CapabilityError, Outcome classification (Unavailable/Denied/Cancelled/typed failures)
    cancel.rs       # CancelToken + the drop-is-abort contract for dialog/copy operations
    files.rs        # Files trait; PickedSource, ShareableBlob, ExportTarget, LocalFileRef, FileObjectKind
    storage.rs      # Preferences + SecretStore + PrivateDirectory ownership/durability facts
    lifecycle.rs    # Lifecycle trait; LifecycleEvent, LifecycleSubscription (bounded fan-out)
    launcher.rs     # UrlLauncher trait; SafeExternalUrl (allowlisted-scheme constructor)
    clipboard.rs    # Clipboard + Share traits; ShareContent
    navigation.rs   # Navigation trait; Route, back-intent handling
    window.rs       # WindowActions trait; WindowEvent, WindowCommand
    fake/
      mod.rs        # deterministic in-process fakes (feature = "fake")
      recorder.rs   # call-recording + scriptable outcomes shared by the fakes
      shapes.rs     # browser / desktop / android fixture shapes (durability, availability, content-uri modelling)
  tests/
    boundaries.rs   # wasm-graph exclusion, no-serde_json::Value, no cfg forks, content-uri-is-not-a-path (CI)
    taxonomy.rs     # every method surfaces Unavailable/Denied/Cancelled distinctly; cancel never becomes Ok
    files.rs        # pick → stage (bounded, size-enforced, cancel-cleans-up) → export/open/share, per shape
    storage.rs      # preference round-trip, session-vs-persistent durability, write-honesty, custody separation
    lifecycle.rs    # resume/restore/back/navigation/window events representable and losslessly delivered
```

**`jeliya-ui` adoption (the mechanical change #176 promised).** `crates/jeliya-ui/src/services.rs` (the provisional seam and `WebPlatformServices`) is **deleted**. `jeliya-ui`'s `ui` feature gains an optional dependency on `jeliya-platform` with its `fake` feature enabled, and `lib.rs` re-exports the canonical types (`pub use jeliya_platform::{PlatformServices, …}`). `compose.rs`/`app.rs` change only where the provisional method names differ from the canonical ones (`preference`/`set_preference`/`open_url`/`write_clipboard`/`navigate` → the canonical trait methods). No component gains a `cfg` fork.

**Boundary invariants (asserted, not merely intended), mirroring `jeliya-api`/`jeliya-client`/`jeliya-ui`:**

- Crate-level `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`.
- The **library graph** (default and `wasm32`) must not pull Iroh, `jeliya-core`, `jeliyad`, `jeliya-ffi`, any WebSocket crate, a native transport, `quinn`/`rustls`/`tokio`, `wry`/`tao`, `openssl-sys`, `native-tls`, or **Dioxus**. `jeliya-platform` is renderer-agnostic; a Dioxus prop needs only `Clone + PartialEq`, which the facade provides without depending on Dioxus. Enforced by a `cargo tree` graph test and a manifest scan, exactly like `crates/jeliya-ui/tests/boundaries.rs`.
- No `serde_json::Value` token in any public signature; the crate consumes `jeliya_api` value types and never a raw JSON door.
- No `tokio`, `std::time`/`tokio::time`, or `wasm-bindgen`-specific timing in the library. Concurrency and the lifecycle fan-out are executor-agnostic and `wasm32`-safe (reuse `futures` primitives, as `jeliya-client` does).
- The **`fake` feature** is the only feature that adds `futures`; the default library build is trait/type definitions only, so the MSRV `--workspace --all-targets` job compiles it to essentially nothing (the discipline `jeliya-client`'s `mock` and `jeliya-ui`'s `ui` features already use).

## 4. Design decisions

### D1 — One cloneable facade carrying per-capability traits, injected separately from `ClientHandle`

`PlatformServices` is a concrete `#[derive(Clone)]` struct holding one `Arc<dyn …>` per capability family (or one `Arc<dyn Platform>` supertrait; see D2). Cloning is an `Arc` bump; every clone shares one implementation. `PartialEq` is pointer identity (a `clone()` handed to a child is the same services object, so a component holding it does not re-render merely because a fresh clone arrived), and `Debug` is opaque — exactly the shape `jeliya_client::ClientHandle` uses, and the exact shape a Dioxus prop needs.

It is injected **separately** from `ClientHandle` (both are distinct `AppRoot` props) and **never entangled** — the architecture states this twice (§"Decision 3": "`ClientHandle` and `PlatformServices` are injected separately"). A component that needs a call uses the handle; a component that needs platform authority uses the services; neither reaches through the other.

### D2 — Capability traits are object-safe; the facade owns typed accessors

Each capability family is an **object-safe trait** (`Files`, `Preferences`, `SecretStore`, `Lifecycle`, `UrlLauncher`, `Clipboard`, `Share`, `Navigation`, `WindowActions`) returning boxed futures where async is needed (`futures::future::BoxFuture`), so the erased implementations stay behind `Arc<dyn …>`. The facade exposes typed accessor methods (`services.files()`, `services.preferences()`, …) returning `&dyn Trait`, so components name a capability explicitly rather than reaching a god-object. A `Platform` supertrait bundles the accessors so one `Arc<dyn Platform>` can back the facade; the fakes and each future target impl implement `Platform`.

Object safety here plays the role that "backend erasure stays internal" plays for the client seam: the concrete target type never leaks into shared components, so composition can swap a browser impl for a desktop impl behind the same facade with no component change.

### D3 — A closed outcome taxonomy; cancellation is a first-class outcome (Security: "cancellation must not become success")

Every fallible capability returns `Result<T, CapabilityError>` where `CapabilityError` is a **closed** enum whose arms a component must not conflate:

```rust
pub enum CapabilityError {
    /// The platform structurally does not offer this capability (window
    /// minimize on a browser tab; a private directory on the web). NOT a
    /// failure of the user's action — a fact the UI renders as "not available
    /// here", never as an error the user caused.
    Unavailable,
    /// The user or OS refused permission (clipboard, file access, a share
    /// target). Distinct from Cancelled: the user did not dismiss, the
    /// platform said no.
    Denied,
    /// The user dismissed the picker / save dialog / share sheet, or a
    /// cancellation token fired. This is NEVER Ok and NEVER a generic failure:
    /// a cancelled export wrote nothing, a cancelled share shared nothing, a
    /// cancelled stage left no daemon-shareable blob. Callers branch on it to
    /// keep the prior state untouched.
    Cancelled,
    /// A typed operation failure with a discriminant the UI can localize.
    Failed(FailureKind),
}

pub enum FailureKind {
    /// The picked file exceeds the daemon-reported max_shared_file_bytes.
    FileTooLarge { size: u64, limit: u64 },
    /// The picked file is empty; there is nothing to share (distinct code, so
    /// a zero-byte share never reads as success — mirrors `file_empty`).
    FileEmpty,
    /// The source could not be read (vanished, unreadable, content stream
    /// closed).
    Unreadable,
    /// Persisting a preference did not reach durable storage (the setter's
    /// in-memory change still applies for this session; see D6).
    WriteNotDurable,
    /// A platform I/O error with no more specific typed meaning.
    Io,
}
```

This mirrors `jeliya_client::CallError`'s philosophy: a closed set of failure classes, no outcome silently collapsed, and a rendered form that carries a **discriminant and no payload identifiers** (§K1). The three "the action did not happen" reasons — `Unavailable`, `Denied`, `Cancelled` — are kept apart because they render as three different truths and drive three different UI branches.

### D4 — Files: safe types that distinguish platform object kinds (AC-1)

The files capability never traffics in raw strings. Five distinct types carry the distinct object kinds a file surface touches, and the one *name* type is validated rather than merely promised: **`FileName`** is constructed only through `FileName::parse(...) -> Result<Self, InvalidFileName>`, which fails closed on an empty name, on `.`/`..`, on any `/` or `\` separator, and on control characters — the same discipline `SafeExternalUrl::parse` uses for schemes. A peer-supplied name therefore cannot carry path syntax into a native sink's artifact naming, and the error carries no payload (K1). The type guarantees exactly one thing — the name cannot navigate directories — and deliberately leaves Unicode normalization, Windows reserved names, trailing dots, and length caps to the platform sink.


- **`PickedSource`** — an **opaque handle** to a user-selected source, produced by `pick()`. It carries display metadata (`display_name: FileName`, `size: u64`, `mime: Option<Mime>`) and an internal, private `FileObjectKind`:
  - `BrowserBlob` (a browser `File`/blob reference — bytes reachable only via the platform, never a path),
  - `NativePath` (a desktop filesystem path, held in a `NativeFilePath` newtype),
  - `ContentUri` (an Android `content://` URI, held in a `ContentUri` newtype).
  The `FileObjectKind` is inspectable (`kind() -> FileObjectKind` discriminant) so tests and the staging step can assert the shape, but the **underlying path/URI is not exposed to shared components** — a component cannot read a `content://` URI as a path because it cannot reach either spelling. This is the type-level enforcement of "a local file path and a `content://` URI are not interchangeable".
- **`ShareableBlob`** — a daemon-shareable handle produced **only** by `stage_for_share()` (D5). It is the sole value `file.share` accepts. Components cannot construct one from a path, so the daemon's anti-exfiltration invariant is preserved at the type level (the UI cannot forge a daemon path).
- **`FetchedArtifact`** — the inbound mirror, produced **only** by committing a `ShareSink` (below): a handle to bytes the *service* custodies after the UI pumped a `file.read` stream into it, and the sole file value the platform share sheet accepts. Deliberately a distinct type from `ShareableBlob` so the two directions cannot be crossed — a fetched artifact must never flow into `read_staged` or the daemon's `file.share`. A successful share consumes it ("delete after share"), so re-sharing the handle fails closed.
- **`ExportTarget`** — a destination for a fetched file, produced by `pick_export_target()`. Internally `BrowserDownload` (a suggested filename; the browser owns the destination), `NativePath` (a save-dialog path), or `AndroidDocument` (a SAF document `content://` write target). Distinct from `PickedSource`: an export target is a place to *write*, a picked source is a place to *read*.
- **`LocalFileRef`** — a typed reference to a room file, `(RoomId, FileId)` (reusing `jeliya_api` ids) — the identifier pair the UI feeds to the client seam's `file.read`. It is an identity, not an attachment: sharing a room file means pumping its bytes through `share_sink` into the service's own custody first (D9), so the platform never has to resolve an id it cannot read. It is deliberately not a URL: in protocol v2 the `GET /api/files/local` HTTP edge is retired (file bytes travel the byte-stream framing), so no token-carrying URL exists and none is resolved (D9).

The `Files` trait methods:

```rust
fn pick(&self, ct: &CancelToken) -> BoxFuture<'_, Result<Option<PickedSource>, CapabilityError>>;
fn stage_for_share(&self, src: PickedSource, limit: u64, progress: ProgressSink, ct: &CancelToken)
    -> BoxFuture<'_, Result<ShareableBlob, CapabilityError>>;
fn read_staged(&self, blob: &ShareableBlob)
    -> BoxFuture<'_, Result<Box<dyn StagedBlobReader>, CapabilityError>>;
fn pick_export_target(&self, suggested: FileName, ct: &CancelToken)
    -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>>;
fn export_sink(&self, to: ExportTarget, ct: &CancelToken)
    -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>>;
fn open_sink(&self, name: FileName, declared: Option<Mime>, ct: &CancelToken)
    -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>>;
fn share_sink(&self, name: FileName, declared: Option<Mime>, ct: &CancelToken)
    -> BoxFuture<'_, Result<Box<dyn ShareSink>, CapabilityError>>;
fn share_content(&self, content: ShareContent, ct: &CancelToken)
    -> BoxFuture<'_, Result<(), CapabilityError>>;
```

Bytes cross the boundary through three small object-safe traits, never through paths or URLs: **`StagedBlobReader`** (`size()`, `next_chunk(max_len)`) is the pull side — the v2 `file.share` upload pulls exactly what CREDIT permits, one bounded chunk per DATA frame it may send; **`FileSink`** (`write(chunk)`, `commit()`) is the push side — the UI drives the client seam's `file.read` and pumps DATA chunks in, advancing its stream credit only as each `write` resolves (the protocol's receiver-driven credit rule), and dropping an uncommitted sink deletes the partial artifact (D12/K2). **`ShareSink`** (`write(chunk)`, `commit() -> FetchedArtifact`) is the same push shape aimed at the *service's own* staging custody: `share_sink(name, declared, ct)` is how a fetched room file becomes shareable, since a share sheet needs bytes and the platform service has neither a `ClientHandle` nor a URL (K5/K11). It is a separate trait because `FileSink::commit` yields nothing and an export/open sink must not be able to fabricate an attachable artifact. `open_sink`'s and `share_sink`'s `declared` is the peer-declared content type — an untrusted hint, never a trust decision. The handoff contract: the UI adapts `Box<dyn StagedBlobReader>` to the client seam's upload input in `jeliya-ui` composition, keeping K11 (no shared type between the two facades); note that the client-side upload input (#269's depth) must accept `!Send` byte sources, because platform futures are `!Send` by design.

`pick`/`pick_export_target` return `Ok(None)` **only** where the platform reports a clean no-selection that is not a user dismissal (rare); an actual user dismissal is `Err(Cancelled)`. The two are kept distinct so a caller never treats a dismissed picker as "no files exist". (The fakes make the distinction explicit and testable.)

### D5 — Staging is bounded, size-enforced, cancel-cleans-up, and daemon-authoritative (Security)

`stage_for_share` is the one method that turns a `PickedSource` into a `ShareableBlob`. Its contract encodes the native staging convention (`dart/jeliya_protocol/lib/src/daemon_http.dart`) and the Android streaming requirement (issue: "stream/copy Android content through bounded staging"):

- **Size is enforced against the daemon-reported `max_shared_file_bytes`**, passed in as `limit` (from `jeliya_api`'s server-limits view model — never a constant redefined here; the 100 MiB figure lives once, in protocol v2). A known-oversize `PickedSource` fails with `FileTooLarge` **before any copy**; a streamed source (`content://`, where the size is not known up front) is copied through a **bounded buffer** and aborts with `FileTooLarge` the instant the running total would exceed `limit`. Zero bytes fails with `FileEmpty`.
- **The copy is bounded and cancellable.** It reads the content stream in bounded chunks (never buffering the whole file in memory), reports progress on `ProgressSink`, and observes the `CancelToken`. On cancel or any failure it **deletes the partial staged file** and returns `Cancelled` / `Failed`, never `Ok` — the exact `finally { delete }` honesty the Dart convention has (a failed or cancelled share must not leak bytes into the data dir).
- **The daemon stays authoritative.** The staging service produces a handle to bytes it holds; it does not itself call `file.share` (that is the client seam's `file.share` operation) and it does not read daemon files. The daemon re-enforces the size limit and its anti-exfiltration invariant on `file.share` regardless of what the service produced. On the browser, "staging" is holding the picked blob client-side until the v2 upload pulls it through `read_staged` — the v1 server-side `POST /api/files/share` edge is a legacy reference only (protocol-v2.md: `file.share` combines v1's RPC and its separate HTTP upload edge).

Per-platform mechanism (for the fakes' shapes and the M3–M5 impls; **not** implemented here beyond the fakes). Export and open consume the v2 `file.read` byte stream through a `FileSink` on every platform; only the sink's destination differs:

| Platform | pick source | stage_for_share | export sink | open sink |
|---|---|---|---|---|
| Browser | `<input type=file>` → blob | hold the blob client-side; upload pulls via `read_staged` | browser download | object-URL tab on commit |
| Desktop | native file dialog → path | copy path → staging area, delete after share | save-dialog path | OS viewer on the committed temp file |
| Android | SAF picker → `content://` | stream `content://` → protected staging dir, bounded, delete after share | SAF create-document → `content://` | OS viewer on the committed copy |

Materializing a room file for the share sheet (turning a `(RoomId, FileId)` into a shareable local artifact) happens through `share_sink`: the UI drives `file.read` and pumps DATA in, and `commit` produces the `FetchedArtifact` that `ShareAttachment::Fetched` carries. Only the per-target *staging mechanism* is deferred to M3-M5 (Android protected staging dir, desktop temp dir, browser in-memory blob); the contract surface for it is not, because a target crate implements this trait set and cannot add methods to it. Stating it here keeps it from being smuggled back in through a URL.

### D6 — Storage: preferences, secret custody, and private-directory ownership are explicit (AC-2), with honest durability

Three **separate** storage concerns, deliberately not merged:

- **`Preferences`** — a namespaced, non-secret key/value store for device-local UI state (last room, per-room drafts, aliases incl. self label, pin/archive flags, last-seen marks, text locale, formatting locale — two independent rows, per the product contract's "text locale != formatting locale from day one"). Keys are a typed `PreferenceKey` (a validated newtype over an allowlisted key set, so a component cannot write an arbitrary or legacy key). Methods: `get(key) -> Option<String>`, `set(key, value) -> WriteOutcome`, `remove(key) -> WriteOutcome`. `WriteOutcome` is `{ Durable, SessionOnly }` (or a `Result` whose error is `WriteNotDurable`): a write that does not reach durable storage still applies **for this session** — the setter's in-memory change stands — but the caller learns it was not written, so the UI can honestly say "applies this session, not saved" instead of implying a false success. This is the `PrefsStore.lastWriteOk` honesty (`app/lib/src/session/prefs_store.dart`) lifted into the type.
- **Durability is a queryable platform fact.** `Preferences::durability() -> Durability` returns `SessionScoped` (an ordinary browser tab — nothing survives reload; held in memory, gone with the tab) or `Persistent` (packaged desktop / Android — survives restart). The product contract forbids the UI from pretending a browser reload restored state; making durability a fact the UI reads is how "the UI must never pretend otherwise" is enforceable. The persisting platforms' key namespaces are **not named here** (they belong to #178 web, #185 desktop, #173 Android); the contract carries no legacy key name.
- **`SecretStore`** — custody of the only browser-held secrets (the tab-scoped session credential and its tickets), kept separate from `Preferences` so credential material never lands in the non-secret store, and so a diagnostics/export path that reads preferences can never read secrets. On the browser these are session-scoped and die with the tab; the daemon token never enters this store on the untrusted side (§K5, the token stays native).
- **`PrivateDirectory` ownership facts** — the protected/backup-excluded directory the daemon owns (Android `<noBackupFilesDir>/engine` via the `protectedEngineDataDir` platform channel; desktop app-support). Exposed as **capability facts** (`is_backup_excluded`, `is_owned_by_daemon`, availability), **not** as a raw path the UI can traverse — moving daemon filesystem authority into UI code is an explicit non-goal. On the browser this capability is `Unavailable`.

### D7 — Lifecycle: app resume, process restoration, back, navigation, and window events are representable (AC-3)

A `Lifecycle` capability exposes a **bounded, multi-consumer subscription** of `LifecycleEvent`, reusing the loss-visible fan-out philosophy of `jeliya_client::EventBus` (a slow consumer that missed events is told so; nothing is silently dropped). The event model:

```rust
pub enum LifecycleEvent {
    /// Foreground/usable. Only this is truly foreground; every other phase is
    /// background (mirrors `AppLifecycleState.resumed`).
    Resumed,
    /// Left the foreground, carrying which phase (Inactive/Paused/Hidden/Detached)
    /// so a consumer can distinguish "obscured" from "backgrounded".
    Backgrounded { phase: BackgroundPhase },
    /// The OS restored a previously killed process; any in-memory-only state is
    /// gone and the client must re-establish authoritatively (Android). This is
    /// NOT a reconnect — it is a fresh process that must resync.
    ProcessRestored,
    /// A system/predictive Back intent. The shared router consumes it and
    /// answers from the route; Back never mutates unseen state, it only changes
    /// where the user is standing (`app/lib/src/screens/shell.dart::_back`).
    BackRequested,
    /// A platform navigation intent (deep link / external route change).
    NavigationRequested { route: Route },
    /// A window event on platforms that have windows (desktop): focus, blur,
    /// resize, close-requested. `Unavailable` platforms never emit these.
    Window(WindowEvent),
}
```

Ownership rules the contract fixes:

- **Back and exit are the router's / platform's decision, surfaced as an intent, never auto-handled by the service.** The service delivers `BackRequested`; the shared router decides whether to pop an in-app destination or hand the gesture back to the platform (an explicit `WindowActions::request_exit()` / `Navigation::hand_back_to_platform()`), exactly as the Flutter `PopScope` policy does. A `BackRequested` that the router does not consume must **not silently become an exit** — the exit is a separate, explicit action.
- **`ProcessRestored` triggers authoritative resync, not a fabricated reconnect** — this is the honest Android `DirectClient` lifecycle difference the architecture already fixed (§"Decision 4"); the lifecycle capability names it so the shared UI can drive a `stream.resync` rather than pretend a socket reconnected.
- **Lifecycle delivery is loss-visible.** The subscription is bounded; an overflowed consumer learns it lagged (as with the client event bus). **Control intents that must never be silently lost or reordered** — `BackRequested`, `ProcessRestored`, and terminal window events — survive overflow losslessly *and* boundedly: a saturated mailbox run-length-encodes a Back burst (every Back still delivered, one per poll), and a close/restore restated while the identical intent is still undelivered is absorbed into the pending one (a lost Back that silently exits, or a dropped restore that skips resync, would be exactly the honesty failure the clean-slate generation removes — and an unbounded mailbox would be its own failure).

### D8 — URLs: an allowlisted safe launcher (AC-4)

`UrlLauncher::open_external(SafeExternalUrl)` opens a URL through the platform (a new browser tab; `launchUrl(..., externalApplication)` on native). `SafeExternalUrl` is a **validated newtype** constructed only via `SafeExternalUrl::parse(&str) -> Result<Self, UnsafeUrl>`, which **fails closed** on any scheme outside a fixed allowlist (`https`, `mailto`, `tel`; `http` intentionally excluded or explicitly justified). It rejects `javascript:`, `data:`, `file:`, `content:`, and unknown schemes — the same discipline the Slack/canvas link rules and browser security guidance encode. Components cannot call `open_external` with a raw string, so an un-vetted URL cannot reach the platform launcher. A **failed launch is never a success**: `open_external` returns `Result`, and the UI surfaces the miss (settings_panel records `url_launch_failed` today and tells the user their diagnostics are already on the clipboard — that honesty is preserved by returning the failure).

### D9 — Clipboard and share; the daemon token stays native

- **`Clipboard::write_text(&str) -> BoxFuture<Result<(), CapabilityError>>`** — asynchronous, because the browser's `navigator.clipboard.writeText` settles a promise and a permission denial is that promise's rejection: a synchronous signature could only fire-and-forget and falsely report `Ok`. The future resolves to `Err`, never `Ok`, on failure; the UI must not read a failed copy as success (`settings_panel.dart` clears the "copied" flag and raises a manual-copy note on failure). Clipboard *read* is out of scope unless a required behavior needs it (none in the inventory); if added later it is a separate method with its own `Denied` path.
- **`Share::share(ShareContent) -> Result<(), CapabilityError>`** — the OS share sheet (invite tickets via `SharePlus`; Android file share). `ShareContent` carries text and/or a `ShareableBlob`/`FetchedArtifact` — in both cases a handle to bytes the producing service already custodies, never a bare id or path — plus an optional anchor rect for iPad popover presentation. Dismissing the share sheet is `Cancelled`, not `Ok`.
- **No token-carrying URL exists in v2 (§K5).** File bytes travel the byte-stream framing (`file.read`), pumped into `export_sink`/`open_sink` sinks — the retired `GET /api/files/local` HTTP edge is gone, so there is no URL for a service to resolve and no place a token could ride one. The daemon-token half of K5 is enforced by `SecretStore` custody alone: the token never appears on any public type, and `LocalFileRef` stays a typed `(room_id, file_id)` reference, never a URL string.

### D10 — Navigation: the route is the navigation state

`Navigation` exposes `route() -> Route`, `navigate(Route)`, and consumes `BackRequested`/`NavigationRequested` from the lifecycle stream. `Route` is a typed, parsed route (not a raw string), so the router and the platform agree on one state machine — the product contract's "the URL *is* the navigation state; no second state machine may disagree with it". On the browser the impl is the history/hash route; on desktop/Android an in-memory router. The last-open-room restore (D6 preference) is applied **once per launch, only from the bare root**, and always loses to an explicit route — a rule the navigation + preferences capabilities express together, not a component `cfg`.

### D11 — Window actions (desktop), `Unavailable` elsewhere

`WindowActions` (`minimize`, `set_title`, `request_close`, `request_exit`, `focus`) is meaningful on packaged desktop and returns `Unavailable` on browser and Android — a **structural fact the fakes model per shape**, not a `cfg` fork in a component. A component that offers a window control renders it as absent where the capability is `Unavailable`, driven by the returned outcome, never by `cfg(target_os=…)`.

### D12 — Cancellation model (`CancelToken`, drop-is-abort)

Every method that opens a user dialog or runs a bounded copy takes a `&CancelToken`. Firing the token, **or dropping the returned future**, aborts the operation, runs its cleanup (delete a partial staged file, dismiss a dialog if the platform allows), and yields `Cancelled`. The token is executor-agnostic (a shared flag + waker, `wasm32`-safe). This is the mechanism behind the Security requirement "cancellation must not become success": an aborted operation has one settled outcome, `Cancelled`, and it is not `Ok`.

## 5. Key invariants (K-numbered, asserted by tests where notes say "test")

- **K1 — Closed, redacting outcomes.** `CapabilityError` is exhaustive; its `Display`/`Debug` render a discriminant and no path/URI/identifier payload (a staged filename or a `content://` URI must not leak into an error string — mirror `CallError`'s §K15). *(test: taxonomy.rs)*
- **K2 — Cancellation is not success and not a generic failure.** For every cancellable method, a fired token and a dropped future both yield exactly `Err(Cancelled)`; a staged copy leaves no partial blob behind. *(test: taxonomy.rs, files.rs)*
- **K3 — `Unavailable` ≠ `Denied` ≠ `Cancelled`.** Each is distinctly observable per capability and per fake shape (window actions `Unavailable` on browser; a scripted permission refusal `Denied`; a dismissed picker `Cancelled`). *(test: taxonomy.rs)*
- **K4 — Object kinds are non-interchangeable, and handles are unforgeable.** A `content://` `PickedSource` never exposes a filesystem path; attempting to spell a path from a content URI does not compile. `ShareableBlob`/`FetchedArtifact` are not constructible by shared components — but that claim is **two-tier**, because Cargo unifies features per package across a build graph:
  - In any graph without an implementation crate (default, `fake`, every CI job, `jeliya-ui` alone) the factory module is not compiled and the types are literally unconstructible. *(test: the `compile_fail` doctest at the crate root)*
  - In a unified target binary the module IS compiled in, so three other things carry the boundary: the factories are **path-addressed free functions** (no inherent method reachable by bare method syntax), so a call site must spell `implementation` — scanned for across every non-door workspace member; `crates/jeliya-platform-implementation` is the **sole** manifest allowed to enable the feature, and the shared UI's graph is asserted to have no **dependency edge** to it (edges do not unify); and every minted-token registry **fails closed** on a token it did not mint, with fake tokens carrying issuer provenance so even another live service's genuine handle fails. *(test: boundaries.rs workspace manifest + code scan with a red-fixture self-test, `cargo tree` edge test; capabilities.rs cross-service and forged-token cases)*
- **K5 — The daemon token stays native.** No public type carries the daemon token, and no token-carrying URL exists for it to ride (the v1 local-file HTTP edge is retired in v2 — file bytes travel the byte stream); custody is `SecretStore`'s alone, and the token never appears in component-facing state. *(test: boundaries.rs source scan for the retired URL edge; review)*
- **K6 — Durability honesty.** `Preferences::durability()` reports `SessionScoped` on the browser fake and `Persistent` on the desktop/Android fakes; a scripted non-durable write returns `SessionOnly`/`WriteNotDurable` while the in-memory value still reads back this session. *(test: storage.rs)*
- **K7 — Size enforced against the daemon limit, before and during copy.** `stage_for_share` fails `FileTooLarge` before copy for a known-oversize source and mid-copy for a streamed one; `FileEmpty` for zero bytes; the limit is the passed-in daemon value, never a redefined constant. *(test: files.rs)*
- **K8 — Lifecycle intents are lossless and bounded.** Every `BackRequested` is delivered (a saturated mailbox run-length-encodes a Back burst rather than growing without bound, and delivers it one Back per poll); `ProcessRestored` and terminal window events are delivered distinctly, a restatement absorbed only while the identical intent is still undelivered; an overflowed consumer is told it lagged rather than silently starved. The mailbox never exceeds its capacity plus a fixed control allowance. *(test: lifecycle.rs)*
- **K9 — Allowlisted launcher fails closed.** `SafeExternalUrl::parse` rejects every non-allowlisted scheme; `open_external` returns its failure rather than swallowing it. *(test: launcher path in taxonomy.rs)*
- **K10 — No platform `cfg` forks in shared code.** The trait/type surface carries no `cfg(target_…)`/feature fork; the extended `no_cfg_target_forks_in_shared_components` scan (now covering `jeliya-ui` components consuming the services) passes. *(test: boundaries.rs; jeliya-ui boundaries.rs)*
- **K11 — Injected separately.** `PlatformServices` and `ClientHandle` are distinct `AppRoot` props with no shared type; neither is reachable through the other. *(review + app.rs signature)*
- **K12 — Clean-slate.** No contract type names a legacy key (`jeliya.lastRoom`, `app_prefs.json`, `jeliya.aliases.v1`, the retired staging dir), and no fake reads legacy storage. Old helpers/values are references only. *(test: boundaries.rs string scan; review)*

## 6. Deterministic test implementation (AC-6, the only code this issue ships)

`crates/jeliya-platform/src/fake/` provides one deterministic, in-process implementation of every capability, plus three **target-shaped fixtures**:

- **A shared `Recorder`** captures every effect (opened URLs, clipboard writes, shares, navigations, staged blobs with their bytes, preference writes) in order, for assertions — the pattern `jeliya-ui`'s `WebPlatformServices` already uses (`opened_urls()`, `last_clipboard()`), generalized.
- **Scriptable outcomes.** Any capability method can be scripted to return `Unavailable` / `Denied` / `Cancelled` / a typed `Failed`, so the denied/unavailable/cancelled paths the Verification section demands are exercisable without a device.
- **Three shapes (`fake::shapes`)** so shared components compile and behave against each target's structural facts:
  - **Browser fake** — `Preferences::durability() == SessionScoped`; `PrivateDirectory`/`WindowActions` `Unavailable`; `PickedSource` kind `BrowserBlob`; staging models the server-side upload (bytes recorded, no local staging dir).
  - **Desktop fake** — `Persistent`; window actions available; `PickedSource`/`ExportTarget` kind `NativePath`; staging models copy-into-uploads-then-delete (a partial-then-cancel leaves the recorder's staging set empty).
  - **Android fake** — `Persistent`; a protected, backup-excluded `PrivateDirectory`; `WindowActions` `Unavailable`; `PickedSource` kind `ContentUri`; staging **streams** a scripted `content://` byte source through the bounded copy, enforcing the limit mid-stream, and `ProcessRestored` is emittable.
- **Determinism** matches the mock's discipline: no wall clock, no RNG, no reliance on task-scheduling order; a scripted picker/dialog/share resolves only when the test advances it (a controller handle, like `MockController`), so cancellation-vs-reply ordering is explicit, not a race. `wasm32`-safe throughout.

## 7. Verification and test strategy

- **Shared-component compile, both targets, three shapes.** A `jeliya-platform` example (or a `jeliya-ui` test component) uses files/urls/clipboard/lifecycle/navigation through the injected `PlatformServices`, and is compiled and driven against each of the browser/desktop/Android fakes for **both** `wasm32-unknown-unknown` and a native target, with **no `cfg` in the component** — the direct analogue of #167's shared-component two-target check.
- **Per-capability behavior tests** (`tests/…`) map each AC and K-invariant, with explicit denied / unavailable / cancelled cases per capability (Verification: "test denied/unavailable/cancelled capabilities").
- **Boundary tests** (`tests/boundaries.rs`) — the `cargo tree` wasm-graph exclusion, the manifest scan, the no-`serde_json::Value` scan, the extended no-`cfg`-fork scan, and a compile-fail (`trybuild`, optional) proving a `content://` source cannot be read as a path and a `ShareableBlob` cannot be forged.
- **`jeliya-ui` adoption tests** stay green after the seam is replaced by the re-export (the existing `services.rs` tests move/adapt to `jeliya-platform`; `compose.rs`/`app.rs` compile against the canonical methods).
- **Focused local gate:** `cargo test -p jeliya-platform` and the `jeliya-ui` boundary tests; the workspace `cargo check --locked --workspace --all-targets` (MSRV) must stay renderer-free and OpenSSL-free with the new crate present. Reserve a full workspace test+clippy for the final review.

## 8. Documentation changes

- `docs/dioxus-architecture.md`: flip the #174 slice row from **Planned** to **Landed**, and update the `jeliya-ui` note that today calls its `PlatformServices` a "provisional seam pending #174" to record that the canonical `jeliya-platform` contract has landed and `jeliya-ui` re-exports it. Frontmatter must stay within the 10-field docs profile.
- `docs/product-behavior-contract.md` §"PlatformServices": no rule changes (this issue implements those rules), but add a pointer to `crates/jeliya-platform` as the contract's home.
- `docs/known-gaps-roadmap.md` / `CHANGELOG.md`: record the new crate, the retirement of the provisional seam, and that target implementations remain M3–M5.
- New crate `README.md`/`lib.rs` docs mirroring `jeliya-api`/`jeliya-client`: the boundary invariants and the "where this crate and the record disagree, the record wins" clause.

## 9. Acceptance criteria → where satisfied

| AC | Satisfied by |
|---|---|
| File pick/stage/export/open/share contracts distinguish platform object types | D4 (`PickedSource`/`FileObjectKind`, `ShareableBlob`, `ExportTarget`, `LocalFileRef`), D5, K4 |
| Preferences and secure/private directory ownership are explicit | D6 (`Preferences` + `Durability`/`WriteOutcome`, `SecretStore`, `PrivateDirectory` facts), K6, K12 |
| App resume/process restoration/back/navigation/window events are representable | D7 (`LifecycleEvent`), D10, D11, K8 |
| External URLs use an allowlisted safe launcher | D8 (`SafeExternalUrl`, fail-closed), K9 |
| Shared components contain no platform business-logic `cfg` forks | D1/D2 (facade + object-safe traits, composition at the root), K10; extended `jeliya-ui` boundary scan |
| Every service has a deterministic test implementation | §6 (the `fake` module + three shapes, scriptable), §7 |

## 10. Risks and mitigations

- **Over-modelling window/lifecycle events the first targets never emit.** Mitigation: the event model is a closed enum the fakes drive; a target that cannot produce an event simply never emits it, and window actions report `Unavailable` — no speculative capability method is added without an inventory behavior behind it.
- **The `content://`-vs-path type distinction leaking through a convenience accessor.** Mitigation: the internal `FileObjectKind` path/URI fields are private; only a discriminant is public; a compile-fail test pins that no path can be spelled from a content source.
- **Staging memory blow-up on large `content://` streams.** Mitigation: D5 mandates a bounded chunked copy with mid-stream limit enforcement; the Android fake exercises a streamed source to prove the copy never buffers the whole file.
- **Durability dishonesty regressions in later target impls.** Mitigation: `Durability`/`WriteOutcome` are part of the contract and asserted per shape now; a target impl that persists on the browser, or reports a failed write as durable, fails K6.
- **Scope creep into real target services.** Mitigation: the issue's non-goals are restated in §2; this issue ships only traits, types, and fakes; M3–M5 own the real impls behind the unchanged facade.
- **The bundled-`Arc<dyn Platform>` supertrait forcing every fake/impl to implement all capabilities.** Mitigation: acceptable and intended — a target that lacks a capability implements it as `Unavailable`, which is the very fact the UI needs; there is no partial `PlatformServices`.

## 11. Open questions

- **O-1 — One `Arc<dyn Platform>` supertrait vs. one `Arc<dyn …>` per capability in the facade.** The supertrait keeps one injection point and matches "every service has a deterministic implementation" (there is no half-services object); per-capability `Arc`s allow finer composition. This spec assumes the **supertrait** (D2). Confirm with the cross-platform maintainers before implementation.
- **O-2 — `http` in the external-URL allowlist.** D8 excludes plain `http` by default (the product is loopback-first and external links are `https`/`mailto`/`tel`). Confirm no required behavior opens a plain-`http` link before finalizing the allowlist.
- **O-3 — Clipboard read.** The inventory only writes to the clipboard. If a paste-from-clipboard behavior is required (e.g. paste an invite ticket), add `Clipboard::read_text` with its own `Denied` path; otherwise leave it out (smaller attack surface).
- **O-4 — Lifecycle fan-out reuse.** Whether to depend on a small shared fan-out extracted from `jeliya-client::EventBus` or to carry a local bounded broadcast in `jeliya-platform`. This spec assumes a **local** executor-agnostic bounded broadcast (no dependency on `jeliya-client`, keeping the platform crate free of the client seam); confirm the duplication is acceptable versus extracting a shared primitive.
- **O-5 — Crate name.** `jeliya-platform` is proposed. If the maintainers prefer `jeliya-platform-services` or keeping the contract inside `jeliya-ui` behind a stable re-export, the layout in §3 adjusts, but the dedicated crate is recommended for the wasm-graph and target-dependency isolation reasons in §3.
