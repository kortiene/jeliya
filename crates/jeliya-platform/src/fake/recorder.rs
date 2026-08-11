//! The call-recording and scriptable-outcome substrate shared by the fakes
//! (#174 §6).
//!
//! A [`Recorder`] captures every effect in order — opened URLs, clipboard
//! writes, shares, navigations, staged blobs with their bytes, preference
//! writes, window commands — for assertions. A [`Script`] lets any capability
//! method be forced to return `Unavailable` / `Denied` / `Cancelled` / a typed
//! `Failed`, so the denied / unavailable / cancelled paths the Verification
//! section demands are exercisable without a device. Neither the recorder nor
//! the script uses a wall clock, an RNG, or any task-scheduling order.

use std::collections::HashMap;
use std::collections::VecDeque;

use crate::clipboard::ShareContent;
use crate::error::CapabilityError;
use crate::files::{ExportTargetKind, FileName, Mime};
use crate::launcher::SafeExternalUrl;
use crate::navigation::Route;
use crate::storage::{PreferenceKey, SecretKey};
use crate::window::WindowCommand;

/// A capability method that can be scripted to fail on its next call.
///
/// Structural facts (window actions on a browser, a private directory on the
/// web) are modelled by the fake **shape**, not scripted here; this enum names
/// the methods whose *action* outcome a test forces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Capability {
    /// [`crate::Files::pick`].
    Pick,
    /// [`crate::Files::pick_export_target`].
    PickExport,
    /// [`crate::Files::stage_for_share`].
    Stage,
    /// [`crate::Files::export_sink`].
    ExportSink,
    /// [`crate::Files::open_sink`].
    OpenSink,
    /// [`crate::Files::share_sink`].
    ShareSink,
    /// [`crate::FileSink::write`] — a destination that fails **mid-transfer**
    /// (a full disk, a revoked SAF document), which is a materially different
    /// path from a sink that never opens: the caller must stop advancing
    /// CREDIT, abort the stream, and drop the uncommitted artifact.
    FileSinkWrite,
    /// [`crate::files::ShareSink::write`] — the same mid-transfer failure on
    /// the inbound staging path.
    ShareSinkWrite,
    /// [`crate::FileSink::commit`] — a destination that accepted every chunk
    /// and then failed to **finalize**: a flush error, a SAF document that
    /// would not close, a browser download that never published, an opener the
    /// platform refused. Distinct from a write failure, because the caller has
    /// already sent every byte and must still not report success.
    FileSinkCommit,
    /// [`crate::files::ShareSink::commit`] — the same finalization failure on
    /// the inbound staging path; no [`crate::FetchedArtifact`] is minted.
    ShareSinkCommit,
    /// [`crate::Files::release_staged`], [`crate::Files::release_artifact`],
    /// [`crate::Files::discard_source`], and
    /// [`crate::Files::discard_export_target`] — cleanup that itself fails (a
    /// locked temporary file, a permission change). The API can return an
    /// error, and a caller must then be able to retry, so a failed release
    /// leaves the entry in place rather than reaping it anyway.
    Release,
    /// [`crate::Files::read_staged`] — the staged file failing to **open**: a
    /// permission changed before the upload started. A different moment from a
    /// read failing partway through, because nothing has been sent yet and the
    /// caller has no stream to abort.
    ReadStaged,
    /// [`crate::StagedBlobReader::next_chunk`] — the **source** failing
    /// mid-upload, the mirror of a sink failing mid-download: the staged file
    /// became unreadable after the reader was opened, so the uploader must stop
    /// sending DATA and settle without reaping bytes a retry still needs.
    StagedReadChunk,
    /// [`crate::Files::share_content`].
    ShareContent,
    /// [`crate::Share::share`].
    Share,
    /// [`crate::Clipboard::write_text`].
    Clipboard,
    /// [`crate::UrlLauncher::open_external`].
    OpenExternal,
    /// Any [`crate::WindowActions`] command on a platform that HAS windows — a
    /// compositor refusing a raise, an OS vetoing a close. Distinct from the
    /// structural `Unavailable` a browser or Android shape returns, which is a
    /// fact about the platform rather than an outcome of the request.
    WindowAction,
}

/// One recorded effect, in the order it happened.
///
/// Secret values are never recorded ([`RecordedEffect::SecretWrite`] carries
/// only the key), so a recorder dump cannot leak a credential (§K1/§K5).
#[derive(Clone, PartialEq, Debug)]
pub enum RecordedEffect {
    /// A file source was picked (records the display name only).
    Picked {
        /// The picked source's display name.
        name: String,
    },
    /// A source was staged for share (records the copied size and bytes).
    Staged {
        /// The number of bytes staged.
        size: u64,
        /// The staged bytes (the fake's stand-in for the staging file).
        bytes: Vec<u8>,
    },
    /// An export target was chosen.
    PickedExport {
        /// The chosen target's kind.
        kind: ExportTargetKind,
    },
    /// An export sink was committed: fetched bytes reached their destination.
    /// Recorded only on [`crate::FileSink::commit`] — a dropped, uncommitted
    /// sink records nothing (the partial artifact is deleted, §D12/K2).
    ExportedLocal {
        /// The target kind written to.
        kind: ExportTargetKind,
        /// The committed bytes, in write order.
        bytes: Vec<u8>,
    },
    /// An open sink was committed: fetched bytes were handed to the platform
    /// opener. Same commit-only discipline as
    /// [`RecordedEffect::ExportedLocal`].
    OpenedLocal {
        /// The display name the artifact was opened under.
        name: FileName,
        /// The peer-declared content type — untrusted, an opener hint only.
        declared: Option<Mime>,
        /// The committed bytes, in write order.
        bytes: Vec<u8>,
    },
    /// A share sink was committed: a fetched room file reached the service's
    /// own staging custody and became attachable. Same commit-only discipline
    /// as [`RecordedEffect::ExportedLocal`] — a dropped, uncommitted share sink
    /// records nothing and mints no handle.
    StagedFetched {
        /// The display name the artifact was materialized under.
        name: String,
        /// The peer-declared content type — untrusted, a share-sheet hint only.
        declared: Option<Mime>,
        /// The committed bytes, in write order.
        bytes: Vec<u8>,
    },
    /// An abandoned picked source was released, dropping the file object or
    /// URI grant the service held for it.
    DiscardedSource,
    /// An abandoned export target was released, dropping its write grant.
    DiscardedExportTarget,
    /// A fetched artifact was released without being shared — the abandoned
    /// counterpart of a successful share consuming it.
    ReleasedArtifact,
    /// A staged blob's bytes were released after the daemon's `file.share`
    /// settled — the outbound "delete after share" reap. Carries nothing: the
    /// staging location never leaves the service (§K1).
    ReleasedStaged,
    /// Content was shared through the OS share sheet.
    Shared {
        /// The shared content.
        content: ShareContent,
        /// How many bytes the attachment actually carried at settlement, read
        /// from the service's own hold rather than looked up again. `None`
        /// when the content had no attachment. This is what makes "the bytes
        /// were still real when the sheet settled" an assertable fact: a blob
        /// released while the sheet was open is already out of the registry,
        /// so a size that still reports here can only have come from the hold.
        attached_bytes: Option<u64>,
    },
    /// An external URL was opened.
    OpenedUrl {
        /// The opened, vetted URL.
        url: SafeExternalUrl,
    },
    /// Text was written to the clipboard.
    ClipboardWrite {
        /// The written text.
        text: String,
    },
    /// A preference was written (`value` is `None` for a removal).
    PreferenceWrite {
        /// The preference key.
        key: PreferenceKey,
        /// The new value, or `None` if removed.
        value: Option<String>,
    },
    /// A secret was written (the value is deliberately not recorded).
    SecretWrite {
        /// The secret key.
        key: SecretKey,
    },
    /// The route was navigated.
    Navigated {
        /// The route navigated to.
        route: Route,
    },
    /// An unconsumed back gesture was handed back to the platform.
    HandBackToPlatform,
    /// A window command was invoked.
    Window {
        /// The invoked command.
        command: WindowCommand,
    },
}

/// The ordered log of effects a fake produced.
#[derive(Default)]
pub struct Recorder {
    effects: Vec<RecordedEffect>,
}

impl Recorder {
    /// Append one effect.
    pub fn record(&mut self, effect: RecordedEffect) {
        self.effects.push(effect);
    }

    /// Every recorded effect, in order.
    pub fn effects(&self) -> Vec<RecordedEffect> {
        self.effects.clone()
    }
}

/// Forced outcomes, keyed by capability. Each capability holds a queue, so a
/// test can script several calls in a row; a call with no scripted outcome
/// takes the shape's default (usually success).
#[derive(Default)]
pub struct Script {
    forced: HashMap<Capability, VecDeque<CapabilityError>>,
    /// While set, **every** preference/secret write reports
    /// [`crate::WriteOutcome::SessionOnly`] even on a persistent shape, so the
    /// write-honesty invariant (§K6) is exercisable.
    ///
    /// A latch, not a one-shot: it models a durability *regime* (storage that
    /// stopped persisting), which in reality persists across writes — unlike
    /// the per-call action outcomes the `forced` queues above model. It stays
    /// in force until cleared, and recovery is scripted explicitly via
    /// [`Script::set_force_session_only`]`(false)`.
    force_session_only: bool,
}

impl Script {
    /// Force the next call of `capability` to fail with `error`.
    pub fn force(&mut self, capability: Capability, error: CapabilityError) {
        self.forced.entry(capability).or_default().push_back(error);
    }

    /// Take the forced outcome for `capability`, if one is queued.
    pub fn take(&mut self, capability: Capability) -> Option<CapabilityError> {
        self.forced
            .get_mut(&capability)
            .and_then(VecDeque::pop_front)
    }

    /// Set whether writes report session-only durability. A latch: the value
    /// holds until set again — not a one-shot queue entry.
    pub fn set_force_session_only(&mut self, force: bool) {
        self.force_session_only = force;
    }

    /// Whether writes are currently forced session-only.
    pub fn force_session_only(&self) -> bool {
        self.force_session_only
    }
}
