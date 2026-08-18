# Dioxus/Web — Files, Pipes, and the browser file/share PlatformServices bindings (#181)

**Issue:** #181 `[Dioxus][Web]: Port Files, Pipes, and browser PlatformServices`
**Program:** #156 (Dioxus clean-slate). **Milestone:** M3 (shared web foundation) — the room-destination content slices that ride on top of the #178 shell.
**Blocked by / depends on:** #178 (shell/routing/browser prefs — merged; provides `platform_web::WebPlatform`, the `RoomDest::Files`/`RoomDest::Pipes` routes, and the room-shell skeleton this issue fills), #174 (`jeliya-platform` `PlatformServices` — merged; provides the `Files`/`Share`/`Clipboard`/`UrlLauncher` contracts and the `jeliya-platform-implementation` factory door), #171 (`WsWeb` browser session adapter — the real `ClientHandle` transport; **not yet in this worktree**), #92 (shared-file size policy — decided, `docs/shared-file-size.md`), #79 (authoritative presence semantics).
**Authoritative product contract:** `docs/product-behavior-contract.md` §"Required destinations" (Files/Pipes rows), §"Status vocabulary and truthful states" (the **Pipe** and **Peer reachability** rows, and invariant 4 — presence ≠ provider availability ≠ pipe reachability), §"Canonical routes" (rule 6).
**Protocol authority:** `docs/protocol-v2.md` §`file.share`/`file.list`/`file.fetch`/`file.read`/`transfer.cancel` and §`pipe.publish`/`pipe.list`/`pipe.connect`/`pipe.release`/`pipe.revoke`, and §"Byte-stream framing".
**Size policy:** `docs/shared-file-size.md` (#92 — 100 MiB, **served not assumed**, distinctive over-limit error, preflight, no false accusation).
**Architecture record:** `docs/dioxus-architecture.md` (Decision-3 layering / no-`cfg`-in-shared-components, Decision-4 one-seam-four-adapters, Decision-7 the PlatformServices boundary).
**Status:** SPEC — not yet implemented. This document is a build plan; it changes no production code.

> Where this spec and `docs/product-behavior-contract.md`, `docs/protocol-v2.md`, `docs/shared-file-size.md`, or `docs/dioxus-architecture.md` disagree, those records are authoritative and this spec has a bug — say which in the PR, exactly as the architecture record requires of every slice that tests against it.

---

## 1. Outcome and scope

Deliver the browser **Files** and **Pipes** room destinations: file pick → bounded staged upload (`file.share`), the file list with **evidence-backed** provider availability, fetch → local read → export/download (`file.fetch` + `file.read`), safe external preview handling, transfer cancellation and progress, and the full Pipe lifecycle (publish/expose, list, connect, release, revoke) with truthful reachability. All of it flows through **two already-typed seams** — `jeliya_client::ClientHandle` for daemon operations and `jeliya_platform::PlatformServices` for platform authority (pickers, staging, export, share sheet, clipboard) — and through the **real browser bindings** for the file/share capabilities that #178 left delegated to the deterministic fake.

Two things make this issue more than "render two panes":

1. **Truthful, non-inferred availability.** Membership presence, file-provider availability, and Pipe publisher reachability are **three distinct protocol facts** (#50/#79/#94, contract invariant 4). This issue reads each from its own protocol source (`room.peers`, `file.list` provider rows / `fetchable` / `self_hosted`, `pipe.list` `link` + `connected`) and **never infers one from another**. A reachable provider serves a file regardless of a stale display state; a genuinely unavailable provider fails honestly; a reachable authorized Pipe owner serves the Pipe, while a genuinely offline owner yields a distinctive `pipe_unreachable`.
2. **Confinement retained without a path in sight.** v2 removed every filesystem path from the file protocol (`file.share.path`, `file.fetch.save_dir`, `file.list.local_path`). The #122 destination-escape and symlink regressions (fixed upstream at `8cc24fe`) are retained **structurally** by this stack: bytes cross the boundary through capability sinks, never a path or URL the UI can forge, and `FileName::parse` fails closed on separators, dot-segments, and control characters. This issue adds the coverage that proves the confinement holds in the new stack, independent of the closed issue.

### In scope

- The browser real implementations of the **file/share PlatformServices** capabilities #178 stubbed: `WebFiles` (pick/stage/export/open/share sinks), `WebShare` (OS/Web Share sheet where available, else `Unavailable`), `WebClipboard` (`navigator.clipboard`), `WebUrlLauncher` (allowlisted external open). These mint the `PickedSource`/`ShareableBlob`/`ExportTarget`/`FetchedArtifact` handles through the `jeliya-platform-implementation` factory door.
- Shared, host-testable **Files and Pipes orchestration** modules in `jeliya-ui` that drive the `ClientHandle` operations and the `PlatformServices` capabilities: the upload state machine, the file-list model with truthful availability, the fetch→read→export state machine, cancellation, progress, and the pipe lifecycle model.
- The **Files** and **Pipes** room-destination **components** that replace the #178 skeleton panes at `/rooms/:roomId/files` and `/rooms/:roomId/pipes`, including the optional selected-item sub-segment (`RoomDest::Files { item }` / `RoomDest::Pipes { item }`, #67), responsive across compact/medium/wide, EN + FR, with the a11y floors.
- The **v2 maximum-file-size** enforcement and its distinctive, *explained* over-limit surface (from the served `Limits.max_shared_file_bytes`, never a compiled-in constant, never a baked catalog number).
- **Safe external preview**: peer-declared `declared_content_type` is treated as untrusted; peer bytes are never inline-rendered on its strength; any external open goes through the allowlisted `UrlLauncher`.
- **Cancellation** that reaches both the local copy (`PlatformServices` `CancelToken`) and the wire transfer (`ClientHandle` stream ABORT / `transfer.cancel`), with `Cancelled` never collapsing to `Ok`.
- **Diagnostics** for the Files/Pipes flows that carry a discriminant and no bytes, path, URI, bearer token, or full identity.
- l10n catalog additions (EN/FR parity, compile-enforced) and the Playwright + `wasm-bindgen-test` suites (mock and real daemon) the Verification section requires.

### Explicitly out of scope (non-goals, from the issue)

- **Native filesystem paths.** No path appears in any request, sink, handle, or component prop. `PlatformServices` owns paths; the protocol carries none.
- **Inferring provider or Pipe availability from membership.** Never derive "can serve" from roster/presence display state.
- **Expanding daemon file authority.** The daemon keeps `file.share`/`file.read` and the anti-exfiltration invariant; this issue never reads arbitrary daemon files nor mints a daemon path.
- **Reintroducing an unrestricted `save_dir`.** Export destinations are opaque `ExportTarget` handles the browser owns; the UI cannot name a write path.
- **Solving underlying network/presence semantics inside UI code.** Reachability truth is #79/#168's; this issue *renders* it, it does not compute it.
- The **desktop/Android** file pickers, SAF, and native share sheets (#184/#192) — those target crates implement the same `PlatformServices` traits behind their own bindings.
- **Chunked/resumable transfer** (a `file.share`/`file.read` stream is a single bounded non-resumable byte sequence per protocol §"Byte-stream framing").
- The room **Activity/People/Agents** panes (#179/#180) — this issue touches only the Files and Pipes destinations of the already-routed room shell.

### Platform applicability

**Web only.** Every orchestration unit (upload state machine, file-list availability model, fetch/export state machine, pipe lifecycle) is written **target-agnostic** against the two injected seams, so the desktop (#184) and Android (#192) Files/Pipes slices reuse the shared modules behind their own `PlatformServices` bindings. Only the browser bindings (`WebFiles`/`WebShare`/…) and the `web`-gated composition are web-specific.

---

## 2. What already exists vs. what #181 builds

| Concern | Already defined / merged | #181 builds |
|---|---|---|
| File capability contract | `jeliya_platform::files::{Files, PickedSource, ShareableBlob, ExportTarget, FetchedArtifact, LocalFileRef, FileName, Mime, ProgressSink, StagedBlobReader, FileSink, ShareSink, CapabilityError, FailureKind}` (#174) + the browser fake | The **real** `WebFiles`/`WebShare`/`WebClipboard`/`WebUrlLauncher` bindings that mint/resolve those handles through `jeliya-platform-implementation` |
| Factory door | `jeliya-platform-implementation` (the sole crate allowed to enable `implementation`; free functions `picked_source`/`shareable_blob`/`export_target`/`fetched_artifact` + token wrap/unwrap) | The web bindings' **dependency edge** to the door; the token registries that fail closed on unminted/foreign tokens |
| Daemon operations | Typed `jeliya_api` ops: `FileShare`, `FileList`/`FileRow`, `FileFetch`, `FileRead`, `TransferCancel`, `PipePublish`, `PipeList`/`PipeRow`, `PipeConnect`, `PipeRelease`, `PipeRevoke`; issued via `ClientHandle::call` / `call_stream` (`StreamCall` with `cancel(execution)`) | The **orchestration** that sequences pick→stage→`file.share`, `file.list`→row model, `file.fetch`→`file.read`→export, and the pipe lifecycle calls |
| Typed error surface | `jeliya_api::push::{ApiError, ErrorCode}` variants: `FileTooLarge{declared_bytes,limit_bytes,enforced_at}`, `DeclaredSizeMismatch`, `ProviderUnreachable{file_id,providers}`, `DigestMismatch`, `FileUnknown`, `FileNotFetched`, `TransferStalled`, `TransferDeadlineExceeded`, `StreamAborted{reason}`, `TransferUnknown`, `PipeTargetRefused{target}`, `PolicyRefused`, `PipeUnknown`, `PipeUnreachable`, `PipeRevoked`, `PipeNotPublisher`, `ConnectionUnknown` | The **localized, redacting** UI mapping of each (discriminant + structured integers, never a substring match, never a leaked path/id/byte) |
| Served limit | `jeliya_api::push::{Hello, Limits}` carries `max_shared_file_bytes: u64` on the `Hello` frame; #178's `ConnectionSnapshot { subject, storage_generation }` is the read-only capture from `Hello` | The **additive extension** that surfaces the served `max_shared_file_bytes` to the Files pane (D3), so preflight uses the served value, never a constant |
| Routes + shell | `RoomDest::Files { item: Option<FileId> }`, `RoomDest::Pipes { item: Option<PipeId> }` in `jeliya_platform::navigation`; `components/room_shell.rs` renders the routing frame + per-destination **skeleton** | The **real Files and Pipes panes** that replace the skeletons, including the selected-item sub-route |
| Presence/provider facts | `jeliya_api` `FileRow.providers: Vec<PeerRow>` (each with a `link: Link`), `FileRow.fetchable`, `FileRow.self_hosted`; `PipeRow.link`, `PipeRow.connected`; `room.peers` | The **UI model** that keeps the three facts distinct and renders each from its own source |
| Composition | `compose.rs` `web_composition()` injects `WebPlatform::build()` (real nav/lifecycle/prefs/secrets, **fake** files/share/clipboard/url) + a **mock** `ClientHandle` | The composition swap: real file/share bindings; `ClientHandle` stays mock until #171 (§R2) |

The React `ui/src/components/*Files*` / staging / upload code and the Dart `daemon_http.dart` staging convention are **requirements-mining input only** — there is no legacy reader, no `?tab=files|pipes` query, no `POST /api/files/share`/`GET /api/files/local` HTTP edge, and no `save_dir`. The v2 shapes above supersede all of them.

---

## 3. Owning crate and module layout

### 3.1 The door-crate decision (D0) — a new `crates/jeliya-platform-web` for the real bindings

The real browser `Files`/`Share` impls **mint** `PickedSource`/`ShareableBlob`/`ExportTarget`/`FetchedArtifact` handles, which is only possible through the `jeliya-platform-implementation` factory door. #174's **K4** boundary forbids the *shared* `jeliya-ui` component graph from carrying a dependency edge to that door (a token a shared component could forge would break anti-exfiltration). #178 deferred the browser crate and put `WebPlatform` in `jeliya-ui/src/platform_web.rs` behind the `web` feature — acceptable while Files were an honest `Unavailable` stub with **no door edge**. #181 is the trigger #178 named: introducing a real `Files` impl forces the door edge, so the bindings move to their own crate.

**Decision:** create **`crates/jeliya-platform-web`** (wasm-only target crate, the sibling the architecture record wants) and move the browser bindings there. It depends on `jeliya-platform` (contract) **and** `jeliya-platform-implementation` (door). `jeliya-ui`'s `web`-gated `compose.rs`/`bin/web.rs` depend on it and assemble `WebPlatform`; the **default** `jeliya-ui` graph (what shared components compile against) gains **no** edge to the door, so K4's "shared UI graph has no door edge" assertion holds when evaluated on default features.

```
crates/jeliya-platform-web/
  Cargo.toml                 # wasm32-only; deps: jeliya-platform, jeliya-platform-implementation,
                             #   web-sys, wasm-bindgen, js-sys, futures. #![forbid(unsafe_code)]
  src/
    lib.rs                   # WebPlatform: Platform  (moved from jeliya-ui::platform_web)
    navigation.rs            # WebNavigation   (moved verbatim from #178)
    lifecycle.rs             # WebLifecycle    (moved verbatim from #178)
    preferences.rs           # WebPreferences  (moved verbatim from #178; schema still in jeliya-ui::prefs)
    secrets.rs               # WebSecretStore  (moved verbatim from #178)
    files.rs                 # WebFiles: Files — the browser picker/stage/export/open/share bindings
    registry.rs              # per-kind minted-token tables; fail-closed on unminted/foreign tokens (K4)
    share.rs                 # WebShare: Share (navigator.share where present, else Unavailable) + WebClipboard
    launcher.rs              # WebUrlLauncher: UrlLauncher (allowlisted open_external)
  tests/
    files_wasm.rs            # wasm-bindgen-test: pick/stage/export round-trips, token fail-closed
    boundaries.rs            # wasm-only graph; no tokio/wry/native transport; door edge present HERE, not in shared jeliya-ui
```

> **Alternative considered (and rejected as primary):** extend `jeliya-ui/src/platform_web.rs` with the real `WebFiles` behind an optional, `web`-gated dependency on `jeliya-platform-implementation`. Rejected because it puts the door edge inside `jeliya-ui` and forces the K4 boundary assertion to special-case the `web` feature — exactly the entanglement the target-crate architecture exists to avoid. If the team prefers to avoid crate proliferation, this fallback is acceptable **only** if the K4 door-edge test is re-scoped to default features and the wasm-graph guard is updated; it is not recommended. Moving #178's four already-web-sys modules is a mechanical, behavior-preserving relocation (their pure logic already lives in `jeliya-ui::prefs`/`shell`), so the crate is cheap.

### 3.2 New host-testable orchestration modules in `crates/jeliya-ui` (behind the `ui` feature)

These are pure, renderer-and-web-sys-free state machines — the *decisions* — unit-tested on the host against the deterministic mock `ClientHandle` and the `jeliya-platform` fakes. They are target-agnostic (desktop/Android reuse them).

```
crates/jeliya-ui/src/
  files/
    mod.rs                   # public model + re-exports
    list.rs                  # FileListModel: rows, provider availability, fetchable/self_hosted (truthful)
    upload.rs                # UploadFlow: pick -> stage_for_share(limit) -> file.share stream -> settled
    fetch.rs                 # FetchFlow: file.fetch -> file.read stream -> FileSink (export/open) -> settled
    transfer.rs              # shared progress + cancellation glue (CancelToken <-> StreamCall/transfer.cancel)
    size.rs                  # served-limit plumbing + over-limit classification/formatting (from Limits)
    preview.rs               # safe-preview policy: declared_content_type is untrusted; no inline peer bytes
  pipes/
    mod.rs                   # public model + re-exports
    list.rs                  # PipeListModel: rows, link (reachability) vs connected (local), revoked -> absent
    publish.rs               # PublishFlow: loopback target validation -> pipe.publish
    connect.rs               # ConnectFlow: pipe.connect -> connection handle; release/revoke lifecycle
  components/
    files_pane.rs            # /rooms/:roomId/files : list + share + selected-file detail (fetch/export/preview)
    pipes_pane.rs            # /rooms/:roomId/pipes : list + expose + selected-pipe detail (connect/release/revoke)
```

### 3.3 Composition and shell changes in `crates/jeliya-ui`

- `components/room_shell.rs`: the two `RoomDest::Files`/`RoomDest::Pipes` skeleton placeholders are replaced by `files_pane::FilesPane` / `pipes_pane::PipesPane`, wired to the injected `ClientHandle` + `PlatformServices` + the current `Route` (for the selected-item sub-segment) + the served `max_shared_file_bytes` (D3). Activity/People/Agents stay skeletons (#179/#180).
- `compose.rs` `web_composition()`: inject the real file/share bindings (via `jeliya-platform-web`) in place of the fake delegations; keep the **mock `ClientHandle`** until #171 (§R2). The room-shell wiring is transport-agnostic — when `WsWeb` lands, only the handle line changes.
- No shared component gains a `cfg(target_…)` fork (Decision-3 / K10); target selection stays at the crate root + `web` bin.

---

## 4. Key design decisions

### D1 — Two seams, no third: operations via `ClientHandle`, authority via `PlatformServices`

Every daemon operation (`file.share`, `file.list`, `file.fetch`, `file.read`, `transfer.cancel`, `pipe.*`) is issued through `ClientHandle::call`/`call_stream`. Every platform effect (pick a file, stage bytes, choose an export target, write the download, present a share sheet, copy text, open an external URL) goes through a `PlatformServices` capability method. The two are **injected separately** (#174 K11) and never entangled: a component never mints a daemon path from a platform handle, and never reads platform bytes through the client seam. Bearer/session handling stays at the HTTP/WS edge inside `WsWeb`/`ClientHandle`; **only bounded safe metadata** (a `FileName`, a `Mime`, a `u64` size, a typed id, a discriminant) crosses into components.

### D2 — Availability is read from its own protocol fact, never inferred (Security invariant 4; #50/#79/#94)

Three facts, three sources, kept apart in the models and never crossed:

| Fact | Source | UI vocabulary | Rule |
|---|---|---|---|
| **Membership / presence** | `room.peers`, roster | Direct / Relay / Connected / Connecting / Offline; "No peers connected" | Never used to decide whether a file or pipe can be served |
| **File-provider availability** | `FileRow.providers[].link` + `FileRow.fetchable` + `FileRow.self_hosted` | Named provider devices with per-device `link`; **Fetchable** when evidence backs it; **On this device** when `self_hosted` | A reachable provider serves the file regardless of any stale roster display (#50). `fetchable:false` renders as "not currently fetchable" with the provider evidence, **not** "the sharer left" |
| **Pipe reachability** | `PipeRow.link` (publisher device) **and** `PipeRow.connected` (this daemon's local connection) — two separate fields | **Connected** (local connection held) / **Open** (published, nothing connected locally); publisher reachability from `link` (**Direct**/**Relay**/unavailable-with-reason); a revoked pipe is **absent**, never "Closed" | `link` and `connected` are never conflated; an offline publisher yields the distinctive `pipe_unreachable` on connect, not a generic failure (#94) |

`FileListModel` and `PipeListModel` carry these as distinct typed fields; a test asserts no code path derives one from another (e.g. no "provider unavailable because peer offline" shortcut), mirroring the contract's invariant 4.

### D3 — The size limit is served, surfaced additively, and never a constant (#92)

`stage_for_share(src, limit, …)` requires the daemon's `max_shared_file_bytes`. It lives on the `Hello` frame (`jeliya_api::push::Limits`), captured by #178's `ConnectionSnapshot`. #178's snapshot carries only `{ subject, storage_generation }`; #181 **additively extends** it (or adds a sibling read on `ClientHandle`) to surface `max_shared_file_bytes: u64`. Coordinate the exact shape with #178/#270's `Connected`/`connection()` seam rather than inventing a parallel read.

- **No number in code, catalog, or script.** The 100 MiB figure appears nowhere in `jeliya-ui`/`jeliya-platform-web`; the localized over-limit copy **interpolates** the served integer (`docs/shared-file-size.md` Decision 1). A source-scan gate (mirroring #177's literal-copy gate) fails if `104857600`, `100 MiB`, `100 MB`, or a `max_shared_file_bytes` constant appears in the shipped shell.
- **Preflight before any copy.** `UploadFlow` refuses a known-oversize `PickedSource` with `FileTooLarge` before staging; a size-unknown source (browser blob still reports size, but the contract path is retained) is counted into bounded staging that stops at `limit + 1` (protocol §`file.share` "unknown-source path"). The daemon re-enforces at `stage_declared` and `stage_stream`; the UI renders `ErrorCode::FileTooLarge { declared_bytes, limit_bytes, enforced_at }` with the served integers, **explained** (what the limit is, that this file exceeds it, that chunking is not offered).
- **No false accusation (Decision 3).** A size refusal is **never** rendered as a digest/integrity failure. On fetch, the signed `FileRow.bytes` is preflighted against the served limit before `file.fetch`; `DigestMismatch` is reserved for genuine content mismatches and rendered with `expected`/`observed` discriminants only.

### D4 — Confinement is structural; the #122 regressions are retained without a path (Security)

v2 carries no filesystem path, so the #122 destination-escape and symlink hazards cannot be *expressed* in a request. This issue keeps that true and proves it:

- **No path or URL in any handle, prop, sink, or request.** Export writes go to an opaque `ExportTarget` (browser download / save target the platform owns); `FileName::parse` already fails closed on `.`/`..`, `/`, `\`, control chars, and empty — the shared type that stops a peer-supplied name from carrying portable path syntax into a sink's artifact naming.
- **Peer-supplied names are validated at ingest.** `FileRow.name` and `file.share` `name` cross as `FileName` (parsed, fail-closed); a hostile name (`../../etc/passwd`, `a/b`, ` `, a Windows drive-qualified `C:evil`) is rejected before it reaches a sink, and the rejection carries no payload (#174 K1).
- **Retained regression coverage.** New tests (host + wasm/Playwright) drive hostile names/MIMEs through the ingest and export paths and assert they fail safely and that no path spelling is reachable — the new-stack analogue of the `8cc24fe` fixtures, owned here and independent of the closed #122.

### D5 — Cancellation reaches both the local copy and the wire; it is never `Ok` (#174 D3/K2; protocol `transfer.cancel`)

A user cancel fires **two** things through `files/transfer.rs`:

1. the `PlatformServices` `CancelToken` — stops the local bounded copy, deletes the partial staged blob (`stage_for_share`) or drops the uncommitted export sink (`file.read`→`FileSink`), yielding `CapabilityError::Cancelled`; and
2. the client stream — for `file.share`/`file.read`, `StreamCall::cancel(execution)` (in-band ABORT, protocol: `file.read`/`file.share` cancel in-band, not via `transfer.cancel`); for `file.fetch`, `transfer.cancel { transfer_op_id }` (op_id-addressed, survives reconnect; the original fetch records `stream_aborted` reason `cancelled`).

The `Execution` classification is preserved (`DefinitelyNot` before the stream opens, `Unknown` once bytes may have gone out) so a cancel never masquerades as success and a partially-uploaded file is never reported as shared. Dropping the flow future is equivalent to cancel (drop-is-abort). A settled flow has exactly one outcome; `Cancelled` is one of them and is not `Ok`.

### D6 — Safe external preview: `declared_content_type` is untrusted data, never a render authorization

Protocol `file.read` carries `declared_content_type` as an explicitly untrusted field (v1's never-render-inline HTTP headers became **data**). `files/preview.rs` encodes the policy:

- **Never inline-render peer bytes on the strength of `declared_content_type`.** The Files pane shows metadata (name, size, digest short-form, provider evidence, declared type **labeled as declared/untrusted**) and offers **export/download** and, at most, a sandboxed/opt-in preview that does not execute or auto-load peer content as active markup.
- **Any external open is allowlisted.** If a preview or link opens externally, it goes through `UrlLauncher::open_external(SafeExternalUrl)` — which fails closed on `javascript:`/`data:`/`file:`/`content:` and any non-`https`/`mailto`/`tel` scheme (#174 D8). The UI cannot open a raw string.
- **Pipe previews are constrained too.** A pipe row shows its `target` (loopback host/port), `audience`, `link`, and `connected` facts; it never auto-connects, and `pipe.publish` targets are validated **loopback-only** client-side before the call (`PipeTargetRefused` for anything outside `127.0.0.0/8`/`::1` and `1..=65535`), so a hostile target costs no round trip and reveals nothing.

### D7 — Bearer/session never crosses into components; diagnostics redact (Security)

Session credentials and any bearer/ticket live in `WebSecretStore` (tab-scoped, dies with the tab) and at the `WsWeb`/`ClientHandle` edge; **no** Files/Pipes prop, model field, error, or diagnostic carries a token. The v2 file protocol has no token-carrying URL to leak (the `GET /api/files/local` edge is retired). Diagnostics for these flows carry a discriminant + bounded integers only (bytes transferred, limit) — never file bytes, a name's full string beyond what the user already sees, a digest beyond a short form, a `content://`/path (none exist), or a full identity. A source-scan test asserts no `SecretKey`/token type reaches a component-facing signature (mirrors #174 K5).

### D8 — Truthful, non-inferred states everywhere (contract §"Bootstrap", §"Status vocabulary")

Booting is *unknown*, not *zero*: before `file.list`/`pipe.list` answer, the panes render the route's loading state, never "no files"/"no pipes". A failed list surfaces the daemon's real error (`FileIndexUnreadable`/`PipeIndexUnreadable`), with Retry, not an empty pane. A departed/archived room suppresses share/fetch/pipe actions as **typed capabilities** (contract invariant 5 / #91), not disabled buttons scattered through UI code — the Files/Pipes panes read a read-only-archive fact and render the read-only historical surface.

---

## 5. Implementation workstreams

### 5.A — Browser file/share bindings (`jeliya-platform-web`)

1. **`WebFiles: Files`** implements the full trait against `web-sys`:
   - `pick` → `<input type="file">`; a clean no-selection is `Ok(None)`, a user dismissal is `Err(Cancelled)` (kept distinct, #174 D4). Mints a `PickedSource` (`FileObjectKind::BrowserBlob`) via `implementation::picked_source`, keeping the real `File`/blob in the crate's private `registry` keyed by the minted `SourceToken`.
   - `stage_for_share(src, limit, progress, ct)` → **hold the blob client-side** (browser staging is client-side custody, not a server upload edge); enforce `limit` (known size fails `FileTooLarge` before any read; a streamed read stops at `limit+1`); zero bytes → `FileEmpty`; on cancel/failure leave **no** retained bytes; mint a `ShareableBlob` via `implementation::shareable_blob`. `read_staged` returns a `StagedBlobReader` that the `file.share` upload pulls per CREDIT.
   - `pick_export_target(suggested, ct)` → a browser download target (`ExportTargetKind::BrowserDownload`, suggested `FileName`); `export_sink(to, ct)` / `open_sink(name, declared, ct)` return a `FileSink` that assembles bytes and, on `commit`, triggers the download / object-URL open. `share_sink(name, declared, ct)` returns a `ShareSink` whose `commit` yields a `FetchedArtifact` for the share sheet.
   - `discard_source`/`release_staged`/`release_artifact`/`discard_export_target` release the registry entries (borrow-not-consume; a failed release stays recoverable, #174 D5).
   - **`registry.rs`** fails closed on a token it did not mint (issuer provenance), so a foreign/forged handle never resolves (#174 K4). All futures are `!Send` (browser single-thread), matching the crate's `BoxFuture` shape.
2. **`WebShare: Share`** → `navigator.share`/`navigator.canShare` where present (files + text), else `Availability::Unavailable`; dismissal is `Cancelled`, refusal `Denied`. Content is taken by reference; a `ShareableBlob` is never consumed by sharing, a `FetchedArtifact` only by a completed share.
3. **`WebClipboard: Clipboard`** → `navigator.clipboard.writeText` (async; a permission denial resolves `Err(Denied)`, never a false `Ok`).
4. **`WebUrlLauncher: UrlLauncher`** → `window.open(url, "_blank")` for a `SafeExternalUrl`; a failed open returns `Err`, surfaced by the UI (never swallowed).
5. **`WebPlatform`** now returns the real bindings from `files()`/`share()`/`clipboard()`/`url_launcher()` instead of delegating to the fake (nav/lifecycle/prefs/secrets unchanged from #178).

### 5.B — Files orchestration (`jeliya-ui/src/files/`, host-testable)

- **`list.rs` — `FileListModel`.** Calls `file.list` (cursor/limit paging, `truncated`), builds rows carrying `{ file_id, name: FileName, bytes, digest_short, declared_content_type (labeled untrusted), providers: Vec<ProviderView>, fetchable, self_hosted }`. `ProviderView` renders each provider's `link` verbatim; **availability comes only from these fields** (D2). Subscribes to `file_shared` pushes to append/refresh rows (via the reconciler event stream) — a new share appears without a manual refresh; a stale row never claims fetchability the evidence does not support.
- **`upload.rs` — `UploadFlow`.** State machine `Idle → Picking → Staging(progress) → Uploading(progress) → Settled(Shared|Failed|Cancelled)`. `pick` → `stage_for_share(limit)` (D3 preflight) → `read_staged` → `ClientHandle::call_stream::<FileShare>` pumping `StagedBlobReader` chunks per CREDIT → on terminal, `release_staged`. Every over-limit/empty/unreadable/cancel path maps to a typed outcome; the daemon's `FileShareOut { file_id, bytes, digest }` is the success proof.
- **`fetch.rs` — `FetchFlow`.** `Idle → Fetching(progress) → Reading(progress) → Exporting → Settled`. Preflight signed `bytes` vs served limit (D3); `file.fetch` (requires `op_id`, so `Dedup` carries it for cross-reconnect cancel); then `file.read` streamed into a `FileSink` from `export_sink`/`open_sink`. `ProviderUnreachable{providers}` renders the attempted-provider evidence; `DigestMismatch` renders expected/observed short-forms; a size refusal is never shown as a hash mismatch (D3).
- **`transfer.rs`** — the `CancelToken` ⇄ `StreamCall`/`transfer.cancel` glue (D5) and a `ProgressSink` → UI progress adapter (bounded, monotonic; `transfer` pushes only for `file.fetch`).
- **`size.rs`** — served-limit plumbing (D3) + over-limit formatting from the served integer (binary units rendered client-side).
- **`preview.rs`** — the D6 safe-preview policy as a pure classifier (`declared_content_type` → {export-only | sandboxed-optional}, never inline-active).

### 5.C — Pipes orchestration (`jeliya-ui/src/pipes/`, host-testable)

- **`list.rs` — `PipeListModel`.** `pipe.list` paging; rows `{ pipe_id, published_by, device_id, published_at, target, audience, link, connected, is_own }`. **`link` (reachability) and `connected` (local connection) are separate fields, never merged** (D2). A revoked pipe is **absent** from the list (drop on `pipe_revoked` push), never rendered "Closed".
- **`publish.rs` — `PublishFlow`.** Validate `target` is loopback (`127.0.0.0/8`/`::1`, port `1..=65535`) **client-side before** `pipe.publish` (mirrors the daemon's local pre-publish refusal; a private-range `192.168.x` target is refused). `audience` is required (`room` or `subjects`), stated explicitly. Maps `PipeTargetRefused{target}` (bad target — user fixes the target) vs `PolicyRefused` (not permitted here — different response) distinctly.
- **`connect.rs` — `ConnectFlow` + release/revoke.** `pipe.connect` → `{ connection_id, local }`; a reachable authorized owner connects (**Connected**), an offline owner yields the distinctive `pipe_unreachable`, an out-of-audience/absent pipe yields `pipe_unknown` (indistinguishable, not an oracle), a revoked pipe `pipe_revoked`. `pipe.release { connection_id }` releases the **local** connection (by connection, not pipe); `pipe.revoke { pipe_id }` withdraws a **published** pipe as a signed fact (owner-only; `pipe_not_publisher` otherwise; re-revoke returns the original withdrawal).

### 5.D — Components (`files_pane.rs`, `pipes_pane.rs`)

- **`FilesPane`** (`/rooms/:roomId/files`, and `…/files/:fileId` via `RoomDest::Files { item }`): the file list (truthful availability, D2/D8), a **Share** action (pick→stage→upload with progress + cancel), and, when a file is selected, a detail surface (metadata, provider evidence, declared-type-as-untrusted, **Fetch**/**Export/Download** with progress + cancel, safe preview per D6). Over-limit share and fetch render the explained, served-value message (D3). Read-only-archive rooms suppress share/fetch as typed capabilities (D8).
- **`PipesPane`** (`/rooms/:roomId/pipes`, and `…/pipes/:pipeId`): the pipe list (Connected/Open + publisher reachability, D2), an **Expose** action (loopback target + required audience, D6), and per-pipe **Connect**/**Release** and, for own pipes, **Revoke**. `pipe_unreachable` vs `pipe_unknown` vs `pipe_revoked` render distinctly.
- Both panes are responsive across compact/medium/wide (the selected item is the inspector; compact opens it as nested navigation, medium as a drawer, wide as the third column — the shell already routes this), EN + FR, with the 44px/58px floors and single-live-region status announcements (#177).

### 5.E — l10n catalog additions (`jeliya-ui/src/l10n/{mod.rs,en.rs,fr.rs}`)

Declare once in the `Catalog` trait, implement in `En` and `Fr` (compile-enforced parity, #177): Files list/empty/loading/failed; provider availability labels (Fetchable / On this device / not-currently-fetchable-with-reason); share/fetch/export action + progress + cancel labels; the **over-limit** message (interpolating the served integer — no baked number); digest-mismatch and provider-unreachable copy; Pipes list/empty/loading/failed; Connected/Open; publisher reachability (Direct/Relay/unavailable-with-reason); expose target/audience labels + loopback-refused and policy-refused copy; connect/release/revoke labels; `pipe_unreachable`/`pipe_unknown`/`pipe_revoked` copy. The Node-side #177 gates (empty value, `fr==en`, French typography, literal-scan including the size-number scan) apply unchanged.

---

## 6. Error mapping (typed, redacting, no substring matching)

Every daemon failure is a typed `ErrorCode`; the UI maps discriminant + structured integers, never English substrings (retiring the `share limit` / `HashMismatch` substring hacks). Representative mapping:

| `ErrorCode` | Flow | UI surface (localized, redacting) |
|---|---|---|
| `FileTooLarge { declared_bytes, limit_bytes, enforced_at }` | share / fetch preflight | Explained over-limit message interpolating `limit_bytes`; states chunking is not offered (D3) |
| `DeclaredSizeMismatch { declared_bytes, observed_bytes }` | share | "size disagreement", never "corruption" (D3) |
| `ProviderUnreachable { file_id, providers }` | fetch | "no reachable provider", listing attempted providers' `link` evidence (D2) |
| `DigestMismatch { expected, observed }` | fetch | integrity failure with short-form digests; **never** for a size refusal (D3) |
| `FileUnknown` / `FileNotFetched` | fetch / read | "no such file" / "not held locally — fetch first" |
| `TransferStalled` / `TransferDeadlineExceeded` / `StreamAborted { reason }` | share / fetch / read | stall / deadline / aborted, with bytes-so-far + total (or genuinely-unknown) |
| `PipeTargetRefused { target }` | publish | "target not allowed (loopback only)", echoing the rejected target |
| `PolicyRefused` | publish | "publishing not permitted in this room" (distinct from target refusal) |
| `PipeUnreachable` | connect | the **distinctive** offline-publisher failure (#94), distinct from `pipe_unknown` |
| `PipeUnknown` | connect | "no such pipe" — indistinguishable from out-of-audience (not an oracle) |
| `PipeRevoked` / `PipeNotPublisher` / `ConnectionUnknown` | connect / revoke / release | withdrawn / not the publisher / no such local connection |
| `RoomNotLive` | fetch / connect | "room not live — activate first" |

`CapabilityError` (platform side) maps in parallel: `Unavailable` → "not available in this browser", `Denied` → permission refused, `Cancelled` → the action did nothing (state untouched), `Failed(FileTooLarge/FileEmpty/Unreadable/…)` → the typed local failure. **`Cancelled` never renders as an error the user caused or as success.**

---

## 7. Security and correctness

- **Confinement without a path (D4).** No path/URL in any request, handle, prop, or sink; `FileName::parse` fail-closed at every ingest; hostile-name/symlink regression coverage retained in the new stack, independent of closed #122.
- **Availability is never inferred (D2).** Provider/pipe availability comes only from `file.list`/`pipe.list`/`room.peers` fields; a test asserts no cross-inference.
- **Size served, explained, no false accusation (D3).** Preflight against the served limit; distinctive over-limit error with structured integers; size refusals never rendered as integrity failures; no number in code/catalog/script.
- **Untrusted preview (D6).** `declared_content_type` never authorizes inline rendering of peer bytes; external opens are allowlisted; pipe targets are loopback-validated pre-call.
- **No bearer/token leak (D7).** Tokens stay at the edge and in `WebSecretStore`; components carry bounded safe metadata only; diagnostics redact.
- **Truthful states (D8).** Booting = unknown ≠ zero; failed lists show the real error + Retry; departed rooms suppress actions as typed capabilities.
- **Cancellation is honest (D5).** Reaches both local copy and wire; never `Ok`; drop-is-abort.

---

## 8. Test strategy

The canonical gate is `cargo` + the wasm/Playwright web suite (#176/#177: `crates/jeliya-ui/e2e/*`, `scripts/check-jeliya-ui-wasm-graph.sh`, the design-token/l10n/literal gates). Add:

### 8.1 Host unit tests (pure modules, `cargo test -p jeliya-ui`, against the mock + `jeliya-platform` fakes)
- **Upload flow:** pick→stage→`file.share` happy path; known-oversize → `FileTooLarge` before copy; streamed → limit-enforced mid-copy; empty → `FileEmpty`; mid-copy cancel → `Cancelled` + no retained blob + `release_staged`; a pre-fired cancel never becomes `Ok`.
- **Fetch flow:** `file.fetch`→`file.read`→export happy path; `ProviderUnreachable` renders provider evidence; `DigestMismatch` distinct from a size refusal; preflight signed size vs served limit; cancel via `transfer.cancel` for fetch and in-band ABORT for read; export sink drop deletes the partial artifact.
- **File-list model:** provider availability read only from `providers`/`fetchable`/`self_hosted`; a stale roster does not change fetchability; `file_shared` push appends a row; **no cross-inference** (assertion).
- **Size:** served-limit plumbing; over-limit formatting interpolates the served integer; the number-scan gate finds no literal.
- **Pipe list/publish/connect:** `link` vs `connected` kept separate; revoked → absent; loopback validation (accept `127.0.0.1`/`::1`, reject `192.168.1.10`, reject port 0/65536); `PipeTargetRefused` vs `PolicyRefused` distinct; `pipe_unreachable` vs `pipe_unknown` vs `pipe_revoked` distinct; release-by-connection vs revoke-by-pipe; re-revoke returns the original withdrawal.
- **Confinement/names (D4):** hostile names (`../../x`, `a/b`, `C:evil`, ` `, empty) rejected at ingest and export; no path spelling reachable (compile-fail/`trybuild` where useful).
- **Error mapping:** every `ErrorCode` above maps to a discriminant with no leaked payload; no substring matching.

### 8.2 `jeliya-platform-web` wasm tests (`wasm-bindgen-test`, headless Chromium)
- `WebFiles` pick/stage/export round-trips; token registry fails closed on an unminted/foreign token; `WebShare`/`WebClipboard`/`WebUrlLauncher` availability and denied/dismissed paths; `SafeExternalUrl` fail-closed on hostile schemes.
- Boundary: wasm-only graph (no tokio/wry/native transport); the door edge is present **here**, not in the shared `jeliya-ui` default graph.

### 8.3 Browser e2e (Playwright, `crates/jeliya-ui/e2e/`), mock **and** real supervised daemon
Covering the Verification section explicitly: upload/fetch **success/failure/cancel**; **connected** vs **unavailable** provider (honest failure regression); **connected-provider-serves-despite-stale-display** (#50 regression); Pipe **direct/relay** lifecycle (expose/connect/release/revoke) and **unavailable** publisher → `pipe_unreachable`; **hostile** names/MIME/URLs fail safely; **token redaction** (no bearer in any DOM/network-visible surface the component controls); **destination confinement** (no path escape, symlink-style hostile name rejected); over-limit share/fetch shows the served-value explanation; responsive fractional breakpoints (360/899/900/920/1280, 899.98) in EN + FR with the 44px/58px floors and no overflow (French first).

### 8.4 Focused-first guidance
Run the pure host tests (`cargo test -p jeliya-ui files:: pipes::`) and the relevant e2e spec first; reserve the full web build + Playwright matrix and `wasm-bindgen-test` for the review gate. web-sys bindings are proven only in-browser, never on the host.

---

## 9. Acceptance-criteria traceability

| AC (issue) | Where satisfied |
|---|---|
| File pick/upload/download flows are bounded and cancellable | §5.A (`WebFiles` bounded staging/sinks), §5.B `upload.rs`/`fetch.rs`, D5, tests §8.1/§8.3 |
| Availability and progress are not inferred from membership | D2, §5.B `list.rs`/§5.C `list.rs`, tests §8.1 (no-cross-inference) |
| Connected-provider success and honest unavailable-provider failure regressions pass | D2, `fetch.rs` (`ProviderUnreachable` evidence), tests §8.3 (#50/#94 regressions) |
| The v2 maximum-file-size policy is enforced and explained | D3, §5.B `size.rs`, error mapping §6, tests §8.1 + literal-scan gate |
| Destination confinement and hostile symlink/path cases remain covered | D4, §5.A/§5.B ingest+export, tests §8.1/§8.3 |
| Pipes expose/connect/close truthfully across reachable and unavailable paths | D2, §5.C, error mapping §6, tests §8.1/§8.3 |
| Hostile path/name/MIME/URL cases fail safely | D4/D6, `FileName::parse`/`SafeExternalUrl::parse`/loopback validation, tests §8.1/§8.2/§8.3 |
| Files/Pipes routes pass all responsive/a11y scenarios | §5.D, the #178 shell routing + #177 a11y foundation, tests §8.3 |

---

## 10. Documentation changes

- `docs/dioxus-architecture.md`: flip the #181 slice row to reflect the browser Files/Pipes landing and the new `jeliya-platform-web` target crate (the door-edge home); note the M3 room-content status.
- `docs/product-behavior-contract.md` §"Required destinations": no rule change (this issue implements the Files/Pipes rows); add a pointer to the shared `jeliya-ui::files`/`jeliya-ui::pipes` modules and `jeliya-platform-web`.
- `docs/shared-file-size.md`: record that the served-limit + distinctive over-limit surface is now implemented for the browser Files pane (implementation_status), retaining the "no baked number" requirement.
- `docs/known-gaps-roadmap.md` / `CHANGELOG.md`: record the browser Files/Pipes landing, the new crate, the retained #122 confinement coverage in the new stack, and that desktop/Android Files/Pipes remain #184/#192; note the downstream security qualification #196 consumes this surface.
- New crate `README.md`/`lib.rs` docs mirroring the sibling crates, including the boundary invariants and the "where this crate and the record disagree, the record wins" clause.

---

## 11. Risks and mitigations

- **R1 — The served size limit is not yet on the `ClientHandle` seam (D3).** `Hello.limits.max_shared_file_bytes` exists in `jeliya_api` but #178's `ConnectionSnapshot` does not surface it. Mitigation: additively extend the snapshot/accessor (coordinate with #178/#270's `connection()`); script it against the mock now; the real value arrives with #171. If the extension slips, the pane can read the limit from a `file.list`-adjacent server-limits view model, but the snapshot is the recommendation. **Never** fall back to a compiled-in constant (that is the exact defect #92 closes).
- **R2 — #171 `WsWeb` not merged (not in this worktree).** The orchestration and panes are transport-agnostic and build/review against the **mock** `ClientHandle` (as #176/#178 do); only the compose handle line changes when `WsWeb` lands. The *live* real-daemon Verification (upload/fetch/pipe against a supervised jeliyad) is fully exercised only once #171 provides the real transport — state this honestly in the PR, and gate the clean-slate cutover on the real-daemon qualification (§13).
- **R3 — Door-edge boundary (D0).** Introducing a real `Files` impl forces the `jeliya-platform-implementation` edge. Mitigation: the new `jeliya-platform-web` crate keeps the edge out of the shared `jeliya-ui` default graph; re-scope/extend #174's K4 door-edge test and the wasm-graph guard to assert the edge lives only in the target crate + web binary, never the shared component graph. If the team rejects a new crate, the module fallback (§3.1) is possible but must re-scope K4 explicitly.
- **R4 — Over-limit *stream* observability (#92 Decision 3).** An over-limit stream (peer serves more than declared) currently surfaces as `HashMismatch` upstream until the `iroh-rooms` outcome enum gains a size variant; `DeclaredSizeMismatch` covers the send side, but the receive-side stream case may still arrive as a content mismatch. Mitigation: preflight the signed size (closes the common case honestly); render `DeclaredSizeMismatch`/`FileTooLarge` where the typed code is available; do not pretend the residual upstream gap is closed — cite it in the PR and defer to #161's upstream change.
- **R5 — `!Send` browser futures vs shared orchestration.** Platform futures are `!Send` by design; the client upload input must accept `!Send` byte sources (#269 depth). Mitigation: the shared `files`/`pipes` modules stay executor-agnostic and `!Send`-tolerant (as `jeliya-client` is on wasm); the adaptation of `StagedBlobReader` → upload input happens in composition, keeping no shared type between the two facades (#174 K11).
- **R6 — Scope creep into #179/#180 or into daemon authority.** Mitigation: touch only the Files/Pipes room destinations; Activity/People/Agents stay skeletons; the daemon keeps file/pipe authority; no path, no `save_dir`, no daemon-file read is added.
- **R7 — Preview safety regressions.** A convenience "preview" that inline-renders peer bytes on `declared_content_type` would reintroduce the exact hazard v2's data-not-headers change closed. Mitigation: `preview.rs` is a pure classifier with a test that no code path inline-renders active peer content; external opens go only through `SafeExternalUrl`.

---

## 12. Open questions

- **Q1 (owner: #181 + #178/#270):** exact shape/placement of the served `max_shared_file_bytes` on the `ClientHandle`/`ConnectionSnapshot` seam (extend the snapshot vs. a `limits()` accessor vs. a server-limits view model). Recommendation: extend the D2 snapshot additively, resolved jointly with #270's `Connected` path.
- **Q2 (owner: #181 + maintainers):** new crate `jeliya-platform-web` (D0, recommended) vs. extending `jeliya-ui::platform_web` behind the `web` feature with a gated door dep. Confirm the crate before implementation; it decides how the K4 door-edge test is scoped.
- **Q3:** how much of a **preview** the Files pane offers beyond export/download. Recommendation: export/download + a strictly sandboxed, opt-in, non-active preview only for a small allowlist of inert types; **no** inline rendering on `declared_content_type`. Confirm the allowlist (or ship export-only for #181 and defer preview to a follow-up).
- **Q4:** whether `WebShare` (Web Share Level 2, file sharing) is broadly enough supported to ship, or whether the browser share action is export/download-only with `Share` reporting `Unavailable`. Recommendation: feature-detect `navigator.canShare({ files })`; fall back to export/download honestly (`Unavailable`), never a silent no-op.
- **Q5:** the residual over-limit-*stream* case (R4) — does #181 wait on #161's upstream outcome-enum change, or ship with preflight + `DeclaredSizeMismatch` and a documented gap? Recommendation: ship with preflight and the documented gap; do not block the pane on the upstream change.
- **Q6:** whether `file.fetch` progress (the only operation with `transfer` pushes) needs a dedicated cancel affordance distinct from the `file.read` in-band ABORT in the UI, or one unified "Cancel" that the flow routes to the correct mechanism. Recommendation: one unified Cancel; `transfer.rs` routes to `transfer.cancel` (fetch) vs ABORT (read/share).

## 13. Clean-slate cutover

Dioxus Files/Pipes becomes canonical **only after** its security and real-daemon qualification gates pass: the Playwright suite against a real supervised jeliyad (§8.3), the confinement/hostile-input coverage (D4), the served-limit/over-limit behavior (D3), and the downstream security qualification #196 that consumes this surface. Until #171's `WsWeb` lands, the panes ship and review against the mock, and the "reflect real daemon truth" gate is stated as pending in the PR — the cutover does not complete on the mock alone.
