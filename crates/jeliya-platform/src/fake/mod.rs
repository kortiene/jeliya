//! The deterministic in-process fakes (#174 §6, AC-6) — the only
//! implementations this crate ships.
//!
//! One [`FakePlatform`] implements every capability, deterministically: **no
//! wall clock, no RNG, no reliance on task-scheduling order**. A scripted
//! picker / dialog / share stays **open** until the test advances it via
//! [`FakeController::deliver_next`] — the controller-driven settlement
//! discipline of `jeliya-client`'s mock — so cancellation-vs-reply ordering is
//! explicit, never a race; the outcome is bound at call time, so release order
//! can never re-pair outcomes with calls. Three target-shaped constructors —
//! [`browser`], [`desktop`], [`android`] — carry each target's structural
//! facts (durability, availability, source kind, streamed staging) so a shared
//! component compiles and behaves against all three without a device.
//!
//! The fakes add no dependencies: everything here is `std` plus the crate's own
//! types, so the behaviour is identical on `wasm32` and native — the discipline
//! `jeliya-client`'s mock uses.

pub mod recorder;
pub mod shapes;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::cancel::{CancelToken, Cancelled};
use crate::clipboard::{Clipboard, Share, ShareAttachment, ShareContent};
use crate::error::{Availability, CapabilityError, FailureKind};
use crate::files::{
    ArtifactToken, ExportTarget, ExportTargetKind, FetchedArtifact, FileName, FileObjectKind,
    FileSink, Files, Mime, PickedSource, ProgressSink, ShareSink, ShareableBlob, SourceToken,
    StageProgress, StagedBlobReader,
};
use crate::launcher::{SafeExternalUrl, UrlLauncher};
use crate::lifecycle::{Lifecycle, LifecycleBus, LifecycleEvent, LifecycleSubscription};
use crate::navigation::{Navigation, Route};
use crate::services::{Platform, PlatformServices};
use crate::storage::{
    Durability, PreferenceKey, Preferences, PrivateDirectory, Secret, SecretKey, SecretStore,
    WriteOutcome,
};
use crate::window::{WindowActions, WindowCommand};
use crate::BoxFuture;

pub use recorder::{Capability, RecordedEffect, Recorder, Script};
pub use shapes::Shape;

/// The bounded read-buffer chunk size the fake copies through. Deliberately
/// tiny so a small test file still exercises multiple chunks (streaming and
/// mid-copy limit enforcement); the contract is "bounded, never the whole file
/// at once", which any positive chunk size satisfies.
const STAGE_CHUNK_BYTES: usize = 8;

/// Distinct ids handed out to fakes, packed into the high half of every token
/// they mint.
///
/// A token therefore carries **issuer provenance**: two fakes running the same
/// call sequence no longer mint the same numbers, so one service's handle can
/// never resolve in another's registries — the cross-service half of the §K4
/// fail-closed gate, which a per-instance counter alone cannot give. Ids start
/// at 1, so every minted token has a nonzero high half and can never collide
/// with a small forged raw value.
static NEXT_SERVICE_ID: AtomicU64 = AtomicU64::new(1);

/// The bytes and streaming disposition behind one picked source.
struct SourceBody {
    bytes: Arc<Vec<u8>>,
    /// A `content://` source is streamed: its size is treated as unknown up
    /// front, so the limit is enforced mid-copy rather than before it.
    streamed: bool,
}

/// The shared state behind a fake and its controller.
struct FakeInner {
    shape: Shape,
    recorder: Mutex<Recorder>,
    script: Mutex<Script>,
    prefs: Mutex<BTreeMap<PreferenceKey, String>>,
    secrets: Mutex<BTreeMap<SecretKey, Secret>>,
    route: Mutex<Route>,
    lifecycle: LifecycleBus,
    sources: Mutex<HashMap<u64, SourceBody>>,
    /// Export-target tokens this service minted → their kind. `export_sink`
    /// resolves the target through here, so a forged [`ExportTarget`] fails.
    export_targets: Mutex<HashMap<u64, ExportTargetKind>>,
    /// Staged-blob tokens this service minted → the staged bytes (size is the
    /// length). `read_staged` and `share` resolve a [`ShareableBlob`] through
    /// here, so a blob the service did not stage can neither be read nor
    /// shared (the anti-forgery half of §K4).
    staged_blobs: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
    /// Fetched-artifact tokens this service minted → the committed bytes — the
    /// fake's stand-in for the private staging area a [`ShareSink`] commits
    /// into. `share` resolves a [`FetchedArtifact`] through here (and consumes
    /// it on success), so an artifact this service did not materialize is not
    /// shareable.
    share_artifacts: Mutex<HashMap<u64, Vec<u8>>>,
    pending_pick: Mutex<Option<PickedSource>>,
    pending_export: Mutex<Option<ExportTarget>>,
    /// Tokens whose bytes an in-flight share is holding: a claimed fetched
    /// artifact, or a staged blob a sheet is keeping alive. They are out of (or
    /// independent of) the registries above, so `retained_handles` has to union
    /// them in — otherwise it reports zero while a pending share still owns
    /// file bytes, which is exactly the false clean bill a cleanup test would
    /// believe.
    ///
    /// There is one of these per registry, and the rule is uniform: **any guard
    /// that holds an entry out of its registry registers here for as long as it
    /// does.** That covers an in-flight share's claim, a staged blob held by an
    /// open sheet or an open (or merely pending) reader, and a cleanup that has
    /// reserved its entry but not yet completed — all of which are the service
    /// still owning the resource.
    in_flight_sources: Mutex<HashMap<u64, usize>>,
    in_flight_export_targets: Mutex<HashMap<u64, usize>>,
    in_flight_artifacts: Mutex<HashMap<u64, usize>>,
    in_flight_blobs: Mutex<HashMap<u64, usize>>,
    /// The open scripted dialogs awaiting controller advancement (§6).
    dialogs: Mutex<DialogQueue>,
    /// This fake's issuer id — see [`NEXT_SERVICE_ID`].
    service_id: u64,
    next_id: AtomicU64,
}

impl FakeInner {
    fn new(shape: Shape) -> Self {
        Self {
            shape,
            recorder: Mutex::new(Recorder::default()),
            script: Mutex::new(Script::default()),
            prefs: Mutex::new(BTreeMap::new()),
            secrets: Mutex::new(BTreeMap::new()),
            route: Mutex::new(Route::Rooms),
            lifecycle: LifecycleBus::new(),
            sources: Mutex::new(HashMap::new()),
            export_targets: Mutex::new(HashMap::new()),
            staged_blobs: Mutex::new(HashMap::new()),
            share_artifacts: Mutex::new(HashMap::new()),
            in_flight_sources: Mutex::new(HashMap::new()),
            in_flight_export_targets: Mutex::new(HashMap::new()),
            in_flight_artifacts: Mutex::new(HashMap::new()),
            in_flight_blobs: Mutex::new(HashMap::new()),
            pending_pick: Mutex::new(None),
            pending_export: Mutex::new(None),
            dialogs: Mutex::new(DialogQueue::default()),
            service_id: NEXT_SERVICE_ID.fetch_add(1, Ordering::Relaxed),
            next_id: AtomicU64::new(1),
        }
    }

    /// Mint the next opaque id for this service: the issuer id in the high
    /// half, a per-service counter in the low half. Packed rather than drawn
    /// from one flat global counter so a *forged* small raw token (the §K4
    /// forgery test's `7777`) can never collide with a minted id no matter how
    /// many tokens the process mints. Determinism is untouched — token values
    /// are opaque and never recorded or asserted on.
    fn next_id(&self) -> u64 {
        let local = self.next_id.fetch_add(1, Ordering::Relaxed);
        debug_assert!(local < 1 << 32, "fake token space exhausted");
        debug_assert!(self.service_id < 1 << 32, "fake service space exhausted");
        (self.service_id << 32) | local
    }

    /// Note one more in-flight hold of `token` in `which`.
    fn hold_enter(which: &Mutex<HashMap<u64, usize>>, token: u64) {
        *which
            .lock()
            .expect("in-flight holds poisoned")
            .entry(token)
            .or_insert(0) += 1;
    }

    /// Release one in-flight hold of `token`, dropping the entry at zero.
    fn hold_exit(which: &Mutex<HashMap<u64, usize>>, token: u64) {
        // Best-effort under poisoning, as everywhere in a Drop path.
        if let Ok(mut holds) = which.lock() {
            if let Some(count) = holds.get_mut(&token) {
                *count -= 1;
                if *count == 0 {
                    holds.remove(&token);
                }
            }
        }
    }

    fn record(&self, effect: RecordedEffect) {
        self.recorder
            .lock()
            .expect("recorder poisoned")
            .record(effect);
    }

    fn take_forced(&self, capability: Capability) -> Option<CapabilityError> {
        self.script
            .lock()
            .expect("script poisoned")
            .take(capability)
    }

    fn write_outcome(&self) -> WriteOutcome {
        let session_only = self
            .script
            .lock()
            .expect("script poisoned")
            .force_session_only()
            || self.shape.durability() == Durability::SessionScoped;
        if session_only {
            WriteOutcome::SessionOnly
        } else {
            WriteOutcome::Durable
        }
    }

    /// Whether a [`ShareContent`]'s attachment (if any) names bytes this
    /// service holds. An attachment this service never minted is not
    /// shareable — the anti-forgery half of §K4, checked against the
    /// minted-token registries. Matched exhaustively on purpose: a new
    /// attachment kind must decide how it resolves rather than default to
    /// "shareable".
    /// Hold a staged blob's bytes for the lifetime of one share sheet.
    ///
    /// A presence check alone would be a lie by the time the sheet settles:
    /// `release_staged` can reap the blob while the sheet is still open, and
    /// the share would then report success for bytes the service no longer
    /// has. Holding the `Arc` keeps them alive for exactly this operation —
    /// the same "an open reader outlives the unlink" model `read_staged` has —
    /// so a share that opened against real bytes stays honest. Returns `None`
    /// for a blob this service did not stage.
    fn hold_staged(self: &Arc<Self>, blob: &ShareableBlob) -> Option<BlobHold> {
        self.hold_staged_token(blob.token().get())
    }

    /// [`FakeInner::hold_staged`] by raw token.
    fn hold_staged_token(self: &Arc<Self>, token: u64) -> Option<BlobHold> {
        let bytes = self
            .staged_blobs
            .lock()
            .expect("staged blobs poisoned")
            .get(&token)
            .cloned()?;
        FakeInner::hold_enter(&self.in_flight_blobs, token);
        Some(BlobHold {
            inner: Arc::clone(self),
            token,
            bytes,
        })
    }

    /// Take a fetched artifact's bytes out of the registry for the duration of
    /// one share, in **call order**.
    ///
    /// The claim is the anti-forgery check and the consume-on-success gate at
    /// once, and taking it here rather than at settle time is what makes the
    /// outcome independent of poll order: with two shares of one artifact, the
    /// first *call* claims it and the second fails immediately without ever
    /// opening a sheet, however the executor later polls them. Returns `None`
    /// for an artifact this service did not materialize or has already handed
    /// to another in-flight share.
    fn claim_artifact(self: &Arc<Self>, artifact: &FetchedArtifact) -> Option<ArtifactClaim> {
        let token = artifact.token().get();
        let bytes = self
            .share_artifacts
            .lock()
            .expect("share artifacts poisoned")
            .remove(&token)?;
        FakeInner::hold_enter(&self.in_flight_artifacts, token);
        Some(ArtifactClaim {
            inner: Arc::clone(self),
            token,
            bytes: Some(bytes),
        })
    }

    /// Register an open dialog (ticket order == call order) and return the
    /// parked turn its operation awaits. The caller bound its outcome BEFORE
    /// registering — the mock's dispatch discipline — so release order can
    /// never re-pair outcomes with calls.
    fn open_dialog(self: &Arc<Self>, capability: Capability, ct: &CancelToken) -> DialogTurn {
        // The guard is a statement temporary: it is dropped at the end of this
        // `let`, so the waker loop below runs lock-free.
        let (ticket, wakers) = self
            .dialogs
            .lock()
            .expect("dialogs poisoned")
            .open(capability);
        for waker in wakers {
            waker.wake();
        }
        DialogTurn {
            inner: Arc::clone(self),
            ticket,
            done: false,
            cancelled: ct.cancelled(),
        }
    }
}

/// The open scripted dialogs, in call order, plus parked
/// [`FakeController::pending_dialog`] waiters — the fake's analogue of the
/// mock's `pending`/`pending_wakers` pair.
#[derive(Default)]
struct DialogQueue {
    open: Vec<OpenDialog>,
    /// Keyed per waiter: a re-poll replaces its slot, a dropped waiter
    /// unregisters.
    waiters: HashMap<u64, Waker>,
    next_ticket: u64,
    next_waiter: u64,
}

/// One scripted dialog awaiting controller advancement.
struct OpenDialog {
    ticket: u64,
    capability: Capability,
    /// Set by [`DialogQueue::deliver_next`]; consumed by the operation's next
    /// poll.
    released: bool,
    /// The operation's parked waker, when it has polled and found itself
    /// unreleased.
    waker: Option<Waker>,
}

impl DialogQueue {
    /// Register a dialog and return its ticket plus the waiters to wake.
    ///
    /// **The wakers are returned, not woken here.** Every waker in this fake is
    /// invoked only after the queue lock is released: an inline-executor waker
    /// that immediately re-polls would otherwise re-enter and re-lock this
    /// non-reentrant mutex on the same thread. That is the discipline
    /// `jeliya-client`'s event bus fixed in `event.rs` (collect under the lock,
    /// wake after the guard drops), and the fake follows it so it stays
    /// executor-agnostic.
    fn open(&mut self, capability: Capability) -> (u64, Vec<Waker>) {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.open.push(OpenDialog {
            ticket,
            capability,
            released: false,
            waker: None,
        });
        // A parked pending_dialog() waiter resumes BECAUSE a dialog opened.
        let wakers = self.waiters.drain().map(|(_, waker)| waker).collect();
        (ticket, wakers)
    }

    fn withdraw(&mut self, ticket: u64) {
        self.open.retain(|dialog| dialog.ticket != ticket);
    }

    fn take_released(&mut self, ticket: u64) -> bool {
        match self
            .open
            .iter()
            .position(|dialog| dialog.ticket == ticket && dialog.released)
        {
            Some(index) => {
                self.open.remove(index);
                true
            }
            None => false,
        }
    }

    fn park(&mut self, ticket: u64, waker: Waker) {
        if let Some(dialog) = self.open.iter_mut().find(|dialog| dialog.ticket == ticket) {
            dialog.waker = Some(waker);
        }
    }

    /// Release the oldest un-advanced dialog, returning whether one was
    /// released and the waker to invoke **after** the caller drops the queue
    /// guard (see [`DialogQueue::open`]).
    fn deliver_next(&mut self) -> (bool, Option<Waker>) {
        match self.open.iter_mut().find(|dialog| !dialog.released) {
            Some(dialog) => {
                dialog.released = true;
                (true, dialog.waker.take())
            }
            None => (false, None),
        }
    }
}

/// The parked half of one open scripted dialog: `Ok(())` when the controller
/// releases it ([`FakeController::deliver_next`]), `Err(Cancelled)` when the
/// token fires first. Dropping it withdraws the dialog — drop-is-abort
/// (§D12/K2).
struct DialogTurn {
    inner: Arc<FakeInner>,
    ticket: u64,
    done: bool,
    cancelled: Cancelled,
}

impl Future for DialogTurn {
    type Output = Result<(), CapabilityError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // Cancellation wins every race with a release: "cancellation must not
        // become success" biases ties toward Cancelled, deterministically.
        if Pin::new(&mut this.cancelled).poll(cx).is_ready() {
            this.done = true;
            this.inner
                .dialogs
                .lock()
                .expect("dialogs poisoned")
                .withdraw(this.ticket);
            return Poll::Ready(Err(CapabilityError::Cancelled));
        }
        let mut queue = this.inner.dialogs.lock().expect("dialogs poisoned");
        if queue.take_released(this.ticket) {
            this.done = true;
            return Poll::Ready(Ok(()));
        }
        queue.park(this.ticket, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for DialogTurn {
    fn drop(&mut self) {
        if !self.done {
            // Best-effort under poisoning: the panic being unwound is the
            // real failure.
            if let Ok(mut queue) = self.inner.dialogs.lock() {
                queue.withdraw(self.ticket);
            }
        }
    }
}

/// Which private registry a [`BoundEntry`] guard reaps from.
#[derive(Clone, Copy)]
enum BoundRegistry {
    Sources,
    ExportTargets,
}

/// Reaps a picker-bound private entry unless its handle actually reaches the
/// caller.
///
/// `pick` takes the armed source out of `pending_pick` at call time, so between
/// then and settlement the only reference to that file/blob/URI grant is inside
/// this operation. If the dialog is cancelled or the future dropped, the caller
/// never receives the [`PickedSource`] and so can never call
/// [`Files::discard_source`] — nothing else would ever reap it. Delivering the
/// handle calls [`BoundEntry::keep`]; every other exit drops the guard, which
/// reaps. `pick_export_target` uses the same guard for its target.
struct BoundEntry {
    inner: Arc<FakeInner>,
    registry: BoundRegistry,
    token: u64,
    live: bool,
}

impl BoundEntry {
    fn new(inner: &Arc<FakeInner>, registry: BoundRegistry, token: u64) -> Self {
        Self {
            inner: Arc::clone(inner),
            registry,
            token,
            live: true,
        }
    }

    /// The handle reached the caller: it is theirs to consume or discard.
    fn keep(mut self) {
        self.live = false;
    }
}

impl Drop for BoundEntry {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        // Best-effort under poisoning, as everywhere in Drop.
        match self.registry {
            BoundRegistry::Sources => {
                if let Ok(mut sources) = self.inner.sources.lock() {
                    sources.remove(&self.token);
                }
            }
            BoundRegistry::ExportTargets => {
                if let Ok(mut targets) = self.inner.export_targets.lock() {
                    targets.remove(&self.token);
                }
            }
        }
    }
}

/// A fetched artifact held out of the registry while one share is in flight.
///
/// Dropped without [`ArtifactClaim::consume`] it puts the bytes back, so a
/// dismissed, cancelled, scripted-failure, or simply dropped share leaves the
/// artifact staged for a retry that needs no second download — the same
/// drop-is-abort honesty [`DialogTurn`] has. Only a share that actually
/// completes consumes it.
struct ArtifactClaim {
    inner: Arc<FakeInner>,
    token: u64,
    bytes: Option<Vec<u8>>,
}

impl ArtifactClaim {
    /// How many bytes this claim is holding — recorded at settlement, which is
    /// what makes the hold observable rather than merely modelled.
    fn len(&self) -> Option<u64> {
        self.bytes.as_ref().map(|bytes| bytes.len() as u64)
    }

    /// The share completed: the bytes are gone for good ("delete after share").
    fn consume(mut self) {
        self.bytes = None;
    }
}

impl Drop for ArtifactClaim {
    fn drop(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            // Best-effort under poisoning, as everywhere in Drop.
            if let Ok(mut artifacts) = self.inner.share_artifacts.lock() {
                artifacts.insert(self.token, bytes);
            }
        }
        FakeInner::hold_exit(&self.inner.in_flight_artifacts, self.token);
    }
}

/// A picked source a staging call has taken out of the registry and is copying
/// from.
///
/// Staging consumes its source on every outcome, so this does **not** restore
/// on drop the way [`CleanupClaim`] does — but the service is genuinely still
/// holding the file/blob/URI grant for as long as the copy is in flight, so it
/// registers in `in_flight_sources` and releases when the future completes or
/// is dropped.
struct StagingSource {
    inner: Arc<FakeInner>,
    token: u64,
    body: SourceBody,
}

impl StagingSource {
    fn new(inner: &Arc<FakeInner>, token: u64, body: SourceBody) -> Self {
        FakeInner::hold_enter(&inner.in_flight_sources, token);
        Self {
            inner: Arc::clone(inner),
            token,
            body,
        }
    }
}

impl Drop for StagingSource {
    fn drop(&mut self) {
        FakeInner::hold_exit(&self.inner.in_flight_sources, self.token);
    }
}

/// One registry entry a cleanup has reserved. Held out of its registry for the
/// duration of the call so two cleanups of the same handle are decided by CALL
/// order, not by which future the executor polls first — the fake's
/// scheduling-independent contract.
enum ReservedEntry {
    Source(SourceBody),
    Artifact(Vec<u8>),
    ExportTarget(ExportTargetKind),
    StagedBlob(Arc<Vec<u8>>),
}

impl ReservedEntry {
    /// The in-flight map this kind of entry is counted in while reserved.
    fn in_flight_map<'a>(&self, inner: &'a Arc<FakeInner>) -> &'a Mutex<HashMap<u64, usize>> {
        match self {
            ReservedEntry::Source(_) => &inner.in_flight_sources,
            ReservedEntry::Artifact(_) => &inner.in_flight_artifacts,
            ReservedEntry::ExportTarget(_) => &inner.in_flight_export_targets,
            ReservedEntry::StagedBlob(_) => &inner.in_flight_blobs,
        }
    }
}

/// A reserved cleanup entry. Dropped without [`CleanupClaim::consume`] it puts
/// the entry back, so a scripted failure or a dropped future leaves the handle
/// exactly as it was and the caller can retry — the whole reason cleanup is
/// fallible and borrows its handle.
struct CleanupClaim {
    inner: Arc<FakeInner>,
    token: u64,
    entry: Option<ReservedEntry>,
}

impl CleanupClaim {
    /// Reserve `entry`, counting it as retained until this claim is consumed or
    /// dropped: a cleanup that has taken the entry out but not yet completed is
    /// still the service owning that resource.
    fn new(inner: &Arc<FakeInner>, token: u64, entry: ReservedEntry) -> Self {
        FakeInner::hold_enter(entry.in_flight_map(inner), token);
        Self {
            inner: Arc::clone(inner),
            token,
            entry: Some(entry),
        }
    }

    /// The cleanup succeeded: the entry is gone for good.
    fn consume(mut self) {
        self.entry = None;
    }
}

impl Drop for CleanupClaim {
    fn drop(&mut self) {
        let Some(entry) = self.entry.take() else {
            // Consumed: the entry is gone, so the hold ends with it. `consume`
            // cleared the payload but the count is still ours to release.
            FakeInner::hold_exit(&self.inner.in_flight_sources, self.token);
            FakeInner::hold_exit(&self.inner.in_flight_export_targets, self.token);
            FakeInner::hold_exit(&self.inner.in_flight_artifacts, self.token);
            FakeInner::hold_exit(&self.inner.in_flight_blobs, self.token);
            return;
        };
        FakeInner::hold_exit(entry.in_flight_map(&self.inner), self.token);
        // Best-effort under poisoning, as everywhere in Drop.
        match entry {
            ReservedEntry::Source(body) => {
                if let Ok(mut m) = self.inner.sources.lock() {
                    m.insert(self.token, body);
                }
            }
            ReservedEntry::Artifact(bytes) => {
                if let Ok(mut m) = self.inner.share_artifacts.lock() {
                    m.insert(self.token, bytes);
                }
            }
            ReservedEntry::ExportTarget(kind) => {
                if let Ok(mut m) = self.inner.export_targets.lock() {
                    m.insert(self.token, kind);
                }
            }
            ReservedEntry::StagedBlob(bytes) => {
                if let Ok(mut m) = self.inner.staged_blobs.lock() {
                    m.insert(self.token, bytes);
                }
            }
        }
    }
}

/// A staged blob's bytes kept alive for the lifetime of one share sheet, and
/// counted as retained while it lives — see [`FakeInner::hold_staged`].
struct BlobHold {
    inner: Arc<FakeInner>,
    token: u64,
    bytes: Arc<Vec<u8>>,
}

impl Drop for BlobHold {
    fn drop(&mut self) {
        FakeInner::hold_exit(&self.inner.in_flight_blobs, self.token);
    }
}

/// The deterministic fake implementing every capability. Constructed via
/// [`browser`], [`desktop`], or [`android`].
pub struct FakePlatform {
    inner: Arc<FakeInner>,
}

/// How many distinct tokens a registry and its in-flight holds name between
/// them. A union rather than a sum: a blob can be both registered and held by
/// an open sheet, and it is one set of bytes either way.
fn union_len<V>(
    registry: &Mutex<HashMap<u64, V>>,
    in_flight: &Mutex<HashMap<u64, usize>>,
    poison: &str,
) -> usize {
    let registry = registry.lock().expect(poison);
    let in_flight = in_flight.lock().expect("in-flight holds poisoned");
    registry
        .keys()
        .chain(in_flight.keys())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// The private entries a fake is still holding — see
/// [`FakeController::retained_handles`]. All zero means the service is holding
/// no file object, grant, or staged byte on the caller's behalf.
///
/// **What is counted, and why that is the line.** These are the resources a
/// caller can strand *by doing nothing*: each is named by an opaque handle, and
/// forgetting the handle leaves the service owning the thing forever. So every
/// registry is counted, and so is every guard that holds an entry *out* of its
/// registry while an operation is in flight — an in-flight share's claim, a
/// staged blob held by an open sheet or by an open or merely pending reader, a
/// cleanup that has reserved its entry, a staging copy that has taken its
/// source.
///
/// An open [`FileSink`] or [`ShareSink`] is deliberately **not** counted, and
/// the distinction is the point: the caller holds the sink itself, so dropping
/// it *is* the release (§D12/K2 drop-is-abort). There is no handle to forget
/// and nothing to strand, which is exactly what makes it a different kind of
/// resource from the ones above.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RetainedHandles {
    /// Picked sources whose grant the service still owns.
    pub sources: usize,
    /// Export targets whose write grant the service still owns.
    pub export_targets: usize,
    /// Staged blobs whose bytes the service still holds.
    pub staged_blobs: usize,
    /// Fetched artifacts the service has materialized and not yet released.
    pub share_artifacts: usize,
}

/// A test-side handle that arms scripted outcomes, drives lifecycle events, and
/// reads the recorded effects of a fake.
#[derive(Clone)]
pub struct FakeController {
    inner: Arc<FakeInner>,
}

/// The browser-shaped fake and its controller.
pub fn browser() -> (PlatformServices, FakeController) {
    build(Shape::Browser)
}

/// The desktop-shaped fake and its controller.
pub fn desktop() -> (PlatformServices, FakeController) {
    build(Shape::Desktop)
}

/// The Android-shaped fake and its controller.
pub fn android() -> (PlatformServices, FakeController) {
    build(Shape::Android)
}

fn build(shape: Shape) -> (PlatformServices, FakeController) {
    let inner = Arc::new(FakeInner::new(shape));
    let platform: Arc<dyn Platform> = Arc::new(FakePlatform {
        inner: inner.clone(),
    });
    (PlatformServices::new(platform), FakeController { inner })
}

impl FakeController {
    /// The shape this fake models.
    pub fn shape(&self) -> Shape {
        self.inner.shape
    }

    /// Arm the next [`Files::pick`] to return a source of the shape's kind,
    /// carrying `bytes` for a later [`Files::stage_for_share`].
    ///
    /// # Panics
    ///
    /// Panics if `name` is not a valid [`FileName`]. Controller seeds are
    /// test-authored, so an invalid seed is a test bug worth failing fast on —
    /// unlike a peer-supplied name, which the contract rejects as an error.
    pub fn arm_pick(&self, name: &str, mime: Option<Mime>, bytes: Vec<u8>) {
        self.arm_pick_of_kind(name, self.inner.shape.source_kind(), mime, bytes);
    }

    /// Arm the next [`Files::pick`] with an explicit object kind (to exercise a
    /// cross-shape source).
    ///
    /// # Panics
    ///
    /// Panics if `name` is not a valid [`FileName`] (see [`arm_pick`](Self::arm_pick)).
    pub fn arm_pick_of_kind(
        &self,
        name: &str,
        kind: FileObjectKind,
        mime: Option<Mime>,
        bytes: Vec<u8>,
    ) {
        let id = self.inner.next_id();
        let size = bytes.len() as u64;
        self.inner.sources.lock().expect("sources poisoned").insert(
            id,
            SourceBody {
                bytes: Arc::new(bytes),
                streamed: matches!(kind, FileObjectKind::ContentUri),
            },
        );
        let source = PickedSource::new(
            SourceToken::new(id),
            FileName::parse(name).expect("armed file name must be a valid FileName"),
            size,
            mime,
            kind,
        );
        // Re-arming replaces the pending handle, and the one it replaces can
        // never reach a caller — so nothing could ever discard it. Reap it
        // here, or setup code that revises an armed result inflates
        // `retained_handles` for the rest of the fake's life.
        let replaced = self
            .inner
            .pending_pick
            .lock()
            .expect("pick poisoned")
            .replace(source);
        if let Some(replaced) = replaced {
            self.inner
                .sources
                .lock()
                .expect("sources poisoned")
                .remove(&replaced.token().get());
        }
    }

    /// Arm the next [`Files::pick_export_target`] to return a target of `kind`.
    ///
    /// # Panics
    ///
    /// Panics if `suggested` is not a valid [`FileName`] (see
    /// [`arm_pick`](Self::arm_pick)).
    pub fn arm_export_target(&self, kind: ExportTargetKind, suggested: &str) {
        let id = self.inner.next_id();
        // Register the minted token so a later `export_sink` can resolve the
        // destination through it — a forged target has no entry.
        self.inner
            .export_targets
            .lock()
            .expect("export targets poisoned")
            .insert(id, kind);
        let target = ExportTarget::new(
            crate::files::ExportToken::new(id),
            kind,
            FileName::parse(suggested).expect("armed export name must be a valid FileName"),
        );
        // Same overwrite reap as `arm_pick`.
        let replaced = self
            .inner
            .pending_export
            .lock()
            .expect("export poisoned")
            .replace(target);
        if let Some(replaced) = replaced {
            self.inner
                .export_targets
                .lock()
                .expect("export targets poisoned")
                .remove(&replaced.token().get());
        }
    }

    /// Force the next call of `capability` to fail with `error` (the
    /// denied / unavailable / cancelled / typed-failure paths).
    pub fn force_error(&self, capability: Capability, error: CapabilityError) {
        self.inner
            .script
            .lock()
            .expect("script poisoned")
            .force(capability, error);
    }

    /// Make preference/secret writes report [`WriteOutcome::SessionOnly`] even
    /// on a persistent shape (§K6), until called again with `false`.
    ///
    /// A durability-regime latch, not a one-shot script entry: it models
    /// storage that stopped persisting, so it spans both stores and every
    /// write. To observe recovery, clear it and write again.
    pub fn force_writes_session_only(&self, force: bool) {
        self.inner
            .script
            .lock()
            .expect("script poisoned")
            .set_force_session_only(force);
    }

    /// Emit a lifecycle event to every subscriber.
    pub fn emit_lifecycle(&self, event: LifecycleEvent) {
        self.inner.lifecycle.emit(event);
    }

    /// Close the lifecycle bus (every subscription ends after draining).
    pub fn close_lifecycle(&self) {
        self.inner.lifecycle.close();
    }

    /// Release the oldest un-advanced open dialog so its bound outcome settles
    /// on the operation's next poll — the fake's `MockController::deliver_next`.
    /// Returns `false` when no dialog is awaiting advancement.
    pub fn deliver_next(&self) -> bool {
        // Same wake-after-unlock discipline as `open_dialog`: the guard is a
        // statement temporary, so a waker that synchronously re-polls the
        // released operation cannot deadlock on the queue lock.
        let (delivered, waker) = self
            .inner
            .dialogs
            .lock()
            .expect("dialogs poisoned")
            .deliver_next();
        if let Some(waker) = waker {
            waker.wake();
        }
        delivered
    }

    /// How many private entries the service is still holding: picked sources,
    /// export targets, staged blobs, and fetched artifacts.
    ///
    /// Every one of these is a real resource on a target — an open `File`, a
    /// SAF write grant, a staging file — so "did this operation let go of it?"
    /// is a question a test must be able to ask directly. Without this the
    /// retention rules are invisible from outside the fake, and a leak looks
    /// exactly like a clean abort.
    pub fn retained_handles(&self) -> RetainedHandles {
        RetainedHandles {
            sources: union_len(
                &self.inner.sources,
                &self.inner.in_flight_sources,
                "sources poisoned",
            ),
            export_targets: union_len(
                &self.inner.export_targets,
                &self.inner.in_flight_export_targets,
                "export targets poisoned",
            ),
            staged_blobs: union_len(
                &self.inner.staged_blobs,
                &self.inner.in_flight_blobs,
                "staged blobs poisoned",
            ),
            share_artifacts: union_len(
                &self.inner.share_artifacts,
                &self.inner.in_flight_artifacts,
                "share artifacts poisoned",
            ),
        }
    }

    /// The dialogs currently open, in call order — diagnostics for ordering
    /// and drop-withdrawal assertions.
    pub fn open_dialogs(&self) -> Vec<Capability> {
        self.inner
            .dialogs
            .lock()
            .expect("dialogs poisoned")
            .open
            .iter()
            .map(|dialog| dialog.capability)
            .collect()
    }

    /// Resolve once at least one scripted dialog is open and **not yet
    /// released** — that is, once [`FakeController::deliver_next`] would
    /// actually advance something. The event-driven complement to
    /// `deliver_next` for drivers sharing an executor with the code under test
    /// (the mock's `pending_call` discipline).
    ///
    /// A released-but-unsettled dialog is deliberately not counted: it is
    /// already spoken for, so resolving on it would hand a controller loop a
    /// dialog it cannot deliver. It does still appear in
    /// [`FakeController::open_dialogs`], which reports what is open rather than
    /// what is deliverable.
    pub fn pending_dialog(&self) -> PendingDialog {
        PendingDialog {
            inner: self.inner.clone(),
            waiter: None,
        }
    }

    /// Every recorded effect, in order.
    pub fn effects(&self) -> Vec<RecordedEffect> {
        self.inner
            .recorder
            .lock()
            .expect("recorder poisoned")
            .effects()
    }

    /// Every external URL opened, in order.
    pub fn opened_urls(&self) -> Vec<SafeExternalUrl> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::OpenedUrl { url } => Some(url),
                _ => None,
            })
            .collect()
    }

    /// The most recent clipboard write, if any.
    pub fn last_clipboard(&self) -> Option<String> {
        self.effects()
            .into_iter()
            .rev()
            .find_map(|effect| match effect {
                RecordedEffect::ClipboardWrite { text } => Some(text),
                _ => None,
            })
    }

    /// Every staged blob's `(size, bytes)`, in order. Empty when a stage was
    /// cancelled or failed — a partial stage leaves nothing behind.
    pub fn staged(&self) -> Vec<(u64, Vec<u8>)> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::Staged { size, bytes } => Some((size, bytes)),
                _ => None,
            })
            .collect()
    }

    /// Every pushed navigation, in order.
    pub fn navigations(&self) -> Vec<Route> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::Navigated { route } => Some(route),
                _ => None,
            })
            .collect()
    }

    /// Every replacing navigation (#178 D4), in order — the canonicalizing
    /// redirects that must not grow Back history.
    pub fn replaced_navigations(&self) -> Vec<Route> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::NavigatedReplace { route } => Some(route),
                _ => None,
            })
            .collect()
    }

    /// Every window command invoked, in order.
    pub fn window_commands(&self) -> Vec<WindowCommand> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::Window { command } => Some(command),
                _ => None,
            })
            .collect()
    }
}

// ---- Platform accessor supertrait ---------------------------------------

impl Platform for FakePlatform {
    fn files(&self) -> &dyn Files {
        self
    }
    fn preferences(&self) -> &dyn Preferences {
        self
    }
    fn secret_store(&self) -> &dyn SecretStore {
        self
    }
    fn private_directory(&self) -> &dyn PrivateDirectory {
        self
    }
    fn lifecycle(&self) -> &dyn Lifecycle {
        self
    }
    fn url_launcher(&self) -> &dyn UrlLauncher {
        self
    }
    fn clipboard(&self) -> &dyn Clipboard {
        self
    }
    fn share(&self) -> &dyn Share {
        self
    }
    fn navigation(&self) -> &dyn Navigation {
        self
    }
    fn window(&self) -> &dyn WindowActions {
        self
    }
}

// ---- Files --------------------------------------------------------------

impl Files for FakePlatform {
    fn pick(
        &self,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<PickedSource>, CapabilityError>> {
        let inner = self.inner.clone();
        // A pre-fired token never opens the dialog and consumes nothing (the
        // operation never starts — the mock's dispatch-after-stop rule).
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        // Bind this call's outcome NOW (call order); settle it on advancement.
        let bound = match inner.take_forced(Capability::Pick) {
            Some(error) => Err(error),
            None => Ok(inner.pending_pick.lock().expect("pick poisoned").take()),
        };
        // The armed source is out of `pending_pick` now, so guard its private
        // entry until the handle actually reaches the caller.
        let mut guard = match &bound {
            Ok(Some(source)) => Some(BoundEntry::new(
                &inner,
                BoundRegistry::Sources,
                source.token().get(),
            )),
            _ => None,
        };
        let turn = inner.open_dialog(Capability::Pick, ct);
        Box::pin(async move {
            turn.await?;
            match bound? {
                Some(source) => {
                    if let Some(guard) = guard.take() {
                        guard.keep();
                    }
                    inner.record(RecordedEffect::Picked {
                        name: source.display_name().as_str().to_owned(),
                    });
                    Ok(Some(source))
                }
                // A clean no-selection (not a dismissal): the platform reported
                // no file without the user cancelling. A dismissal is scripted
                // as `Cancelled` via the controller instead — and either way,
                // the outcome is delivered only on controller advancement.
                None => Ok(None),
            }
        })
    }

    fn discard_source(&self, src: &PickedSource) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let token = src.token();
        // Reserve the entry NOW, in call order: two cleanups of the same
        // handle must not be decided by which future is polled first, and a
        // handle this service does not hold reports its own `Unreadable`
        // without consuming a script scripted for the next valid cleanup.
        let reserved = inner
            .sources
            .lock()
            .expect("sources poisoned")
            .remove(&token.get());
        let Some(entry) = reserved else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        let claim = CleanupClaim::new(&inner, token.get(), ReservedEntry::Source(entry));
        // Bound at CALL time, like every other scripted outcome. A failed
        // cleanup drops the claim, which puts the entry back so the caller can
        // retry.
        let bound = inner.take_forced(Capability::Release);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            claim.consume();
            inner.record(RecordedEffect::DiscardedSource);
            Ok(())
        })
    }

    fn release_artifact(
        &self,
        artifact: &FetchedArtifact,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let token = artifact.token();
        // Reserve the entry NOW, in call order: two cleanups of the same
        // handle must not be decided by which future is polled first, and a
        // handle this service does not hold reports its own `Unreadable`
        // without consuming a script scripted for the next valid cleanup.
        let reserved = inner
            .share_artifacts
            .lock()
            .expect("share artifacts poisoned")
            .remove(&token.get());
        let Some(entry) = reserved else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        let claim = CleanupClaim::new(&inner, token.get(), ReservedEntry::Artifact(entry));
        // Bound at CALL time, like every other scripted outcome. A failed
        // cleanup drops the claim, which puts the entry back so the caller can
        // retry.
        let bound = inner.take_forced(Capability::Release);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            claim.consume();
            inner.record(RecordedEffect::ReleasedArtifact);
            Ok(())
        })
    }

    fn discard_export_target(
        &self,
        target: &ExportTarget,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let token = target.token();
        // Reserve the entry NOW, in call order: two cleanups of the same
        // handle must not be decided by which future is polled first, and a
        // handle this service does not hold reports its own `Unreadable`
        // without consuming a script scripted for the next valid cleanup.
        let reserved = inner
            .export_targets
            .lock()
            .expect("export targets poisoned")
            .remove(&token.get());
        let Some(entry) = reserved else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        let claim = CleanupClaim::new(&inner, token.get(), ReservedEntry::ExportTarget(entry));
        // Bound at CALL time, like every other scripted outcome. A failed
        // cleanup drops the claim, which puts the entry back so the caller can
        // retry.
        let bound = inner.take_forced(Capability::Release);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            claim.consume();
            inner.record(RecordedEffect::DiscardedExportTarget);
            Ok(())
        })
    }

    fn stage_for_share(
        &self,
        src: &PickedSource,
        limit: u64,
        progress: ProgressSink,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<ShareableBlob, CapabilityError>> {
        let inner = self.inner.clone();
        // Claim the source FIRST, synchronously, because `src` was taken by
        // value and no caller can reach `discard_source` for it afterwards.
        // Every exit below — a pre-fired token, a scripted failure, a
        // cancellation, or a dropped future — then drops the body and the
        // service's private file/blob/URI grant with it, which is what
        // "consumed whatever the outcome" has to mean.
        let claimed = inner
            .sources
            .lock()
            .expect("sources poisoned")
            .remove(&src.token().get());
        // A pre-fired token never starts the operation and consumes no SCRIPT —
        // the dispatch-after-stop rule `pick` documents, applied uniformly.
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        // Neither does a call that cannot legitimately start. A source this
        // service never produced (or already staged, or discarded) reports its
        // own failure, and a known-oversize one fails BEFORE any copy exactly
        // as §D5 says — so neither steals the outcome scripted for the next
        // real staging call.
        let Some(body) = claimed else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        // The grant is out of the registry but the service still holds it for
        // as long as this copy is in flight — including before the future is
        // first polled. Same rule as every other guard: hold it out, count it.
        let body = StagingSource::new(&inner, src.token().get(), body);
        let known_len = (!body.body.streamed).then(|| body.body.bytes.len() as u64);
        if let Some(len) = known_len {
            // Both known-size rejections happen here, before the script: a
            // source whose size is known up front fails on its own terms
            // without stealing the outcome scripted for the next real call.
            // A streamed source has neither figure yet, so its emptiness and
            // its limit are both judged during the copy.
            if len == 0 {
                return Box::pin(async { Err(CapabilityError::Failed(FailureKind::FileEmpty)) });
            }
            if len > limit {
                return Box::pin(async move {
                    Err(CapabilityError::Failed(FailureKind::FileTooLarge {
                        size: len,
                        limit,
                    }))
                });
            }
        }
        // Bind this call's scripted outcome NOW, in call order: a script says
        // "the next call fails", so poll order must not re-pair it with a
        // different call. The clone is still needed for the mid-copy checks.
        let bound = inner.take_forced(Capability::Stage);
        let ct = ct.clone();
        Box::pin(async move {
            // Defence in depth: the call-time gate cannot see a token fired
            // between the call and the first poll. Cancellation wins the tie,
            // as it does in `DialogTurn::poll`.
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            let bytes = Arc::clone(&body.body.bytes);
            // The known-size limit was already enforced at call time, before
            // any copy, so the mid-copy check below can only ever fire for a
            // streamed source (`content://`), which has no authoritative size
            // up front. `total` is `None` for exactly that case.
            let total = known_len;
            let mut copied: u64 = 0;
            let mut staged: Vec<u8> = Vec::new();
            for chunk in bytes.chunks(STAGE_CHUNK_BYTES) {
                copied += chunk.len() as u64;
                // Mid-copy enforcement: abort the instant the running total
                // would exceed the limit, before accumulating this chunk. The
                // partial `staged` is dropped here (never recorded), which is
                // the fake's "delete the partial staged file".
                if copied > limit {
                    return Err(CapabilityError::Failed(FailureKind::FileTooLarge {
                        size: copied,
                        limit,
                    }));
                }
                // Accumulate BEFORE reporting: `transferred` is documented as
                // bytes copied so far, and a progress sink that cancels would
                // otherwise be told a chunk landed that never entered the
                // partial artifact. The partial is still discarded on every
                // early return — it is only recorded on success.
                staged.extend_from_slice(chunk);
                // Report progress BEFORE the cancel check, so a progress sink
                // that fires the token mid-copy is observed on the very next
                // check — the deterministic "cancel mid-stream" path.
                progress.report(StageProgress {
                    transferred: copied,
                    total,
                });
                if ct.is_cancelled() {
                    return Err(CapabilityError::Cancelled);
                }
            }
            if copied == 0 {
                return Err(CapabilityError::Failed(FailureKind::FileEmpty));
            }
            let id = inner.next_id();
            inner
                .staged_blobs
                .lock()
                .expect("staged blobs poisoned")
                .insert(id, Arc::new(staged.clone()));
            inner.record(RecordedEffect::Staged {
                size: copied,
                bytes: staged,
            });
            Ok(ShareableBlob::new(crate::files::BlobToken::new(id), copied))
        })
    }

    fn read_staged(
        &self,
        blob: &ShareableBlob,
    ) -> BoxFuture<'_, Result<Box<dyn StagedBlobReader>, CapabilityError>> {
        let inner = self.inner.clone();
        let token = blob.token();
        // Token resolution stays inside the service, and comes FIRST: a blob
        // this service did not stage (a forged token, another service's blob,
        // one already released) fails closed on its own and must not consume
        // an open failure scripted for the next valid open — the same
        // validation-before-script rule the share sheet and the zero-bound
        // pull follow.
        // The hold is taken the moment the `Arc` is cloned, not when the
        // future is polled: an unpolled open already owns the bytes and can
        // still yield a working reader later, so it is already the service
        // holding them. The guard's `Drop` covers a future that is never
        // polled at all.
        let Some(hold) = inner.hold_staged_token(token.get()) else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        // Opening is its own failure moment, before any byte is pulled.
        let bound = inner.take_forced(Capability::ReadStaged);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            Ok(Box::new(FakeStagedReader { hold, offset: 0 }) as Box<dyn StagedBlobReader>)
        })
    }

    fn release_staged(&self, blob: &ShareableBlob) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let token = blob.token();
        // Reserve the entry NOW, in call order: two cleanups of the same
        // handle must not be decided by which future is polled first, and a
        // handle this service does not hold reports its own `Unreadable`
        // without consuming a script scripted for the next valid cleanup.
        let reserved = inner
            .staged_blobs
            .lock()
            .expect("staged blobs poisoned")
            .remove(&token.get());
        let Some(entry) = reserved else {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Unreadable)) });
        };
        let claim = CleanupClaim::new(&inner, token.get(), ReservedEntry::StagedBlob(entry));
        // Bound at CALL time, like every other scripted outcome. A failed
        // cleanup drops the claim, which puts the entry back so the caller can
        // retry.
        let bound = inner.take_forced(Capability::Release);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            claim.consume();
            inner.record(RecordedEffect::ReleasedStaged);
            Ok(())
        })
    }

    fn pick_export_target(
        &self,
        suggested: FileName,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>> {
        let inner = self.inner.clone();
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        // Bind at call time, settle on advancement — same discipline as pick.
        let bound = match inner.take_forced(Capability::PickExport) {
            Some(error) => Err(error),
            None => Ok(inner.pending_export.lock().expect("export poisoned").take()),
        };
        let mut guard = match &bound {
            Ok(Some(target)) => Some(BoundEntry::new(
                &inner,
                BoundRegistry::ExportTargets,
                target.token().get(),
            )),
            _ => None,
        };
        // Record what the CALLER asked for, not just what the controller armed:
        // otherwise a component that suggests the wrong download name looks
        // correct, because the fake answers with the armed target regardless.
        inner.record(RecordedEffect::RequestedExportName {
            suggested: suggested.as_str().to_owned(),
        });
        let turn = inner.open_dialog(Capability::PickExport, ct);
        Box::pin(async move {
            turn.await?;
            match bound? {
                Some(target) => {
                    if let Some(guard) = guard.take() {
                        guard.keep();
                    }
                    inner.record(RecordedEffect::PickedExport {
                        kind: target.kind(),
                    });
                    Ok(Some(target))
                }
                None => Ok(None),
            }
        })
    }

    fn export_sink(
        &self,
        to: &ExportTarget,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>> {
        let inner = self.inner.clone();
        // Claim the destination FIRST, for the same reason `stage_for_share`
        // claims its source: `to` was taken by value, so no caller can reach
        // `discard_export_target` for it after this, and every exit below must
        // therefore drop the grant rather than strand it.
        let kind = to.kind();
        let claimed = inner
            .export_targets
            .lock()
            .expect("export targets poisoned")
            .remove(&to.token().get());
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        // A forged, re-used, or discarded target reports its own fail-closed
        // `Io` and consumes no script — the same rule as everywhere else.
        if claimed.is_none() {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Io)) });
        }
        let bound = inner.take_forced(Capability::ExportSink);
        let ct = ct.clone();
        Box::pin(async move {
            // Defence in depth: the call-time gate cannot see a token fired
            // between the call and the first poll. Cancellation wins the tie,
            // as it does in `DialogTurn::poll`.
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            Ok(Box::new(FakeFileSink {
                inner,
                dest: SinkDest::Export { kind },
                written: Vec::new(),
                ct,
            }) as Box<dyn FileSink>)
        })
    }

    fn open_sink(
        &self,
        name: FileName,
        declared: Option<Mime>,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn FileSink>, CapabilityError>> {
        let inner = self.inner.clone();
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = inner.take_forced(Capability::OpenSink);
        let ct = ct.clone();
        Box::pin(async move {
            // Defence in depth: the call-time gate cannot see a token fired
            // between the call and the first poll. Cancellation wins the tie,
            // as it does in `DialogTurn::poll`.
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            Ok(Box::new(FakeFileSink {
                inner,
                dest: SinkDest::Open { name, declared },
                written: Vec::new(),
                ct,
            }) as Box<dyn FileSink>)
        })
    }

    fn share_sink(
        &self,
        name: FileName,
        declared: Option<Mime>,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Box<dyn ShareSink>, CapabilityError>> {
        let inner = self.inner.clone();
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = inner.take_forced(Capability::ShareSink);
        let ct = ct.clone();
        Box::pin(async move {
            // Defence in depth: the call-time gate cannot see a token fired
            // between the call and the first poll. Cancellation wins the tie,
            // as it does in `DialogTurn::poll`.
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            Ok(Box::new(FakeShareSink {
                inner,
                name,
                declared,
                written: Vec::new(),
                ct,
            }) as Box<dyn ShareSink>)
        })
    }

    fn share_content(
        &self,
        content: &ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        self.gated_share(Capability::ShareContent, content, ct)
    }
}

/// The destination one [`FakeFileSink`] commits to.
enum SinkDest {
    /// A consumed export target of this kind.
    Export { kind: ExportTargetKind },
    /// The platform opener, with the peer-declared (untrusted) type.
    Open {
        name: FileName,
        declared: Option<Mime>,
    },
}

/// The fake's [`FileSink`]: accumulates written chunks in memory and records
/// the committed artifact. Dropped uncommitted, it records **nothing** — the
/// fake's "delete the partial artifact" (§D12/K2).
struct FakeFileSink {
    inner: Arc<FakeInner>,
    dest: SinkDest,
    written: Vec<u8>,
    /// The token the sink was opened under. A transfer is a bounded copy
    /// (§D12), so cancellation must reach it *after* it starts — otherwise a
    /// mid-copy cancel becomes a published artifact, and an adapter that keeps
    /// advancing CREDIT past a fired token looks correct against this fake.
    ct: CancelToken,
}

impl FileSink for FakeFileSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>> {
        // Bound at call time, like every other scripted outcome. A destination
        // that fails partway through is the path a client adapter must handle
        // differently from one that never opens — stop advancing CREDIT, abort
        // the stream, drop the artifact — so it has to be scriptable, and the
        // chunk that failed is NOT accumulated.
        // A token already fired before this call means the operation never
        // starts, so it consumes no script — the same dispatch-after-stop rule
        // the rest of the file follows. The in-body check below still covers a
        // token that fires between the call and the first poll.
        if self.ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = self.inner.take_forced(Capability::FileSinkWrite);
        Box::pin(async move {
            // Cancellation reaches the transfer, not just its opening: a
            // fired token means this chunk must not be accepted and the
            // caller must stop advancing CREDIT.
            if self.ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            self.written.extend_from_slice(&chunk);
            Ok(())
        })
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), CapabilityError>> {
        // A destination that took every byte and then failed to finalize is its
        // own path: the caller has nothing left to send and must still not
        // report success. Nothing is recorded, so the artifact was never
        // published.
        // A token already fired before this call means the operation never
        // starts, so it consumes no script — the same dispatch-after-stop rule
        // the rest of the file follows. The in-body check below still covers a
        // token that fires between the call and the first poll.
        if self.ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = self.inner.take_forced(Capability::FileSinkCommit);
        let this_ct = self.ct.clone();
        Box::pin(async move {
            // A cancelled transfer must not publish: the artifact is dropped
            // exactly as it is when the sink is dropped uncommitted.
            if this_ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            let this = *self;
            match this.dest {
                SinkDest::Export { kind } => this.inner.record(RecordedEffect::ExportedLocal {
                    kind,
                    bytes: this.written,
                }),
                SinkDest::Open { name, declared } => {
                    this.inner.record(RecordedEffect::OpenedLocal {
                        name,
                        declared,
                        bytes: this.written,
                    });
                }
            }
            Ok(())
        })
    }
}

/// The fake's [`ShareSink`]: accumulates the pumped `file.read` bytes in
/// memory and, on commit, materializes them in the service's registry — the
/// fake's stand-in for a protected staging directory. Dropped uncommitted it
/// records **nothing** and mints no handle, the same drop-is-abort honesty
/// [`FakeFileSink`] has (§D12/K2).
struct FakeShareSink {
    inner: Arc<FakeInner>,
    name: FileName,
    declared: Option<Mime>,
    written: Vec<u8>,
    /// See [`FakeFileSink::ct`].
    ct: CancelToken,
}

impl ShareSink for FakeShareSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), CapabilityError>> {
        // Same mid-transfer failure path as [`FakeFileSink::write`].
        // A token already fired before this call means the operation never
        // starts, so it consumes no script — the same dispatch-after-stop rule
        // the rest of the file follows. The in-body check below still covers a
        // token that fires between the call and the first poll.
        if self.ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = self.inner.take_forced(Capability::ShareSinkWrite);
        Box::pin(async move {
            // Cancellation reaches the transfer, not just its opening: a
            // fired token means this chunk must not be accepted and the
            // caller must stop advancing CREDIT.
            if self.ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            self.written.extend_from_slice(&chunk);
            Ok(())
        })
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<FetchedArtifact, CapabilityError>> {
        // Same finalization path as [`FakeFileSink::commit`]: no artifact is
        // minted and nothing is recorded, so a failed staging commit cannot be
        // mistaken for a shareable artifact.
        // A token already fired before this call means the operation never
        // starts, so it consumes no script — the same dispatch-after-stop rule
        // the rest of the file follows. The in-body check below still covers a
        // token that fires between the call and the first poll.
        if self.ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        let bound = self.inner.take_forced(Capability::ShareSinkCommit);
        let this_ct = self.ct.clone();
        Box::pin(async move {
            // A cancelled transfer must not publish: the artifact is dropped
            // exactly as it is when the sink is dropped uncommitted.
            if this_ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if let Some(error) = bound {
                return Err(error);
            }
            let this = *self;
            let id = this.inner.next_id();
            let size = this.written.len() as u64;
            this.inner.record(RecordedEffect::StagedFetched {
                name: this.name.as_str().to_owned(),
                declared: this.declared,
                bytes: this.written.clone(),
            });
            this.inner
                .share_artifacts
                .lock()
                .expect("share artifacts poisoned")
                .insert(id, this.written);
            Ok(FetchedArtifact::new(
                ArtifactToken::new(id),
                this.name,
                size,
            ))
        })
    }
}

/// The fake's [`StagedBlobReader`]: bounded pulls over the staged bytes. The
/// per-pull bound is `max_len` capped by [`STAGE_CHUNK_BYTES`], so even a
/// generous caller exercises multiple chunks — the same "bounded, never the
/// whole file at once" contract the staging copy holds.
struct FakeStagedReader {
    /// The bytes, plus the retention hold on them. An open reader deliberately
    /// outlives `release_staged` (an fd outliving an unlink), so it is still
    /// the service holding those bytes; the guard keeps `retained_handles`
    /// honest for exactly as long as that is true, and releases on drop.
    hold: BlobHold,
    offset: usize,
}

impl StagedBlobReader for FakeStagedReader {
    fn size(&self) -> u64 {
        self.hold.bytes.len() as u64
    }

    fn next_chunk(
        &mut self,
        max_len: usize,
    ) -> BoxFuture<'_, Result<Option<Vec<u8>>, CapabilityError>> {
        // The source can fail after the reader is open — a staging file reaped
        // or a native read erroring partway through the upload. Bound at call
        // time like every other scripted outcome, and the position is not
        // consumed, so the failure is observable without corrupting the read.
        // A zero bound is always a caller error, so it is judged BEFORE the
        // script is dequeued: an invalid pull must not consume a scripted
        // source failure meant for the next real read.
        if max_len == 0 {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Io)) });
        }
        let bound = self.hold.inner.take_forced(Capability::StagedReadChunk);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            // Kept for position-independence even though the call-time guard
            // above already rejects it: a
            // zero bound would otherwise yield an endless run of empty
            // non-EOF chunks with the offset never advancing — a credit-driven
            // puller with no credit spinning instead of waiting. Neither
            // `Ok(None)` (a lie about EOF) nor a clamp (a lie about "at most
            // max_len") is honest, so it fails.
            if max_len == 0 {
                return Err(CapabilityError::Failed(FailureKind::Io));
            }
            if self.offset >= self.hold.bytes.len() {
                return Ok(None);
            }
            let len = max_len
                .min(STAGE_CHUNK_BYTES)
                .min(self.hold.bytes.len() - self.offset);
            let chunk = self.hold.bytes[self.offset..self.offset + len].to_vec();
            self.offset += len;
            Ok(Some(chunk))
        })
    }
}

// ---- Preferences / SecretStore / PrivateDirectory -----------------------

impl Preferences for FakePlatform {
    fn get(&self, key: &PreferenceKey) -> Option<String> {
        self.inner
            .prefs
            .lock()
            .expect("prefs poisoned")
            .get(key)
            .cloned()
    }

    fn set(&self, key: PreferenceKey, value: &str) -> WriteOutcome {
        // The in-memory change stands regardless of durability (honest "applies
        // this session"); the outcome tells the caller whether it was written.
        self.inner
            .prefs
            .lock()
            .expect("prefs poisoned")
            .insert(key.clone(), value.to_owned());
        self.inner.record(RecordedEffect::PreferenceWrite {
            key,
            value: Some(value.to_owned()),
        });
        self.inner.write_outcome()
    }

    fn remove(&self, key: &PreferenceKey) -> WriteOutcome {
        self.inner.prefs.lock().expect("prefs poisoned").remove(key);
        self.inner.record(RecordedEffect::PreferenceWrite {
            key: key.clone(),
            value: None,
        });
        self.inner.write_outcome()
    }

    fn durability(&self) -> Durability {
        self.inner.shape.durability()
    }
}

impl SecretStore for FakePlatform {
    fn get(&self, key: &SecretKey) -> Option<Secret> {
        self.inner
            .secrets
            .lock()
            .expect("secrets poisoned")
            .get(key)
            .cloned()
    }

    fn set(&self, key: SecretKey, secret: Secret) -> WriteOutcome {
        self.inner
            .secrets
            .lock()
            .expect("secrets poisoned")
            .insert(key.clone(), secret);
        // Record the key only — never the secret value (§K1/§K5).
        self.inner.record(RecordedEffect::SecretWrite { key });
        self.inner.write_outcome()
    }

    fn remove(&self, key: &SecretKey) -> WriteOutcome {
        self.inner
            .secrets
            .lock()
            .expect("secrets poisoned")
            .remove(key);
        self.inner.write_outcome()
    }

    fn durability(&self) -> Durability {
        self.inner.shape.durability()
    }
}

impl PrivateDirectory for FakePlatform {
    fn availability(&self) -> Availability {
        self.inner.shape.private_directory_availability()
    }

    fn is_backup_excluded(&self) -> Result<bool, CapabilityError> {
        if self
            .inner
            .shape
            .private_directory_availability()
            .is_available()
        {
            Ok(self.inner.shape.private_directory_backup_excluded())
        } else {
            Err(CapabilityError::Unavailable)
        }
    }

    fn is_owned_by_daemon(&self) -> Result<bool, CapabilityError> {
        if self
            .inner
            .shape
            .private_directory_availability()
            .is_available()
        {
            Ok(true)
        } else {
            Err(CapabilityError::Unavailable)
        }
    }
}

// ---- Lifecycle ----------------------------------------------------------

impl Lifecycle for FakePlatform {
    fn subscribe(&self) -> LifecycleSubscription {
        self.inner.lifecycle.subscribe()
    }
}

// ---- UrlLauncher / Clipboard / Share ------------------------------------

impl UrlLauncher for FakePlatform {
    fn open_external(&self, url: SafeExternalUrl) -> Result<(), CapabilityError> {
        if let Some(error) = self.inner.take_forced(Capability::OpenExternal) {
            return Err(error);
        }
        self.inner.record(RecordedEffect::OpenedUrl { url });
        Ok(())
    }
}

impl Clipboard for FakePlatform {
    fn write_text(&self, text: &str) -> BoxFuture<'_, Result<(), CapabilityError>> {
        // Not a dialog (§6 scopes controller advancement to picker/dialog/
        // share), so the outcome resolves on first poll — but only through
        // the returned future, matching how a browser denial arrives as the
        // writeText promise's rejection. The outcome is nonetheless BOUND at
        // call time: a script says "the next call fails", so two futures
        // created in call order must keep their outcomes however they are
        // polled.
        let inner = self.inner.clone();
        let text = text.to_owned();
        let bound = inner.take_forced(Capability::Clipboard);
        Box::pin(async move {
            if let Some(error) = bound {
                return Err(error);
            }
            inner.record(RecordedEffect::ClipboardWrite { text });
            Ok(())
        })
    }
}

impl FakePlatform {
    /// Shared share-sheet gate for [`Files::share_content`] and
    /// [`Share::share`]: validation precedes the sheet (empty or forged
    /// content never opens a dialog); forced and armed outcomes bind at call
    /// time and settle only on controller advancement.
    fn gated_share(
        &self,
        capability: Capability,
        content: &ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        if ct.is_cancelled() {
            return Box::pin(async { Err(CapabilityError::Cancelled) });
        }
        // Validation precedes the script, not just the sheet. An operation that
        // never starts must consume nothing — the same rule a pre-fired token
        // follows — so a caller error leaves a queued outcome for the next
        // share that actually starts, instead of silently eating it and
        // masking the empty-content or anti-forgery failure in a test.
        if content.is_empty() {
            return Box::pin(async { Err(CapabilityError::Failed(FailureKind::Io)) });
        }
        // Matched exhaustively on purpose: a new attachment kind must decide
        // how it resolves rather than default to "shareable".
        let mut held: Option<BlobHold> = None;
        let claim = match &content.attachment {
            None => None,
            // A staged blob is NOT consumed by sharing — the daemon's
            // `file.share` reads it, and `release_staged` is what reaps it — so
            // presence is the whole check.
            Some(ShareAttachment::Blob(blob)) => match inner.hold_staged(blob) {
                // Held, not consumed: the daemon's `file.share` reads a staged
                // blob and `release_staged` reaps it, so sharing one twice is
                // legitimate — but the bytes must not vanish mid-sheet.
                Some(bytes) => {
                    held = Some(bytes);
                    None
                }
                None => {
                    return Box::pin(async {
                        Err(CapabilityError::Failed(FailureKind::Unreadable))
                    })
                }
            },
            // A fetched artifact IS consumed, so the claim is taken now, in
            // call order, and released again if this share does not complete.
            Some(ShareAttachment::Fetched(artifact)) => match inner.claim_artifact(artifact) {
                Some(claim) => Some(claim),
                None => {
                    return Box::pin(async {
                        Err(CapabilityError::Failed(FailureKind::Unreadable))
                    })
                }
            },
        };
        let bound = match inner.take_forced(capability) {
            Some(error) => Err(error),
            None => Ok(content.clone()),
        };
        let turn = inner.open_dialog(capability, ct);
        Box::pin(async move {
            // Every early return below drops `claim`, which puts the artifact
            // back: a dismissal, a cancellation, a scripted failure, or a
            // dropped future all leave it staged for a retry.
            turn.await?;
            let content = bound?;
            // Read the size from the hold, not from the registry: that is the
            // point of holding it. A blob released while this sheet was open is
            // gone from the registry, and the share is still honest because
            // these bytes never went anywhere.
            let attached_bytes = held
                .as_ref()
                .map(|held| held.bytes.len() as u64)
                .or_else(|| claim.as_ref().and_then(ArtifactClaim::len));
            if let Some(claim) = claim {
                claim.consume();
            }
            // The hold ends with the sheet.
            drop(held);
            inner.record(RecordedEffect::Shared {
                content,
                attached_bytes,
            });
            Ok(())
        })
    }
}

impl Share for FakePlatform {
    fn share(
        &self,
        content: &ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        self.gated_share(Capability::Share, content, ct)
    }
}

// ---- Navigation ---------------------------------------------------------

impl Navigation for FakePlatform {
    fn route(&self) -> Route {
        self.inner.route.lock().expect("route poisoned").clone()
    }

    fn navigate(&self, route: Route) {
        *self.inner.route.lock().expect("route poisoned") = route.clone();
        self.inner.record(RecordedEffect::Navigated { route });
    }

    fn navigate_replace(&self, route: Route) {
        *self.inner.route.lock().expect("route poisoned") = route.clone();
        self.inner
            .record(RecordedEffect::NavigatedReplace { route });
    }

    fn hand_back_to_platform(&self) {
        self.inner.record(RecordedEffect::HandBackToPlatform);
    }
}

// ---- WindowActions ------------------------------------------------------

impl FakePlatform {
    fn window_command(&self, command: WindowCommand) -> Result<(), CapabilityError> {
        // Structural availability first: a browser has no window to act on, and
        // that is a shape fact rather than a scripted one.
        if !self.inner.shape.window_availability().is_available() {
            return Err(CapabilityError::Unavailable);
        }
        // …then the scripted action outcome. A window manager can refuse a
        // request that is perfectly available (a compositor denying a raise, a
        // close vetoed by the OS), and §6 promises every capability method's
        // action outcome is forceable — this one was not.
        if let Some(error) = self.inner.take_forced(Capability::WindowAction) {
            return Err(error);
        }
        self.inner.record(RecordedEffect::Window { command });
        Ok(())
    }
}

impl WindowActions for FakePlatform {
    fn availability(&self) -> Availability {
        self.inner.shape.window_availability()
    }

    fn minimize(&self) -> Result<(), CapabilityError> {
        self.window_command(WindowCommand::Minimize)
    }

    fn set_title(&self, title: &str) -> Result<(), CapabilityError> {
        self.window_command(WindowCommand::SetTitle(title.to_owned()))
    }

    fn request_close(&self) -> Result<(), CapabilityError> {
        self.window_command(WindowCommand::RequestClose)
    }

    fn request_exit(&self) -> Result<(), CapabilityError> {
        self.window_command(WindowCommand::RequestExit)
    }

    fn focus(&self) -> Result<(), CapabilityError> {
        self.window_command(WindowCommand::Focus)
    }
}

// ---- PendingDialog -------------------------------------------------------

/// Future returned by [`FakeController::pending_dialog`]: ready when at least
/// one scripted dialog is open and **not yet released** — that is, one
/// [`FakeController::deliver_next`] would actually advance.
pub struct PendingDialog {
    inner: Arc<FakeInner>,
    /// This waiter's slot, once parked: a re-poll REPLACES it (no duplicates),
    /// readiness clears it, `Drop` unregisters it — the keyed-waker discipline
    /// of the mock's `PendingCall`.
    waiter: Option<u64>,
}

impl Future for PendingDialog {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let mut queue = this.inner.dialogs.lock().expect("dialogs poisoned");
        // Only a dialog still AWAITING advancement counts. A released entry
        // lingers in `open` until its operation's next poll removes it, and it
        // is already spoken for: `deliver_next` skips it, so resolving here
        // would hand a controller loop a dialog it cannot deliver.
        if queue.open.iter().any(|dialog| !dialog.released) {
            if let Some(id) = this.waiter.take() {
                queue.waiters.remove(&id);
            }
            return Poll::Ready(());
        }
        let id = match this.waiter {
            Some(id) => id,
            None => {
                let id = queue.next_waiter;
                queue.next_waiter += 1;
                this.waiter = Some(id);
                id
            }
        };
        queue.waiters.insert(id, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for PendingDialog {
    fn drop(&mut self) {
        if let Some(id) = self.waiter.take() {
            // Best-effort under poisoning, as everywhere in Drop.
            if let Ok(mut queue) = self.inner.dialogs.lock() {
                queue.waiters.remove(&id);
            }
        }
    }
}
