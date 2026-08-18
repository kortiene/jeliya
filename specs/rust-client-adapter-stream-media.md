# Spec — Adapter stream media drive: real bytes on WsNative and WsWeb

- **Relates to:** kortiene/jeliya#233 (client framing remainder), #269 (kernel stream
  lifecycle, landed), #171/#172 (WsWeb/WsNative adapters, landed), #175 (adapter parity),
  #182 (real-daemon qualification slice).
- **Authority:** `docs/protocol-v2.md` §*Byte-stream framing* and `specs/rust-client-kernel-stream-lifecycle.md`
  (§S2/§S3 especially) outrank this document; disagreement is a bug.
- **Constraint:** implementation slice record (unlike the #269 planning doc, this ships code).

## Outcome

`call_stream::<FileShare>` / `::<FileRead>` move **real bytes** over the live
adapters (native tokio WebSocket; browser `web_sys::WebSocket`), not just the
deterministic in-memory rig. A file shared through either adapter against a live
`jeliyad` completes end-to-end, and a `file.read` streams the bytes back.

## The three gaps this closes

1. **Media identity at the driver.** The core's media actions
   (`Action::ProduceData`, `Action::WriteSink`) carried only `CallId`, which a
   driver can never map to wire identity (it sees `RequestId`s). Both actions now
   also carry the stream's wire `id`. Additive, byte-free; `Input::Produced`/
   `SourceEnd`/`SinkAccepted`/… keep their existing `CallId` shape.
2. **A public media seam.** Callers could not supply or collect bytes. New
   `jeliya_client::media` types (`ByteSource`, `SharedBytes`,
   `ByteSink`, `CollectedBytes`, `StreamMedia`) reach the driver via
   `ClientHandle::register_stream_media(op_id, media)` →
   `ClientBackend::register_stream_media` (default: honest
   `LocalError::UnsupportedMedia` — the mock refuses rather than pretending).
   Registration is keyed by the call's `Dedup::Key` `OpId`: the driver binds
   `op_id → wire id` when it performs the `Action::Send` for a stream op
   (`file.share`/`file.read`, recognized by wire name exactly as the kernel's
   replay gate does). An unregistered stream that reaches `ProduceData`/`
   WriteSink` reports `SourceFailed`/`SinkFailed` — an honest abort, never a
   stall or fake success.
3. **Driver fulfillment.** Both real adapters implement the record/media arms
   their runtime shells currently drop with `debug_assert!`:
   - `WsNative` (`adapter/runtime.rs`, `adapter/ws_native.rs`): a
     `MediaRegistry` shared (like `ConnRegistry`) between `NativeIo`, the dial
     task and the read loop. `send_record` frames via `jeliya-codec` onto the
     existing write channel. At OPEN the read loop spawns **one media task per
     stream** holding the registered source; `ProduceData` hands the grant to
     that task, which reads ≤ `up_to` bytes, frames ≤ `max_stream_data_bytes`
     DATA records, sends them through the writer (awaiting its backpressure),
     and injects `Input::Produced`/`SourceEnd` through the shell's `inject`.
     Inbound Binary records are decoded in the read loop; DATA payloads buffer
     in a per-stream, window-bounded map until `WriteSink` writes them to the
     sink and queues `SinkAccepted` through `take_pending_media`. Teardown
     (connection loss, cancel_dial, Drop) aborts media tasks and clears the
     registry through `ConnRegistry::tear_down` — a stream never survives its
     connection, mirroring the core's §S10 rule.
   - `WsWeb` (`kernel/runtime.rs`, `ws_web/socket.rs`): the mailbox runtime
     grows `IoAction::SendRecord`/`ProduceData`/`WriteSink`/`RegisterMedia`
     arms and the `Driver` trait grows `send_record`/`produce`/`write_sink`
     (defaults that report failure honestly, keeping `DirectClient`'s driver
     compiling and honest until its own slice). Media inputs return to the
     core as a new `DriverEvent::Media` arm. `onmessage` Binary decodes via
     `jeliya-codec`; DATA payloads buffer exactly as on native; outbound
     records go out with `send_with_u8_array`.

## Rules carried over (binding)

- The core stays byte-free; **all framing stays in `jeliya-codec`**, called only
  at the driver boundary (§S2). No JBS2 constant appears outside the codec.
- Buffers are bounded: outbound read-ahead by the grant, inbound quarantine by
  the credit window the core granted; the per-stream payload buffer enforces the
  same cap defensively.
- `Debug` of any new type renders no payload, name, or digest (§S12).
- Streams never replay across reconnects (§S8, already enforced); on
  `Interrupted` the driver drops every stream's media exactly as the core drops
  the streams.
- Media registration is caller-owned memory (`SharedBytes`/`CollectedBytes` are
  bounded by the caller); `PlatformServices`-backed sources plug into the same
  `ByteSource` seam later (#174) with no adapter change.

## Qualification (the definition of done)

- `scripts/run-ws-web-browser-tests.sh` against a real `jeliyad`: a
  `file.share` (browser-produced bytes) → `file.read` round-trip over the
  socket, asserting the returned bytes and the daemon's digest — not the mock.
- The native adapter exercises the same round-trip in its integration harness
  (`tests/ws_native_adapter.rs` server rig).
- The PR states exactly what was qualified live vs. mock.

## Non-goals

- Resumable/chunked transfer (#209), `stream.*` subscriptions in DirectClient
  (#302), UI composition changes, HTTP staging routes.
