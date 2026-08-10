//! The object-safe internal backend trait and the erased payload boundary
//! (#167 §D8).
//!
//! None of these types are exported. The typed convenience calls live on the
//! concrete [`ClientHandle`](crate::ClientHandle) as generic methods; the
//! backend trait is object-safe (no generic methods, returns boxed futures)
//! and carries only erased JSON *text*. That is how "backend erasure stays
//! internal" is satisfied without losing compile-time request/output pairing:
//! the type work happens at the handle's edges, and the kernel (#168) fills in
//! the machinery behind this same trait without a breaking change.

use futures::future::BoxFuture;
use jeliya_api::OpId;

use crate::error::CallError;
use crate::event::{EventSubscription, State};

/// A JSON text blob. A *text* newtype, not `serde_json::Value`, so the
/// no-`Value`-in-source rule holds (asserted by `tests/boundaries.rs`) and the
/// erased boundary carries nothing but bytes.
pub(crate) struct RawJson(Box<str>);

impl RawJson {
    /// Wrap serialized JSON text.
    pub(crate) fn from_string(text: String) -> Self {
        Self(text.into_boxed_str())
    }

    /// Borrow the JSON text for decoding.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The serialized success output of one operation, as it would arrive on the
/// wire. Used by the mock's `ReplyOk` program; the handle decodes it into the
/// operation's paired `Output`.
pub(crate) type RawOut = RawJson;

/// One erased call handed to the backend. The handle has already done the type
/// work — serialized the input and resolved the envelope `op_id` — so the
/// backend sees only bytes plus the routing facts the kernel needs.
pub(crate) struct ErasedCall {
    /// The operation's wire name (`O::PATH`).
    pub(crate) op: &'static str,
    /// Whether the operation mutates (`O::MUTATING`) — the kernel's replay
    /// ledger (#168) consults this; retained here so the trait stays
    /// sufficient for it without a later breaking change.
    #[allow(dead_code)]
    pub(crate) mutating: bool,
    /// The envelope-level deduplication key, from [`Dedup`](crate::Dedup).
    #[allow(dead_code)]
    pub(crate) op_id: Option<OpId>,
    /// The serialized `in` object. Consumed by a real transport backend (#168);
    /// the reference mock routes on `op` alone, so this is retained for the
    /// kernel rather than read here.
    #[allow(dead_code)]
    pub(crate) input: RawJson,
}

/// The internal, object-safe backend the [`ClientHandle`](crate::ClientHandle)
/// wraps as `Arc<dyn ClientBackend>`.
///
/// Object safety is load-bearing: it is what lets one cloneable concrete
/// handle erase any of the four adapters (`WsWeb`, `WsNative`, `DirectClient`,
/// the mock) behind a single `Arc`. A generic method here would break `dyn`;
/// the `_assert_object_safe` guard below fails the build if that ever regresses.
pub(crate) trait ClientBackend: Send + Sync {
    /// Dispatch one erased call, resolving to erased reply bytes or a
    /// classified [`CallError`]. The future is `'static` so the handle can
    /// await it without borrowing the backend.
    fn dispatch(&self, call: ErasedCall) -> BoxFuture<'static, Result<RawJson, CallError>>;

    /// Register an independent event subscription (the fan-out, §D3).
    fn subscribe(&self) -> EventSubscription;

    /// A snapshot of the current lifecycle [`State`].
    fn state(&self) -> State;

    /// Begin connecting/activating. Idempotent (§D6).
    fn start(&self);

    /// Graceful shutdown: settle all accepted work, close all event streams,
    /// then resolve (§D6).
    fn stop(&self) -> BoxFuture<'static, ()>;
}

/// Compile-time proof that [`ClientBackend`] stays object-safe. If a future
/// change adds a generic method or a `Self: Sized`-free-but-unsizable
/// signature, `dyn ClientBackend` stops coercing and this fails to compile.
#[allow(dead_code)]
fn _assert_object_safe(backend: &dyn ClientBackend) -> &dyn ClientBackend {
    backend
}
