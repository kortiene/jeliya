//! The deterministic in-process fakes (#174 §6, AC-6) — the only
//! implementations this crate ships.
//!
//! One [`FakePlatform`] implements every capability, deterministically: **no
//! wall clock, no RNG, no reliance on task-scheduling order**. A scripted
//! picker / dialog / share resolves to the outcome the test armed on its
//! [`FakeController`]; nothing depends on when a task is polled. Three
//! target-shaped constructors — [`browser`], [`desktop`], [`android`] — carry
//! each target's structural facts (durability, availability, source kind,
//! streamed staging) so a shared component compiles and behaves against all
//! three without a device.
//!
//! The fakes add no dependencies: everything here is `std` plus the crate's own
//! types, so the behaviour is identical on `wasm32` and native — the discipline
//! `jeliya-client`'s mock uses.

pub mod recorder;
pub mod shapes;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::cancel::CancelToken;
use crate::clipboard::{Clipboard, Share, ShareAttachment, ShareContent};
use crate::error::{Availability, CapabilityError, FailureKind};
use crate::files::{
    ExportTarget, ExportTargetKind, FileName, FileObjectKind, Files, LocalFileRef, Mime,
    PickedSource, ProgressSink, ShareableBlob, SourceToken, StageProgress,
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
    /// Export-target tokens this service minted → their kind. `export_local`
    /// resolves the target through here, so a forged [`ExportTarget`] fails.
    export_targets: Mutex<HashMap<u64, ExportTargetKind>>,
    /// Staged-blob tokens this service minted → their size. `share` resolves a
    /// [`ShareableBlob`] through here, so a blob the service did not stage
    /// cannot be shared (the anti-forgery half of §K4).
    staged_blobs: Mutex<HashMap<u64, u64>>,
    pending_pick: Mutex<Option<PickedSource>>,
    pending_export: Mutex<Option<ExportTarget>>,
    next_id: AtomicU64,
    /// The internal daemon/session token. It is used only to resolve local-file
    /// URLs inside the service and is **never** exposed on any public type
    /// (§K5).
    token: Secret,
}

impl FakeInner {
    fn new(shape: Shape) -> Self {
        Self {
            shape,
            recorder: Mutex::new(Recorder::default()),
            script: Mutex::new(Script::default()),
            prefs: Mutex::new(BTreeMap::new()),
            secrets: Mutex::new(BTreeMap::new()),
            route: Mutex::new(Route::Root),
            lifecycle: LifecycleBus::new(),
            sources: Mutex::new(HashMap::new()),
            export_targets: Mutex::new(HashMap::new()),
            staged_blobs: Mutex::new(HashMap::new()),
            pending_pick: Mutex::new(None),
            pending_export: Mutex::new(None),
            next_id: AtomicU64::new(1),
            token: Secret::new("fake-native-token"),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
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

    /// Resolve a [`LocalFileRef`] to its token-carrying URL **inside the
    /// service**. The returned string is used to drive the platform opener and
    /// is never recorded, returned, or placed on any component-facing type — so
    /// the daemon token stays native (§K5).
    /// Whether a [`ShareContent`]'s blob attachment (if any) was staged by this
    /// service. A blob this service never minted is not shareable — the
    /// anti-forgery half of §K4, checked against the minted-token registry.
    fn blob_is_staged(&self, content: &ShareContent) -> bool {
        match &content.attachment {
            Some(ShareAttachment::Blob(blob)) => self
                .staged_blobs
                .lock()
                .expect("staged blobs poisoned")
                .contains_key(&blob.token().get()),
            _ => true,
        }
    }

    fn resolve_local_url(&self, file: &LocalFileRef) -> String {
        format!(
            "/api/files/local?room={}&file={}&token={}",
            file.room_id(),
            file.file_id(),
            self.token.expose()
        )
    }
}

/// The deterministic fake implementing every capability. Constructed via
/// [`browser`], [`desktop`], or [`android`].
pub struct FakePlatform {
    inner: Arc<FakeInner>,
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
    pub fn arm_pick(&self, name: &str, mime: Option<Mime>, bytes: Vec<u8>) {
        self.arm_pick_of_kind(name, self.inner.shape.source_kind(), mime, bytes);
    }

    /// Arm the next [`Files::pick`] with an explicit object kind (to exercise a
    /// cross-shape source).
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
        let source = PickedSource::new(SourceToken::new(id), FileName::new(name), size, mime, kind);
        *self.inner.pending_pick.lock().expect("pick poisoned") = Some(source);
    }

    /// Arm the next [`Files::pick_export_target`] to return a target of `kind`.
    pub fn arm_export_target(&self, kind: ExportTargetKind, suggested: &str) {
        let id = self.inner.next_id();
        // Register the minted token so a later `export_local` can resolve the
        // destination through it — a forged target has no entry.
        self.inner
            .export_targets
            .lock()
            .expect("export targets poisoned")
            .insert(id, kind);
        let target = ExportTarget::new(
            crate::files::ExportToken::new(id),
            kind,
            FileName::new(suggested),
        );
        *self.inner.pending_export.lock().expect("export poisoned") = Some(target);
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

    /// Make the next preference/secret writes report
    /// [`WriteOutcome::SessionOnly`] even on a persistent shape (§K6).
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

    /// Every navigation, in order.
    pub fn navigations(&self) -> Vec<Route> {
        self.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                RecordedEffect::Navigated { route } => Some(route),
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
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::Pick) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            match inner.pending_pick.lock().expect("pick poisoned").take() {
                Some(source) => {
                    inner.record(RecordedEffect::Picked {
                        name: source.display_name().as_str().to_owned(),
                    });
                    Ok(Some(source))
                }
                // A clean no-selection (not a dismissal): the platform reported
                // no file without the user cancelling. A dismissal is scripted
                // as `Cancelled` via the controller instead.
                None => Ok(None),
            }
        })
    }

    fn stage_for_share(
        &self,
        src: PickedSource,
        limit: u64,
        progress: ProgressSink,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<ShareableBlob, CapabilityError>> {
        let inner = self.inner.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::Stage) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            let (bytes, streamed) = {
                let sources = inner.sources.lock().expect("sources poisoned");
                match sources.get(&src.token().get()) {
                    Some(body) => (body.bytes.clone(), body.streamed),
                    // The source vanished before staging.
                    None => return Err(CapabilityError::Failed(FailureKind::Unreadable)),
                }
            };
            let len = bytes.len() as u64;
            // Known-size sources are rejected BEFORE any copy; a streamed source
            // (`content://`) has no authoritative size up front, so its limit is
            // enforced during the copy instead.
            if !streamed && len > limit {
                return Err(CapabilityError::Failed(FailureKind::FileTooLarge {
                    size: len,
                    limit,
                }));
            }
            let total = if streamed { None } else { Some(len) };
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
                staged.extend_from_slice(chunk);
            }
            if copied == 0 {
                return Err(CapabilityError::Failed(FailureKind::FileEmpty));
            }
            let id = inner.next_id();
            inner
                .staged_blobs
                .lock()
                .expect("staged blobs poisoned")
                .insert(id, copied);
            inner.record(RecordedEffect::Staged {
                size: copied,
                bytes: staged,
            });
            Ok(ShareableBlob::new(crate::files::BlobToken::new(id), copied))
        })
    }

    fn pick_export_target(
        &self,
        suggested: FileName,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<Option<ExportTarget>, CapabilityError>> {
        let inner = self.inner.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::PickExport) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            let _ = suggested;
            match inner.pending_export.lock().expect("export poisoned").take() {
                Some(target) => {
                    inner.record(RecordedEffect::PickedExport {
                        kind: target.kind(),
                    });
                    Ok(Some(target))
                }
                None => Ok(None),
            }
        })
    }

    fn export_local(
        &self,
        file: LocalFileRef,
        to: ExportTarget,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::ExportLocal) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            // Resolve the destination through the minted-token registry; a
            // forged `ExportTarget` (one this service never produced) has no
            // entry and cannot be written to.
            let kind = to.kind();
            if inner
                .export_targets
                .lock()
                .expect("export targets poisoned")
                .remove(&to.token().get())
                .is_none()
            {
                return Err(CapabilityError::Failed(FailureKind::Io));
            }
            // Resolve the token-carrying URL internally; it never surfaces.
            let _url = inner.resolve_local_url(&file);
            inner.record(RecordedEffect::ExportedLocal {
                room_id: file.room_id().clone(),
                file_id: file.file_id().clone(),
                kind,
            });
            Ok(())
        })
    }

    fn open_local(&self, file: LocalFileRef) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::OpenLocal) {
                return Err(error);
            }
            let _url = inner.resolve_local_url(&file);
            inner.record(RecordedEffect::OpenedLocal {
                room_id: file.room_id().clone(),
                file_id: file.file_id().clone(),
            });
            Ok(())
        })
    }

    fn share_content(
        &self,
        content: ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::ShareContent) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if content.is_empty() {
                return Err(CapabilityError::Failed(FailureKind::Io));
            }
            if !inner.blob_is_staged(&content) {
                return Err(CapabilityError::Failed(FailureKind::Unreadable));
            }
            inner.record(RecordedEffect::Shared { content });
            Ok(())
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
    fn write_text(&self, text: &str) -> Result<(), CapabilityError> {
        if let Some(error) = self.inner.take_forced(Capability::Clipboard) {
            return Err(error);
        }
        self.inner.record(RecordedEffect::ClipboardWrite {
            text: text.to_owned(),
        });
        Ok(())
    }
}

impl Share for FakePlatform {
    fn share(
        &self,
        content: ShareContent,
        ct: &CancelToken,
    ) -> BoxFuture<'_, Result<(), CapabilityError>> {
        let inner = self.inner.clone();
        let ct = ct.clone();
        Box::pin(async move {
            if let Some(error) = inner.take_forced(Capability::Share) {
                return Err(error);
            }
            if ct.is_cancelled() {
                return Err(CapabilityError::Cancelled);
            }
            if content.is_empty() {
                return Err(CapabilityError::Failed(FailureKind::Io));
            }
            if !inner.blob_is_staged(&content) {
                return Err(CapabilityError::Failed(FailureKind::Unreadable));
            }
            inner.record(RecordedEffect::Shared { content });
            Ok(())
        })
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

    fn hand_back_to_platform(&self) {
        self.inner.record(RecordedEffect::HandBackToPlatform);
    }
}

// ---- WindowActions ------------------------------------------------------

impl FakePlatform {
    fn window_command(&self, command: WindowCommand) -> Result<(), CapabilityError> {
        if !self.inner.shape.window_availability().is_available() {
            return Err(CapabilityError::Unavailable);
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
