//! `RoomSupervisor` — the daemon's map of `room_id -> open RoomSession`, plus
//! every room flow `jeliyad` exposes over the protocol.
//!
//! A [`RoomSession`] owns one experimental SDK [`Node`] (one node per open
//! room, spawned via `Node::spawn_room` exactly the way the reference CLI's
//! `room tail` spawns its long-running session), its [`SyncEngine`] over the
//! shared persistent SQLite [`EventStore`] (`<data-dir>/rooms.db`), and blob
//! serving (`BlobServeConfig` on a per-room blobs dir) so shared files stay
//! fetchable while the room is open.
//!
//! Offline flows (create/invite/list/timeline/members and the join bootstrap)
//! mirror the reference CLI's `room.rs` / `invite.rs` / `join.rs` modules:
//! author with the stable-tier builders, self-validate through
//! `validate_wire_bytes`, fold-check through `RoomMembership::ingest`, then
//! persist (directly, or through the live node's `publish` when the room is
//! open so the engine both persists and fans out).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use iroh::TransportAddr;
#[cfg(test)]
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex as TokioMutex};

use iroh_rooms::events::constants::{
    MAX_ARTIFACT_REFS, MAX_MESSAGE_BODY_BYTES, MAX_PREV_EVENTS, MAX_SHARED_FILE_BYTES,
    MAX_STATUS_LABEL_BYTES, MAX_STATUS_MESSAGE_BYTES, SHORT_ID_LEN,
};
use iroh_rooms::events::{
    build_agent_status, build_message_text, capability_hash, validate_wire_bytes, Content, EventId,
    EventType, RejectReason, SignedEvent, ValidationContext, WireEvent,
};
use iroh_rooms::experimental::pipe_runtime::{is_loopback_target, PipeError, PipeForwarder};
#[cfg(test)]
use iroh_rooms::experimental::session::PeerConnState;
use iroh_rooms::experimental::session::{
    Admission, AdmissionView, AllowlistAdmission, BlobServeConfig, BootstrapProof, ConnEvent,
    EndpointAddr, EndpointId, JoinBootstrapAdmission, NetConfig, NetMode, Node, SecretKey,
    SnapshotAdmission, TracingAudit, DEFAULT_TICK,
};
use iroh_rooms::experimental::store::{EventStore, StoreOptions, StoredEvent};
use iroh_rooms::experimental::sync::{SyncConfig, SyncEngine};
use iroh_rooms::files::build_file_shared;
use iroh_rooms::identity::{DeviceBinding, DeviceKey, IdentityKey, SigningKey};
#[cfg(test)]
use iroh_rooms::room::Role;
use iroh_rooms::room::{
    build_member_invited, build_member_joined, build_member_left, build_member_removed,
    build_room_created, derive_room_id, Ingest, MembershipSnapshot, RoomId, RoomInviteTicket,
    RoomMembership, Status,
};

use crate::error::{CoreError, CoreResult, ErrorKind};
#[cfg(test)]
use crate::fleet::{self, Liveness};
use crate::identity::SecretKeys;
#[cfg(test)]
use crate::materializer::{self, role_label};
use crate::projection::{bare_event_hex, file_handle};
use crate::{localstate, now_ms};

/// BLAKE3 KDF domain separator for [`derive_room_device`].
///
/// This string is part of the on-wire contract: changing it changes every
/// derived `device_id`, which the membership fold has already bound in every
/// room created or joined under the old string. Rotating it would strand those
/// rooms exactly the way losing a stored seed would, so it is versioned and
/// must not be edited in place.
const ROOM_DEVICE_KDF_CONTEXT: &str = "jeliya room-scoped device key v1";

/// The room-scoped device key this identity authors with in `room_id`.
///
/// iroh-rooms treats `EndpointId == device_id` as the P2P routing key, so two
/// live rooms sharing one device key also share one `EndpointId` and inbound
/// traffic collapses onto whichever endpoint bound last. Giving each room its
/// own device key is what lets several rooms stay reachable at once.
///
/// The key is **derived, never stored**: BLAKE3's KDF mode over this identity's
/// device seed and the room id. That makes it reproducible from
/// `identity.secret` alone, so it survives a lost, rolled-back, or
/// older-daemon-rewritten `state.json`, needs no migration, and adds no second
/// secret-bearing file. The room id is available before the genesis is built
/// (`derive_room_id` covers only sender/nonce/timestamp, not the device), so
/// the creator can derive it too — there is no circularity.
fn derive_room_device(device: &SigningKey, room_id: &RoomId) -> SigningKey {
    let mut kdf = blake3::Hasher::new_derive_key(ROOM_DEVICE_KDF_CONTEXT);
    kdf.update(device.to_seed().as_slice());
    kdf.update(room_id.as_bytes());
    SigningKey::from_seed(kdf.finalize().as_bytes())
}

/// How many of a room's most recent causally-placed events `room.list` scans
/// for the recency projection (`docs/room-attention.md` decision 2).
///
/// Causal order is not timestamp order once authors' clocks disagree, so the
/// projection takes the maximum `created_at` over this window rather than the
/// single causally-last event. Bounded so the projection stays one cheap read
/// per room: a window this size absorbs any realistic skew between peers, and
/// an event older than 64 causal positions is not this room's "last activity"
/// under any reading.
#[cfg(test)]
const RECENCY_SCAN: u32 = 64;

/// The single event-store database file under the data dir (mirrors the CLI).
pub(crate) const DB_FILE: &str = "rooms.db";
/// Root for the per-room durable blob stores.
const BLOBS_DIR: &str = "blobs";
/// Maximum number of bytes accepted for one shared file, exposed so the daemon's
/// browser-upload endpoint can reject over-limit bodies before staging them.
pub(crate) const FILE_UPLOAD_MAX_BYTES: u64 = MAX_SHARED_FILE_BYTES;
/// Default downloads directory for `file.fetch` when `save_dir` is omitted.
const DOWNLOADS_DIR: &str = "downloads";
/// Room-name cap, mirroring the CLI (spec IR-0102 D7).
const MAX_ROOM_NAME_BYTES: usize = 128;
/// CSPRNG nonce length seeding `derive_room_id`.
const ROOM_NONCE_LEN: usize = 16;
/// Time budget for the join bootstrap (membership pull + active confirm).
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// Poll interval for join/bootstrap waits.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Grace after a publish so writer queues flush before an ephemeral node
/// tears down (mirrors the CLI's flush grace).
const FLUSH_GRACE: Duration = Duration::from_millis(500);
/// Per-provider connect+transfer budget for `file.fetch`.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `pipe.connect` waits for the `pipe.opened` to sync.
const PIPE_SYNC_WAIT: Duration = Duration::from_secs(10);
/// Backoff between attempts to reclaim an owned `Node` for shutdown while an
/// in-flight network op still borrows the session (see `reclaim_session`).
const RECLAIM_POLL: Duration = Duration::from_millis(50);
/// Event types that the sync protocol serves through the never-windowed
/// authorization pull.
const MEMBERSHIP_EVENT_TYPES: [EventType; 5] = [
    EventType::RoomCreated,
    EventType::MemberInvited,
    EventType::MemberJoined,
    EventType::MemberLeft,
    EventType::MemberRemoved,
];

/// A verified local file copy that can be served by the loopback HTTP endpoint.
#[derive(Debug, Clone)]
pub(crate) struct LocalFile {
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
    pub bytes: u64,
}

/// The typed facts produced after a staged file has been durably imported and
/// its signed `file.shared` event has committed. This is runtime plumbing, not
/// a protocol projection; [`crate::typed`] converts it to `FileShareOut`.
#[derive(Debug, Clone)]
pub(crate) struct StagedFileShare {
    pub file_id: String,
    pub event_id: String,
    pub bytes: u64,
    pub digest: String,
}

/// A one-shot authorization capability for a protocol-streamed file share.
///
/// The typed validation pipeline creates this only after room visibility,
/// standing, declared-size policy, and live-session checks have succeeded.
/// It deliberately retains the exact live session and membership snapshot
/// that won those checks: finalization consumes the capability and does not
/// re-run authorization after a potentially long upload.
pub(crate) struct AuthorizedFileShare {
    room_id: RoomId,
    session: Arc<RoomSession>,
    secret: SecretKeys,
    snapshot: MembershipSnapshot,
    display_name: String,
    mime_type: String,
    declared_bytes: u64,
}

/// One imported blob paired with the authorization capability that may author
/// its single `file.shared` event.
///
/// Protocol uploads deliberately split import from publication so their
/// private staging name can be removed successfully before any event is
/// authored. The capability is neither cloneable nor externally constructible,
/// so publication can consume it at most once.
pub(crate) struct ImportedAuthorizedFileShare {
    authorized: AuthorizedFileShare,
    size_bytes: u64,
    hash: [u8; 32],
}

impl ImportedAuthorizedFileShare {
    pub(crate) const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub(crate) const fn hash(&self) -> [u8; 32] {
        self.hash
    }
}

/// A finalization failure that carries the one typed fact the supervisor can
/// discover only while importing. Infrastructure failures retain the ordinary
/// core taxonomy; a count disagreement is kept separate so the typed boundary
/// can produce `declared_size_mismatch` without parsing prose.
pub(crate) enum FinalizeFileShareError {
    CountDisagreement { observed_bytes: u64 },
    Core(CoreError),
}

impl From<CoreError> for FinalizeFileShareError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

/// Semantic result of `member.remove`; infrastructure failures remain
/// [`CoreError`]s, while the typed boundary maps these closed outcomes to the
/// protocol's exact errors.
pub(crate) enum RemoveMemberOutcome {
    Removed(String),
    Authority,
    Unknown,
}

/// The verified local result of a completed fetch. The path is host-only
/// runtime state and never crosses the protocol boundary.
#[derive(Debug, Clone)]
pub(crate) struct FetchedFile {
    #[cfg(test)]
    pub path: PathBuf,
    pub bytes: u64,
    pub provider_device: DeviceKey,
}

/// One connector-side local pipe connection. Connections are keyed by their
/// opaque `connection_id`, not by `pipe_id`, so sibling connections to the
/// same published pipe have independent lifetimes.
struct LocalPipeConnection {
    pipe_id: [u8; SHORT_ID_LEN],
    forwarder: PipeForwarder,
}

/// Connector-side pipe runtime state guarded by one lock.
///
/// Revocation tombstones share the lock with connection insertion so a
/// `pipe.connect` that finishes its network await after a revoke cannot
/// resurrect a forwarder for the withdrawn pipe.
#[derive(Default)]
struct LocalPipeRegistry {
    connections: HashMap<String, LocalPipeConnection>,
    revoked: BTreeSet<[u8; SHORT_ID_LEN]>,
}

/// One open room: the SDK node (transport + engine + blob serving), the live
/// connection-event subscription, connector-side pipe forwarders, and the
/// pushed-event dedupe set.
///
/// A session is shared as `Arc<RoomSession>` so a client's long-running network
/// call (a `file.fetch`, a `pipe.connect`) runs on a cloned handle **without**
/// holding any supervisor-wide lock — the whole point of the daemon no longer
/// freezing every client and the push loop on one slow request. All the SDK
/// `Node` methods take `&self`, so concurrent reads/fetches/publishes share the
/// one node freely; the mutable session bits sit behind their own small
/// std mutexes.
pub(crate) struct RoomSession {
    pub(crate) node: Node,
    conn_rx: StdMutex<broadcast::Receiver<ConnEvent>>,
    /// The node's live `room.event` push stream (issue #83): every event the
    /// engine ingests (own or remote) is broadcast here the moment it commits,
    /// so the daemon pushes with sub-second latency instead of synthesizing
    /// pushes from a hot `room_tail` poll. Lossy by design (a lagged receiver
    /// drops events); the `seen` dedupe plus the reconcile poll close the gap.
    ///
    /// Held behind its OWN `Arc` (independent of the session `Arc`) so the push
    /// pump can park on `recv().await` while cloning only this handle — never a
    /// session clone. A parked receiver must not pin the session, or
    /// `room.close`'s `reclaim_session` would spin forever waiting for the pump
    /// to drop a strong reference the pump never gets to release until the
    /// broadcast closes (which only happens once the node is shut down, i.e.
    /// AFTER reclaim). Dropping the session pin breaks that cycle: reclaim
    /// unwraps immediately, `Node::shutdown` drops the broadcast senders, and
    /// the parked `recv` wakes with `Closed`.
    room_rx: Arc<TokioMutex<broadcast::Receiver<StoredEvent>>>,
    forwarders: StdMutex<LocalPipeRegistry>,
    seen: StdMutex<BTreeSet<EventId>>,
    /// The next dense rank this room's push stream expects to serve (the
    /// per-room high-water mark). An in-order append advances it by one; a
    /// NEW committed event whose canonical rank falls below it has reordered
    /// already-served history and is served as a corrective gap instead of a
    /// normal append (see `collect_committed`). Starts at 0 (genesis rank).
    next_push_rank: StdMutex<u64>,
    /// Live gate for join-bootstrap provisional admission (an unknown device may
    /// pull the membership sub-DAG). Flipped on so that a stranger can only
    /// bootstrap while this owner session actually has a pending invite open,
    /// not for the whole session lifetime.
    accept_joins: Arc<AtomicBool>,
    /// Whether this identity is the room owner (fixed for the room's lifetime).
    is_owner: bool,
}

/// The daemon's room supervisor: shared data dir + one session per open room.
///
/// `sessions` sits behind a *std* mutex that is only ever held for the brief
/// span of a map lookup/insert/remove — never across an `.await`. Network work
/// runs on the cloned `Arc<RoomSession>` after the guard is dropped, so no
/// client request or the push loop can be head-of-line blocked by another
/// client's slow call. `structural` serializes the two flows that spawn or
/// tear a node down (`room.open` / `room.close`) so they never race the same
/// room's exclusive blob-store lock; it is deliberately *not* taken by the
/// message/fetch/share/pipe/peers/push paths (since #84 `file.share` imports
/// in-session and spawns/tears no node, so it needs no structural lock).
pub(crate) struct RoomSupervisor {
    data_dir: PathBuf,
    loopback: bool,
    sessions: StdMutex<HashMap<RoomId, Arc<RoomSession>>>,
    structural: TokioMutex<()>,
    /// Per-room membership-fold cache for CLOSED rooms, keyed on a cheap
    /// fingerprint of the room's stored event set (its `EventStore::count`, a
    /// single `SELECT COUNT(*)` with no crypto/fold). A closed room's store
    /// cannot change during a daemon run, so a hit (same count) returns the
    /// cached snapshot and a miss folds exactly once — retiring the old
    /// O(full-history) re-fold that `room.list` / `agents.fleet` paid on every
    /// call. OPEN rooms never consult this cache: their live engine already
    /// maintains the same fold incrementally (`Node::snapshot`), so the cache
    /// can never go stale against a growing open room.
    snapshot_cache: StdMutex<HashMap<RoomId, (u64, MembershipSnapshot)>>,
    #[cfg(test)]
    fold_invocations: AtomicUsize,
}

fn internal(context: &str, err: impl std::fmt::Display) -> CoreError {
    CoreError::internal(format!("{context}: {err}"))
}

/// Whether the folded membership has any subject still merely `Invited` (an
/// open invite that has not yet been redeemed) — the condition under which an
/// owner session legitimately hosts join bootstraps.
fn any_pending_invite(snapshot: &MembershipSnapshot) -> bool {
    snapshot.members().any(|m| m.status == Status::Invited)
}

/// One newly-pushed committed event plus whether it reorders already-served
/// history. The engine turns a `reordered_at` into an explicit `gap` push so
/// subscribers discard and resync the shifted suffix rather than trust a
/// silently renumbered one.
#[derive(Debug)]
pub(crate) struct CommittedEvent {
    /// The committed event, carrying its dense canonical rank as `pos`.
    pub event: jeliya_api::Event,
    /// `Some(pos)` when this event's rank is at or below one the stream
    /// already served (a late concurrent sibling interleaved below the
    /// frontier): the first position the client must discard and resync from.
    /// `None` for an in-order append.
    pub reordered_at: Option<u64>,
}

/// Rank the newly-pushed committed events of a room's **full canonical tail**
/// densely and detect reordering. Both typed push paths call this with the
/// full tail: the reconcile poll already scans it, and the hot path must too
/// — only the full tail reveals whether a late concurrent sibling interleaved
/// below an already-served position. (This is one tail read per push batch on
/// top of the 300 ms reconcile, both over the same already-materialized rows;
/// the store serves them from the indexed cache, so the per-batch cost is one
/// ordered scan, not a decode per row — non-committed rows are rejected on
/// their signed type alone.)
///
/// Only COMMITTED kinds hold a rank (`is_committed`), so a non-committed row
/// consumes no position. `seen` dedupes exactly-once; `next_push_rank` is the
/// per-room high-water mark — the rank the next in-order append takes.
///
/// An event whose canonical rank is at or below the high-water mark has
/// reordered history the stream already served: it is emitted with
/// `reordered_at = Some(rank)`, the mark is reset to that rank, and every
/// later new event is likewise marked (the whole suffix shifted), so the
/// engine emits ONE corrective gap from the first shifted position.
fn collect_committed(
    tail: &[StoredEvent],
    snapshot: &MembershipSnapshot,
    seen: &mut BTreeSet<EventId>,
    next_push_rank: &mut u64,
) -> Vec<CommittedEvent> {
    // The pushed event's `author.standing` is the same derivation `room.members`
    // and `room.timeline` serve, so it is folded from the same tail rather than
    // assumed active. One pre-pass over rows already in hand.
    let departures = crate::projection::Departures::from_rows(tail.iter());
    let mut out = Vec::new();
    let mut rank = 0u64;
    for se in tail {
        if !crate::projection::is_committed(se) {
            continue; // not a committed event: holds no position
        }
        let this_rank = rank;
        rank += 1;
        if !seen.insert(se.event_id) {
            continue; // already pushed by an earlier batch or reconcile
        }
        let event = crate::projection::materialize(se, 0, snapshot, &departures)
            .expect("is_committed implies materializable");
        if this_rank >= *next_push_rank {
            // In-order append at or past the high-water mark.
            *next_push_rank = this_rank + 1;
            out.push(CommittedEvent {
                event: jeliya_api::Event {
                    pos: this_rank,
                    ..event
                },
                reordered_at: None,
            });
        } else {
            // Reorder: this new event interleaved below the frontier. Serve it
            // at its true rank, mark it so the engine emits a corrective gap
            // from the first shifted position, and rewind the mark so the
            // whole shifted suffix is re-served (and re-marked) after resync.
            *next_push_rank = this_rank;
            out.push(CommittedEvent {
                event: jeliya_api::Event {
                    pos: this_rank,
                    ..event
                },
                reordered_at: Some(this_rank),
            });
        }
    }
    out
}

impl RoomSupervisor {
    /// Create the supervisor (and the data dir, owner-only).
    pub(crate) fn new(data_dir: PathBuf, loopback: bool) -> CoreResult<Self> {
        crate::identity::ensure_dir(&data_dir)?;
        Ok(Self {
            data_dir,
            loopback,
            sessions: StdMutex::new(HashMap::new()),
            structural: TokioMutex::new(()),
            snapshot_cache: StdMutex::new(HashMap::new()),
            #[cfg(test)]
            fold_invocations: AtomicUsize::new(0),
        })
    }

    /// Brief lock over the session map. Held only for a map operation, never
    /// across an `.await`.
    fn sessions(&self) -> MutexGuard<'_, HashMap<RoomId, Arc<RoomSession>>> {
        self.sessions.lock().expect("sessions mutex poisoned")
    }

    /// The resolved data directory.
    #[must_use]
    pub(crate) fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Room ids of all open sessions (protocol string form).
    #[must_use]
    pub(crate) fn open_rooms(&self) -> Vec<String> {
        self.sessions().keys().map(ToString::to_string).collect()
    }

    /// Room ids of all open sessions (typed, for the push loop).
    #[must_use]
    pub(crate) fn open_room_ids(&self) -> Vec<RoomId> {
        self.sessions().keys().copied().collect()
    }

    // ------------------------------------------------------------------
    // Shared plumbing
    // ------------------------------------------------------------------

    pub(crate) fn db_path(&self) -> PathBuf {
        self.data_dir.join(DB_FILE)
    }

    fn room_blobs_dir(&self, room_id: &RoomId) -> PathBuf {
        // Per-room stores: the iroh-blobs FsStore lock is exclusive per
        // directory, so two open rooms must not share one serving store.
        let hex_part = room_id
            .to_string()
            .strip_prefix("blake3:")
            .map_or_else(|| room_id.to_string(), str::to_owned);
        self.data_dir.join(BLOBS_DIR).join(hex_part)
    }

    pub(crate) fn open_store(&self) -> CoreResult<EventStore> {
        // Open with an explicit 5s SQLITE_BUSY timeout: the daemon opens several
        // writer connections on one shared WAL `rooms.db` (one per open room's
        // `SyncEngine`, plus transient create_room / create_invite inserts), and
        // WAL allows a single writer at a time. The busy_timeout lets a colliding
        // writer wait inside SQLite instead of erroring instantly, which retires
        // the old application-level `with_busy_retry` backoff loop.
        EventStore::open_with(
            &self.db_path(),
            &StoreOptions::new(Some(Duration::from_millis(5000))),
        )
        .map_err(|e| internal("could not open the event store", e))
    }

    /// Confine `file.fetch`'s destination to the downloads tree and resolve it.
    ///
    /// `assert_shareable_path` confines the read direction; this confines the
    /// write direction, which was previously unconfined. `file.fetch` writes
    /// attacker-influenced bytes — any blob a room peer shared — under a
    /// mostly attacker-influenced name, so an arbitrary destination is an
    /// arbitrary-file-write primitive: `~/.config/autostart/x.desktop` or
    /// `~/Library/LaunchAgents/` turn it into local code execution as the user,
    /// in the one process that holds the identity keys. Confining to the
    /// downloads tree also keeps a caller from writing beside the identity and
    /// secret files at the data-dir root.
    ///
    /// The destination need not exist yet, so the deepest existing ancestor is
    /// canonicalized: a symlink planted inside the tree must not redirect the
    /// write outside it.
    fn resolve_fetch_dir(&self, save_dir: Option<&str>) -> CoreResult<PathBuf> {
        let root = std::fs::canonicalize(&self.data_dir)
            .map_err(|e| internal("could not resolve the data dir", e))?
            .join(DOWNLOADS_DIR);

        let refuse = |candidate: &Path| {
            Err(CoreError::invalid(format!(
                "file.fetch is confined to the downloads dir; refusing to write {}",
                candidate.display()
            ))
            .with_hint("omit save_dir, or pass a path inside <data-dir>/downloads"))
        };

        // The default destination is validated too: `<data-dir>/downloads` can
        // itself be a symlink out of the data dir, and skipping the checks
        // below for the omitted case would leave the path the UI actually uses
        // outside the confinement.
        let candidate = match save_dir.map(str::trim).filter(|s| !s.is_empty()) {
            None => root.clone(),
            Some(raw) => {
                let requested = PathBuf::from(raw);
                if requested.is_absolute() {
                    requested
                } else {
                    root.join(requested)
                }
            }
        };
        // Reject traversal before touching the filesystem: `..` can escape the
        // prefix check below even when every component resolves.
        if candidate
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return refuse(&candidate);
        }

        // Canonicalize the deepest existing ancestor, then re-attach the part
        // that does not exist yet.
        let mut existing = candidate.as_path();
        while !existing.exists() {
            match existing.parent() {
                Some(parent) => existing = parent,
                None => return refuse(&candidate),
            }
        }
        let resolved = std::fs::canonicalize(existing)
            .map_err(|e| internal("could not resolve the save directory", e))?;
        let remainder = candidate
            .strip_prefix(existing)
            .unwrap_or_else(|_| Path::new(""));
        let target = resolved.join(remainder);
        if target != root && !target.starts_with(&root) {
            return refuse(&candidate);
        }
        Ok(target)
    }

    /// Confine `file.share` to files inside the daemon's data dir, excluding the
    /// daemon's own blob store and secret/state files.
    ///
    /// Without this the daemon is an arbitrary-local-file read primitive: a
    /// hostile local (or cross-site-WebSocket) client could `file.share` a path
    /// like `~/.ssh/id_rsa`, importing the bytes as a room blob that any room
    /// peer can then `file.fetch`. `canonical` must already be canonicalized.
    fn assert_shareable_path(&self, canonical: &Path) -> CoreResult<()> {
        let root = std::fs::canonicalize(&self.data_dir)
            .map_err(|e| internal("could not resolve the data dir", e))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::invalid(format!(
                "file.share is confined to the daemon data dir; refusing to read {}",
                canonical.display()
            ))
            .with_hint("place the file under the daemon data dir to share it"));
        }
        if canonical.starts_with(root.join(BLOBS_DIR)) {
            return Err(CoreError::invalid(
                "refusing to share the daemon's internal blob store",
            ));
        }
        let is_reserved_child = canonical.parent() == Some(root.as_path())
            && canonical
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name == crate::identity::IDENTITY_FILE
                        || name == crate::identity::SECRET_FILE
                        || name.starts_with(DB_FILE)
                        || name.starts_with(localstate::STATE_FILE)
                });
        if is_reserved_child {
            return Err(
                CoreError::invalid("refusing to share a daemon secret/state file")
                    .with_hint("that path holds daemon-private data"),
            );
        }
        Ok(())
    }

    pub(crate) fn secrets(&self) -> CoreResult<SecretKeys> {
        SecretKeys::load(&self.data_dir)
    }

    /// A cloned handle to an open room's session (an `Arc`), or `RoomNotOpen`.
    /// The map lock is released before the caller does any network work.
    pub(crate) fn session(&self, room_id: &RoomId) -> CoreResult<Arc<RoomSession>> {
        self.sessions().get(room_id).cloned().ok_or_else(|| {
            CoreError::new(
                ErrorKind::RoomNotOpen,
                format!("room {room_id} is not open"),
            )
        })
    }

    /// A cloned handle to an open room's session, if any.
    pub(crate) fn session_opt(&self, room_id: &RoomId) -> Option<Arc<RoomSession>> {
        self.sessions().get(room_id).cloned()
    }

    /// Whether a room currently has an open session.
    pub(crate) fn is_open(&self, room_id: &RoomId) -> bool {
        self.sessions().contains_key(room_id)
    }

    /// Re-fold a room's persisted log (re-validating every stored event), the
    /// same projection the reference CLI's `fold_room` builds.
    fn fold(
        &self,
        store: &EventStore,
        room_id: &RoomId,
    ) -> CoreResult<(RoomMembership, MembershipSnapshot)> {
        #[cfg(test)]
        self.fold_invocations.fetch_add(1, Ordering::Relaxed);
        let ids = store
            .room_event_ids(room_id)
            .map_err(|e| internal("could not read room events", e))?;
        if ids.is_empty() {
            return Err(CoreError::new(
                ErrorKind::RoomUnknown,
                format!("no room {room_id} in {}", self.data_dir.display()),
            ));
        }
        let ctx = ValidationContext::for_room(*room_id);
        let mut validated = Vec::with_capacity(ids.len());
        for id in &ids {
            let stored = store
                .get(id)
                .map_err(|e| internal("could not read a stored event", e))?
                .ok_or_else(|| CoreError::internal(format!("stored event {id} vanished")))?;
            let event = validate_wire_bytes(&stored.wire.to_bytes(), &ctx).map_err(|reason| {
                CoreError::internal(format!(
                    "stored event {id} failed re-validation ({})",
                    reason.code()
                ))
            })?;
            validated.push(event);
        }
        let membership = RoomMembership::from_events(*room_id, validated);
        let snapshot = membership.snapshot();
        Ok((membership, snapshot))
    }

    #[cfg(test)]
    fn fold_invocation_count(&self) -> usize {
        self.fold_invocations.load(Ordering::Relaxed)
    }

    /// A room's current [`MembershipSnapshot`] — byte-for-byte the SAME
    /// projection [`fold`](Self::fold) produces, but WITHOUT re-validating and
    /// re-folding the whole log on every call (the O(full-history)-per-call
    /// cost that made `room.list` / `agents.fleet` unusable, ~25s on a room
    /// with real history).
    ///
    /// * **Open room** — the live [`SyncEngine`] already maintains this exact
    ///   membership fold incrementally, so `Node::snapshot` returns it in O(1)
    ///   and always reflects the newest ingested event (a just-joined member is
    ///   visible immediately; the cache is never consulted, so it can never go
    ///   stale against a growing open room).
    /// * **Closed room** — folded once and cached, keyed on the room's stored
    ///   event count (`EventStore::count`, a single `SELECT COUNT(*)`, no
    ///   crypto/fold). A hit (same count) returns the cached snapshot; a miss
    ///   folds once and caches. A closed room's log is *append-only* but not
    ///   frozen (e.g. `create_invite` can still append to it), so the count is
    ///   the invalidation signal: any appended event bumps it and forces a
    ///   re-fold. See the load-bearing count-before-fold ordering note below.
    ///
    /// Never takes an `&EventStore` argument: an async fn captures its
    /// parameters for the whole future, and `&EventStore` is `!Send`, so the
    /// closed path opens its own short-lived read handle (WAL allows many).
    pub(crate) async fn snapshot_for(&self, room_id: &RoomId) -> CoreResult<MembershipSnapshot> {
        if let Some(session) = self.session_opt(room_id) {
            return session
                .node
                .snapshot()
                .await
                .map_err(|e| internal("could not read the membership snapshot", e));
        }
        // Closed room: no `.await` from here on, so the `!Sync` store never
        // crosses an await and this future stays `Send`.
        let store = self.open_store()?;
        // LOAD-BEARING ORDER: read the fingerprint (count) BEFORE folding, and
        // cache under this pre-fold count — never re-read count at insert time
        // or after the fold. A closed room is not truly immutable within a run
        // (e.g. `create_invite` appends directly to a closed room's store on its
        // own connection without the structural lock or a cache invalidation),
        // and `count`/`fold` are separate autocommit SELECTs on one WAL handle,
        // not one transaction. So a concurrent writer can commit `k` events
        // between the two reads, yielding a snapshot of `N+k` events cached under
        // key `N`. Because per-room event count is append-only/monotonic, the
        // true count is already `>= N+k > N` and can never fall back to `N`, so
        // this "ahead-of-key" entry is never hit — safe, merely a wasted slot.
        // Reversing the order (caching a snapshot of `N` events under key `N+k`)
        // WOULD be returned at true count `N+k`: a genuine stale snapshot.
        let fingerprint = store
            .count(room_id)
            .map_err(|e| internal("could not count the room's stored events", e))?;
        if let Some(snapshot) = self.cached_snapshot(room_id, fingerprint) {
            return Ok(snapshot);
        }
        let (_, snapshot) = self.fold(&store, room_id)?;
        self.snapshot_cache
            .lock()
            .expect("snapshot cache poisoned")
            .insert(*room_id, (fingerprint, snapshot.clone()));
        Ok(snapshot)
    }

    /// The public identity bound to this data directory.
    ///
    /// Read authorization deliberately uses the public profile rather than
    /// loading signing seeds. A read path must never need secret key material,
    /// and a missing identity defaults to denial.
    pub(crate) fn local_identity_key(&self) -> CoreResult<IdentityKey> {
        let profile = crate::identity::load_profile(&self.data_dir)?.ok_or_else(|| {
            CoreError::new(
                ErrorKind::IdentityMissing,
                "create an identity before reading room data",
            )
        })?;
        profile.identity_id.parse::<IdentityKey>().map_err(|e| {
            CoreError::internal(format!(
                "stored identity profile has an invalid identity_id: {e}"
            ))
        })
    }

    /// Default-deny the room read boundary without disclosing whether a
    /// never-authorized room exists in the shared local store.
    ///
    /// A subject that actually joined may read its local archive. Active,
    /// voluntarily-left, and removed members retain a device binding; an
    /// invited-only subject does not and cannot read room content before its
    /// join is accepted. An identity absent from the membership fold gets the
    /// same `room_unknown` result as an id with no stored rows.
    pub(crate) fn require_local_room_access(
        snapshot: &MembershipSnapshot,
        self_id: &IdentityKey,
    ) -> CoreResult<()> {
        if snapshot
            .member(self_id)
            .is_some_and(|member| member.device.is_some())
        {
            return Ok(());
        }
        Err(CoreError::new(
            ErrorKind::RoomUnknown,
            "room is not available to this identity",
        ))
    }

    /// Cheap prefilter backed by the daemon-local accepted-room index.
    ///
    /// Local provenance is written before `room.created` becomes durable, or
    /// after a proposed join passes its local fold but before `member.joined`
    /// is published. Sync alone never adds an entry. This lets us reject a
    /// foreign room before folding or decoding any of its shared store rows;
    /// the membership snapshot remains the authoritative second check, so an
    /// inert or tampered index entry cannot grant access.
    fn require_locally_known_room(&self, room_id: &RoomId) -> CoreResult<()> {
        let known = localstate::load(&self.data_dir)?
            .rooms
            .contains_key(&room_id.to_string());
        if known {
            return Ok(());
        }
        Err(CoreError::new(
            ErrorKind::RoomUnknown,
            "room is not available to this identity",
        ))
    }

    /// Cached/live snapshot plus the shared read-authorization guard.
    pub(crate) async fn readable_snapshot(
        &self,
        room_id: &RoomId,
    ) -> CoreResult<MembershipSnapshot> {
        self.require_locally_known_room(room_id)?;
        let self_id = self.local_identity_key()?;
        let snapshot = self.snapshot_for(room_id).await?;
        Self::require_local_room_access(&snapshot, &self_id)?;
        Ok(snapshot)
    }

    /// The cached closed-room snapshot iff its fingerprint still matches the
    /// room's current stored event count.
    fn cached_snapshot(&self, room_id: &RoomId, fingerprint: u64) -> Option<MembershipSnapshot> {
        self.snapshot_cache
            .lock()
            .expect("snapshot cache poisoned")
            .get(room_id)
            .filter(|(fp, _)| *fp == fingerprint)
            .map(|(_, snapshot)| snapshot.clone())
    }

    /// Current DAG heads for `prev_events` from the live engine, truncated
    /// deterministically to the protocol bound.
    async fn node_heads(node: &Node) -> CoreResult<Vec<EventId>> {
        let mut heads = node
            .heads()
            .await
            .map_err(|e| internal("could not read the room heads", e))?;
        heads.truncate(MAX_PREV_EVENTS);
        Ok(heads)
    }

    /// Current heads inside the never-windowed authorization class: every
    /// membership event plus every admin-authored event. Membership writes use
    /// these heads so late join bootstrap can pull every parent via
    /// `WantMembership`, while admin-authored writes still advance the admin
    /// sequence through admin content events.
    fn authorization_class_heads(
        store: &EventStore,
        room_id: &RoomId,
        admin: &IdentityKey,
    ) -> CoreResult<Vec<EventId>> {
        let mut ids = BTreeSet::new();
        let mut events = Vec::new();
        for ty in MEMBERSHIP_EVENT_TYPES {
            for stored in store
                .by_type(room_id, ty)
                .map_err(|e| internal("could not read membership events", e))?
            {
                if ids.insert(stored.event_id) {
                    events.push(stored);
                }
            }
        }
        for stored in store
            .by_sender(room_id, admin)
            .map_err(|e| internal("could not read admin-authored events", e))?
        {
            if ids.insert(stored.event_id) {
                events.push(stored);
            }
        }

        let mut cited = BTreeSet::new();
        for stored in &events {
            let validated = validate_wire_bytes(
                &stored.wire.to_bytes(),
                &ValidationContext::for_room(*room_id),
            )
            .map_err(|reason| {
                CoreError::internal(format!(
                    "stored authorization event failed validation ({})",
                    reason.code()
                ))
            })?;
            for parent in validated.event.prev_events {
                if ids.contains(&parent) {
                    cited.insert(parent);
                }
            }
        }

        let mut heads: Vec<EventId> = ids.difference(&cited).copied().collect();
        heads.truncate(MAX_PREV_EVENTS);
        Ok(heads)
    }

    fn downloaded_file_meta(
        &self,
        file_id: &[u8; SHORT_ID_LEN],
        name: &str,
        bytes: u64,
    ) -> Option<localstate::FetchedFileMeta> {
        let clean_name = sanitize_name(name, *file_id);
        let dir = self.data_dir.join(DOWNLOADS_DIR);
        let candidates = [
            dir.join(&clean_name),
            dir.join(format!("{}_{}", hex::encode(file_id), clean_name)),
        ];
        for path in candidates {
            let ok = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == bytes);
            if ok {
                return Some(localstate::FetchedFileMeta {
                    path,
                    bytes,
                    fetched_at_ms: 0,
                });
            }
        }
        None
    }

    /// Self-validate a freshly authored wire event and publish it through the
    /// open session's node (the engine persists it and fans it out).
    async fn publish_authored(
        node: &Node,
        room_id: &RoomId,
        wire: &WireEvent,
    ) -> CoreResult<EventId> {
        let bytes = wire.to_bytes();
        let validated = validate_wire_bytes(&bytes, &ValidationContext::for_room(*room_id))
            .map_err(|reason| {
                CoreError::internal(format!(
                    "freshly built event failed validation ({})",
                    reason.code()
                ))
            })?;
        let event_id = validated.event_id;
        node.publish(bytes)
            .await
            .map_err(|e| internal("could not publish the event", e))?;
        Ok(event_id)
    }

    fn net_config(&self) -> NetConfig {
        NetConfig {
            mode: if self.loopback {
                NetMode::Loopback
            } else {
                NetMode::RealNetwork
            },
            ..NetConfig::default()
        }
    }

    /// The room's persisted peer dial hints, parsed. Loopback mode has no
    /// discovery: without these the managed session's `PeerManager` dials
    /// bare endpoint ids that can never resolve, and two daemons' open
    /// sessions never sync (the CLI's `room tail --peer` equivalent).
    fn stored_hints(&self, room_id: &RoomId) -> Vec<EndpointAddr> {
        let raw = localstate::peer_hints(&self.data_dir, &room_id.to_string());
        parse_peers(&raw).unwrap_or_default()
    }

    /// Harvest fresh `"<endpoint_id>@<ip:port,...>"` dial hints from a live
    /// node's address book (addresses actually learned from its peers'
    /// connections), so a respawned session can redial them. A session cycle
    /// rebinds a new ephemeral UDP port, so peers' hints toward *us* go
    /// stale — redialing *them* from the fresh node heals the link.
    async fn harvest_peer_hints(node: &Node) -> Vec<String> {
        let endpoint = node.endpoint();
        let mut out = Vec::new();
        for (device, _entry) in node.peer_entries() {
            let Some(info) = endpoint.remote_info(device).await else {
                continue;
            };
            let socks: Vec<String> = info
                .addrs()
                .filter_map(|a| match a.addr() {
                    TransportAddr::Ip(sock) => Some(sock.to_string()),
                    _ => None,
                })
                .collect();
            if !socks.is_empty() {
                out.push(format!("{device}@{}", socks.join(",")));
            }
        }
        out
    }

    /// A dialable address for `id`: the bare endpoint id enriched with every
    /// socket address the live session or the persisted hints know for it
    /// (loopback mode cannot resolve a bare id).
    async fn enriched_addr(&self, node: &Node, room_id: &RoomId, id: EndpointId) -> EndpointAddr {
        let mut addr = EndpointAddr::new(id);
        if let Some(info) = node.endpoint().remote_info(id).await {
            for a in info.addrs() {
                if let TransportAddr::Ip(sock) = a.addr() {
                    addr = addr.with_ip_addr(*sock);
                }
            }
        }
        for hint in self.stored_hints(room_id) {
            if hint.id == id {
                for sock in hint.ip_addrs() {
                    addr = addr.with_ip_addr(*sock);
                }
            }
        }
        addr
    }

    /// The device key this daemon must sign `room_id`'s events with, and bind
    /// its endpoint to.
    ///
    /// **The room's own signed log is the authority, not local state.** Rooms
    /// created or joined by this build bind the derived room-scoped device
    /// ([`derive_room_device`]); rooms from before it bind the one global
    /// device from `identity.secret`. The membership fold has already recorded
    /// which, and peers reject any event whose `device_id` is not the bound one
    /// (`UnboundDevice`), so resolving against the snapshot is the only answer
    /// that cannot drift: it is correct after a state.json loss, after a
    /// downgrade-and-upgrade, and on a fresh install restored from the log.
    ///
    /// When the bound device is neither key — a room bound to a device this
    /// identity does not hold — the derived key is returned so the attempt
    /// fails loudly as `unbound_device` rather than silently signing with a key
    /// that is equally wrong but harder to diagnose.
    fn authoring_device_key(
        &self,
        snapshot: &MembershipSnapshot,
        secret: &SecretKeys,
        room_id: &RoomId,
    ) -> SigningKey {
        let bound = snapshot
            .member(&secret.identity.identity_key())
            .and_then(|m| m.device);
        if bound == Some(secret.device.device_key()) {
            // Legacy room: the log binds the global device and no rebinding
            // path exists for it (the admin's device is genesis-bound), so this
            // room keeps the historical one-endpoint-across-rooms limitation.
            SigningKey::from_seed(&secret.device.to_seed())
        } else {
            derive_room_device(&secret.device, room_id)
        }
    }

    /// Spawn the managed room session node (the CLI `room tail` pattern):
    /// live `SnapshotAdmission` refreshed by the pump, join bootstrap hosted
    /// while we are the room owner, blob serving from the room's store, and
    /// the room's persisted peer hints as the dial set.
    async fn spawn_node(&self, room_id: &RoomId) -> CoreResult<(Node, Arc<AtomicBool>, bool)> {
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let store = self.open_store()?;
        let (_, snapshot) = self.fold(&store, room_id)?;
        let room_device = self.authoring_device_key(&snapshot, &secret, room_id);
        if !snapshot.is_active(&self_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({self_id}) is not an active member of room {room_id}"),
            ));
        }
        // Provisional join-bootstrap admission (a stranger pulling the membership
        // sub-DAG) is a privacy window: the SDK ties it to "caller-is-admin AND a
        // pending invite is open". We are the room's long-running owner, so we
        // must keep hosting joins for invites minted *after* room.open — but only
        // while such an invite is actually pending. `accept_joins` starts at that
        // live condition and is kept in sync by `create_invite` (opens it) and
        // the push poll (closes it once every invite has been redeemed). The
        // on-log gate_join stays the convergent membership authority regardless.
        let is_owner = snapshot.admin() == Some(&self_id);
        let accept_joins = Arc::new(AtomicBool::new(is_owner && any_pending_invite(&snapshot)));
        let admission_cell = Arc::new(StdMutex::new(AdmissionView::from_snapshot(&snapshot, &[])));
        let admission: Arc<dyn Admission> = Arc::new(JoinBootstrapAdmission::new_dynamic(
            SnapshotAdmission::new(admission_cell.clone()),
            accept_joins.clone(),
        ));
        let engine = SyncEngine::open(store, *room_id, SyncConfig::default())
            .map_err(|e| internal("could not open the sync engine", e))?;
        let secret_key = SecretKey::from_bytes(&room_device.to_seed());
        let node = Node::spawn_room(
            secret_key,
            admission,
            Arc::new(TracingAudit),
            engine,
            self.net_config(),
            DEFAULT_TICK,
            self.stored_hints(room_id),
            admission_cell,
            Some(BlobServeConfig {
                blobs_dir: self.room_blobs_dir(room_id),
            }),
        )
        .await
        .map_err(|e| internal("could not bring up the room node", e))?;
        Ok((node, accept_joins, is_owner))
    }

    /// Build a shared session around a freshly spawned node, seeding the push
    /// dedupe set with `seen` (the caller passes the full current history at
    /// open time) and the push high-water mark with the number of COMMITTED
    /// events already in that history (the next rank a fresh push serves).
    fn make_session(
        node: Node,
        accept_joins: Arc<AtomicBool>,
        is_owner: bool,
        seen: BTreeSet<EventId>,
        committed_so_far: u64,
    ) -> Arc<RoomSession> {
        let conn_rx = node.conn_events();
        let room_rx = node.room_events();
        Arc::new(RoomSession {
            node,
            conn_rx: StdMutex::new(conn_rx),
            room_rx: Arc::new(TokioMutex::new(room_rx)),
            forwarders: StdMutex::new(LocalPipeRegistry::default()),
            seen: StdMutex::new(seen),
            next_push_rank: StdMutex::new(committed_so_far),
            accept_joins,
            is_owner,
        })
    }

    /// Reclaim the owned `Node` from a shared session so it can be shut down
    /// (`Node::shutdown` consumes `self`, and only shutdown releases the blob
    /// store's exclusive on-disk lock). Waits for any in-flight network op still
    /// borrowing the session (a `file.fetch`, a `pipe.connect`) to drop its
    /// clone — those ops are all bounded by their own timeouts, so this
    /// terminates. Tears any local pipe forwarders down first.
    async fn reclaim_session(session: Arc<RoomSession>) -> Node {
        let forwarders: Vec<PipeForwarder> = session
            .forwarders
            .lock()
            .expect("forwarders poisoned")
            .connections
            .drain()
            .map(|(_, connection)| connection.forwarder)
            .collect();
        for forwarder in forwarders {
            forwarder.shutdown();
        }
        let mut arc = session;
        loop {
            match Arc::try_unwrap(arc) {
                Ok(owned) => return owned.node,
                Err(shared) => {
                    arc = shared;
                    tokio::time::sleep(RECLAIM_POLL).await;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Rooms
    // ------------------------------------------------------------------

    /// `room.create`: author + self-validate + persist the genesis
    /// `room.created` (the creator becomes the room's single immutable owner).
    pub(crate) fn create_room(&self, name: &str) -> CoreResult<String> {
        validate_room_name(name)?;
        let secret = self.secrets()?;

        let mut room_nonce = [0u8; ROOM_NONCE_LEN];
        getrandom::fill(&mut room_nonce).map_err(|e| internal("OS CSPRNG unavailable", e))?;
        let created_at = now_ms();
        let sender_id = secret.identity.identity_key();
        let room_id = derive_room_id(&sender_id, &room_nonce, created_at);
        // The room id covers sender/nonce/timestamp only, so it is settled
        // before the genesis is signed and the room-scoped device can be
        // derived from it here. The genesis binds this device permanently for
        // the owner (the fold never re-resolves the admin's device).
        let room_device = derive_room_device(&secret.device, &room_id);

        let wire = build_room_created(
            &secret.identity,
            &room_device,
            name,
            &room_nonce,
            created_at,
        );
        let validated =
            validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(room_id)).map_err(
                |reason| {
                    CoreError::internal(format!(
                        "freshly built genesis failed validation ({})",
                        reason.code()
                    ))
                },
            )?;

        // Provenance is the first durable mutation. If state.json cannot be
        // updated, no genesis is committed that the public read guard would
        // later hide. A later SQLite failure can leave only an inert index
        // entry; the authoritative membership/device-binding check still
        // denies it and room.list skips it.
        localstate::remember_room(&self.data_dir, &room_id.to_string(), Some(name))?;
        let mut store = self.open_store()?;
        store
            .insert(&validated)
            .map_err(|e| internal("could not persist the room genesis", e))?;
        Ok(room_id.to_string())
    }

    /// `room.list`: every locally known room with name/role/member count/open.
    #[cfg(test)]
    pub(crate) async fn list_rooms(&self) -> CoreResult<Vec<Value>> {
        if !self.db_path().exists() {
            return Ok(Vec::new());
        }
        let self_key = match self.local_identity_key() {
            Ok(key) => key,
            // Protocol v1 historically exposes an empty onboarding state, not
            // an error. Returning no rows is also the strictest privacy result:
            // without an identity there is no authorized room projection.
            Err(error) if error.kind == ErrorKind::IdentityMissing => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        // Enumerate only the daemon-local accepted-room index. Sync can place
        // foreign rows in the shared store, but it cannot add an index entry;
        // therefore even a membership fold is delayed until local provenance
        // has passed this first boundary.
        let room_ids: Vec<RoomId> = localstate::load(&self.data_dir)?
            .rooms
            .keys()
            .filter_map(|room_id| room_id.parse().ok())
            .collect();
        let mut rooms = Vec::with_capacity(room_ids.len());
        for room_id in room_ids {
            let Ok(snapshot) = self.snapshot_for(&room_id).await else {
                continue; // a corrupt room fails its own reads, not the index
            };
            // "YOUR ROOMS" must mean rooms this identity actually belongs (or
            // belonged) to. A room can land in the local store purely because a
            // shared peer's sync backfilled its membership sub-DAG — e.g. the
            // room's owner is also our peer in a DIFFERENT room — even though we
            // were never invited to it. Such a room has no entry for us in the
            // member set. Listing it would both leak a room we are not in (its
            // name and member count) and hand the UI a room that answers every
            // `room.open` with `not_a_member`. Skip it. `member.left`/
            // `member.removed` keep the subject in the member set, so archived
            // (left/removed) rooms still list.
            if Self::require_local_room_access(&snapshot, &self_key).is_err() {
                continue;
            }
            // Authorization succeeded. Only now may the display name or any
            // other room-derived metadata leave the shared store.
            let name = {
                let store = self.open_store()?;
                genesis_name(&store, &room_id)
                    .or_else(|| localstate::local_name(&self.data_dir, &room_id.to_string()))
            };
            let self_member = snapshot.member(&self_key);
            let role = snapshot.role(&self_key).map(role_label);
            let store = self.open_store()?;
            let departures = departure_sets(&store, &room_id)?;
            let status = self_member.map(|member| {
                status_label(
                    member.status,
                    &self_key,
                    &departures.removed,
                    &departures.left,
                )
            });
            // Recency projection (docs/room-attention.md decision 2): the
            // `created_at` the newest signed event's author actually signed —
            // never the wall clock, never render time. One bounded store read
            // with no live session, so a closed room answers exactly like an
            // open one. Both fields stay null when the room has no readable
            // event, so a client renders no recency rather than a fabricated
            // one.
            //
            // MAX BY TIMESTAMP, not causal order. `room_tail` returns canonical
            // `(lamport, event_id)` order, and independently clocked authors
            // can place an event causally last while signing an OLDER
            // `created_at`. Taking the causally-last event would then report a
            // recency that moves backward on the next refresh — breaking both
            // the room ordering and the unread comparison this field exists to
            // serve, and disagreeing with the mock oracles, which already pick
            // max-by-ts. `event_id` breaks ties so the answer is deterministic.
            //
            // A read failure PROPAGATES rather than degrading to null, matching
            // the `departure_sets` call above. Clients read a null recency on a
            // listed row as "this daemon predates the projection" and adjust
            // their unread baseline accordingly (docs/room-attention.md
            // decision 3), so a current daemon emitting null for a transient
            // store error would be misread as a legacy one and would swallow a
            // genuine unread. Every listed room has folded at least one event
            // by this point, so this yields `Some` or fails loudly.
            let (last_event_ts, last_event_kind) = store
                .room_tail(&room_id, RECENCY_SCAN)
                .map_err(|e| internal("could not read the room's recency", e))?
                .iter()
                .filter_map(|se| {
                    materializer::stored_event_recency(se).map(|(ts, kind)| (ts, se.event_id, kind))
                })
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
                .map_or((None, None), |(ts, _, kind)| (Some(ts), kind));
            rooms.push(json!({
                "room_id": room_id.to_string(),
                "name": name,
                "role": role,
                "status": status,
                "member_count": snapshot.members().count(),
                "open": self.is_open(&room_id),
                "last_event_ts": last_event_ts,
                "last_event_kind": last_event_kind,
            }));
        }
        Ok(rooms)
    }

    /// Activate a room session without constructing a protocol projection.
    /// Optional `peers` (`"<endpoint_id>@<ip:port>"`) merge into the room's
    /// persisted dial hints before the spawn (loopback mode has no discovery).
    pub(crate) async fn activate_room(
        &self,
        room_id_str: &str,
        peers: &[String],
    ) -> CoreResult<()> {
        let room_id = parse_room_id(room_id_str)?;
        // Activation is also a read boundary. Authorize before persisting
        // caller-supplied dial hints or attempting a node spawn, otherwise a
        // foreign room present in the shared store becomes an existence oracle
        // with a different error.
        self.readable_snapshot(&room_id).await?;
        if !peers.is_empty() {
            parse_peers(peers)?; // validate before persisting
            localstate::add_peer_hints(&self.data_dir, &room_id.to_string(), peers)?;
        }
        // Serialize node spawn/teardown so two structural flows never race the
        // room's exclusive blob-store lock.
        let _structural = self.structural.lock().await;
        // ONE ENDPOINT-ID PER LIVE NODE — see `close_colliding_live_sessions`
        // below for the multi-room reception bug this guards against.
        if !self.is_open(&room_id) {
            // Resolve the EndpointId this room will bind and clear any collision
            // BEFORE spawning, so two nodes with the same id are never live at
            // once. Only legacy rooms can collide (see the helper's docs).
            self.close_colliding_live_sessions(&room_id).await?;
            let (node, accept_joins, is_owner) = self.spawn_node(&room_id).await?;
            // Seed the push dedupe set with the full history the caller receives
            // here, BEFORE the session is visible to the push loop, so it never
            // re-emits history and never races an un-seeded window.
            let rows = node
                .room_tail(u32::MAX)
                .await
                .map_err(|e| internal("could not read the timeline", e))?;
            let seen: BTreeSet<EventId> = rows.iter().map(|se| se.event_id).collect();
            // The push high-water mark starts past the COMMITTED history the
            // open-time read already covered (non-committed rows hold no rank).
            let committed_so_far = rows
                .iter()
                .filter(|se| crate::projection::is_committed(se))
                .count() as u64;
            let session = Self::make_session(node, accept_joins, is_owner, seen, committed_so_far);
            self.sessions().insert(room_id, session);
        }
        Ok(())
    }

    /// Retired v1 `room.open` projection retained only for internal regression
    /// tests. Protocol-v2 calls use [`Self::activate_room`] and build their
    /// typed answer in `typed.rs`.
    #[cfg(test)]
    pub(crate) async fn open_room(&self, room_id_str: &str, peers: &[String]) -> CoreResult<Value> {
        self.activate_room(room_id_str, peers).await?;
        let room_id = parse_room_id(room_id_str)?;
        let members = self.members(room_id_str).await?;
        let session = self.session(&room_id)?;
        let rows = session
            .node
            .room_tail(u32::MAX)
            .await
            .map_err(|e| internal("could not read the timeline", e))?;
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        let timeline: Vec<Value> = rows
            .iter()
            .filter_map(|se| materializer::materialize(se, &snapshot))
            .collect();
        let node = &session.node;
        Ok(json!({
            "endpoint": {
                "endpoint_id": node.id().to_string(),
                "addr": dialable_addr(node),
            },
            "members": members,
            "timeline": timeline,
        }))
    }

    /// Shut down an already-removed session (pipes first, then the node).
    async fn shutdown_session(
        &self,
        room_id: &RoomId,
        session: Arc<RoomSession>,
    ) -> CoreResult<()> {
        // Keep the freshest peer addresses so a later re-open can redial them.
        // This write is best-effort: a corrupt/unwritable state.json must never
        // leave the live node leaked (its pump + blob-store lock) by aborting
        // before shutdown.
        let harvested = Self::harvest_peer_hints(&session.node).await;
        if let Err(err) =
            localstate::add_peer_hints(&self.data_dir, &room_id.to_string(), &harvested)
        {
            eprintln!("warning: could not persist peer hints for {room_id}: {err}");
        }
        let node = Self::reclaim_session(session).await;
        node.shutdown()
            .await
            .map_err(|e| internal("could not shut the room node down", e))?;
        Ok(())
    }

    /// `room.close`: shut the session down without changing membership.
    pub(crate) async fn close_room(&self, room_id_str: &str) -> CoreResult<()> {
        let room_id = parse_room_id(room_id_str)?;
        let _structural = self.structural.lock().await;
        let Some(session) = self.sessions().remove(&room_id) else {
            return Err(CoreError::new(
                ErrorKind::RoomNotOpen,
                format!("room {room_id} is not open"),
            ));
        };
        self.shutdown_session(&room_id, session).await
    }

    /// Shut down every OTHER open session that would present the SAME
    /// `EndpointId` as the node about to be spawned for `opening`.
    ///
    /// iroh-rooms v1 treats `EndpointId == device_id` as the P2P routing key:
    /// `Node::spawn_room` binds a fresh `iroh::Endpoint` per call, the
    /// endpoint's public key IS the device key, and the accept-side admission
    /// authorizes purely on `Connection::remote_id()` (the QUIC/TLS-proven
    /// EndpointId). When two live nodes share one EndpointId, a remote peer
    /// dialing it is routed — by iroh's per-EndpointId address cache in
    /// loopback mode, and by relay/DNS discovery in real-network mode — to
    /// whichever endpoint bound LAST. Inbound room-A traffic then lands on the
    /// room-B node: if this identity is in both rooms, room-B admission accepts
    /// the link and feeds room-A frames to the room-B engine, which drops them
    /// (room_id mismatch); if not, room-B admission rejects the room-A member
    /// outright. Either way every room but the last-opened goes dark — the
    /// "only the last-joined room receives" symptom. The reference
    /// `iroh-rooms room tail` never hits this: it is one process = one room =
    /// one Endpoint.
    ///
    /// This guard is **collision-aware, not blanket**. Rooms created or joined
    /// by this build derive a distinct device key per room
    /// ([`derive_room_device`]), so they present distinct EndpointIds, this
    /// guard closes nothing, and they stay live together — that is the point of
    /// the derivation. It fires only for **legacy** rooms, whose logs bind the
    /// one global device and which have no rebinding path (the owner's device
    /// is genesis-bound). Two such rooms genuinely cannot both receive, so this
    /// closes one instead of letting it sit open and silently deaf. The closed
    /// room stays fully readable offline (its events live in the shared SQLite
    /// store) and re-opens on demand.
    ///
    /// Called from `open_room` under the structural lock and BEFORE the new
    /// node is spawned, so two nodes sharing an EndpointId are never live at
    /// the same time. Best-effort: a session whose shutdown fails is logged and
    /// dropped from the map regardless, so a stuck teardown never blocks
    /// opening the requested room.
    ///
    /// Cost note: `shutdown_session` waits for in-flight ops on the closed room
    /// to release their session handle, and `open_room` holds `structural`
    /// throughout. For legacy rooms this makes `room.open` wait on another
    /// room's slow `file.fetch`. That is confined to the legacy path and goes
    /// away as rooms are recreated under derived keys.
    async fn close_colliding_live_sessions(&self, opening: &RoomId) -> CoreResult<()> {
        let secret = self.secrets()?;
        let snapshot = self.snapshot_for(opening).await?;
        let device = self.authoring_device_key(&snapshot, &secret, opening);
        let new_id = endpoint_id_of(device.device_key())?;
        let colliders: Vec<RoomId> = self
            .sessions()
            .iter()
            .filter(|(id, sess)| **id != *opening && sess.node.id() == new_id)
            .map(|(id, _)| *id)
            .collect();
        for room_id in colliders {
            // `shutdown_session` already harvests the freshest peer hints so a
            // later re-open of this room can redial.
            let Some(session) = self.sessions().remove(&room_id) else {
                continue;
            };
            // Say it out loud: a room the user had open is going offline, and
            // the only other signal is `room.list`'s `open` flag flipping.
            eprintln!(
                "warning: closing room {room_id} to open {opening} — both are legacy rooms bound \
                 to this identity's global device, so they cannot be online at the same time"
            );
            if let Err(err) = self.shutdown_session(&room_id, session).await {
                eprintln!("warning: could not close room {room_id} while opening another: {err}");
            }
        }
        Ok(())
    }

    /// `room.leave`: publish a signed `member.left` for this identity, then
    /// close this daemon's local live session if one is open. The immutable room
    /// owner cannot leave yet: the protocol has no ownership transfer, and an
    /// owner-authored `member.left` would not remove the genesis admin anyway.
    pub(crate) async fn leave_room(&self, room_id_str: &str) -> CoreResult<String> {
        let room_id = parse_room_id(room_id_str)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let _structural = self.structural.lock().await;

        let event_id = if let Some(session) = self.session_opt(&room_id) {
            let snapshot = session
                .node
                .snapshot()
                .await
                .map_err(|e| internal("could not read the membership snapshot", e))?;
            ensure_can_leave(&snapshot, &self_id, &room_id)?;
            let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
            let admin_identity = snapshot
                .admin()
                .copied()
                .ok_or_else(|| CoreError::internal("room snapshot has no admin"))?;
            let heads = {
                let store = self.open_store()?;
                Self::authorization_class_heads(&store, &room_id, &admin_identity)?
            };
            let wire = build_member_left(
                &secret.identity,
                &room_device,
                &room_id,
                None,
                &heads,
                now_ms(),
            );
            let validated =
                validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(room_id))
                    .map_err(|reason| {
                        CoreError::internal(format!(
                            "freshly built member.left failed validation ({})",
                            reason.code()
                        ))
                    })?;
            let event_id = validated.event_id;
            {
                let store = self.open_store()?;
                let (mut membership, _) = self.fold(&store, &room_id)?;
                match membership.ingest(validated) {
                    Ingest::Accepted { .. } => {}
                    Ingest::Rejected { reason, .. } => {
                        return Err(CoreError::internal(format!(
                            "freshly built member.left was rejected by the fold ({})",
                            reason.code()
                        )))
                    }
                    Ingest::Buffered { .. } => {
                        return Err(CoreError::internal(
                            "freshly built member.left is causally incomplete",
                        ))
                    }
                }
            }
            session
                .node
                .publish(wire.to_bytes())
                .await
                .map_err(|e| internal("could not publish the leave", e))?;
            // Give connected peers a brief chance to ingest the departure before
            // this daemon tears down its room node and stops serving the session.
            tokio::time::sleep(FLUSH_GRACE).await;
            drop(session);
            let removed_session = { self.sessions().remove(&room_id) };
            if let Some(session) = removed_session {
                self.shutdown_session(&room_id, session).await?;
            }
            event_id
        } else {
            let mut store = self.open_store()?;
            let (mut membership, snapshot) = self.fold(&store, &room_id)?;
            ensure_can_leave(&snapshot, &self_id, &room_id)?;
            let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
            let admin_identity = snapshot
                .admin()
                .copied()
                .ok_or_else(|| CoreError::internal("room snapshot has no admin"))?;
            let heads = Self::authorization_class_heads(&store, &room_id, &admin_identity)?;
            let wire = build_member_left(
                &secret.identity,
                &room_device,
                &room_id,
                None,
                &heads,
                now_ms(),
            );
            let validated =
                validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(room_id))
                    .map_err(|reason| {
                        CoreError::internal(format!(
                            "freshly built member.left failed validation ({})",
                            reason.code()
                        ))
                    })?;
            match membership.ingest(validated.clone()) {
                Ingest::Accepted { .. } => {}
                Ingest::Rejected { reason, .. } => {
                    return Err(CoreError::internal(format!(
                        "freshly built member.left was rejected by the fold ({})",
                        reason.code()
                    )))
                }
                Ingest::Buffered { .. } => {
                    return Err(CoreError::internal(
                        "freshly built member.left is causally incomplete",
                    ))
                }
            }
            let event_id = validated.event_id;
            store
                .insert(&validated)
                .map_err(|e| internal("could not persist the leave", e))?;
            event_id
        };

        Ok(bare_event_hex(&event_id))
    }

    /// Author the authority's signed removal of one active member. Repeating a
    /// removal returns the first committed removal event and authors nothing.
    pub(crate) async fn remove_member(
        &self,
        room_id: &RoomId,
        subject: &IdentityKey,
    ) -> CoreResult<RemoveMemberOutcome> {
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let _structural = self.structural.lock().await;

        let session = self.session_opt(room_id);
        let (wire, validated) = {
            let store = self.open_store()?;
            let (mut membership, snapshot) = self.fold(&store, room_id)?;
            if snapshot.admin() != Some(&self_id) {
                return Err(CoreError::new(
                    ErrorKind::PipeDenied,
                    "only the room authority may remove a member",
                ));
            }
            if snapshot.admin() == Some(subject) {
                return Ok(RemoveMemberOutcome::Authority);
            }
            let Some(member) = snapshot.member(subject) else {
                return Ok(RemoveMemberOutcome::Unknown);
            };
            // An invitation or even a malicious removal of a never-joined
            // identity creates a fold row with no device. Protocol v2 keeps
            // outstanding invitations separate from membership and must not
            // let that row become a removable member.
            if member.device.is_none() {
                return Ok(RemoveMemberOutcome::Unknown);
            }
            if member.status == Status::Removed {
                return Ok(match current_member_removal(&store, room_id, subject)? {
                    Some(existing) => RemoveMemberOutcome::Removed(bare_event_hex(&existing)),
                    None => RemoveMemberOutcome::Unknown,
                });
            }
            if member.status != Status::Active {
                return Ok(RemoveMemberOutcome::Unknown);
            }

            let room_device = self.authoring_device_key(&snapshot, &secret, room_id);
            let heads = Self::authorization_class_heads(&store, room_id, &self_id)?;
            let binding =
                DeviceBinding::create(room_id, &secret.identity, room_device.device_key());
            let wire = build_member_removed(
                &secret.identity,
                &room_device,
                room_id,
                subject,
                None,
                Some(binding),
                &heads,
                now_ms(),
            );
            let validated =
                validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(*room_id))
                    .map_err(|reason| {
                        CoreError::internal(format!(
                            "freshly built member.removed failed validation ({})",
                            reason.code()
                        ))
                    })?;
            match membership.ingest(validated.clone()) {
                Ingest::Accepted { .. } => {}
                Ingest::Rejected { reason, .. } => {
                    return Err(CoreError::internal(format!(
                        "freshly built member.removed was rejected by the fold ({})",
                        reason.code()
                    )))
                }
                Ingest::Buffered { .. } => {
                    return Err(CoreError::internal(
                        "freshly built member.removed is causally incomplete",
                    ))
                }
            }
            (wire, validated)
        };

        let event_id = validated.event_id;
        if let Some(session) = session {
            Self::publish_authored(&session.node, room_id, &wire).await?;
        } else {
            let mut store = self.open_store()?;
            store
                .insert(&validated)
                .map_err(|e| internal("could not persist the member removal", e))?;
        }
        Ok(RemoveMemberOutcome::Removed(bare_event_hex(&event_id)))
    }

    /// `room.timeline`: chronological `TimelineEvent`s from the local log
    /// (an offline read — works whether or not the room is open; a second
    /// read handle on the WAL-mode store sees the engine's committed writes).
    #[cfg(test)]
    pub(crate) async fn timeline(
        &self,
        room_id_str: &str,
        limit: Option<u32>,
    ) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        // Sender roles come from the fast membership snapshot (live snapshot for
        // an open room, cached fold for a closed one) — NOT a full O(history)
        // re-fold of the whole log on every timeline read. This also yields
        // `RoomUnknown` for a room with no stored events, exactly like `fold`.
        let snapshot = self.readable_snapshot(&room_id).await?;
        let store = self.open_store()?;
        let rows = store
            .room_tail(&room_id, limit.unwrap_or(200))
            .map_err(|e| internal("could not read the timeline", e))?;
        Ok(rows
            .iter()
            .filter_map(|se| materializer::materialize(se, &snapshot))
            .collect())
    }

    /// `room.members`: the folded roster with the display-status refinement
    /// (`active|invited|removed|left`, mirroring the CLI's D5 projection).
    #[cfg(test)]
    pub(crate) async fn members(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        let snapshot = self.readable_snapshot(&room_id).await?;
        let store = self.open_store()?;
        let departures = departure_sets(&store, &room_id)?;
        Ok(snapshot
            .members()
            .map(|m| {
                json!({
                    "identity_id": m.identity.to_string(),
                    "role": role_label(m.role),
                    "status": status_label(
                        m.status,
                        &m.identity,
                        &departures.removed,
                        &departures.left,
                    ),
                })
            })
            .collect())
    }

    // ------------------------------------------------------------------
    // Invites & joins
    // ------------------------------------------------------------------

    /// `invite.create`: mint a key-bound invite ticket (owner only). When the
    /// room is open the `member.invited` publishes through the live node (so
    /// it also fans out); otherwise it persists directly, like the CLI.
    /// [`Self::create_invite_at`] with a **relative** expiry spec
    /// (`<int>{s|m|h|d}`), resolved against this call's clock.
    ///
    /// Test-only: protocol v2's `invite.mint` carries an absolute `<ts>`, so
    /// the daemon path takes [`Self::create_invite_at`]. The relative form
    /// survives because it is how the lifecycle tests express "an hour from
    /// now" without pinning a clock.
    #[cfg(test)]
    pub(crate) async fn create_invite(
        &self,
        room_id_str: &str,
        invitee_hex: &str,
        role: &str,
        expiry: Option<&str>,
    ) -> CoreResult<String> {
        let absolute = match expiry {
            Some(spec) => Some(parse_expiry(spec, now_ms())?),
            None => None,
        };
        self.create_invite_at(room_id_str, invitee_hex, role, absolute)
            .await
    }

    /// Mint a key-bound invite ticket with an **absolute** expiry in ms since
    /// the epoch, which is what protocol v2's `invite.mint` carries.
    ///
    /// The absolute form exists so the expiry the caller asked for is the
    /// expiry the capability is signed with, exactly. Converting to a relative
    /// spec and re-resolving it here against a later clock shifted the signed
    /// instant off the requested one, so `invite.mint`'s reply, the
    /// `invite.list` row, and the capability itself could all disagree — and a
    /// faithful `op_id` retry that resent the reply's value became an
    /// `op_id_conflict`.
    pub(crate) async fn create_invite_at(
        &self,
        room_id_str: &str,
        invitee_hex: &str,
        role: &str,
        expires_at_ms: Option<u64>,
    ) -> CoreResult<String> {
        let room_id = parse_room_id(room_id_str)?;
        if role != "member" && role != "agent" {
            return Err(CoreError::invalid(format!(
                "role must be \"member\" or \"agent\", got {role:?}"
            )));
        }
        let invitee_key: IdentityKey = invitee_hex.trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid identity_id (expected 64-char hex): {e}"))
        })?;
        let secret = self.secrets()?;
        let admin_identity = secret.identity.identity_key();
        if invitee_key == admin_identity {
            return Err(CoreError::invalid("cannot invite your own identity"));
        }

        let created_at = now_ms();
        let mut invite_id = [0u8; SHORT_ID_LEN];
        getrandom::fill(&mut invite_id).map_err(|e| internal("OS CSPRNG unavailable", e))?;
        let mut secret_bytes = zeroize::Zeroizing::new([0u8; SHORT_ID_LEN]);
        getrandom::fill(secret_bytes.as_mut_slice())
            .map_err(|e| internal("OS CSPRNG unavailable", e))?;
        let cap_hash = capability_hash(&room_id, &invite_id, &secret_bytes);
        // The absolute instant the caller named, signed verbatim.
        let expires_at = expires_at_ms;
        if expires_at.is_some_and(|at| at <= created_at) {
            return Err(CoreError::invalid(
                "expiry must be in the future at the moment the capability is signed",
            ));
        }

        let is_open = self.is_open(&room_id);
        // The whole store-backed authoring path lives in one sync scope so no
        // !Sync store borrow crosses the publish await below.
        let (wire, room_device) = {
            let mut store = self.open_store()?;
            let (mut membership, snapshot) = self.fold(&store, &room_id)?;
            if snapshot.admin() != Some(&admin_identity) {
                return Err(CoreError::new(
                    ErrorKind::NotAMember,
                    format!("only the room owner can issue invites for {room_id}"),
                ));
            }
            let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
            let heads = Self::authorization_class_heads(&store, &room_id, &admin_identity)?;

            let wire = build_member_invited(
                &secret.identity,
                &room_device,
                &room_id,
                &invite_id,
                &cap_hash,
                role,
                &invitee_key,
                expires_at,
                None,
                &heads,
                created_at,
            );
            let validated =
                validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(room_id))
                    .map_err(|reason| {
                        CoreError::internal(format!(
                            "freshly built member.invited failed validation ({})",
                            reason.code()
                        ))
                    })?;
            match membership.ingest(validated.clone()) {
                Ingest::Accepted { .. } => {}
                Ingest::Rejected { reason, .. } => {
                    return Err(CoreError::internal(format!(
                        "freshly built member.invited was rejected by the fold ({})",
                        reason.code()
                    )))
                }
                Ingest::Buffered { .. } => {
                    return Err(CoreError::internal(
                        "freshly built member.invited is causally incomplete",
                    ))
                }
            }
            if !is_open {
                store
                    .insert(&validated)
                    .map_err(|e| internal("could not persist the invite", e))?;
            }
            // The ticket's `discovery` hint must name the endpoint this owner
            // actually binds for this room, so it travels out of the same
            // resolution the room's log dictates.
            (wire, room_device)
        };
        if let Some(session) = self.session_opt(&room_id) {
            // The engine owns the persistence path while the room is open.
            session
                .node
                .publish(wire.to_bytes())
                .await
                .map_err(|e| internal("could not publish the invite", e))?;
            // We are the confirmed owner (checked above) and there is now a
            // pending invite: open the join-bootstrap window so the invitee can
            // pull the membership sub-DAG. The push poll closes it again once the
            // invite has been redeemed (no more `Invited` members).
            session.accept_joins.store(true, Ordering::Relaxed);
        }

        let ticket = RoomInviteTicket {
            room_id,
            invite_id,
            capability_secret: *secret_bytes,
            invitee_key,
            role: role.to_owned(),
            expires_at,
            inviter_identity: admin_identity,
            discovery: vec![room_device.device_key()],
        };
        Ok(ticket.to_string())
    }

    /// `room.join`: redeem a ticket — bootstrap the membership sub-DAG from
    /// the admin over an ephemeral node, author + fold-check + publish the
    /// `member.joined`, and record the room locally (mirrors the CLI join).
    pub(crate) async fn join_room(
        &self,
        ticket_str: &str,
        display_name: Option<&str>,
        peers: &[String],
    ) -> CoreResult<String> {
        let ticket: RoomInviteTicket =
            ticket_str
                .trim()
                .parse()
                .map_err(|e: iroh_rooms::room::TicketError| {
                    CoreError::new(
                        ErrorKind::BadTicket,
                        format!("bad ticket ({}): {e}", e.code()),
                    )
                })?;
        let secret = self.secrets()?;
        // The ticket carries the room id, so the room-scoped device is derived
        // before the first dial: the endpoint this joiner binds and the device
        // its `member.joined` binds are the same key, by construction.
        let room_device = derive_room_device(&secret.device, &ticket.room_id);
        let self_id = secret.identity.identity_key();
        if self_id != ticket.invitee_key {
            return Err(CoreError::new(
                ErrorKind::BadTicket,
                format!(
                    "this ticket is bound to a different identity ({}); yours is {self_id}",
                    ticket.invitee_key
                ),
            )
            .with_hint("ask the admin to re-issue the invite for your identity id"));
        }
        if let Some(expires_at) = ticket.expires_at {
            if expires_at < now_ms() {
                return Err(CoreError::new(
                    ErrorKind::TicketExpired,
                    "this invite ticket has expired",
                ));
            }
        }
        let room_id = ticket.room_id;
        if self.is_open(&room_id) {
            return Err(CoreError::invalid(format!(
                "room {room_id} is already open on this daemon"
            )));
        }

        // Dial set: the ticket's discovery devices, with any caller-supplied
        // "<endpoint_id>@<ip:port>" hints matched by id.
        let peer_addrs = parse_peers(peers)?;
        let mut dial_set: Vec<EndpointAddr> = Vec::new();
        let mut seen_ids = BTreeSet::new();
        for dev in &ticket.discovery {
            let id = endpoint_id_of(*dev)?;
            if !seen_ids.insert(id) {
                continue;
            }
            dial_set.push(
                peer_addrs
                    .iter()
                    .find(|a| a.id == id)
                    .cloned()
                    .unwrap_or_else(|| EndpointAddr::new(id)),
            );
        }
        if dial_set.is_empty() {
            return Err(CoreError::new(
                ErrorKind::PeerUnreachable,
                "the invite ticket carries no admin discovery hint",
            )
            .with_hint("pass peers: [\"<endpoint_id>@<ip:port>\"] in room.join"));
        }

        // Joiner node: talk only to the inviter (allowlist), like the CLI.
        let mut admission = AllowlistAdmission::new();
        for dev in &ticket.discovery {
            admission = admission.bind_device(endpoint_id_of(*dev)?, ticket.inviter_identity);
        }
        let admission = admission.set_active(ticket.inviter_identity);

        let store = self.open_store()?;
        let engine = SyncEngine::open(store, room_id, SyncConfig::default())
            .map_err(|e| internal("could not open the sync engine", e))?;
        let secret_key = SecretKey::from_bytes(&room_device.to_seed());
        // The admin serves the membership closure only after this node proves it
        // holds the invite (upstream issue #112); a plain `Node::spawn` joiner is
        // never bootstrapped and times out.
        let node = Node::spawn_join_bootstrap(
            secret_key,
            Arc::new(admission),
            Arc::new(TracingAudit),
            engine,
            self.net_config(),
            DEFAULT_TICK,
            BootstrapProof {
                room_id,
                invite_id: ticket.invite_id,
                capability_secret: ticket.capability_secret,
            },
        )
        .await
        .map_err(|e| internal("could not bring up the join node", e))?;
        for addr in dial_set {
            node.connect_to(addr);
        }

        let outcome = self
            .bootstrap_and_join(&node, &secret, &room_device, &ticket, display_name, peers)
            .await;
        let shutdown = node.shutdown().await;
        let joined = outcome?;
        shutdown.map_err(|e| internal("could not shut the join node down", e))?;

        Ok(joined.to_string())
    }

    /// The post-bring-up half of the join (split so the node always shuts
    /// down): wait invited, build + fold-check + publish, confirm active.
    async fn bootstrap_and_join(
        &self,
        node: &Node,
        secret: &SecretKeys,
        room_device: &SigningKey,
        ticket: &RoomInviteTicket,
        display_name: Option<&str>,
        peers: &[String],
    ) -> CoreResult<RoomId> {
        let room_id = ticket.room_id;
        let self_id = secret.identity.identity_key();

        // Wait for the membership sub-DAG (genesis + our naming invite) to
        // pull + persist, so we resolve as Invited. A fresh read handle per
        // poll sees the engine's committed pulls (WAL); the handle is scoped
        // so no !Sync store borrow ever lives across an await.
        let deadline = tokio::time::Instant::now() + JOIN_TIMEOUT;
        loop {
            let invited = {
                self.open_store()
                    .and_then(|store| self.fold(&store, &room_id))
                    .is_ok_and(|(_, snapshot)| snapshot.status(&self_id).is_some())
            };
            if invited {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::new(
                    ErrorKind::PeerUnreachable,
                    format!(
                        "could not reach the room admin to bootstrap the join within {JOIN_TIMEOUT:?}"
                    ),
                )
                .with_hint("ask the inviter to open the room, then retry room.join"));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        let heads = {
            let store = self.open_store()?;
            Self::authorization_class_heads(&store, &room_id, &ticket.inviter_identity)?
        };
        let created_at = now_ms();
        let binding = DeviceBinding::create(&room_id, &secret.identity, room_device.device_key());
        let wire = build_member_joined(
            &secret.identity,
            room_device,
            &room_id,
            &ticket.invite_id,
            &ticket.capability_secret,
            &ticket.role,
            binding,
            display_name,
            &heads,
            created_at,
        );
        let validated =
            validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(room_id)).map_err(
                |reason| {
                    CoreError::internal(format!(
                        "freshly built member.joined failed validation ({})",
                        reason.code()
                    ))
                },
            )?;

        // Local fold-check: the deterministic verdict every peer reaches —
        // a bad secret / expiry / role fails here instead of a doomed push.
        {
            let store = self.open_store()?;
            let (mut membership, _) = self.fold(&store, &room_id)?;
            match membership.ingest(validated) {
                Ingest::Accepted { .. } => {}
                Ingest::Rejected { reason, .. } => return Err(join_reject_error(&reason)),
                Ingest::Buffered { .. } => {
                    return Err(CoreError::new(
                        ErrorKind::PeerUnreachable,
                        "the membership history is incomplete; retry once the admin has synced",
                    ))
                }
            }
        }

        // The proposed member.joined is now locally accepted, but has not been
        // published. Persist room provenance, display name, and dial hints in
        // one state transaction at this exact boundary. If the write fails the
        // invite remains redeemable; bootstrap may have stored genesis/invite
        // rows, but no device-bound membership exists and every public read
        // remains default-denied.
        localstate::remember_room_with_peer_hints(
            &self.data_dir,
            &room_id.to_string(),
            display_name,
            peers,
        )?;

        node.publish(wire.to_bytes())
            .await
            .map_err(|e| internal("could not publish the join", e))?;

        // Confirm the local Active transition, then a brief flush grace so
        // the admin ingests the join before the ephemeral node tears down.
        let active = tokio::time::timeout(JOIN_TIMEOUT, async {
            loop {
                if let Ok(snapshot) = node.snapshot().await {
                    if snapshot.is_active(&self_id) {
                        return;
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await;
        if active.is_err() {
            return Err(CoreError::internal(
                "published the join but did not observe the local active transition",
            ));
        }
        tokio::time::sleep(FLUSH_GRACE).await;
        Ok(room_id)
    }

    // ------------------------------------------------------------------
    // Messages & agent status
    // ------------------------------------------------------------------

    /// `message.send` (requires the room to be open — the daemon's live node
    /// persists and fans the frame out).
    pub(crate) async fn send_message(&self, room_id_str: &str, body: &str) -> CoreResult<String> {
        if body.is_empty() {
            return Err(CoreError::invalid("message body must not be empty"));
        }
        if body.len() > MAX_MESSAGE_BODY_BYTES {
            return Err(CoreError::invalid(format!(
                "message body must be at most {MAX_MESSAGE_BODY_BYTES} bytes"
            )));
        }
        let room_id = parse_room_id(room_id_str)?;
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        let sender_id = secret.identity.identity_key();
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        if !snapshot.is_active(&sender_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({sender_id}) is not an active member of room {room_id}"),
            ));
        }
        let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
        let heads = Self::node_heads(&session.node).await?;
        let wire = build_message_text(
            &secret.identity,
            &room_device,
            &room_id,
            body,
            None,
            None,
            &[],
            &heads,
            now_ms(),
        );
        let event_id = Self::publish_authored(&session.node, &room_id, &wire).await?;
        Ok(bare_event_hex(&event_id))
    }

    /// `status.post`: author + publish a signed `agent.status` (any active
    /// member may post — the protocol rule).
    pub(crate) async fn post_status(
        &self,
        room_id_str: &str,
        label: &str,
        message: Option<&str>,
        progress: Option<u64>,
        artifacts: &[String],
    ) -> CoreResult<String> {
        if label.is_empty() || label.len() > MAX_STATUS_LABEL_BYTES {
            return Err(CoreError::invalid(format!(
                "label must be 1..={MAX_STATUS_LABEL_BYTES} bytes"
            )));
        }
        if let Some(msg) = message {
            if msg.len() > MAX_STATUS_MESSAGE_BYTES {
                return Err(CoreError::invalid(format!(
                    "message must be at most {MAX_STATUS_MESSAGE_BYTES} bytes"
                )));
            }
        }
        if let Some(pct) = progress {
            if pct > 100 {
                return Err(CoreError::invalid("progress must be 0..=100"));
            }
        }
        if artifacts.len() > MAX_ARTIFACT_REFS {
            return Err(CoreError::invalid(format!(
                "at most {MAX_ARTIFACT_REFS} artifacts"
            )));
        }
        let artifact_ids = artifacts
            .iter()
            .map(|s| parse_file_id(s))
            .collect::<CoreResult<Vec<_>>>()?;

        let room_id = parse_room_id(room_id_str)?;
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        let sender_id = secret.identity.identity_key();
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        if !snapshot.is_active(&sender_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({sender_id}) is not an active member of room {room_id}"),
            ));
        }
        let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
        let heads = Self::node_heads(&session.node).await?;
        let wire = build_agent_status(
            &secret.identity,
            &room_device,
            &room_id,
            label,
            message,
            &artifact_ids,
            progress,
            &heads,
            now_ms(),
        );
        let event_id = Self::publish_authored(&session.node, &room_id, &wire).await?;
        Ok(bare_event_hex(&event_id))
    }

    // ------------------------------------------------------------------
    // Files
    // ------------------------------------------------------------------

    /// Capture the one live-session capability needed to finish a streamed
    /// upload whose typed room gate has already succeeded.
    ///
    /// This method intentionally does not re-fold membership or re-check
    /// standing. The supplied snapshot is the exact authorization decision
    /// made by the typed pipeline; retaining it and the live session makes the
    /// later consuming finalization a continuation of that decision rather
    /// than a second operation with a new authorization instant.
    pub(crate) fn authorize_file_share_once(
        &self,
        room_id: RoomId,
        snapshot: MembershipSnapshot,
        display_name: String,
        mime_type: String,
        declared_bytes: u64,
    ) -> CoreResult<AuthorizedFileShare> {
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        Ok(AuthorizedFileShare {
            room_id,
            session,
            secret,
            snapshot,
            display_name,
            mime_type,
            declared_bytes,
        })
    }

    /// Consume one previously authorized share capability and import its
    /// sealed source without authoring an event yet.
    pub(crate) async fn import_authorized_file_share(
        &self,
        authorized: AuthorizedFileShare,
        path: &Path,
    ) -> Result<ImportedAuthorizedFileShare, FinalizeFileShareError> {
        let meta = std::fs::metadata(path).map_err(|error| {
            CoreError::internal(format!(
                "could not inspect protocol upload staging: {error}"
            ))
        })?;
        if !meta.is_file() {
            return Err(
                CoreError::internal("protocol upload staging is no longer a regular file").into(),
            );
        }
        if meta.len() != authorized.declared_bytes {
            return Err(FinalizeFileShareError::CountDisagreement {
                observed_bytes: meta.len(),
            });
        }
        let import_path = std::fs::canonicalize(path).map_err(|error| {
            CoreError::internal(format!(
                "could not resolve protocol upload staging: {error}"
            ))
        })?;
        self.assert_shareable_path(&import_path)?;

        let import = authorized
            .session
            .node
            .blob_import(&import_path)
            .await
            .map_err(|error| internal("could not import the file into the blob store", error))?;
        if import.size_bytes != authorized.declared_bytes {
            return Err(FinalizeFileShareError::CountDisagreement {
                observed_bytes: import.size_bytes,
            });
        }

        Ok(ImportedAuthorizedFileShare {
            authorized,
            size_bytes: import.size_bytes,
            hash: import.hash,
        })
    }

    /// Consume one imported capability and author exactly one `file.shared`
    /// event for it.
    pub(crate) async fn publish_imported_file_share(
        &self,
        imported: ImportedAuthorizedFileShare,
    ) -> Result<StagedFileShare, FinalizeFileShareError> {
        let ImportedAuthorizedFileShare {
            authorized,
            size_bytes,
            hash,
        } = imported;

        let mut file_id = [0u8; SHORT_ID_LEN];
        getrandom::fill(&mut file_id).map_err(|error| internal("OS CSPRNG unavailable", error))?;
        let room_device = self.authoring_device_key(
            &authorized.snapshot,
            &authorized.secret,
            &authorized.room_id,
        );
        let heads = Self::node_heads(&authorized.session.node).await?;
        let digest = iroh_rooms::files::HashRef::from_bytes(hash);
        let wire = build_file_shared(
            &authorized.secret.identity,
            &room_device,
            &authorized.room_id,
            file_id,
            &authorized.display_name,
            &authorized.mime_type,
            size_bytes,
            digest,
            Some("raw"),
            &[room_device.device_key()],
            &heads,
            now_ms(),
        );

        let event_id =
            Self::publish_authored(&authorized.session.node, &authorized.room_id, &wire).await?;
        Ok(StagedFileShare {
            file_id: file_handle(&file_id),
            event_id: bare_event_hex(&event_id),
            bytes: size_bytes,
            digest: digest.to_string(),
        })
    }

    /// Preserve the host-staged API's combined import-and-publish operation.
    /// Protocol streams use the two capabilities separately so cleanup can be
    /// made a pre-publication condition.
    pub(crate) async fn finalize_authorized_file_share(
        &self,
        authorized: AuthorizedFileShare,
        path: &Path,
    ) -> Result<StagedFileShare, FinalizeFileShareError> {
        let imported = self.import_authorized_file_share(authorized, path).await?;
        self.publish_imported_file_share(imported).await
    }

    /// `file.share`: import the file into the room's durable blob store and
    /// author + publish the signed `file.shared` reference.
    ///
    /// Since issue #84 the import runs in-session via `Node::blob_import` on the
    /// live serving node — it reuses the store handle the node already owns, so
    /// there is no session cycle: the endpoint, engine pump, and every peer link
    /// stay up, and the node's dial address never goes stale (the old
    /// stale-addr-after-share bug is gone). A concurrent `room.close` still tears
    /// down cleanly — its `reclaim_session` waits for this in-flight share to
    /// finish, exactly as it waits for a `file.fetch`.
    pub(crate) async fn share_file(
        &self,
        room_id_str: &str,
        path_str: &str,
        name: Option<&str>,
        mime: Option<&str>,
    ) -> CoreResult<StagedFileShare> {
        let room_id = parse_room_id(room_id_str)?;
        let secret = self.secrets()?;
        let sender_id = secret.identity.identity_key();

        // Classify + confine the path before touching anything (a bad or
        // out-of-bounds share writes nothing).
        let path = Path::new(path_str);
        let meta = std::fs::metadata(path)
            .map_err(|e| CoreError::invalid(format!("cannot read {}: {e}", path.display())))?;
        if meta.is_dir() {
            return Err(CoreError::invalid(format!(
                "{} is a directory, not a file",
                path.display()
            )));
        }
        if meta.len() > MAX_SHARED_FILE_BYTES {
            return Err(CoreError::invalid(format!(
                "{} is {} bytes; the share limit is {MAX_SHARED_FILE_BYTES} bytes",
                path.display(),
                meta.len()
            )));
        }
        let import_path = std::fs::canonicalize(path)
            .map_err(|e| CoreError::invalid(format!("cannot resolve {}: {e}", path.display())))?;
        self.assert_shareable_path(&import_path)?;

        // file.share is now an ordinary in-session op (like message.send): it
        // holds only its cloned session Arc, taking no `structural` lock — there
        // is no node spawn/teardown to serialize against room.open / room.close.
        let session = self.session(&room_id)?;
        // Access check from the fast membership snapshot (live for this open
        // session) instead of an O(history) re-fold of the whole log.
        let snapshot = self.snapshot_for(&room_id).await?;
        if !snapshot.is_active(&sender_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({sender_id}) is not an active member of room {room_id}"),
            ));
        }

        let display_name = match name {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ => path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned)
                .ok_or_else(|| CoreError::invalid("could not derive a file name; pass name"))?,
        };
        let mime_type = mime
            .filter(|m| !m.is_empty())
            .map_or_else(|| guess_mime(path), str::to_owned);
        let authorized = AuthorizedFileShare {
            room_id,
            session,
            secret,
            snapshot,
            display_name,
            mime_type,
            declared_bytes: meta.len(),
        };
        self.finalize_authorized_file_share(authorized, &import_path)
            .await
            .map_err(|error| match error {
                FinalizeFileShareError::Core(error) => error,
                FinalizeFileShareError::CountDisagreement { observed_bytes } => {
                    CoreError::invalid(format!(
                        "file changed while it was imported (expected {} bytes, observed {observed_bytes})",
                        meta.len()
                    ))
                }
            })
    }

    /// `file.list`: the room's `file.shared` references with honest
    /// availability.
    ///
    /// `available` means "this daemon can `file.fetch` it right now" — i.e. some
    /// OTHER provider device is a currently-connected peer. It deliberately does
    /// NOT include "held locally": `file.fetch` filters this device out of the
    /// provider set and the SDK offers no local-blob read path, so claiming
    /// availability for a self-only file would contradict what fetch can honor
    /// (PROTOCOL.md honesty rule 1). The file is of course still available to
    /// other members while this session serves it — their own `file.list`
    /// reports this device as their online provider.
    #[cfg(test)]
    pub(crate) async fn list_files(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        self.readable_snapshot(&room_id).await?;
        let store = self.open_store()?;
        // Keep an explicit existence check for the established error taxonomy;
        // authorization was already enforced by `readable_snapshot` above.
        if store
            .count(&room_id)
            .map_err(|e| internal("could not count the room's stored events", e))?
            == 0
        {
            return Err(CoreError::new(
                ErrorKind::RoomUnknown,
                format!("no room {room_id} in {}", self.data_dir.display()),
            ));
        }
        let events = store
            .by_type(&room_id, EventType::FileShared)
            .map_err(|e| internal("could not read file.shared events", e))?;
        let session = self.session_opt(&room_id);
        let room_id_str = room_id.to_string();

        let mut files = Vec::with_capacity(events.len());
        for se in &events {
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            let Content::FileShared(f) = ev.content else {
                continue;
            };
            let providers: Vec<DeviceKey> = match &f.providers {
                Some(list) if !list.is_empty() => list.clone(),
                _ => vec![ev.device_id],
            };
            let provider_online = session.as_deref().is_some_and(|s| {
                providers.iter().any(|p| {
                    endpoint_id_of(*p)
                        .is_ok_and(|id| s.node.peer_state(id) == Some(PeerConnState::Connected))
                })
            });
            let file_id = file_handle(&f.file_id);
            let fetched = localstate::fetched_file(&self.data_dir, &room_id_str, &file_id)
                .or_else(|| self.downloaded_file_meta(&f.file_id, &f.name, f.size_bytes));
            files.push(json!({
                "file_id": file_id,
                "name": f.name,
                "size": f.size_bytes,
                "mime": f.mime_type,
                "sender_id": ev.sender_id.to_string(),
                "ts": ev.created_at,
                "available": provider_online,
                "providers": providers.len(),
                "fetched": fetched.is_some(),
                "local_path": fetched.as_ref().map(|meta| meta.path.display().to_string()),
                "local_bytes": fetched.as_ref().map(|meta| meta.bytes),
                "fetched_at_ms": fetched.as_ref().map(|meta| meta.fetched_at_ms),
            }));
        }
        Ok(files)
    }

    /// `file.fetch`: verified retrieval from an asserted provider over the
    /// open session's endpoint, with the honest failure taxonomy — never a
    /// silent partial.
    pub(crate) async fn fetch_file(
        &self,
        room_id_str: &str,
        file_id_str: &str,
        save_dir: Option<&str>,
    ) -> CoreResult<FetchedFile> {
        let room_id = parse_room_id(room_id_str)?;
        let file_id = parse_file_id(file_id_str)?;
        let snapshot = self.readable_snapshot(&room_id).await?;
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
        let self_id = secret.identity.identity_key();
        // The endpoint this node actually binds for this room, so the
        // self-provider filter below matches what peers see us dial from.
        let self_device = endpoint_id_of(room_device.device_key())?;

        // Fetch is stricter than archive reads: the shared guard above proves
        // prior membership, while transfer additionally requires ACTIVE
        // membership at the time of the network request.
        if !snapshot.is_active(&self_id) {
            return Err(CoreError::new(
                ErrorKind::FileUnauthorized,
                format!("this identity ({self_id}) is not an active member of room {room_id}"),
            ));
        }
        // Sync scope: the !Sync store never crosses the fetch awaits below.
        let (shared, author_device) = {
            let store = self.open_store()?;
            let events = store
                .by_type(&room_id, EventType::FileShared)
                .map_err(|e| internal("could not read file.shared events", e))?;
            let Some(found) = find_file_shared(&events, file_id) else {
                return Err(CoreError::new(
                    ErrorKind::FileUnavailable,
                    format!("no such file {file_id_str} in room {room_id}"),
                ));
            };
            found
        };
        if let Some(format) = shared.blob_format.as_deref() {
            if format != "raw" {
                return Err(CoreError::invalid(format!(
                    "file {file_id_str} uses blob_format={format}; only raw is fetchable"
                )));
            }
        }

        // Resolve the destination before any network work. A bad `save_dir`
        // must not cost peer connections and a full transfer first, and must
        // not surface as `file_unavailable` when every provider is offline.
        let dir = self.resolve_fetch_dir(save_dir)?;

        let provider_devices: Vec<DeviceKey> = match &shared.providers {
            Some(list) if !list.is_empty() => list.clone(),
            _ => vec![author_device],
        };
        let provider_ids: Vec<(DeviceKey, EndpointId)> = provider_devices
            .iter()
            .filter_map(|dev| endpoint_id_of(*dev).ok().map(|id| (*dev, id)))
            .filter(|(_, id)| *id != self_device)
            .collect();
        let mut providers: Vec<EndpointAddr> = Vec::with_capacity(provider_ids.len());
        let mut provider_devices = Vec::with_capacity(provider_ids.len());
        for (device, id) in provider_ids {
            providers.push(self.enriched_addr(&session.node, &room_id, id).await);
            provider_devices.push(device);
        }
        if providers.is_empty() {
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!(
                    "file {file_id_str} has no other provider to fetch from; there is no central \
                     inbox and no guaranteed offline delivery"
                ),
            ));
        }

        let declared = *shared.blob_hash.as_bytes();
        let mut fetched: Option<(Vec<u8>, DeviceKey)> = None;
        let (mut denied_at_connect, mut attempted) = (0usize, 0usize);
        for (provider, provider_device) in providers.iter().zip(provider_devices) {
            let (outcome, data) = session
                .node
                .fetch_file(provider.clone(), declared, declared, FETCH_TIMEOUT)
                .await;
            // The outcome enum is matched by name via its Debug string-free
            // variants (the facade re-exports FetchOutcome).
            use iroh_rooms::experimental::blob::FetchOutcome as O;
            match outcome {
                O::Fetched => {
                    fetched = data.map(|b| (b.to_vec(), provider_device));
                    break;
                }
                O::HashMismatch => {
                    // The upstream mismatch arm still hands back the bytes it
                    // rejected, so the digest they actually hash to is
                    // computable here rather than unknowable. It travels as
                    // the error's machine-readable detail so `file.fetch` can
                    // serve `digest_mismatch { expected, observed }` with both
                    // halves real, instead of an empty `observed`.
                    let observed = data.map(|bytes| blake3::hash(&bytes).to_hex().to_string());
                    let error = CoreError::new(
                        ErrorKind::HashMismatch,
                        format!(
                            "integrity check FAILED: fetched bytes do not hash to the declared \
                             {}; refusing to save",
                            shared.blob_hash
                        ),
                    );
                    return Err(match observed {
                        Some(observed) => error.with_detail(observed),
                        None => error,
                    });
                }
                O::DeniedAtConnect => {
                    denied_at_connect += 1;
                    attempted += 1;
                }
                O::DeniedPerHash | O::Unavailable => {
                    attempted += 1;
                }
            }
        }
        let Some((data, provider_device)) = fetched else {
            if attempted > 0 && denied_at_connect == attempted {
                return Err(CoreError::new(
                    ErrorKind::FileUnauthorized,
                    format!(
                        "file {file_id_str} could not be fetched: every provider refused the \
                         connection (this identity may not be an active member from their view)"
                    ),
                ));
            }
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!(
                    "file {file_id_str} is currently unavailable: no peer holding it is online"
                ),
            ));
        };

        // Save atomically under the destination resolved before the fetch
        // (default <data-dir>/downloads), never overwriting an existing file.
        std::fs::create_dir_all(&dir)
            .map_err(|e| internal("could not create the save directory", e))?;
        let mut target = dir.join(sanitize_name(&shared.name, file_id));
        if target.exists() {
            target = dir.join(format!(
                "{}_{}",
                hex::encode(file_id),
                sanitize_name(&shared.name, file_id)
            ));
        }
        save_atomic(&target, &data)?;
        localstate::remember_fetched_file(
            &self.data_dir,
            &room_id.to_string(),
            &file_handle(&file_id),
            &target,
            data.len() as u64,
        )?;

        Ok(FetchedFile {
            #[cfg(test)]
            path: target,
            bytes: data.len() as u64,
            provider_device,
        })
    }

    /// A previously verified local copy addressed by protocol identifiers, never
    /// by a browser-supplied filesystem path.
    pub(crate) async fn local_file(
        &self,
        room_id_str: &str,
        file_id_str: &str,
    ) -> CoreResult<LocalFile> {
        let room_id = parse_room_id(room_id_str)?;
        self.readable_snapshot(&room_id).await?;
        let file_id = parse_file_id(file_id_str)?;
        let store = self.open_store()?;
        let events = store
            .by_type(&room_id, EventType::FileShared)
            .map_err(|e| internal("could not read file.shared events", e))?;
        let Some((shared, _)) = find_file_shared(&events, file_id) else {
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!("no such file {file_id_str} in room {room_id}"),
            ));
        };
        self.local_file_for_shared(&room_id, &file_id, file_id_str, &shared)
    }

    /// Resolve the local copy for a signed `file.shared` fact after the caller
    /// has already passed room authorization and looked that fact up.
    ///
    /// This is the shared no-second-scan seam used by protocol `file.read`.
    /// The outer [`Self::local_file`] wrapper retains its established parsing
    /// and authorization behavior for host-controlled HTTP responses.
    pub(crate) fn local_file_for_shared(
        &self,
        room_id: &RoomId,
        file_id: &[u8; SHORT_ID_LEN],
        file_id_str: &str,
        shared: &iroh_rooms::files::FileShared,
    ) -> CoreResult<LocalFile> {
        let file_id_handle = file_handle(file_id);
        let room_id_key = room_id.to_string();
        let Some(local) = localstate::fetched_file(&self.data_dir, &room_id_key, &file_id_handle)
            .or_else(|| self.downloaded_file_meta(file_id, &shared.name, shared.size_bytes))
        else {
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!("file {file_id_str} has not been fetched on this daemon"),
            )
            .with_hint("fetch the file first, then open the local copy"));
        };
        Ok(LocalFile {
            path: local.path,
            name: shared.name.clone(),
            mime: shared.mime_type.clone(),
            bytes: local.bytes,
        })
    }

    // ------------------------------------------------------------------
    // Pipes
    // ------------------------------------------------------------------

    /// `pipe.expose`: announce + serve a loopback TCP target to exactly one
    /// authorized peer (the runtime rule) through the open session's node.
    #[cfg(test)]
    pub(crate) async fn pipe_expose(
        &self,
        room_id_str: &str,
        target_str: &str,
        peer_identity: &str,
    ) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        let target = SocketAddr::from_str(target_str.trim()).map_err(|e| {
            CoreError::invalid(format!(
                "invalid target {target_str:?} (expected ip:port): {e}"
            ))
        })?;
        if !is_loopback_target(&target) {
            return Err(CoreError::new(
                ErrorKind::PipeDenied,
                format!("refusing to expose non-loopback target {target}"),
            )
            .with_hint("pipes may only forward to 127.0.0.0/8 or ::1"));
        }
        let peer: IdentityKey = peer_identity.trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid peer_identity (expected 64-char hex): {e}"))
        })?;
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        if !snapshot.is_active(&self_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({self_id}) is not an active member of room {room_id}"),
            ));
        }
        let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
        let pipe_id = session
            .node
            .pipe_expose(
                &secret.identity,
                &room_device,
                &room_id,
                target,
                "pipe",
                target_str.trim(),
                &[peer],
                None,
                now_ms(),
            )
            .await
            .map_err(|e| {
                CoreError::new(
                    ErrorKind::PipeDenied,
                    format!("could not expose the pipe: {e:#}"),
                )
            })?;

        let event_id = self
            .find_pipe_event(&room_id, EventType::PipeOpened, pipe_id)
            .await?;
        Ok(json!({
            "pipe_id": hex::encode(pipe_id),
            "event_id": event_id,
        }))
    }

    /// `pipe.publish` (v2): expose a loopback target to a set of authorized
    /// peers — every active member for `audience: room`, or an explicit
    /// subject list. Returns the pipe id and the authored event id.
    pub(crate) async fn pipe_expose_multi(
        &self,
        room_id: &RoomId,
        target: SocketAddr,
        target_hint: &str,
        allowed: &[IdentityKey],
    ) -> CoreResult<([u8; SHORT_ID_LEN], String)> {
        if !is_loopback_target(&target) {
            return Err(CoreError::new(
                ErrorKind::PipeDenied,
                format!("refusing to expose non-loopback target {target}"),
            ));
        }
        if allowed.is_empty() {
            return Err(CoreError::invalid(
                "pipe.publish needs a non-empty audience",
            ));
        }
        let session = self.session(room_id)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        if !snapshot.is_active(&self_id) {
            return Err(CoreError::new(
                ErrorKind::NotAMember,
                format!("this identity ({self_id}) is not an active member of room {room_id}"),
            ));
        }
        let room_device = self.authoring_device_key(&snapshot, &secret, room_id);
        let pipe_id = session
            .node
            .pipe_expose(
                &secret.identity,
                &room_device,
                room_id,
                target,
                "pipe",
                target_hint,
                allowed,
                None,
                now_ms(),
            )
            .await
            .map_err(|e| {
                CoreError::new(
                    ErrorKind::PipeDenied,
                    format!("could not expose the pipe: {e:#}"),
                )
            })?;
        let event_id = self
            .find_pipe_event(room_id, EventType::PipeOpened, pipe_id)
            .await?;
        Ok((pipe_id, event_id))
    }

    /// `pipe.list`: the room's pipes from the local log, with open/closed
    /// state and whether this daemon currently forwards or serves them.
    #[cfg(test)]
    pub(crate) async fn pipe_list(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        self.readable_snapshot(&room_id).await?;
        let store = self.open_store()?;
        let profile = crate::identity::load_profile(&self.data_dir)?;
        let session = self.session_opt(&room_id);

        let closed = closed_pipe_ids(&store, &room_id)?;
        let opened = store
            .by_type(&room_id, EventType::PipeOpened)
            .map_err(|e| internal("could not read pipe.opened events", e))?;
        let mut pipes = Vec::new();
        for se in opened {
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            let Content::PipeOpened(p) = ev.content else {
                continue;
            };
            let is_closed = closed.contains(&p.pipe_id);
            let is_owner = profile
                .as_ref()
                .is_some_and(|prof| prof.identity_id == p.owner_id.to_string());
            // `connected` is truthful per pipe (issue #86): the connector side
            // knows it holds a live forwarder, and the owner side asks the node
            // for THIS pipe's live session count (`live_pipe_sessions_for`)
            // rather than the node-wide aggregate — so it stays honest even with
            // several pipes open at once (no single-open-pipe caveat).
            let connected = session.as_deref().is_some_and(|s| {
                s.forwarders
                    .lock()
                    .expect("forwarders poisoned")
                    .connections
                    .values()
                    .any(|connection| connection.pipe_id == p.pipe_id)
                    || (is_owner && !is_closed && s.node.live_pipe_sessions_for(p.pipe_id) > 0)
            });
            // Every authorized peer, not just the first — a validated remote
            // `pipe.opened` may carry several, and hiding the rest would
            // misrepresent who can reach the exposed loopback target. Our own
            // `pipe.expose` always authorizes exactly one, so the common single
            // value is unchanged.
            let authorized_peer = authorized_peer_value(&p.allowed_members);
            pipes.push(json!({
                "pipe_id": hex::encode(p.pipe_id),
                "target": p.target_hint,
                "opened_by": p.owner_id.to_string(),
                "authorized_peer": authorized_peer,
                "state": if is_closed { "closed" } else { "open" },
                "connected": connected,
            }));
        }
        Ok(pipes)
    }

    /// Whether this daemon currently holds a local connection for one pipe.
    /// Connector-side forwarders and publisher-side accepted sessions are the
    /// two runtime forms of the same v2 fact.
    pub(crate) fn pipe_connection_open(
        &self,
        room_id: &RoomId,
        pipe_id: [u8; SHORT_ID_LEN],
        owner_id: &IdentityKey,
    ) -> bool {
        let Some(session) = self.session_opt(room_id) else {
            return false;
        };
        let connector_open = session
            .forwarders
            .lock()
            .expect("forwarders poisoned")
            .connections
            .values()
            .any(|connection| connection.pipe_id == pipe_id);
        let local_is_owner = self
            .local_identity_key()
            .is_ok_and(|identity| identity == *owner_id);
        connector_open || (local_is_owner && session.node.live_pipe_sessions_for(pipe_id) > 0)
    }

    /// `pipe.connect`: bind a local loopback forwarder toward the pipe owner
    /// and keep it alive inside the session. Returns the local address.
    pub(crate) async fn pipe_connect(
        &self,
        room_id_str: &str,
        pipe_id_hex: &str,
    ) -> CoreResult<String> {
        let room_id = parse_room_id(room_id_str)?;
        let pipe_id = parse_pipe_id(pipe_id_hex)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let session = self.session(&room_id)?;

        // Wait (bounded) for the pipe.opened to sync, so we learn the owner.
        let opened = tokio::time::timeout(PIPE_SYNC_WAIT, async {
            loop {
                if let Some(o) = session.node.pipe_opened(pipe_id).await {
                    return o;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| {
            CoreError::invalid(format!(
                "no pipe {pipe_id_hex} known in room {room_id} (announcement not synced?)"
            ))
        })?;
        if opened.owner_id == self_id {
            return Err(CoreError::invalid(
                "this daemon owns the pipe; connect from the authorized peer instead",
            ));
        }
        let owner_id = EndpointId::from_bytes(opened.owner_endpoint.as_bytes())
            .map_err(|e| CoreError::internal(format!("pipe owner endpoint is invalid: {e}")))?;
        let owner_addr = self.enriched_addr(&session.node, &room_id, owner_id).await;

        let forwarder = match session.node.pipe_connect(owner_addr, pipe_id, 0).await {
            Ok(f) => f,
            Err(err) => {
                return Err(match err.downcast_ref::<PipeError>() {
                    Some(PipeError::OwnerUnreachable(_)) => CoreError::new(
                        ErrorKind::PeerUnreachable,
                        format!("the pipe owner is unreachable: {err:#}"),
                    ),
                    _ => CoreError::new(
                        ErrorKind::PipeDenied,
                        format!("could not connect to the pipe: {err:#}"),
                    ),
                })
            }
        };
        let local_addr = forwarder.local_addr().to_string();
        let mut registry = session.forwarders.lock().expect("forwarders poisoned");
        if registry.revoked.contains(&pipe_id) {
            drop(registry);
            forwarder.shutdown();
            return Err(CoreError::new(
                ErrorKind::PipeDenied,
                format!("pipe {pipe_id_hex} was revoked while the connection opened"),
            ));
        }
        if registry.connections.contains_key(&local_addr) {
            drop(registry);
            forwarder.shutdown();
            return Err(CoreError::internal(format!(
                "local pipe connection id collision at {local_addr}"
            )));
        }
        registry.connections.insert(
            local_addr.clone(),
            LocalPipeConnection { pipe_id, forwarder },
        );
        Ok(local_addr)
    }

    /// Release exactly one connector-side local connection. The connection id
    /// is globally opaque to the host, so search only the currently open room
    /// sessions and never consult signed room state or author an event.
    pub(crate) fn pipe_release(&self, connection_id: &str) -> bool {
        let sessions: Vec<Arc<RoomSession>> = self.sessions().values().cloned().collect();
        for session in sessions {
            let connection = session
                .forwarders
                .lock()
                .expect("forwarders poisoned")
                .connections
                .remove(connection_id);
            if let Some(connection) = connection {
                connection.forwarder.shutdown();
                return true;
            }
        }
        false
    }

    /// Tear down every connector-side local connection for one published pipe.
    /// Used both by the local revoke path and when a remote `pipe.closed`
    /// commits through the typed push loop.
    pub(crate) fn release_pipe_connections(
        &self,
        room_id: &RoomId,
        pipe_id_hex: &str,
    ) -> CoreResult<usize> {
        let pipe_id = parse_pipe_id(pipe_id_hex)?;
        let Some(session) = self.session_opt(room_id) else {
            return Ok(0);
        };
        let forwarders: Vec<PipeForwarder> = {
            let mut registry = session.forwarders.lock().expect("forwarders poisoned");
            registry.revoked.insert(pipe_id);
            let connection_ids: Vec<String> = registry
                .connections
                .iter()
                .filter(|(_, connection)| connection.pipe_id == pipe_id)
                .map(|(connection_id, _)| connection_id.clone())
                .collect();
            connection_ids
                .iter()
                .filter_map(|connection_id| registry.connections.remove(connection_id))
                .map(|connection| connection.forwarder)
                .collect()
        };
        let released = forwarders.len();
        for forwarder in forwarders {
            forwarder.shutdown();
        }
        Ok(released)
    }

    /// `pipe.close`: publish a signed `pipe.closed` and tear down any local
    /// forwarder.
    ///
    /// **The publisher, and only the publisher.** The record makes revocation a
    /// relation, not a role: "`pipe.revoke` is **not** on this list. It is
    /// restricted to the pipe's publisher, which is a narrower relation than
    /// role and answers `pipe_not_publisher`" (`docs/protocol-v2.md`). An
    /// earlier revision also admitted the room's administrator, which let one
    /// subject destroy another subject's published tunnel — a role bypassing a
    /// relation the record deliberately made narrower than any role.
    pub(crate) async fn pipe_close(
        &self,
        room_id_str: &str,
        pipe_id_hex: &str,
    ) -> CoreResult<String> {
        let room_id = parse_room_id(room_id_str)?;
        let pipe_id = parse_pipe_id(pipe_id_hex)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();

        // Access check from the fast membership snapshot (live for an open
        // session, cached fold for a closed room) — not an O(history) re-fold.
        let snapshot = self.snapshot_for(&room_id).await?;
        let room_device = self.authoring_device_key(&snapshot, &secret, &room_id);
        // Sync scope: no !Sync store borrow crosses the pipe_close await.
        {
            let store = self.open_store()?;
            // Unknown-pipe first, so a pipe this daemon has never seen stays
            // `pipe_unknown` rather than being reported as an authorization
            // refusal that would confirm it exists.
            let Some(opened) = open_pipe(&store, &room_id, pipe_id)? else {
                return Err(CoreError::invalid(format!(
                    "no pipe {pipe_id_hex} known in room {room_id}"
                )));
            };
            if opened.owner_id != self_id {
                return Err(CoreError::new(
                    ErrorKind::PipeDenied,
                    "only the pipe's publisher can close it",
                ));
            }
        }

        let session = self.session(&room_id)?;
        session
            .node
            .pipe_close(
                &secret.identity,
                &room_device,
                &room_id,
                pipe_id,
                Some("closed"),
                now_ms(),
            )
            .await
            .map_err(|e| internal("could not publish pipe.closed", e))?;

        self.release_pipe_connections(&room_id, pipe_id_hex)?;
        let event_id = self
            .find_pipe_event(&room_id, EventType::PipeClosed, pipe_id)
            .await?;
        Ok(event_id)
    }

    /// Find the freshest persisted pipe event of `ty` for `pipe_id` (the
    /// engine persists synchronously on publish; a short retry covers WAL
    /// visibility across connections).
    async fn find_pipe_event(
        &self,
        room_id: &RoomId,
        ty: EventType,
        pipe_id: [u8; SHORT_ID_LEN],
    ) -> CoreResult<String> {
        for _ in 0..20 {
            // Sync scope per poll so the !Sync store never crosses the sleep.
            let found = {
                let store = self.open_store()?;
                let rows = store
                    .by_type(room_id, ty)
                    .map_err(|e| internal("could not read pipe events", e))?;
                let mut found = None;
                for se in rows {
                    let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                        continue;
                    };
                    let matches = match &ev.content {
                        Content::PipeOpened(p) => p.pipe_id == pipe_id,
                        Content::PipeClosed(p) => p.pipe_id == pipe_id,
                        _ => false,
                    };
                    if matches {
                        found = Some(bare_event_hex(&se.event_id));
                    }
                }
                found
            };
            if let Some(id) = found {
                return Ok(id);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        Err(CoreError::internal(
            "the pipe event did not appear in the local store",
        ))
    }

    // ------------------------------------------------------------------
    // Peers & pushes
    // ------------------------------------------------------------------

    /// `peers.status`: truthful live peer states + path diagnostics from the
    /// open session's node (never inferred from latency).
    #[cfg(test)]
    pub(crate) async fn peers_status(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        self.readable_snapshot(&room_id).await?;
        let session = self.session(&room_id)?;
        Ok(Self::peers_of(&session.node).await)
    }

    /// The `PeerStatus` list for one live node.
    #[cfg(test)]
    async fn peers_of(node: &Node) -> Vec<Value> {
        let paths: HashMap<EndpointId, &'static str> = node
            .peer_paths()
            .await
            .into_iter()
            .map(|(device, path, _relay)| (device, path.label()))
            .collect();
        node.peer_entries()
            .into_iter()
            .map(|(device, entry)| {
                let state = match entry.state {
                    PeerConnState::Connected => "connected",
                    PeerConnState::Connecting => "connecting",
                    // The protocol enum has no "unauthorized" state; both mean
                    // "no live authorized link right now".
                    PeerConnState::Offline | PeerConnState::Unauthorized => "offline",
                };
                let path = if entry.state == PeerConnState::Connected {
                    match paths.get(&device).copied() {
                        // "mixed" = direct + relay both active; a direct path
                        // exists, so it reads as direct.
                        Some("direct" | "mixed") => json!("direct"),
                        Some("relay") => json!("relay"),
                        _ => Value::Null,
                    }
                } else {
                    Value::Null
                };
                // `identity` is only set once the SDK has bound this device to
                // a membership identity (on admit); null before/during
                // admission is expected, not a bug.
                let identity_id = entry.identity.as_ref().map(|id| id.to_string());
                json!({
                    "endpoint_id": device.to_string(),
                    "state": state,
                    "path": path,
                    "identity_id": identity_id,
                })
            })
            .collect()
    }

    /// Reconcile poll (the push safety net, issue #83): the room's
    /// not-yet-pushed validated events (own or remote), each returned exactly
    /// once, as materialized `TimelineEvent`s.
    ///
    /// Since #83 the primary, sub-second push path is
    /// [`Self::recv_room_events_typed`] (the node's `room_events` broadcast);
    /// this poll stays as the reconcile
    /// safety net that a lossy broadcast (a lagged receiver) cannot let drift,
    /// and it is the sole place that keeps the join-bootstrap `accept_joins`
    /// window tied to live pending-invite state. Both paths dedupe against the
    /// same `seen` set, so an event delivered by either is pushed exactly once.
    ///
    /// Scans the FULL causally-complete tail (`room_tail(u32::MAX)`), not a
    /// fixed 512-row window: `lamport` is causal, not receive-monotonic, so a
    /// late/concurrent event authored against an old frontier arrives with a low
    /// lamport and would sit permanently below a moving top-N cutoff. Scanning
    /// the whole tail and deduping against `seen` guarantees every ingested
    /// event is pushed exactly once regardless of its lamport (PROTOCOL.md
    /// `room.event`). Materialization only runs for genuinely new ids.
    #[cfg(test)]
    pub(crate) async fn poll_new_events(&self, room_id: &RoomId) -> CoreResult<Vec<Value>> {
        let session = self.session(room_id)?;
        let rows = session
            .node
            .room_tail(u32::MAX)
            .await
            .map_err(|e| internal("could not read the timeline", e))?;
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        // Keep the join-bootstrap window tied to live state: an owner hosts joins
        // only while an invite is actually pending (closed again once every
        // invitee has redeemed and no `Invited` member remains).
        session.accept_joins.store(
            session.is_owner && any_pending_invite(&snapshot),
            Ordering::Relaxed,
        );
        let mut seen = session.seen.lock().expect("seen poisoned");
        let mut out = Vec::new();
        for se in &rows {
            if seen.insert(se.event_id) {
                if let Some(v) = materializer::materialize(se, &snapshot) {
                    out.push(v);
                }
            }
        }
        Ok(out)
    }

    /// The typed-v2 reconcile poll: identical to [`Self::poll_new_events`] but
    /// materializes committed events as typed [`CommittedEvent`]s.
    ///
    /// Scans the full canonical tail (the reconcile safety net), so it is the
    /// path that observes a late-arriving concurrent sibling interleaving
    /// below an already-pushed position. `collect_committed` marks that
    /// reorder; the engine turns it into an explicit `gap` so subscribers
    /// discard and resync the shifted suffix rather than trust a silently
    /// renumbered one.
    pub(crate) async fn poll_new_events_typed(
        &self,
        room_id: &RoomId,
    ) -> CoreResult<Vec<CommittedEvent>> {
        let session = self.session(room_id)?;
        let rows = session
            .node
            .room_tail(u32::MAX)
            .await
            .map_err(|e| internal("could not read the timeline", e))?;
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        session.accept_joins.store(
            session.is_owner && any_pending_invite(&snapshot),
            Ordering::Relaxed,
        );
        let mut seen = session.seen.lock().expect("seen poisoned");
        let mut next_rank = session.next_push_rank.lock().expect("next rank poisoned");
        Ok(collect_committed(
            &rows,
            &snapshot,
            &mut seen,
            &mut next_rank,
        ))
    }

    /// The typed-v2 primary push path. It uses the room-event broadcast and
    /// full-tail lag recovery, then materializes committed events as typed
    /// [`CommittedEvent`]s ranked densely with reorder detection (see
    /// [`collect_committed`]).
    pub(crate) async fn recv_room_events_typed(
        &self,
        room_id: &RoomId,
    ) -> CoreResult<Vec<CommittedEvent>> {
        let room_rx = self.session(room_id)?.room_rx.clone();

        // Park on the broadcast until at least one event commits (or the room
        // closes). The batch itself is only the wake-up signal — the
        // authoritative set is the full tail below — so a lossy `Lagged`
        // receiver needs no special branch here: the tail read recovers
        // anything the broadcast dropped, exactly as the reconcile poll does.
        {
            let mut rx = room_rx.lock().await;
            match rx.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CoreError::new(
                        ErrorKind::RoomNotOpen,
                        format!("room {room_id} closed"),
                    ));
                }
            }
        }

        let session = self.session(room_id)?;
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;
        // Rank the room's COMMITTED events densely over the full canonical
        // tail. Only the full tail reveals whether a late concurrent sibling
        // interleaved below an already-served position, and it is the
        // authoritative recovery for a lagged broadcast — so it is ranked on
        // every wake-up, hot path and lag alike. `collect_committed` ranks
        // new events against the high-water mark and marks any reorder so the
        // stream emits a corrective gap.
        let rows = session
            .node
            .room_tail(u32::MAX)
            .await
            .map_err(|e| internal("could not read the timeline", e))?;
        let mut seen = session.seen.lock().expect("seen poisoned");
        let mut next_rank = session.next_push_rank.lock().expect("next rank poisoned");
        Ok(collect_committed(
            &rows,
            &snapshot,
            &mut seen,
            &mut next_rank,
        ))
    }

    /// Drain the session's `conn_events` broadcast; `true` if any peer
    /// connection transition happened since the last drain.
    pub(crate) fn drain_conn_changes(&self, room_id: &RoomId) -> bool {
        let Some(session) = self.session_opt(room_id) else {
            return false;
        };
        let mut conn_rx = session.conn_rx.lock().expect("conn_rx poisoned");
        let mut changed = false;
        loop {
            match conn_rx.try_recv() {
                Ok(_) => changed = true,
                Err(broadcast::error::TryRecvError::Lagged(_)) => changed = true,
                Err(_) => break,
            }
        }
        changed
    }

    // ------------------------------------------------------------------
    // Agents (fleet reads) — docs/agent-orchestration.md §3
    // ------------------------------------------------------------------

    /// `agents.fleet`: the aggregated agent view across every locally known
    /// room this identity belongs or belonged to, open or not. Candidate ids
    /// come only from the daemon-local accepted-room index; sync cannot add to
    /// that index. The membership fold remains the authoritative second check
    /// before a room contributes any count, name, agent, status, or signal.
    ///
    /// A **pure read**: it authors nothing, opens no room, and invents no
    /// count. Every number derives from folded stored events plus live
    /// `PeerConnState` on rooms this daemon has open. Liveness follows the
    /// §1.2 decision table via [`fleet::derive_liveness`] — in particular a
    /// `working` latest status with no connected peer reports `stale`, never
    /// `working`, and a room without an open session can never read
    /// `online-idle`/`working` (no live peer state exists to support it).
    #[cfg(test)]
    pub(crate) async fn agents_fleet(&self) -> CoreResult<Value> {
        let now = crate::now_ms();
        let self_id = self.local_identity_key()?;
        // Candidate rooms: only accepted local creates/joins. A shared SQLite
        // store may contain a foreign room's sync backfill, but sync never
        // writes this index, so foreign rows are not folded or decoded here.
        let known: BTreeSet<String> = localstate::load(&self.data_dir)?
            .rooms
            .keys()
            .cloned()
            .collect();

        let scans: Vec<RoomScan> = if self.db_path().exists() {
            known
                .iter()
                .filter_map(|room_str| {
                    room_str.parse::<RoomId>().ok().map(|room_id| RoomScan {
                        room_id,
                        room_str: room_str.clone(),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut rooms_total = 0usize;
        let mut rooms_covered = 0usize;
        let mut agents: BTreeMap<String, FleetAgentAgg> = BTreeMap::new();

        // Phase 2 (async): fold-free membership via `snapshot_for` (O(1) for
        // open rooms, cached for closed rooms), aggregated over each room's
        // timeline rows. A room whose log will not fold or whose membership is
        // unauthorized is excluded from every fleet count — never a guess.
        //
        // READ ORDER: the snapshot is taken first, then the rows are read from a
        // fresh short-lived store *after* it. Because a room's log is
        // append-only/monotonic, rows read at this later instant are never older
        // than the snapshot, so every member the snapshot reports has its
        // `member.joined` present in the rows — the snapshot and the row-derived
        // signals can never diverge into a just-joined agent that shows up in
        // `agent_ids` yet has no device binding/status in the rows (which would
        // mis-report an active agent as offline). The store is opened, used, and
        // dropped without crossing an `.await`, so this future stays `Send`.
        {
            for scan in &scans {
                let room_id = scan.room_id;
                let room_str = &scan.room_str;
                let Ok(snapshot) = self.snapshot_for(&room_id).await else {
                    continue;
                };
                if Self::require_local_room_access(&snapshot, &self_id).is_err() {
                    continue;
                }
                rooms_total += 1;
                let agent_ids: BTreeSet<IdentityKey> = snapshot
                    .members()
                    .filter(|m| m.role == Role::Agent)
                    .map(|m| m.identity)
                    .collect();
                if agent_ids.is_empty() {
                    continue;
                }
                rooms_covered += 1;
                // Read the display name and rows only AFTER membership access
                // succeeds. Even internal preprocessing must not turn the
                // shared store into a foreign-room discovery path.
                let (room_name, rows) = {
                    let store = self.open_store()?;
                    let room_name = genesis_name(&store, &room_id)
                        .or_else(|| localstate::local_name(&self.data_dir, room_str));
                    let rows = store
                        .room_tail(&room_id, u32::MAX)
                        .map_err(|e| internal("could not read the timeline", e))?;
                    (room_name, rows)
                };
                let rows = &rows;

                // Per-agent signals from the room's real stored events only:
                // device keys (member.joined bindings + authored device_ids),
                // the newest agent_status, and the newest event of any kind.
                let mut signals: BTreeMap<IdentityKey, AgentRoomSignals> = BTreeMap::new();
                for se in rows {
                    let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                        continue;
                    };
                    if let Content::MemberJoined(c) = &ev.content {
                        if agent_ids.contains(&c.device_binding.identity_key) {
                            signals
                                .entry(c.device_binding.identity_key)
                                .or_default()
                                .devices
                                .insert(c.device_binding.device_key);
                        }
                    }
                    if !agent_ids.contains(&ev.sender_id) {
                        continue;
                    }
                    let sig = signals.entry(ev.sender_id).or_default();
                    sig.devices.insert(ev.device_id);
                    sig.last_seen_ts = Some(
                        sig.last_seen_ts
                            .map_or(ev.created_at, |t| t.max(ev.created_at)),
                    );
                    if let Content::AgentStatus(c) = &ev.content {
                        // The tail is causal order; on a ts tie the causally
                        // later status wins.
                        let newer = match &sig.latest {
                            Some(latest) => ev.created_at >= latest.ts,
                            None => true,
                        };
                        if newer {
                            sig.latest = Some(LatestStatus {
                                ts: ev.created_at,
                                label: c.status.clone(),
                                message: c.message.clone(),
                                progress: c.progress_pct,
                            });
                        }
                    }
                }

                // Primary liveness signal: only an OPEN room has live peer
                // state to consult (peers.status source, per the contract).
                let session = self.session_opt(&room_id);
                for identity in &agent_ids {
                    let sig = signals.remove(identity).unwrap_or_default();
                    let connected = session.as_deref().is_some_and(|s| {
                        sig.devices.iter().any(|dev| {
                            endpoint_id_of(*dev).is_ok_and(|id| {
                                s.node.peer_state(id) == Some(PeerConnState::Connected)
                            })
                        })
                    });
                    let liveness = fleet::derive_liveness(
                        connected,
                        sig.latest.as_ref().map(|l| (l.label.as_str(), l.ts)),
                        now,
                    );
                    let agg = agents.entry(identity.to_string()).or_default();
                    agg.rooms
                        .push(json!({ "room_id": room_str, "name": room_name }));
                    agg.per_room_liveness.push(liveness);
                    if let Some(latest) = sig.latest {
                        let newer = match &agg.latest {
                            Some((ts, _)) => latest.ts >= *ts,
                            None => true,
                        };
                        if newer {
                            let view = json!({
                                "label": latest.label,
                                "message": latest.message,
                                "progress": latest.progress,
                                "ts": latest.ts,
                                "room_id": room_str,
                            });
                            agg.latest = Some((latest.ts, view));
                        }
                    }
                    if let Some(seen) = sig.last_seen_ts {
                        agg.last_seen_ts = Some(agg.last_seen_ts.map_or(seen, |t| t.max(seen)));
                    }
                }
            }
        }

        // Aggregate per identity (strongest per-room liveness), then order:
        // liveness rank, last_seen_ts descending (never-seen last), identity.
        let mut rows: Vec<(Liveness, Option<u64>, String, Value)> =
            Vec::with_capacity(agents.len());
        for (identity_id, agg) in agents {
            let liveness = fleet::aggregate_liveness(agg.per_room_liveness.iter().copied());
            let view = json!({
                "identity_id": identity_id,
                "rooms": agg.rooms,
                "liveness": liveness.label(),
                "latest": agg.latest.map(|(_, v)| v),
                "last_seen_ts": agg.last_seen_ts,
            });
            rows.push((liveness, agg.last_seen_ts, identity_id, view));
        }
        rows.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let total = rows.len();
        let active = rows.iter().filter(|r| r.0.is_active()).count();
        let working = rows.iter().filter(|r| r.0 == Liveness::Working).count();
        Ok(json!({
            "active": active,
            "working": working,
            "total": total,
            "rooms_total": rooms_total,
            "rooms_covered": rooms_covered,
            "agents": rows.into_iter().map(|r| r.3).collect::<Vec<Value>>(),
        }))
    }

    /// `agent.history`: one point per real `agent_status` event authored by
    /// `identity_id` in `room_id`, chronological — the newest `limit` events
    /// (default 100). The daemon never interpolates, smooths, or fabricates
    /// intermediate points; an identity with no statuses returns `[]`.
    #[cfg(test)]
    pub(crate) async fn agent_history(
        &self,
        room_id_str: &str,
        identity_hex: &str,
        limit: Option<u32>,
    ) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        self.readable_snapshot(&room_id).await?;
        let identity: IdentityKey = identity_hex.trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid identity_id (expected 64-char hex): {e}"))
        })?;
        let store = self.open_store()?;
        let rows = store
            .room_tail(&room_id, u32::MAX)
            .map_err(|e| internal("could not read the timeline", e))?;
        let mut points = Vec::new();
        for se in &rows {
            if se.event_type != EventType::AgentStatus {
                continue;
            }
            let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
                continue;
            };
            if ev.sender_id != identity {
                continue;
            }
            let Content::AgentStatus(c) = ev.content else {
                continue;
            };
            points.push(json!({
                "ts": ev.created_at,
                "label": c.status,
                "progress": c.progress_pct,
            }));
        }
        // Most-recent-first selection, returned in chronological order.
        let keep =
            usize::try_from(limit.unwrap_or(fleet::HISTORY_DEFAULT_LIMIT)).unwrap_or(usize::MAX);
        if points.len() > keep {
            points.drain(..points.len() - keep);
        }
        Ok(json!({ "points": points }))
    }
}

/// A closed-over, store-free candidate for `agents.fleet`'s async aggregation
/// phase. It deliberately carries neither display name nor timeline rows: both
/// are read only after the local membership guard accepts the room.
#[cfg(test)]
struct RoomScan {
    room_id: RoomId,
    room_str: String,
}

/// One agent's per-room evidence for the fleet read: its known device keys
/// (from `member.joined` bindings and authored events), its newest
/// `agent_status`, and the ts of its newest event of any kind. All fields
/// derive from stored events — nothing is synthesized.
#[cfg(test)]
#[derive(Default)]
struct AgentRoomSignals {
    devices: BTreeSet<DeviceKey>,
    latest: Option<LatestStatus>,
    last_seen_ts: Option<u64>,
}

/// The newest `agent_status` posted by an agent (per room).
#[cfg(test)]
struct LatestStatus {
    ts: u64,
    label: String,
    message: Option<String>,
    progress: Option<u64>,
}

/// One agent's cross-room aggregate for `agents.fleet`.
#[cfg(test)]
#[derive(Default)]
struct FleetAgentAgg {
    rooms: Vec<Value>,
    per_room_liveness: Vec<Liveness>,
    latest: Option<(u64, Value)>,
    last_seen_ts: Option<u64>,
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Render a pipe's authorized-peer set for the view-model: `null` when empty,
/// the single identity when there is one, or every identity comma-joined when a
/// (validated remote) `pipe.opened` declares more than one — never silently
/// dropping the extras.
#[cfg(test)]
fn authorized_peer_value(allowed: &[IdentityKey]) -> Value {
    if allowed.is_empty() {
        Value::Null
    } else {
        Value::String(
            allowed
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

fn parse_room_id(s: &str) -> CoreResult<RoomId> {
    s.trim()
        .parse()
        .map_err(|e| CoreError::invalid(format!("invalid room_id (expected blake3:<hex>): {e}")))
}

/// Parse a `file_<32-hex>` handle (or bare 32-hex) into the 16-byte id.
fn parse_file_id(s: &str) -> CoreResult<[u8; SHORT_ID_LEN]> {
    let trimmed = s.trim();
    let hex_part = trimmed.strip_prefix("file_").unwrap_or(trimmed);
    let bytes =
        hex::decode(hex_part).map_err(|_| CoreError::invalid(format!("invalid file_id {s:?}")))?;
    <[u8; SHORT_ID_LEN]>::try_from(bytes.as_slice())
        .map_err(|_| CoreError::invalid(format!("invalid file_id {s:?} (expected file_<32-hex>)")))
}

/// Parse a 32-hex pipe id into 16 bytes.
fn parse_pipe_id(s: &str) -> CoreResult<[u8; SHORT_ID_LEN]> {
    let bytes =
        hex::decode(s.trim()).map_err(|_| CoreError::invalid(format!("invalid pipe_id {s:?}")))?;
    <[u8; SHORT_ID_LEN]>::try_from(bytes.as_slice())
        .map_err(|_| CoreError::invalid(format!("invalid pipe_id {s:?} (expected 32 hex chars)")))
}

/// Convert a core `DeviceKey` (`device_id`) into an iroh `EndpointId` — the
/// same raw 32 bytes (the CLI's `endpoint_id_of`).
pub(crate) fn endpoint_id_of(dev: DeviceKey) -> CoreResult<EndpointId> {
    EndpointId::from_bytes(dev.as_bytes())
        .map_err(|e| CoreError::internal(format!("invalid device id: {e}")))
}

/// Parse `"<endpoint_id>[@<ip:port>[,<ip:port>...]]"` peer strings.
fn parse_peers(peers: &[String]) -> CoreResult<Vec<EndpointAddr>> {
    peers
        .iter()
        .map(|s| {
            let s = s.trim();
            let (id_part, addr_part) = match s.split_once('@') {
                Some((id, rest)) => (id, Some(rest)),
                None => (s, None),
            };
            let id = EndpointId::from_str(id_part.trim()).map_err(|e| {
                CoreError::invalid(format!("invalid peer endpoint id {id_part:?}: {e}"))
            })?;
            let mut addr = EndpointAddr::new(id);
            if let Some(rest) = addr_part {
                for sock in rest.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let socket = SocketAddr::from_str(sock).map_err(|e| {
                        CoreError::invalid(format!("invalid peer socket address {sock:?}: {e}"))
                    })?;
                    addr = addr.with_ip_addr(socket);
                }
            }
            Ok(addr)
        })
        .collect()
}

/// A dialable `<endpoint_id>@<ip:port,...>` string, or `None` when no socket
/// address is known yet.
#[cfg(test)]
fn dialable_addr(node: &Node) -> Option<String> {
    let addr = node.endpoint_addr().ok()?;
    let socks: Vec<String> = addr.ip_addrs().map(|s| s.to_string()).collect();
    if socks.is_empty() {
        None
    } else {
        Some(format!("{}@{}", addr.id, socks.join(",")))
    }
}

fn validate_room_name(name: &str) -> CoreResult<()> {
    if name.is_empty() {
        return Err(CoreError::invalid("room name must not be empty"));
    }
    if name.len() > MAX_ROOM_NAME_BYTES {
        return Err(CoreError::invalid(format!(
            "room name must be at most {MAX_ROOM_NAME_BYTES} bytes"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(CoreError::invalid(
            "room name must not contain control characters",
        ));
    }
    Ok(())
}

/// The room's genesis `room_name` from the local log, if present.
pub(crate) fn genesis_name(store: &EventStore, room_id: &RoomId) -> Option<String> {
    let genesis = store.by_type(room_id, EventType::RoomCreated).ok()?;
    let stored = genesis.into_iter().next()?;
    let event = SignedEvent::decode(&stored.wire.signed).ok()?;
    match event.content {
        Content::RoomCreated(c) => Some(c.room_name),
        _ => None,
    }
}

/// Log-derived current departure sets (`member.removed` subjects,
/// `member.left` subjects) backing the display-status refinement.
///
/// A later join clears an older terminal fact, and a later leave/removal
/// replaces the other departure kind. Keeping only the canonical current
/// fact prevents an old removal from making a rejoined member look removed
/// forever.
pub(crate) fn departure_sets(
    store: &EventStore,
    room_id: &RoomId,
) -> CoreResult<crate::projection::Departures> {
    let rows = store
        .room_tail(room_id, u32::MAX)
        .map_err(|e| internal("could not read member departure history", e))?;
    Ok(crate::projection::Departures::from_rows(rows.iter()))
}

/// The removal among `subject`'s current causal heads.
///
/// Display order is not causal order: concurrent siblings can sort on either
/// side by event id. A removal remains effective when concurrent with a
/// join/leave (the upstream fold's Removed-dominates rule), and is superseded
/// only when another membership fact causally descends from it.
fn current_member_removal(
    store: &EventStore,
    room_id: &RoomId,
    subject: &IdentityKey,
) -> CoreResult<Option<EventId>> {
    let rows = store
        .room_tail(room_id, u32::MAX)
        .map_err(|e| internal("could not read member history", e))?;
    let mut events = BTreeMap::new();
    let mut touch = BTreeSet::new();
    let mut removals = BTreeSet::new();
    for stored in rows {
        let event = SignedEvent::decode(&stored.wire.signed)
            .map_err(|e| internal("could not decode member history", e))?;
        let touches_subject = match &event.content {
            Content::MemberJoined(join)
                if event.sender_id == *subject && join.device_binding.identity_key == *subject =>
            {
                true
            }
            Content::MemberLeft(left) if left.member_id == *subject => true,
            Content::MemberRemoved(removed) if removed.member_id == *subject => {
                removals.insert(stored.event_id);
                true
            }
            _ => false,
        };
        if touches_subject {
            touch.insert(stored.event_id);
        }
        events.insert(stored.event_id, event);
    }
    Ok(removals.into_iter().find(|removal| {
        !touch
            .iter()
            .any(|other| other != removal && event_is_ancestor(&events, *removal, *other))
    }))
}

/// Whether `ancestor` is in `descendant`'s transitive `prev_events` closure.
fn event_is_ancestor(
    events: &BTreeMap<EventId, SignedEvent>,
    ancestor: EventId,
    descendant: EventId,
) -> bool {
    let mut pending = events
        .get(&descendant)
        .map(|event| event.prev_events.clone())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    while let Some(candidate) = pending.pop() {
        if candidate == ancestor {
            return true;
        }
        if seen.insert(candidate) {
            if let Some(parent) = events.get(&candidate) {
                pending.extend(parent.prev_events.iter().copied());
            }
        }
    }
    false
}

/// `active | invited | removed | left` (the CLI's D5 display refinement: an
/// admin removal dominates a concurrent self-leave).
#[cfg(test)]
fn status_label(
    status: Status,
    subject: &IdentityKey,
    removed_ids: &BTreeSet<IdentityKey>,
    left_ids: &BTreeSet<IdentityKey>,
) -> &'static str {
    match status {
        Status::Active => "active",
        Status::Invited => "invited",
        Status::Removed => {
            if removed_ids.contains(subject) {
                "removed"
            } else if left_ids.contains(subject) {
                "left"
            } else {
                "removed"
            }
        }
    }
}

/// Validate the product policy for voluntary departure.
fn ensure_can_leave(
    snapshot: &MembershipSnapshot,
    self_id: &IdentityKey,
    room_id: &RoomId,
) -> CoreResult<()> {
    if snapshot.admin() == Some(self_id) {
        return Err(CoreError::invalid(
            "room owners cannot leave yet; close the local room session instead",
        ));
    }
    if !snapshot.is_active(self_id) {
        return Err(CoreError::new(
            ErrorKind::NotAMember,
            format!("this identity ({self_id}) is not an active member of room {room_id}"),
        ));
    }
    Ok(())
}

/// Map a `gate_join` rejection onto the protocol taxonomy.
fn join_reject_error(reason: &RejectReason) -> CoreError {
    match reason {
        RejectReason::ExpiredInvite => {
            CoreError::new(ErrorKind::TicketExpired, "this invite has expired")
        }
        RejectReason::BadCapability => CoreError::new(
            ErrorKind::BadTicket,
            "this ticket's secret or identity does not match the invite",
        ),
        RejectReason::InsufficientRole => CoreError::new(
            ErrorKind::BadTicket,
            "the ticket's role does not match the invite",
        ),
        RejectReason::NotAMember | RejectReason::UnboundDevice => CoreError::new(
            ErrorKind::NotAMember,
            format!("the room rejected the join ({})", reason.code()),
        ),
        other => CoreError::internal(format!("the room rejected the join ({})", other.code())),
    }
}

/// Find the `file.shared` matching `file_id`, plus the author's device (the
/// implicit default provider).
fn find_file_shared(
    events: &[StoredEvent],
    file_id: [u8; SHORT_ID_LEN],
) -> Option<(iroh_rooms::files::FileShared, DeviceKey)> {
    for se in events {
        if se.event_type != EventType::FileShared {
            continue;
        }
        let Ok(ev) = SignedEvent::decode(&se.wire.signed) else {
            continue;
        };
        let Content::FileShared(f) = ev.content else {
            continue;
        };
        if f.file_id == file_id {
            return Some((f, ev.device_id));
        }
    }
    None
}

/// The set of pipe ids with a known `pipe.closed` in the room.
#[cfg(test)]
fn closed_pipe_ids(
    store: &EventStore,
    room_id: &RoomId,
) -> CoreResult<BTreeSet<[u8; SHORT_ID_LEN]>> {
    let mut closed = BTreeSet::new();
    for se in store
        .by_type(room_id, EventType::PipeClosed)
        .map_err(|e| internal("could not read pipe.closed events", e))?
    {
        if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
            if let Content::PipeClosed(c) = ev.content {
                closed.insert(c.pipe_id);
            }
        }
    }
    Ok(closed)
}

/// The governing `pipe.opened` for `pipe_id`, if present in the local log.
fn open_pipe(
    store: &EventStore,
    room_id: &RoomId,
    pipe_id: [u8; SHORT_ID_LEN],
) -> CoreResult<Option<iroh_rooms::pipes::PipeOpened>> {
    for se in store
        .by_type(room_id, EventType::PipeOpened)
        .map_err(|e| internal("could not read pipe.opened events", e))?
    {
        if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
            if let Content::PipeOpened(p) = ev.content {
                if p.pipe_id == pipe_id {
                    return Ok(Some(p));
                }
            }
        }
    }
    Ok(None)
}

/// Parse an expiry spec (`<int>{s|m|h|d}`, bare integer = seconds) into an
/// absolute ms timestamp anchored at `now`.
#[cfg(test)]
fn parse_expiry(spec: &str, now: u64) -> CoreResult<u64> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(CoreError::invalid("expiry must not be empty"));
    }
    let (digits, unit_ms): (&str, u64) = match spec.chars().last() {
        Some('s') => (&spec[..spec.len() - 1], 1_000),
        Some('m') => (&spec[..spec.len() - 1], 60_000),
        Some('h') => (&spec[..spec.len() - 1], 3_600_000),
        Some('d') => (&spec[..spec.len() - 1], 86_400_000),
        _ => (spec, 1_000),
    };
    let value: u64 = digits.trim().parse().map_err(|_| {
        CoreError::invalid(format!(
            "expiry must be <int>{{s|m|h|d}} (e.g. \"24h\"); got {spec:?}"
        ))
    })?;
    if value == 0 {
        return Err(CoreError::invalid("expiry must be greater than zero"));
    }
    value
        .checked_mul(unit_ms)
        .and_then(|ms| now.checked_add(ms))
        .ok_or_else(|| CoreError::invalid(format!("expiry {spec:?} is too large")))
}

/// Reduce a peer-supplied file name to a safe basename (path-traversal guard).
fn sanitize_name(name: &str, file_id: [u8; SHORT_ID_LEN]) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        format!("file_{}", hex::encode(file_id))
    } else {
        cleaned.to_owned()
    }
}

/// Write bytes atomically (temp + rename); no partial file is ever visible.
fn save_atomic(target: &Path, bytes: &[u8]) -> CoreResult<()> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    let tmp = dir.join(format!(".{file_name}.part"));
    let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, target));
    if let Err(err) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(internal("could not save the fetched file", err));
    }
    Ok(())
}

/// A dependency-free MIME guess from the extension (mirrors the CLI's table).
fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("txt" | "text" | "log") => "text/plain",
        Some("md" | "markdown") => "text/markdown",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("html" | "htm") => "text/html",
        Some("csv") => "text/csv",
        Some("xml") => "application/xml",
        Some("zip") => "application/zip",
        Some("gz" | "tgz") => "application/gzip",
        Some("tar") => "application/x-tar",
        _ => "application/octet-stream",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        bare_event_hex, collect_committed, parse_expiry, parse_file_id, parse_pipe_id,
        sanitize_name, validate_room_name, Content, EventType, RemoveMemberOutcome, RoomSupervisor,
    };
    use crate::error::ErrorKind;
    use iroh_rooms::events::{
        build_message_text, validate_wire_bytes, SignedEvent, ValidationContext, WireEvent,
    };
    use iroh_rooms::experimental::store::EventStore;
    use iroh_rooms::identity::{DeviceBinding, SigningKey};
    use iroh_rooms::room::{
        build_member_left, build_member_removed, build_room_created, derive_room_id, RoomId,
        RoomInviteTicket, Status,
    };
    use std::collections::BTreeSet;
    use tempfile::tempdir;

    /// Persist an event authored elsewhere directly into the supervisor's
    /// store (validating first) — the way a synced remote event lands.
    fn insert_wire(sup: &RoomSupervisor, room_id: &RoomId, wire: &WireEvent) {
        let validated =
            validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(*room_id))
                .expect("authored event validates");
        let mut store = sup.open_store().unwrap();
        store.insert(&validated).unwrap();
    }

    /// A self-contained room: identity/device keys, the room id, and the
    /// validated genesis — enough to author further events against it.
    struct RoomFixture {
        identity: SigningKey,
        device: SigningKey,
        room_id: RoomId,
        genesis_id: iroh_rooms::events::EventId,
        store: EventStore,
        snapshot: iroh_rooms::room::MembershipSnapshot,
    }

    fn room_fixture() -> RoomFixture {
        const TS: u64 = 1_783_190_000_000;
        let identity = SigningKey::generate();
        let device = SigningKey::generate();
        let nonce = [0x42u8; 16];
        let room_id = derive_room_id(&identity.identity_key(), &nonce, TS);
        let genesis = build_room_created(&identity, &device, "rank room", &nonce, TS);
        let ctx = ValidationContext::for_room(room_id);
        let genesis = validate_wire_bytes(&genesis.to_bytes(), &ctx).unwrap();
        let genesis_id = genesis.event_id;
        let mut store = EventStore::open_in_memory().unwrap();
        store.insert(&genesis).unwrap();
        let snapshot =
            iroh_rooms::room::RoomMembership::from_events(room_id, vec![genesis]).snapshot();
        RoomFixture {
            identity,
            device,
            room_id,
            genesis_id,
            store,
            snapshot,
        }
    }

    impl RoomFixture {
        fn message(&self, body: &str, device: &SigningKey, at: u64) -> WireEvent {
            build_message_text(
                &self.identity,
                device,
                &self.room_id,
                body,
                None,
                None,
                &[],
                &[self.genesis_id],
                at,
            )
        }

        fn own_message(&self, body: &str, at: u64) -> WireEvent {
            build_message_text(
                &self.identity,
                &self.device,
                &self.room_id,
                body,
                None,
                None,
                &[],
                &[self.genesis_id],
                at,
            )
        }

        fn insert(&mut self, wire: &WireEvent) {
            let ctx = ValidationContext::for_room(self.room_id);
            let validated = validate_wire_bytes(&wire.to_bytes(), &ctx).unwrap();
            self.store.insert(&validated).unwrap();
        }

        fn tail(&self) -> Vec<iroh_rooms::experimental::store::StoredEvent> {
            self.store.room_tail(&self.room_id, u32::MAX).unwrap()
        }
    }

    #[test]
    fn collect_committed_serves_dense_in_order_positions() {
        let mut fx = room_fixture();
        let d2 = SigningKey::generate();
        let m1 = fx.own_message("m1", 1_783_190_001_000);
        fx.insert(&m1);
        fx.insert(&fx.message("m2", &d2, 1_783_190_002_000));
        let tail = fx.tail();
        let mut seen = BTreeSet::new();
        let mut next = 0;
        let out = collect_committed(&tail, &fx.snapshot, &mut seen, &mut next);
        let positions: Vec<u64> = out.iter().map(|c| c.event.pos).collect();
        assert_eq!(positions, [0, 1, 2], "dense ranks over the committed tail");
        assert!(
            out.iter().all(|c| c.reordered_at.is_none()),
            "all in-order appends"
        );
        assert_eq!(next, 3, "the high-water mark advanced past all three");
    }

    /// Validate one authored message and return its canonical `EventId`.
    fn validated_id(room_id: &RoomId, wire: &WireEvent) -> iroh_rooms::events::EventId {
        validate_wire_bytes(&wire.to_bytes(), &ValidationContext::for_room(*room_id))
            .expect("authored event validates")
            .event_id
    }

    /// Author two siblings of the genesis (same parent, so the same lamport)
    /// and return them ordered by their canonical `EventId`. The ordering is
    /// deterministic — no random search: the two events are generated first,
    /// then the caller designates the lower or higher one for its scenario.
    fn two_sorted_siblings(fx: &RoomFixture) -> (WireEvent, WireEvent) {
        let a = fx.message("sibling a", &SigningKey::generate(), 1_783_190_001_500);
        let b = fx.message("sibling b", &SigningKey::generate(), 1_783_190_001_500);
        if validated_id(&fx.room_id, &a) <= validated_id(&fx.room_id, &b) {
            (a, b)
        } else {
            (b, a)
        }
    }

    #[test]
    fn collect_committed_marks_a_late_sibling_as_a_reorder() {
        let mut fx = room_fixture();
        // Two siblings: `lower` sorts before `higher` in the canonical order.
        // Serve the HIGHER one first (as m1 at position 1), then commit the
        // LOWER one late — it interleaves below the served tip.
        let (lower, higher) = two_sorted_siblings(&fx);
        fx.insert(&higher);
        let tail = fx.tail();
        let mut seen = BTreeSet::new();
        let mut next = 0;
        let first = collect_committed(&tail, &fx.snapshot, &mut seen, &mut next);
        assert_eq!(first.len(), 2, "genesis + the served sibling");
        assert_eq!(next, 2);

        fx.insert(&lower);
        let tail2 = fx.tail();
        let out2 = collect_committed(&tail2, &fx.snapshot, &mut seen, &mut next);
        assert_eq!(out2.len(), 1, "exactly one new event");
        assert_eq!(
            out2[0].reordered_at,
            Some(out2[0].event.pos),
            "a sibling interleaving below the served tip is a reorder"
        );
        assert_eq!(
            out2[0].event.pos, 1,
            "it took the served sibling's old rank, shifting it up"
        );
        assert_eq!(next, 1, "the mark rewound so the shifted suffix re-serves");
    }

    #[test]
    fn collect_committed_appends_a_tip_sibling_without_reorder() {
        let mut fx = room_fixture();
        // Serve the LOWER sibling first (position 1), then commit the HIGHER
        // one: an ordinary in-order append at the tip, no reorder, no gap.
        let (lower, higher) = two_sorted_siblings(&fx);
        fx.insert(&lower);
        let tail = fx.tail();
        let mut seen = BTreeSet::new();
        let mut next = 0;
        collect_committed(&tail, &fx.snapshot, &mut seen, &mut next);
        assert_eq!(next, 2);

        fx.insert(&higher);
        let tail2 = fx.tail();
        let out2 = collect_committed(&tail2, &fx.snapshot, &mut seen, &mut next);
        assert_eq!(out2.len(), 1);
        assert_eq!(out2[0].reordered_at, None, "a tip append is not a reorder");
        assert_eq!(out2[0].event.pos, 2);
        assert_eq!(next, 3);
    }

    /// Join `agent` keys into the room offline: mint a real invite through
    /// the supervisor, then author + persist the `member.joined` with the
    /// ticket's capability — exactly the event a remote agent runner syncs.
    async fn seed_agent_member(
        sup: &RoomSupervisor,
        room_id_str: &str,
        agent_identity: &SigningKey,
        agent_device: &SigningKey,
    ) {
        let ticket_str = sup
            .create_invite(
                room_id_str,
                &agent_identity.identity_key().to_string(),
                "agent",
                None,
            )
            .await
            .unwrap();
        let ticket: RoomInviteTicket = ticket_str.parse().unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();
        let mut heads = sup.open_store().unwrap().heads(&room_id).unwrap();
        heads.truncate(super::MAX_PREV_EVENTS);
        let binding = DeviceBinding::create(&room_id, agent_identity, agent_device.device_key());
        let wire = super::build_member_joined(
            agent_identity,
            agent_device,
            &room_id,
            &ticket.invite_id,
            &ticket.capability_secret,
            "agent",
            binding,
            Some("fleet-agent"),
            &heads,
            crate::now_ms(),
        );
        insert_wire(sup, &room_id, &wire);
    }

    /// Persist an `agent_status` authored by the given keys at `ts`.
    fn seed_status(
        sup: &RoomSupervisor,
        room_id_str: &str,
        identity: &SigningKey,
        device: &SigningKey,
        label: &str,
        progress: Option<u64>,
        ts: u64,
    ) {
        let room_id: RoomId = room_id_str.parse().unwrap();
        let mut heads = sup.open_store().unwrap().heads(&room_id).unwrap();
        heads.truncate(super::MAX_PREV_EVENTS);
        let wire = super::build_agent_status(
            identity,
            device,
            &room_id,
            label,
            Some("status message"),
            &[],
            progress,
            &heads,
            ts,
        );
        insert_wire(sup, &room_id, &wire);
    }

    /// Persist a `message.text` authored by the given keys at `ts`.
    fn seed_message(
        sup: &RoomSupervisor,
        room_id_str: &str,
        identity: &SigningKey,
        device: &SigningKey,
        body: &str,
        ts: u64,
    ) {
        let room_id: RoomId = room_id_str.parse().unwrap();
        let mut heads = sup.open_store().unwrap().heads(&room_id).unwrap();
        heads.truncate(super::MAX_PREV_EVENTS);
        let wire = super::build_message_text(
            identity,
            device,
            &room_id,
            body,
            None,
            None,
            &[],
            &heads,
            ts,
        );
        insert_wire(sup, &room_id, &wire);
    }

    async fn wait_member_status(
        sup: &RoomSupervisor,
        room_id: &str,
        identity_id: &str,
        status: &str,
    ) -> serde_json::Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let members = sup.members(room_id).await.unwrap();
            if let Some(member) = members
                .iter()
                .find(|m| m["identity_id"].as_str() == Some(identity_id))
            {
                if member["status"] == status {
                    return member.clone();
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for member {identity_id} to be {status}; last members: {members:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Poll `agents.fleet` until `pred` holds (or fail after a deadline).
    async fn wait_fleet(
        sup: &RoomSupervisor,
        what: &str,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let fleet = sup.agents_fleet().await.unwrap();
            if pred(&fleet) {
                return fleet;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}; last fleet: {fleet}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    #[tokio::test]
    async fn fleet_is_empty_and_honest_on_a_fresh_daemon() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();

        // No rooms at all: every count is a real zero, never a guess.
        let fleet = sup.agents_fleet().await.unwrap();
        assert_eq!(fleet["total"], 0);
        assert_eq!(fleet["active"], 0);
        assert_eq!(fleet["working"], 0);
        assert_eq!(fleet["rooms_total"], 0);
        assert_eq!(fleet["rooms_covered"], 0);
        assert_eq!(fleet["agents"].as_array().unwrap().len(), 0);

        // A room with no agent-role member counts toward rooms_total only.
        sup.create_room("No Agents Here").unwrap();
        let fleet = sup.agents_fleet().await.unwrap();
        assert_eq!(fleet["rooms_total"], 1);
        assert_eq!(fleet["rooms_covered"], 0);
        assert_eq!(fleet["total"], 0);
    }

    #[tokio::test]
    async fn fleet_reports_stale_never_working_without_a_connected_peer() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Fleet Room").unwrap();
        let agent_identity = SigningKey::generate();
        let agent_device = SigningKey::generate();
        seed_agent_member(&sup, &room_id, &agent_identity, &agent_device).await;
        let agent_hex = agent_identity.identity_key().to_string();

        // Agent member, no status, room not open: offline — with the real
        // member.joined ts as last_seen (an event timestamp, never "now").
        let fleet = sup.agents_fleet().await.unwrap();
        assert_eq!(fleet["total"], 1);
        assert_eq!(fleet["rooms_total"], 1);
        assert_eq!(fleet["rooms_covered"], 1);
        let agent = &fleet["agents"][0];
        assert_eq!(agent["identity_id"], agent_hex);
        assert_eq!(agent["liveness"], "offline");
        assert!(agent["latest"].is_null());
        assert!(agent["last_seen_ts"].is_u64());
        assert_eq!(agent["rooms"][0]["room_id"], room_id);
        assert_eq!(agent["rooms"][0]["name"], "Fleet Room");

        // THE RULE at the RPC level: a fresh "working" status with no
        // connected peer reads stale — never working, never active.
        let t1 = crate::now_ms();
        seed_status(
            &sup,
            &room_id,
            &agent_identity,
            &agent_device,
            "working",
            Some(40),
            t1,
        );
        let fleet = sup.agents_fleet().await.unwrap();
        let agent = &fleet["agents"][0];
        assert_eq!(agent["liveness"], "stale");
        assert_eq!(fleet["active"], 0);
        assert_eq!(fleet["working"], 0);
        assert_eq!(agent["latest"]["label"], "working");
        assert_eq!(agent["latest"]["progress"], 40);
        assert_eq!(agent["latest"]["ts"], t1);
        assert_eq!(agent["latest"]["room_id"], room_id);
        assert_eq!(agent["last_seen_ts"], t1);

        // An idle-class latest with no peer reads offline.
        seed_status(
            &sup,
            &room_id,
            &agent_identity,
            &agent_device,
            "idle",
            None,
            t1 + 1,
        );
        let fleet = sup.agents_fleet().await.unwrap();
        assert_eq!(fleet["agents"][0]["liveness"], "offline");
        assert_eq!(fleet["agents"][0]["latest"]["label"], "idle");

        // agent.history: one point per real event, chronological; `limit`
        // selects the newest; progress is the event's value or null.
        let history = sup.agent_history(&room_id, &agent_hex, None).await.unwrap();
        let points = history["points"].as_array().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["label"], "working");
        assert_eq!(points[0]["progress"], 40);
        assert_eq!(points[0]["ts"], t1);
        assert_eq!(points[1]["label"], "idle");
        assert!(points[1]["progress"].is_null());
        let limited = sup
            .agent_history(&room_id, &agent_hex, Some(1))
            .await
            .unwrap();
        assert_eq!(limited["points"].as_array().unwrap().len(), 1);
        assert_eq!(limited["points"][0]["label"], "idle");

        // A member with no statuses returns an empty (not fabricated) series.
        let owner_hex = crate::identity::load_profile(dir.path())
            .unwrap()
            .unwrap()
            .identity_id;
        let empty = sup.agent_history(&room_id, &owner_hex, None).await.unwrap();
        assert_eq!(empty["points"].as_array().unwrap().len(), 0);

        // Error taxonomy: unknown room, malformed identity.
        let unknown = format!("blake3:{}", "ee".repeat(32));
        let err = sup
            .agent_history(&unknown, &agent_hex, None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup
            .agent_history(&room_id, "not-hex", None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fleet_liveness_tracks_live_peer_state_loopback() {
        // Two supervisors (two data dirs, two identities) on the loopback
        // transport: liveness must come from the REAL peer connection, and a
        // "working" claim must decay to stale the moment the peer is gone.
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("Fleet Live").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"]
            .as_str()
            .expect("loopback session has a dialable addr")
            .to_owned();

        let agent_dir = tempdir().unwrap();
        let agent_profile = crate::identity::create(agent_dir.path()).unwrap();
        let agent = RoomSupervisor::new(agent_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &agent_profile.identity_id, "agent", None)
            .await
            .unwrap();
        agent
            .join_room(&ticket, None, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        agent.open_room(&room_id, &[owner_addr]).await.unwrap();

        // Connected agent, no working-class claim: online-idle.
        let fleet = wait_fleet(&owner, "online-idle", |f| {
            f["agents"][0]["liveness"] == "online-idle"
        })
        .await;
        assert_eq!(fleet["total"], 1);
        assert_eq!(fleet["active"], 1);
        assert_eq!(fleet["working"], 0);

        // A fresh working status from a connected peer: working.
        agent
            .post_status(&room_id, "working", Some("crunching"), Some(40), &[])
            .await
            .unwrap();
        let fleet = wait_fleet(&owner, "working", |f| {
            f["agents"][0]["liveness"] == "working"
        })
        .await;
        assert_eq!(fleet["working"], 1);
        assert_eq!(fleet["active"], 1);
        assert_eq!(fleet["agents"][0]["latest"]["label"], "working");
        assert_eq!(fleet["agents"][0]["latest"]["progress"], 40);

        // The agent daemon vanishes without posting anything: its last claim
        // is "working" but the peer is gone -> stale, never working.
        agent.close_room(&room_id).await.unwrap();
        let fleet = wait_fleet(&owner, "stale after disconnect", |f| {
            f["agents"][0]["liveness"] == "stale"
        })
        .await;
        assert_eq!(fleet["working"], 0);
        assert_eq!(fleet["active"], 0);

        owner.close_room(&room_id).await.unwrap();
    }

    /// (identity, role-label) pairs from a snapshot, in the snapshot's
    /// deterministic member order — the "members/roles" projection the fix
    /// must preserve exactly.
    fn members_roles(snapshot: &super::MembershipSnapshot) -> Vec<(String, &'static str)> {
        snapshot
            .members()
            .map(|m| (m.identity.to_string(), super::role_label(m.role)))
            .collect()
    }

    /// CORRECTNESS: `snapshot_for` — both the closed cache path and the live
    /// open-session path — yields byte-identical membership to a direct
    /// `fold()` over a log with membership events INTERLEAVED with many
    /// message/agent_status events; and an open room never serves a stale
    /// cache after a new member appears.
    #[tokio::test(flavor = "multi_thread")]
    async fn snapshot_for_matches_fold_over_interleaved_history() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Interleave").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();

        // Interleave membership (invite+joined per agent) with dozens of
        // message.text / agent_status events authored by the members, so the
        // non-membership events sit between membership events as prev_events
        // ancestors (the exact shape the fold must fold through).
        let mut ts = crate::now_ms();
        let mut agents = Vec::new();
        for a in 0..3 {
            let identity = SigningKey::generate();
            let device = SigningKey::generate();
            seed_agent_member(&sup, &room_id_str, &identity, &device).await;
            for i in 0..20 {
                ts += 1;
                seed_status(
                    &sup,
                    &room_id_str,
                    &identity,
                    &device,
                    "working",
                    Some(i),
                    ts,
                );
                ts += 1;
                seed_message(
                    &sup,
                    &room_id_str,
                    &identity,
                    &device,
                    &format!("m{a}-{i}"),
                    ts,
                );
            }
            agents.push((identity, device));
        }

        // Oracle: a direct fold over the whole persisted log.
        let fold_snapshot = {
            let store = sup.open_store().unwrap();
            sup.fold(&store, &room_id).unwrap().1
        };
        assert!(fold_snapshot.members().count() >= 4); // owner + 3 agents

        // Closed-room path: first call MISSES the cache and folds once; the
        // second HITS the cache (same event count). Both equal the oracle.
        assert!(!sup.is_open(&room_id));
        let closed_miss = sup.snapshot_for(&room_id).await.unwrap();
        let closed_hit = sup.snapshot_for(&room_id).await.unwrap();
        assert_eq!(fold_snapshot, closed_miss, "closed miss != fold");
        assert_eq!(fold_snapshot, closed_hit, "closed hit != fold");
        assert_eq!(members_roles(&fold_snapshot), members_roles(&closed_hit));

        // Open-room path: the live engine's incremental fold must match the
        // store fold byte-for-byte — and it must NOT be served from the
        // closed-room cache populated above.
        sup.open_room(&room_id_str, &[]).await.unwrap();
        assert!(sup.is_open(&room_id));
        let open_snapshot = sup.snapshot_for(&room_id).await.unwrap();
        assert_eq!(fold_snapshot, open_snapshot, "open live snapshot != fold");
        assert_eq!(members_roles(&fold_snapshot), members_roles(&open_snapshot));

        // A NEW member appears while the room is OPEN: snapshot_for must
        // reflect it immediately (open rooms never read the cache, so the
        // count cannot be stale). A fresh invite adds one Invited member.
        let before = open_snapshot.members().count();
        let newcomer = SigningKey::generate();
        sup.create_invite(
            &room_id_str,
            &newcomer.identity_key().to_string(),
            "agent",
            None,
        )
        .await
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let grown = loop {
            let snap = sup.snapshot_for(&room_id).await.unwrap();
            if snap.members().count() == before + 1 {
                break snap;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "open snapshot_for never reflected the new member (stale cache?)"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        // The live snapshot still equals a fresh store fold (the invite was
        // persisted through the engine), proving equality holds as it grows.
        let fold_after = {
            let store = sup.open_store().unwrap();
            sup.fold(&store, &room_id).unwrap().1
        };
        assert_eq!(
            fold_after, grown,
            "grown open snapshot != fold after invite"
        );

        sup.close_room(&room_id_str).await.unwrap();
    }

    #[tokio::test]
    async fn invite_after_member_content_does_not_depend_on_chat_heads() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Late Join").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();

        let agent_identity = SigningKey::generate();
        let agent_device = SigningKey::generate();
        seed_agent_member(&sup, &room_id_str, &agent_identity, &agent_device).await;
        seed_message(
            &sup,
            &room_id_str,
            &agent_identity,
            &agent_device,
            "non-admin chat head",
            crate::now_ms(),
        );

        let message_id = {
            let store = sup.open_store().unwrap();
            store
                .by_type(&room_id, EventType::MessageText)
                .unwrap()
                .last()
                .expect("seeded message exists")
                .event_id
        };
        let newcomer = SigningKey::generate();
        let ticket_str = sup
            .create_invite(
                &room_id_str,
                &newcomer.identity_key().to_string(),
                "member",
                None,
            )
            .await
            .unwrap();
        let ticket: RoomInviteTicket = ticket_str.parse().unwrap();

        let invite = {
            let store = sup.open_store().unwrap();
            store
                .by_type(&room_id, EventType::MemberInvited)
                .unwrap()
                .into_iter()
                .find(|stored| {
                    let ev = validate_wire_bytes(
                        &stored.wire.to_bytes(),
                        &ValidationContext::for_room(room_id),
                    )
                    .unwrap();
                    matches!(
                        ev.event.content,
                        Content::MemberInvited(ref invite) if invite.invite_id == ticket.invite_id
                    )
                })
                .expect("new invite event was persisted")
        };
        let invite = validate_wire_bytes(
            &invite.wire.to_bytes(),
            &ValidationContext::for_room(room_id),
        )
        .unwrap();

        assert!(
            !invite.event.prev_events.contains(&message_id),
            "member.invited must keep the membership sub-DAG closed; prev_events contained a chat head"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn late_join_after_agent_message_loopback() {
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("Late Join Live").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let agent_dir = tempdir().unwrap();
        let agent_profile = crate::identity::create(agent_dir.path()).unwrap();
        let agent = RoomSupervisor::new(agent_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &agent_profile.identity_id, "agent", None)
            .await
            .unwrap();
        agent
            .join_room(&ticket, Some("agent"), std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        agent
            .open_room(&room_id, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        wait_member_status(&owner, &room_id, &agent_profile.identity_id, "active").await;

        agent
            .send_message(&room_id, "agent says hello")
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let timeline = owner.timeline(&room_id, None).await.unwrap();
            if timeline
                .iter()
                .any(|event| event["body"].as_str() == Some("agent says hello"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owner never synced the agent message; timeline: {timeline:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let late_dir = tempdir().unwrap();
        let late_profile = crate::identity::create(late_dir.path()).unwrap();
        let late = RoomSupervisor::new(late_dir.path().to_path_buf(), true).unwrap();
        let late_ticket = owner
            .create_invite(&room_id, &late_profile.identity_id, "member", None)
            .await
            .unwrap();
        late.join_room(
            &late_ticket,
            Some("late member"),
            std::slice::from_ref(&owner_addr),
        )
        .await
        .unwrap();
        wait_member_status(&owner, &room_id, &late_profile.identity_id, "active").await;

        late.close_room(&room_id).await.ok();
        agent.close_room(&room_id).await.unwrap();
        owner.close_room(&room_id).await.unwrap();
    }

    /// PERF: the O(full-history)-per-call re-fold is gone. With ~1000
    /// agent_status events in one room, warm `room.list` / `agents.fleet`
    /// calls must be fast (the old fold was ~25s at ~2000 events). Ignored by
    /// default — it authors 1000 events; run with `--ignored`.
    #[tokio::test]
    #[ignore = "perf: authors ~1000 events; run explicitly with --ignored"]
    async fn hot_reads_are_fast_on_a_room_with_real_history() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Busy").unwrap();

        let identity = SigningKey::generate();
        let device = SigningKey::generate();
        seed_agent_member(&sup, &room_id_str, &identity, &device).await;
        let mut ts = crate::now_ms();
        for i in 0..1000 {
            ts += 1;
            seed_status(
                &sup,
                &room_id_str,
                &identity,
                &device,
                "working",
                Some(i % 101),
                ts,
            );
        }

        // Warm the closed-room fold cache once (this call pays the single fold).
        sup.list_rooms().await.unwrap();
        sup.agents_fleet().await.unwrap();

        // Warm calls must be well under the old ~25s (and under the 4s poll):
        // list_rooms is a count() + cache hit; agents_fleet is a linear
        // row-decode + cache hit — no superlinear re-fold.
        let t0 = std::time::Instant::now();
        for _ in 0..5 {
            sup.list_rooms().await.unwrap();
        }
        let list_avg = t0.elapsed() / 5;
        let t1 = std::time::Instant::now();
        for _ in 0..5 {
            sup.agents_fleet().await.unwrap();
        }
        let fleet_avg = t1.elapsed() / 5;

        assert!(
            list_avg < std::time::Duration::from_millis(300),
            "warm room.list too slow: {list_avg:?}"
        );
        assert!(
            fleet_avg < std::time::Duration::from_millis(300),
            "warm agents.fleet too slow: {fleet_avg:?}"
        );
    }

    #[test]
    fn room_name_bounds() {
        assert!(validate_room_name("Build Iroh Rooms MVP").is_ok());
        assert!(validate_room_name("").is_err());
        assert!(validate_room_name(&"a".repeat(129)).is_err());
        assert!(validate_room_name("bad\nname").is_err());
    }

    #[test]
    fn file_and_pipe_id_codecs() {
        let id = [0xabu8; 16];
        assert_eq!(
            parse_file_id(&format!("file_{}", "ab".repeat(16))).unwrap(),
            id
        );
        assert_eq!(parse_file_id(&"ab".repeat(16)).unwrap(), id);
        assert!(parse_file_id("file_xyz").is_err());
        assert_eq!(parse_pipe_id(&"ab".repeat(16)).unwrap(), id);
        assert!(parse_pipe_id("short").is_err());
    }

    #[test]
    fn expiry_parses_units_and_bare_seconds() {
        assert_eq!(parse_expiry("24h", 0).unwrap(), 24 * 3_600_000);
        assert_eq!(parse_expiry("30", 1_000).unwrap(), 31_000);
        assert!(parse_expiry("0s", 0).is_err());
        assert!(parse_expiry("nope", 0).is_err());
    }

    #[test]
    fn sanitize_name_guards_traversal() {
        assert_eq!(sanitize_name("report.pdf", [0; 16]), "report.pdf");
        assert_eq!(
            sanitize_name("../../.ssh/authorized_keys", [0; 16]),
            "authorized_keys"
        );
        assert_eq!(
            sanitize_name("..", [0xaa; 16]),
            format!("file_{}", "aa".repeat(16))
        );
    }

    #[test]
    fn create_room_requires_identity() {
        let dir = tempdir().unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let err = sup.create_room("Room").unwrap_err();
        assert_eq!(err.kind, ErrorKind::IdentityMissing);
    }

    #[test]
    fn create_room_requires_durable_provenance_before_genesis() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let state_path = dir.path().join(crate::localstate::STATE_FILE);
        std::fs::create_dir(&state_path).unwrap();

        let err = sup.create_room("No Partial Room").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);

        let store = sup.open_store().unwrap();
        assert!(
            store.room_ids().unwrap().is_empty(),
            "a failed provenance write must happen before the genesis becomes durable"
        );
    }

    #[test]
    fn room_device_derivation_is_deterministic_and_room_scoped() {
        let device = SigningKey::generate();
        let other_device = SigningKey::generate();
        let room_a: RoomId = format!("blake3:{:064x}", 0xaau32).parse().unwrap();
        let room_b: RoomId = format!("blake3:{:064x}", 0xbbu32).parse().unwrap();

        // Deterministic: the same (device seed, room id) always reproduces the
        // same key. This is what lets the key be derived instead of stored.
        assert_eq!(
            super::derive_room_device(&device, &room_a).device_key(),
            super::derive_room_device(&device, &room_a).device_key(),
        );
        // Room-scoped: distinct rooms get distinct devices, which is the whole
        // point — distinct devices mean distinct EndpointIds.
        assert_ne!(
            super::derive_room_device(&device, &room_a).device_key(),
            super::derive_room_device(&device, &room_b).device_key(),
        );
        // Identity-scoped: two identities never collide in the same room.
        assert_ne!(
            super::derive_room_device(&device, &room_a).device_key(),
            super::derive_room_device(&other_device, &room_a).device_key(),
        );
        // And never the global device itself.
        assert_ne!(
            super::derive_room_device(&device, &room_a).device_key(),
            device.device_key(),
        );
    }

    #[test]
    fn created_rooms_bind_derived_devices_with_distinct_endpoint_ids() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let secret = sup.secrets().unwrap();
        let self_id = secret.identity.identity_key();

        let room_a: RoomId = sup.create_room("Room A").unwrap().parse().unwrap();
        let room_b: RoomId = sup.create_room("Room B").unwrap().parse().unwrap();

        let store = sup.open_store().unwrap();
        let bound = |room: &RoomId| {
            sup.fold(&store, room)
                .unwrap()
                .1
                .member(&self_id)
                .and_then(|m| m.device)
                .expect("the creator is device-bound by the genesis")
        };

        // The genesis binds the DERIVED device, not the global one.
        assert_eq!(
            bound(&room_a),
            super::derive_room_device(&secret.device, &room_a).device_key(),
        );
        assert_ne!(bound(&room_a), secret.device.device_key());

        // Two rooms of the same identity present different EndpointIds, so both
        // can be live at once instead of collapsing onto one endpoint.
        assert_ne!(
            super::endpoint_id_of(bound(&room_a)).unwrap(),
            super::endpoint_id_of(bound(&room_b)).unwrap(),
        );
    }

    #[test]
    fn authoring_device_follows_the_log_not_local_state() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let secret = sup.secrets().unwrap();

        // A room this build created: the log binds the derived device.
        let derived_room: RoomId = sup.create_room("Derived").unwrap().parse().unwrap();

        // A LEGACY room, exactly as a pre-derivation daemon wrote it: genesis
        // signed by the one global device.
        let mut nonce = [0u8; super::ROOM_NONCE_LEN];
        nonce[0] = 7;
        let created_at = crate::now_ms();
        let legacy_room =
            super::derive_room_id(&secret.identity.identity_key(), &nonce, created_at);
        let wire = super::build_room_created(
            &secret.identity,
            &secret.device,
            "Legacy",
            &nonce,
            created_at,
        );
        crate::localstate::remember_room(dir.path(), &legacy_room.to_string(), Some("Legacy"))
            .unwrap();
        insert_wire(&sup, &legacy_room, &wire);

        let store = sup.open_store().unwrap();
        let resolve = |room: &RoomId| {
            let snapshot = sup.fold(&store, room).unwrap().1;
            sup.authoring_device_key(&snapshot, &secret, room)
                .device_key()
        };

        // Legacy room: keep signing with the global device the log already
        // binds, or every event would be rejected as `unbound_device`.
        assert_eq!(resolve(&legacy_room), secret.device.device_key());
        // Derived room: use the derived device.
        assert_eq!(
            resolve(&derived_room),
            super::derive_room_device(&secret.device, &derived_room).device_key(),
        );
    }

    #[test]
    fn authoring_device_survives_losing_local_state() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room: RoomId = sup.create_room("Durable").unwrap().parse().unwrap();

        let expected = {
            let secret = sup.secrets().unwrap();
            let store = sup.open_store().unwrap();
            let snapshot = sup.fold(&store, &room).unwrap().1;
            sup.authoring_device_key(&snapshot, &secret, &room)
                .device_key()
        };
        drop(sup);

        // Nothing about the room device lives in state.json, so deleting it —
        // as a rollback to an older daemon or a wiped profile would — must not
        // change which device this identity authors with. A stored seed would
        // be gone here, and every later event would be rejected.
        std::fs::remove_file(dir.path().join(crate::localstate::STATE_FILE)).unwrap();

        let reopened = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let secret = reopened.secrets().unwrap();
        let store = reopened.open_store().unwrap();
        let snapshot = reopened.fold(&store, &room).unwrap().1;
        assert_eq!(
            reopened
                .authoring_device_key(&snapshot, &secret, &room)
                .device_key(),
            expected,
        );
    }

    /// Issue #151: a profile in two rooms must receive live events in BOTH
    /// while both stay open, instead of only the last-opened one.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_open_rooms_receive_concurrently_loopback() {
        // One user, two rooms, a different remote peer publishing into each.
        let user_dir = tempdir().unwrap();
        crate::identity::create(user_dir.path()).unwrap();
        let user = RoomSupervisor::new(user_dir.path().to_path_buf(), true).unwrap();
        let room_a = user.create_room("Room A").unwrap();
        let room_b = user.create_room("Room B").unwrap();

        let open_a = user.open_room(&room_a, &[]).await.unwrap();
        let open_b = user.open_room(&room_b, &[]).await.unwrap();

        // The fix itself: two live sessions, two distinct EndpointIds. Sharing
        // one is what made inbound traffic collapse onto the last-bound node.
        let id_a = open_a["endpoint"]["endpoint_id"].as_str().unwrap();
        let id_b = open_b["endpoint"]["endpoint_id"].as_str().unwrap();
        assert_ne!(
            id_a, id_b,
            "each open room must bind its own EndpointId, got {id_a} for both"
        );
        assert_eq!(
            user.open_rooms().len(),
            2,
            "both rooms must stay open at once; open_rooms: {:?}",
            user.open_rooms()
        );

        let addr_a = open_a["endpoint"]["addr"].as_str().unwrap().to_owned();
        let addr_b = open_b["endpoint"]["addr"].as_str().unwrap().to_owned();

        // A distinct remote peer joins each room.
        let mut peers = Vec::new();
        for (room, addr, name) in [(&room_a, &addr_a, "peer-a"), (&room_b, &addr_b, "peer-b")] {
            let dir = tempdir().unwrap();
            let profile = crate::identity::create(dir.path()).unwrap();
            let peer = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
            let ticket = user
                .create_invite(room, &profile.identity_id, "member", None)
                .await
                .unwrap();
            peer.join_room(&ticket, Some(name), std::slice::from_ref(addr))
                .await
                .unwrap();
            peer.open_room(room, std::slice::from_ref(addr))
                .await
                .unwrap();
            wait_member_status(&user, room, &profile.identity_id, "active").await;
            peers.push((dir, peer));
        }

        // Both peers publish. Neither room is re-opened or switched to.
        peers[0]
            .1
            .send_message(&room_a, "hello from A")
            .await
            .unwrap();
        peers[1]
            .1
            .send_message(&room_b, "hello from B")
            .await
            .unwrap();

        // The user must see BOTH, with both sessions untouched throughout.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let saw_a = user
                .timeline(&room_a, None)
                .await
                .unwrap()
                .iter()
                .any(|e| e["body"].as_str() == Some("hello from A"));
            let saw_b = user
                .timeline(&room_b, None)
                .await
                .unwrap()
                .iter()
                .any(|e| e["body"].as_str() == Some("hello from B"));
            if saw_a && saw_b {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "only one room received while both were open (room A: {saw_a}, room B: {saw_b}) \
                 — this is the issue #151 collision"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        assert_eq!(
            user.open_rooms().len(),
            2,
            "neither room may have been closed to make the other receive"
        );

        for (_dir, peer) in &peers {
            peer.close_room(&room_a).await.ok();
            peer.close_room(&room_b).await.ok();
        }
        user.close_room(&room_a).await.unwrap();
        user.close_room(&room_b).await.unwrap();
    }

    /// Legacy rooms (genesis bound to the one global device) genuinely cannot
    /// both receive. Opening the second must close the first EXPLICITLY rather
    /// than leave it open and silently deaf.
    #[tokio::test(flavor = "multi_thread")]
    async fn colliding_legacy_rooms_close_instead_of_going_silently_deaf() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let secret = sup.secrets().unwrap();

        // Two rooms exactly as a pre-derivation daemon wrote them: both
        // genesis-signed by the single global device, so both would bind the
        // same EndpointId.
        let mut legacy = Vec::new();
        for (i, name) in ["Legacy One", "Legacy Two"].iter().enumerate() {
            let mut nonce = [0u8; super::ROOM_NONCE_LEN];
            nonce[0] = u8::try_from(i).unwrap() + 1;
            let created_at = crate::now_ms() + u64::try_from(i).unwrap();
            let room_id =
                super::derive_room_id(&secret.identity.identity_key(), &nonce, created_at);
            let wire = super::build_room_created(
                &secret.identity,
                &secret.device,
                name,
                &nonce,
                created_at,
            );
            crate::localstate::remember_room(dir.path(), &room_id.to_string(), Some(name)).unwrap();
            insert_wire(&sup, &room_id, &wire);
            legacy.push(room_id.to_string());
        }

        let first = sup.open_room(&legacy[0], &[]).await.unwrap();
        assert_eq!(sup.open_rooms(), vec![legacy[0].clone()]);

        let second = sup.open_room(&legacy[1], &[]).await.unwrap();
        // Same global device => same EndpointId, which is why they collide.
        assert_eq!(
            first["endpoint"]["endpoint_id"], second["endpoint"]["endpoint_id"],
            "legacy rooms share the global device, so they share an EndpointId"
        );
        // The older session is closed, not left open and unable to receive.
        assert_eq!(
            sup.open_rooms(),
            vec![legacy[1].clone()],
            "opening a colliding legacy room must close the other, not keep both nominally open"
        );
        // The closed room stays readable offline.
        assert!(sup.timeline(&legacy[0], None).await.is_ok());

        sup.close_room(&legacy[1]).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn join_requires_durable_provenance_before_network_mutation() {
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("No Partial Join").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        let state_path = member_dir.path().join(crate::localstate::STATE_FILE);
        std::fs::create_dir(&state_path).unwrap();

        let err = member
            .join_room(&ticket, Some("member"), std::slice::from_ref(&owner_addr))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);

        let typed: RoomId = room_id.parse().unwrap();
        let store = member.open_store().unwrap();
        let (_, snapshot) = member.fold(&store, &typed).unwrap();
        let local_id: iroh_rooms::identity::IdentityKey =
            member_profile.identity_id.parse().unwrap();
        assert!(
            RoomSupervisor::require_local_room_access(&snapshot, &local_id).is_err(),
            "a failed provenance write must happen before member.joined is published"
        );
        drop(store);

        std::fs::remove_dir(&state_path).unwrap();
        member
            .join_room(&ticket, Some("member"), std::slice::from_ref(&owner_addr))
            .await
            .expect("the same ticket remains redeemable after the local write is repaired");
        owner.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn authorized_direct_reads_reuse_the_snapshot_cache() {
        let dir = tempdir().unwrap();
        let profile = crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Cached Reads").unwrap();
        let typed: RoomId = room_id.parse().unwrap();

        sup.readable_snapshot(&typed).await.unwrap();
        let warm_count = sup.fold_invocation_count();

        let err = sup
            .local_file(&room_id, &format!("file_{}", "00".repeat(16)))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::FileUnavailable);
        assert!(sup.pipe_list(&room_id).await.unwrap().is_empty());
        assert!(sup
            .agent_history(&room_id, &profile.identity_id, None)
            .await
            .unwrap()["points"]
            .as_array()
            .unwrap()
            .is_empty());

        assert_eq!(
            sup.fold_invocation_count(),
            warm_count,
            "warm authorized reads must not bypass snapshot_cache and re-fold the room"
        );

        sup.open_room(&room_id, &[]).await.unwrap();
        let open_count = sup.fold_invocation_count();
        let _ = sup
            .local_file(&room_id, &format!("file_{}", "00".repeat(16)))
            .await
            .unwrap_err();
        assert!(sup.pipe_list(&room_id).await.unwrap().is_empty());
        let _ = sup
            .agent_history(&room_id, &profile.identity_id, None)
            .await
            .unwrap();
        assert_eq!(
            sup.fold_invocation_count(),
            open_count,
            "open-room reads must use the live node snapshot without a persisted re-fold"
        );
        sup.close_room(&room_id).await.unwrap();
    }

    #[tokio::test]
    async fn create_room_then_offline_reads_work() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Build Room").unwrap();
        assert!(room_id.starts_with("blake3:"));

        let rooms = sup.list_rooms().await.unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0]["name"], "Build Room");
        assert_eq!(rooms[0]["role"], "owner");
        assert_eq!(rooms[0]["member_count"], 1);
        assert_eq!(rooms[0]["open"], false);
        // The recency projection answers on a CLOSED room — it reads the store,
        // not a session.
        assert_eq!(rooms[0]["last_event_kind"], "room_created");

        let timeline = sup.timeline(&room_id, None).await.unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0]["kind"], "room_created");
        // Recency is the author-signed `created_at` of the newest event, not a
        // clock read at list time.
        assert_eq!(rooms[0]["last_event_ts"], timeline[0]["ts"]);

        let members = sup.members(&room_id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["role"], "owner");
        assert_eq!(members[0]["status"], "active");
    }

    #[tokio::test]
    async fn member_remove_is_signed_exact_and_semantically_idempotent_offline() {
        let dir = tempdir().unwrap();
        let authority = crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Removal Room").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();

        let member_identity = SigningKey::generate();
        let member_device = SigningKey::generate();
        seed_agent_member(&sup, &room_id_str, &member_identity, &member_device).await;
        let member_id = member_identity.identity_key();

        let RemoveMemberOutcome::Removed(first_id) =
            sup.remove_member(&room_id, &member_id).await.unwrap()
        else {
            panic!("active joined member was not removed");
        };
        let stored = sup
            .open_store()
            .unwrap()
            .by_type(&room_id, EventType::MemberRemoved)
            .unwrap();
        assert_eq!(stored.len(), 1, "exactly one removal fact was authored");
        let event = SignedEvent::decode(&stored[0].wire.signed).unwrap();
        let Content::MemberRemoved(removed) = event.content else {
            panic!("wrong content");
        };
        assert_eq!(removed.member_id, member_id);
        assert_eq!(removed.removed_by.to_string(), authority.identity_id);
        assert!(
            removed.device_binding.is_some(),
            "the authority/device relationship is self-attested"
        );
        assert_eq!(
            sup.snapshot_for(&room_id).await.unwrap().status(&member_id),
            Some(iroh_rooms::room::Status::Removed)
        );

        let RemoveMemberOutcome::Removed(replay_id) =
            sup.remove_member(&room_id, &member_id).await.unwrap()
        else {
            panic!("repeat removal did not return the terminal fact");
        };
        assert_eq!(replay_id, first_id);
        assert_eq!(
            sup.open_store()
                .unwrap()
                .by_type(&room_id, EventType::MemberRemoved)
                .unwrap()
                .len(),
            1,
            "a fresh semantic repeat authors no second event"
        );

        let unknown = SigningKey::generate().identity_key();
        assert!(matches!(
            sup.remove_member(&room_id, &unknown).await.unwrap(),
            RemoveMemberOutcome::Unknown
        ));
        let authority_id: iroh_rooms::identity::IdentityKey =
            authority.identity_id.parse().unwrap();
        assert!(matches!(
            sup.remove_member(&room_id, &authority_id).await.unwrap(),
            RemoveMemberOutcome::Authority
        ));
    }

    #[tokio::test]
    async fn concurrent_self_leave_does_not_hide_the_effective_removal() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id_str = sup.create_room("Concurrent Removal").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();
        let member_identity = SigningKey::generate();
        let member_device = SigningKey::generate();
        seed_agent_member(&sup, &room_id_str, &member_identity, &member_device).await;
        let member_id = member_identity.identity_key();

        let secret = sup.secrets().unwrap();
        let store = sup.open_store().unwrap();
        let (_, snapshot) = sup.fold(&store, &room_id).unwrap();
        let room_device = sup.authoring_device_key(&snapshot, &secret, &room_id);
        let admin = snapshot.admin().copied().unwrap();
        let heads = RoomSupervisor::authorization_class_heads(&store, &room_id, &admin).unwrap();
        drop(store);

        let removal = build_member_removed(
            &secret.identity,
            &room_device,
            &room_id,
            &member_id,
            None,
            Some(DeviceBinding::create(
                &room_id,
                &secret.identity,
                room_device.device_key(),
            )),
            &heads,
            crate::now_ms(),
        );
        let removal_id =
            validate_wire_bytes(&removal.to_bytes(), &ValidationContext::for_room(room_id))
                .unwrap()
                .event_id;

        // Force the concurrent leave to sort AFTER the removal in canonical
        // display order. The old scan-order implementation then cleared the
        // effective removal even though neither sibling causally supersedes it.
        let mut ts = crate::now_ms();
        let leave = loop {
            let candidate =
                build_member_left(&member_identity, &member_device, &room_id, None, &heads, ts);
            let candidate_id =
                validate_wire_bytes(&candidate.to_bytes(), &ValidationContext::for_room(room_id))
                    .unwrap()
                    .event_id;
            if candidate_id > removal_id {
                break candidate;
            }
            ts += 1;
        };
        insert_wire(&sup, &room_id, &removal);
        insert_wire(&sup, &room_id, &leave);

        assert_eq!(
            sup.snapshot_for(&room_id).await.unwrap().status(&member_id),
            Some(Status::Removed),
            "Removed dominates a concurrent self-leave"
        );
        let RemoveMemberOutcome::Removed(replayed) =
            sup.remove_member(&room_id, &member_id).await.unwrap()
        else {
            panic!("the effective concurrent removal was not replayed");
        };
        assert_eq!(replayed, bare_event_hex(&removal_id));
        assert_eq!(
            sup.open_store()
                .unwrap()
                .by_type(&room_id, EventType::MemberRemoved)
                .unwrap()
                .len(),
            1,
            "semantic replay authors no second removal"
        );
    }

    /// `docs/room-attention.md` decision 2: recency is the newest signed
    /// event's own `created_at`, projected from the store, per room — and it
    /// agrees with what that room's timeline shows.
    #[tokio::test(flavor = "multi_thread")]
    async fn room_list_recency_tracks_the_newest_event_per_room() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let quiet = sup.create_room("Quiet").unwrap();
        let busy = sup.create_room("Busy").unwrap();

        let row = |rooms: &[serde_json::Value], id: &str| {
            rooms
                .iter()
                .find(|r| r["room_id"] == id)
                .cloned()
                .expect("room is listed")
        };

        let before = sup.list_rooms().await.unwrap();
        let quiet_ts = row(&before, &quiet)["last_event_ts"].as_u64().unwrap();
        assert_eq!(row(&before, &busy)["last_event_kind"], "room_created");

        // Real activity in ONE room, through the ordinary authoring path.
        sup.open_room(&busy, &[]).await.unwrap();
        sup.send_message(&busy, "later").await.unwrap();
        sup.close_room(&busy).await.unwrap();

        let after = sup.list_rooms().await.unwrap();
        let newest = sup
            .timeline(&busy, None)
            .await
            .unwrap()
            .last()
            .cloned()
            .expect("the room has events");
        assert_eq!(newest["kind"], "message");
        // The row's recency IS the newest timeline event's signed timestamp —
        // not a clock read at list time, and consistent with the room's own
        // timeline. Asserted on a CLOSED room: this is a store projection.
        assert_eq!(
            row(&after, &busy)["last_event_ts"].as_u64(),
            newest["ts"].as_u64(),
            "recency must be the newest event's signed created_at"
        );
        assert_eq!(row(&after, &busy)["last_event_kind"], "message");
        // Per-room, not global: the quiet room is untouched.
        assert_eq!(
            row(&after, &quiet)["last_event_ts"].as_u64(),
            Some(quiet_ts),
            "another room's activity must not move this room's recency"
        );
        assert_eq!(row(&after, &quiet)["last_event_kind"], "room_created");
    }

    /// Recency is the MAX signed timestamp, not the causally-last event's.
    /// A peer whose clock lags signs an event that lands causally last with an
    /// older `created_at`; reporting that would move a room's recency backward
    /// and silently clear an unread dot.
    #[tokio::test]
    async fn room_list_recency_does_not_move_backward_on_a_lagging_clock() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let secret = sup.secrets().unwrap();
        let room = sup.create_room("Skewed").unwrap();

        async fn listed(sup: &RoomSupervisor) -> u64 {
            sup.list_rooms().await.unwrap()[0]["last_event_ts"]
                .as_u64()
                .expect("the room has events, so it has recency")
        }

        // A well-clocked author posts at T.
        let ahead = crate::now_ms() + 60_000;
        seed_message(
            &sup,
            &room,
            &secret.identity,
            &secret.device,
            "ahead",
            ahead,
        );
        assert_eq!(listed(&sup).await, ahead);

        // A lagging peer then posts; it is causally LAST (it cites the newer
        // heads) but signs an older timestamp.
        seed_message(
            &sup,
            &room,
            &secret.identity,
            &secret.device,
            "behind",
            ahead - 30_000,
        );
        assert_eq!(
            listed(&sup).await,
            ahead,
            "recency must not regress to a causally-later but older-signed event"
        );
    }

    /// Issue #154: every room this daemon LISTS carries recency. A room with no
    /// stored events fails its own fold (`RoomUnknown`) and is skipped by
    /// `list_rooms`, so a listed room always has at least one event to project
    /// from. Clients rely on this: a listed row with a null `last_event_ts`
    /// means the daemon predates the projection, not that the room is empty.
    #[tokio::test]
    async fn every_listed_room_carries_recency() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();

        // A room recorded in the local index but holding no events: the index
        // entry alone must not put it on the list, precisely because there
        // would be nothing to project recency from.
        let ghost = "blake3:00000000000000000000000000000000000000000000000000000000000000aa";
        crate::localstate::remember_room(dir.path(), ghost, Some("Ghost")).unwrap();

        let created = sup.create_room("Created").unwrap();
        let opened = sup.create_room("Opened").unwrap();
        sup.open_room(&opened, &[]).await.unwrap();

        let rooms = sup.list_rooms().await.unwrap();
        assert!(
            !rooms.iter().any(|r| r["room_id"] == ghost),
            "a room with no stored events must not be listed at all"
        );
        assert_eq!(rooms.len(), 2, "listed: {rooms:?}");
        for room in &rooms {
            assert!(
                room["last_event_ts"].as_u64().is_some(),
                "every listed room must carry recency; {room:?}"
            );
            assert!(
                room["last_event_kind"].is_string(),
                "and the kind of that event; {room:?}"
            );
        }
        assert!(rooms.iter().any(|r| r["room_id"] == created));
        sup.close_room(&opened).await.unwrap();
    }

    #[tokio::test]
    async fn room_reads_exclude_rooms_this_identity_never_belonged_to() {
        // Regression: a foreign room's membership sub-DAG can be backfilled into
        // our store by a shared peer's sync (that peer is in a room WITH us and
        // also in this OTHER room), even though we were never invited. No public
        // read may reveal that room's existence or contents.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = Arc::new(RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap());
        let local_identity = crate::identity::load_profile(dir.path())
            .unwrap()
            .unwrap()
            .identity_id;

        // A room WE own — the control that must still list.
        let mine = sup.create_room("Mine").unwrap();

        // Build a real second room under a different local identity, including
        // an agent member, status and message, then copy its validated wires
        // into our shared store exactly as a sync backfill persists them.
        let foreign_dir = tempdir().unwrap();
        crate::identity::create(foreign_dir.path()).unwrap();
        let foreign = RoomSupervisor::new(foreign_dir.path().to_path_buf(), true).unwrap();
        let foreign_room = foreign.create_room("Not Yours").unwrap();
        let foreign_room_id: RoomId = foreign_room.parse().unwrap();
        let foreign_agent = SigningKey::generate();
        let foreign_device = SigningKey::generate();
        seed_agent_member(&foreign, &foreign_room, &foreign_agent, &foreign_device).await;
        seed_status(
            &foreign,
            &foreign_room,
            &foreign_agent,
            &foreign_device,
            "working",
            Some(73),
            crate::now_ms(),
        );
        seed_message(
            &foreign,
            &foreign_room,
            &foreign_agent,
            &foreign_device,
            "foreign room secret",
            crate::now_ms() + 1,
        );
        // A targeted invite is not read authorization. Until this identity
        // authors an accepted member.joined and gains a device binding, room
        // content remains unavailable.
        foreign
            .create_invite(&foreign_room, &local_identity, "member", None)
            .await
            .unwrap();
        // Seed real file and pipe records too, rather than proving only that
        // empty foreign collections are hidden. The owner opens its own room,
        // authors both records through the production paths, then closes it
        // before the complete validated log is copied below.
        foreign.open_room(&foreign_room, &[]).await.unwrap();
        let foreign_payload = foreign_dir.path().join("foreign-payload.bin");
        std::fs::write(&foreign_payload, b"foreign file secret").unwrap();
        let foreign_file = foreign
            .share_file(
                &foreign_room,
                foreign_payload.to_str().unwrap(),
                Some("foreign-payload.bin"),
                Some("application/octet-stream"),
            )
            .await
            .unwrap();
        let foreign_file_id = foreign_file.file_id;
        let foreign_pipe = foreign
            .pipe_expose(
                &foreign_room,
                "127.0.0.1:9",
                &foreign_agent.identity_key().to_string(),
            )
            .await
            .unwrap();
        let foreign_pipe_id = foreign_pipe["pipe_id"].as_str().unwrap().to_owned();
        foreign.close_room(&foreign_room).await.unwrap();
        let foreign_rows = foreign
            .open_store()
            .unwrap()
            .room_tail(&foreign_room_id, u32::MAX)
            .unwrap();
        for row in &foreign_rows {
            insert_wire(&sup, &foreign_room_id, &row.wire);
        }

        // Sanity: the foreign room's genesis really is in our store.
        {
            let store = sup.open_store().unwrap();
            assert_eq!(
                store.count(&foreign_room_id).unwrap(),
                foreign_rows.len() as u64
            );
        }

        let rooms = sup.list_rooms().await.unwrap();
        let ids: Vec<&str> = rooms.iter().filter_map(|r| r["room_id"].as_str()).collect();
        assert!(ids.contains(&mine.as_str()), "our own room must list");
        assert!(
            !ids.contains(&foreign_room_id.to_string().as_str()),
            "a room we are not a member of must be excluded from room.list"
        );
        assert_eq!(rooms.len(), 1, "only our own room lists; got {rooms:?}");

        // The global fleet read must filter before counting rooms or folding
        // agent/status details. It may not become a discovery oracle.
        let fleet = sup.agents_fleet().await.unwrap();
        assert_eq!(fleet["rooms_total"], 1);
        assert_eq!(fleet["total"], 0);
        let fleet_wire = fleet.to_string();
        assert!(!fleet_wire.contains("Not Yours"));
        assert!(!fleet_wire.contains(&foreign_agent.identity_key().to_string()));
        assert!(!fleet_wire.contains("foreign room secret"));

        // Every room-scoped read uses the same default-deny guard. Return
        // `room_unknown` rather than confirming that an inaccessible room is
        // present in this daemon's shared SQLite store.
        let room_id = foreign_room_id.to_string();
        let err = sup.timeline(&room_id, None).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup.members(&room_id).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup.list_files(&room_id).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup
            .fetch_file(&room_id, &foreign_file_id, None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let direct_read_fold_count = sup.fold_invocation_count();
        let err = sup
            .local_file(&room_id, &foreign_file_id)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup.pipe_list(&room_id).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup
            .agent_history(&room_id, &foreign_agent.identity_key().to_string(), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        assert_eq!(
            sup.fold_invocation_count(),
            direct_read_fold_count,
            "foreign-room direct reads must deny from provenance before decoding or folding rows"
        );
        let err = sup.peers_status(&room_id).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);

        // The transport-neutral dispatch boundary covers every public
        // room-scoped RPC, including mutations whose method-specific errors
        // would otherwise become a stored-room existence oracle.
        let (shutdown_tx, _shutdown_rx) = tokio::sync::mpsc::channel(1);
        let engine = crate::engine::Engine::with_supervisor(
            Arc::clone(&sup),
            crate::engine::EngineConfig {
                port: 0,
                version: crate::engine::CORE_VERSION.to_owned(),
                shutdown_tx,
            },
        );
        let state_path = dir.path().join(crate::localstate::STATE_FILE);
        let state_before = std::fs::read(&state_path).unwrap();
        // The typed v2 dispatch boundary: every room-scoped operation on the
        // foreign room must answer the non-oracle `room_not_available`, never
        // disclosing that the room is present in the shared store.
        use jeliya_api::RoomId as ApiRoomId;
        use jeliya_api::{
            ApiError, Cursor, Direction, FileFetch, FileId, FileList, MessageSend, Page,
            PipeConnect, PipeId, PipeList, Progress, RoomActivate, RoomDeactivate, RoomLeave,
            RoomList, RoomMembers, RoomPeers, RoomTimeline, StatusHistory, StatusLabel, StatusPost,
            SubjectId,
        };
        let _ = RoomList {};
        let froom = ApiRoomId::new(room_id.clone());
        let page = Page {
            cursor: Cursor::Start,
            direction: Direction::Forward,
            limit: 50,
        };
        let cases: Vec<(&str, crate::typed::TypedCall)> = vec![
            (
                "room.activate",
                crate::typed::TypedCall::RoomActivate(RoomActivate {
                    room_id: froom.clone(),
                }),
            ),
            (
                "room.deactivate",
                crate::typed::TypedCall::RoomDeactivate(RoomDeactivate {
                    room_id: froom.clone(),
                }),
            ),
            (
                "room.leave",
                crate::typed::TypedCall::RoomLeave(RoomLeave {
                    room_id: froom.clone(),
                }),
            ),
            (
                "room.timeline",
                crate::typed::TypedCall::RoomTimeline(RoomTimeline {
                    room_id: froom.clone(),
                    page: page.clone(),
                }),
            ),
            (
                "room.members",
                crate::typed::TypedCall::RoomMembers(RoomMembers {
                    room_id: froom.clone(),
                }),
            ),
            (
                "room.peers",
                crate::typed::TypedCall::RoomPeers(RoomPeers {
                    room_id: froom.clone(),
                }),
            ),
            (
                "message.send",
                crate::typed::TypedCall::MessageSend(MessageSend {
                    room_id: froom.clone(),
                    body: "denied".into(),
                }),
            ),
            (
                "status.post",
                crate::typed::TypedCall::StatusPost(StatusPost {
                    room_id: froom.clone(),
                    label: StatusLabel::Working,
                    progress: Progress::Absent,
                }),
            ),
            (
                "file.list",
                crate::typed::TypedCall::FileList(FileList {
                    room_id: froom.clone(),
                    page: page.clone(),
                }),
            ),
            (
                "file.fetch",
                crate::typed::TypedCall::FileFetch(FileFetch {
                    room_id: froom.clone(),
                    file_id: FileId::new(foreign_file_id.clone()),
                }),
            ),
            (
                "pipe.list",
                crate::typed::TypedCall::PipeList(PipeList {
                    room_id: froom.clone(),
                    page: page.clone(),
                }),
            ),
            (
                "pipe.connect",
                crate::typed::TypedCall::PipeConnect(PipeConnect {
                    room_id: froom.clone(),
                    pipe_id: PipeId::new(foreign_pipe_id.clone()),
                }),
            ),
            (
                "status.history",
                crate::typed::TypedCall::StatusHistory(StatusHistory {
                    room_id: froom.clone(),
                    subject_id: SubjectId::new(foreign_agent.identity_key().to_string()),
                    page: page.clone(),
                }),
            ),
        ];
        for (method, call) in cases {
            let err = engine.execute(call).await.reply.unwrap_err();
            assert!(
                matches!(
                    err,
                    ApiError::RoomNotAvailable { .. } | ApiError::RoomNotLive { .. }
                ),
                "{method} must not disclose a stored foreign room: {err:?}"
            );
        }
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            state_before,
            "denied room.open must not persist peer hints or a room index entry"
        );
    }

    #[tokio::test]
    async fn unknown_room_is_room_unknown() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        sup.create_room("Seed").unwrap();
        let unknown = format!("blake3:{}", "de".repeat(32));
        let err = sup.timeline(&unknown, None).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
    }

    #[tokio::test]
    async fn message_send_requires_open_room() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Room").unwrap();
        let err = sup.send_message(&room_id, "hi").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomNotOpen);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_send_timeline_roundtrip_loopback() {
        // The daemon's core happy path, end to end against the real SDK node.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Live Room").unwrap();

        let opened = sup.open_room(&room_id, &[]).await.unwrap();
        assert!(opened["endpoint"]["endpoint_id"].is_string());
        assert_eq!(opened["timeline"][0]["kind"], "room_created");

        let event_id = sup.send_message(&room_id, "hello jeliya").await.unwrap();
        assert_eq!(event_id.len(), 64);

        // The freshly published message is pushed exactly once...
        let typed_room: iroh_rooms::room::RoomId = room_id.parse().unwrap();
        let pushed = sup.poll_new_events(&typed_room).await.unwrap();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0]["kind"], "message");
        assert_eq!(pushed[0]["body"], "hello jeliya");
        assert!(sup.poll_new_events(&typed_room).await.unwrap().is_empty());

        // ...and the offline timeline read sees genesis + message in order.
        let timeline = sup.timeline(&room_id, None).await.unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0]["kind"], "room_created");
        assert_eq!(timeline[1]["kind"], "message");

        sup.close_room(&room_id).await.unwrap();
        assert!(sup.open_rooms().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn owner_cannot_leave_room() {
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Owner Stays").unwrap();
        sup.open_room(&room_id, &[]).await.unwrap();

        let err = sup.leave_room(&room_id).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);
        assert!(err.message.contains("owners cannot leave"));

        let members = sup.members(&room_id).await.unwrap();
        assert_eq!(members[0]["role"], "owner");
        assert_eq!(members[0]["status"], "active");
        sup.close_room(&room_id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn member_leave_is_distinct_from_close_and_blocks_reopen() {
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("Leave Room").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        member
            .join_room(&ticket, Some("leaver"), std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        member.open_room(&room_id, &[]).await.unwrap();
        wait_member_status(&owner, &room_id, &member_profile.identity_id, "active").await;

        // `room.close` is only a local session shutdown: membership remains active.
        member.close_room(&room_id).await.unwrap();
        let mine =
            wait_member_status(&member, &room_id, &member_profile.identity_id, "active").await;
        assert_eq!(mine["role"], "member");

        // `room.leave` authors member.left, closes the local session, and makes
        // the departure visible to both the leaver and connected peers.
        member.open_room(&room_id, &[]).await.unwrap();
        let event_id = member.leave_room(&room_id).await.unwrap();
        assert_eq!(event_id.len(), 64);
        assert!(member.open_rooms().is_empty());
        let mine = wait_member_status(&member, &room_id, &member_profile.identity_id, "left").await;
        assert_eq!(mine["role"], "member");
        wait_member_status(&owner, &room_id, &member_profile.identity_id, "left").await;

        let err = member.open_room(&room_id, &[owner_addr]).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotAMember);

        owner.close_room(&room_id).await.unwrap();
    }

    /// Revocation is a **relation**, not a role: the record restricts
    /// `pipe.revoke` to the pipe's publisher, "a narrower relation than role",
    /// and answers `pipe_not_publisher`. An earlier revision authorized
    /// `is_admin || is_owner`, so the room's authority could destroy a tunnel
    /// another member published — a role bypassing a relation the record
    /// deliberately made narrower than any role. The authority here is even
    /// inside the pipe's audience, and still may not close it.
    #[tokio::test(flavor = "multi_thread")]
    async fn pipe_close_refuses_a_room_authority_that_did_not_publish() {
        let owner_dir = tempdir().unwrap();
        let owner_profile = crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id_str = owner.create_room("Publisher Only").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();
        let opened = owner.open_room(&room_id_str, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id_str, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        member
            .join_room(&ticket, None, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        member
            .open_room(&room_id_str, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        wait_member_status(&owner, &room_id_str, &member_profile.identity_id, "active").await;

        // The MEMBER publishes, authorizing the room's owner as its audience.
        let owner_id: iroh_rooms::identity::IdentityKey =
            owner_profile.identity_id.parse().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (pipe_id, _) = member
            .pipe_expose_multi(
                &room_id,
                target,
                &target.to_string(),
                std::slice::from_ref(&owner_id),
            )
            .await
            .unwrap();
        let pipe_id_hex = hex::encode(pipe_id);

        // Wait for the announcement to reach the owner's log, so the refusal
        // below is the authorization one and not "no such pipe".
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let pipes = owner.pipe_list(&room_id_str).await.unwrap();
            if pipes
                .iter()
                .any(|p| p["pipe_id"].as_str() == Some(pipe_id_hex.as_str()))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the member's pipe.opened to reach the owner"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // The room's authority, inside the audience, is still not the publisher.
        let err = owner
            .pipe_close(&room_id_str, &pipe_id_hex)
            .await
            .expect_err("a room authority that did not publish cannot revoke");
        assert_eq!(
            err.kind,
            ErrorKind::PipeDenied,
            "the authority earns the publisher-only refusal, which typed maps to \
             pipe_not_publisher, never a silent success"
        );

        // The publisher itself can, so the refusal is about the relation and
        // not about the pipe being unclosable.
        member.pipe_close(&room_id_str, &pipe_id_hex).await.unwrap();

        member.close_room(&room_id_str).await.unwrap();
        owner.close_room(&room_id_str).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pipe_release_closes_only_one_of_two_connections_to_the_same_pipe() {
        let owner_dir = tempdir().unwrap();
        let owner_profile = crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id_str = owner.create_room("Pipe Connections").unwrap();
        let room_id: RoomId = room_id_str.parse().unwrap();
        let opened = owner.open_room(&room_id_str, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id_str, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        member
            .join_room(&ticket, None, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        member
            .open_room(&room_id_str, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        wait_member_status(&owner, &room_id_str, &member_profile.identity_id, "active").await;

        let member_id: iroh_rooms::identity::IdentityKey =
            member_profile.identity_id.parse().unwrap();
        let owner_id: iroh_rooms::identity::IdentityKey =
            owner_profile.identity_id.parse().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let (pipe_id, _) = owner
            .pipe_expose_multi(
                &room_id,
                target,
                &target.to_string(),
                std::slice::from_ref(&member_id),
            )
            .await
            .unwrap();
        let pipe_id_hex = hex::encode(pipe_id);

        let first = member
            .pipe_connect(&room_id_str, &pipe_id_hex)
            .await
            .unwrap();
        let second = member
            .pipe_connect(&room_id_str, &pipe_id_hex)
            .await
            .unwrap();
        assert_ne!(first, second, "each connection has its own local identity");
        assert!(member.pipe_connection_open(&room_id, pipe_id, &owner_id));

        assert!(member.pipe_release(&first));
        assert!(
            member.pipe_connection_open(&room_id, pipe_id, &owner_id),
            "the sibling connection remains open"
        );
        assert!(!member.pipe_release(&first), "a released id is unknown");
        assert!(member.pipe_release(&second));
        assert!(
            !member.pipe_connection_open(&room_id, pipe_id, &owner_id),
            "the runtime fact clears after the last local connection"
        );

        let third = member
            .pipe_connect(&room_id_str, &pipe_id_hex)
            .await
            .unwrap();
        let fourth = member
            .pipe_connect(&room_id_str, &pipe_id_hex)
            .await
            .unwrap();
        assert_ne!(third, fourth);
        assert_eq!(
            member
                .release_pipe_connections(&room_id, &pipe_id_hex)
                .unwrap(),
            2,
            "a revocation tears down every sibling connection for the pipe"
        );
        assert!(!member.pipe_connection_open(&room_id, pipe_id, &owner_id));
        let err = member
            .pipe_connect(&room_id_str, &pipe_id_hex)
            .await
            .unwrap_err();
        assert_eq!(
            err.kind,
            ErrorKind::PipeDenied,
            "a connection finishing after revocation must not resurrect locally"
        );
        assert_eq!(
            member
                .open_store()
                .unwrap()
                .by_type(&room_id, EventType::PipeClosed)
                .unwrap()
                .len(),
            0,
            "release does not author a pipe revocation"
        );

        member.close_room(&room_id_str).await.unwrap();
        owner.close_room(&room_id_str).await.unwrap();
        drop(listener);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn own_shared_file_list_and_fetch_agree() {
        // Finding #5: file.list must not claim availability that file.fetch
        // cannot honor. A file whose sole provider is this device shows
        // available:false, and file.fetch returns file_unavailable — never a
        // contradiction.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Files").unwrap();
        sup.open_room(&room_id, &[]).await.unwrap();

        let path = dir.path().join("shared.txt");
        std::fs::write(&path, b"hello jeliya file").unwrap();
        let shared = sup
            .share_file(&room_id, path.to_str().unwrap(), None, None)
            .await
            .unwrap();
        let file_id = shared.file_id;

        let files = sup.list_files(&room_id).await.unwrap();
        let row = files
            .iter()
            .find(|f| f["file_id"] == file_id.as_str())
            .expect("the shared file appears in file.list");
        assert_eq!(
            row["available"], false,
            "self-only provider is not fetchable"
        );

        let err = sup.fetch_file(&room_id, &file_id, None).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::FileUnavailable);

        // The host-facing local-file API historically preserved the caller's
        // valid spelling in its diagnostic, including a bare uppercase id.
        // The shared resolver used by protocol file.read must not canonicalize
        // that externally visible text.
        let original_file_id = file_id
            .strip_prefix("file_")
            .expect("shared ids use the file_ prefix")
            .to_uppercase();
        let err = sup
            .local_file(&room_id, &original_file_id)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::FileUnavailable);
        assert_eq!(
            err.message,
            format!("file {original_file_id} has not been fetched on this daemon")
        );

        let downloads = dir.path().join(super::DOWNLOADS_DIR);
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(downloads.join("shared.txt"), b"hello jeliya file").unwrap();
        let files = sup.list_files(&room_id).await.unwrap();
        let row = files
            .iter()
            .find(|f| f["file_id"] == file_id.as_str())
            .expect("the shared file appears in file.list");
        assert_eq!(
            row["fetched"], true,
            "a previously downloaded default-path copy should suppress Fetch"
        );

        sup.close_room(&room_id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetched_file_state_survives_room_reload_loopback() {
        let owner_dir = tempdir().unwrap();
        crate::identity::create(owner_dir.path()).unwrap();
        let owner = RoomSupervisor::new(owner_dir.path().to_path_buf(), true).unwrap();
        let room_id = owner.create_room("Fetched Files").unwrap();
        let opened = owner.open_room(&room_id, &[]).await.unwrap();
        let owner_addr = opened["endpoint"]["addr"].as_str().unwrap().to_owned();

        let path = owner_dir.path().join("report.txt");
        std::fs::write(&path, b"verified bytes").unwrap();
        let shared = owner
            .share_file(&room_id, path.to_str().unwrap(), None, None)
            .await
            .unwrap();
        let file_id = shared.file_id;

        let member_dir = tempdir().unwrap();
        let member_profile = crate::identity::create(member_dir.path()).unwrap();
        let member = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let ticket = owner
            .create_invite(&room_id, &member_profile.identity_id, "member", None)
            .await
            .unwrap();
        member
            .join_room(&ticket, Some("fetcher"), std::slice::from_ref(&owner_addr))
            .await
            .unwrap();
        member
            .open_room(&room_id, std::slice::from_ref(&owner_addr))
            .await
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let files = member.list_files(&room_id).await.unwrap();
            let row = files
                .iter()
                .find(|f| f["file_id"].as_str() == Some(file_id.as_str()));
            if row.is_some_and(|f| f["available"] == true) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "member never saw the shared file become fetchable; last id: {file_id}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let fetched = member.fetch_file(&room_id, &file_id, None).await.unwrap();
        assert_eq!(fetched.bytes, 14);

        member.close_room(&room_id).await.unwrap();
        let member_restarted = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let files = member_restarted.list_files(&room_id).await.unwrap();
        let row = files
            .iter()
            .find(|f| f["file_id"].as_str() == Some(file_id.as_str()))
            .expect("shared file remains listed after restart");
        assert_eq!(row["fetched"], true);
        assert_eq!(row["local_bytes"], 14);
        assert_eq!(row["local_path"], fetched.path.display().to_string());

        let local = member_restarted
            .local_file(&room_id, &file_id)
            .await
            .unwrap();
        assert_eq!(local.path.display().to_string(), fetched.path);
        assert_eq!(local.name, "report.txt");
        assert_eq!(local.bytes, 14);

        owner.close_room(&room_id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_share_confined_to_the_data_dir() {
        // Finding #9: file.share must not read arbitrary local files.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Files").unwrap();
        sup.open_room(&room_id, &[]).await.unwrap();

        // A file outside the data dir is refused (the exfiltration primitive).
        let outside = tempdir().unwrap();
        let secret = outside.path().join("id_rsa");
        std::fs::write(&secret, b"PRIVATE KEY").unwrap();
        let err = sup
            .share_file(&room_id, secret.to_str().unwrap(), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);

        // The daemon's own secret file is refused even though it is under the dir.
        let own_secret = dir.path().join(crate::identity::SECRET_FILE);
        let err = sup
            .share_file(&room_id, own_secret.to_str().unwrap(), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);

        // The room must still be open (since #84 no share cycles the node, and a
        // refused share returns before importing anything either way).
        assert!(sup.open_rooms().contains(&room_id));
        sup.close_room(&room_id).await.unwrap();
    }

    #[test]
    fn fetch_dir_confined_to_the_downloads_tree() {
        // Issue #122: file.fetch's destination was an arbitrary-file-write
        // primitive. Confinement is asserted here rather than through
        // fetch_file, which needs a live provider before it reaches the sink.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let root = std::fs::canonicalize(dir.path())
            .unwrap()
            .join(super::DOWNLOADS_DIR);

        // Omitted and empty both fall back to the documented default.
        assert_eq!(sup.resolve_fetch_dir(None).unwrap(), root);
        assert_eq!(sup.resolve_fetch_dir(Some("  ")).unwrap(), root);

        // A relative path resolves under the downloads tree, not the cwd.
        assert_eq!(
            sup.resolve_fetch_dir(Some("nested")).unwrap(),
            root.join("nested")
        );

        // The destination need not exist yet.
        let deep = root.join("a").join("b");
        assert_eq!(
            sup.resolve_fetch_dir(Some(deep.to_str().unwrap())).unwrap(),
            deep
        );

        // The code-execution primitive: an absolute path outside the tree.
        let outside = tempdir().unwrap();
        let autostart = outside.path().join(".config/autostart");
        let err = sup
            .resolve_fetch_dir(Some(autostart.to_str().unwrap()))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);

        // Traversal out of the tree is refused before the filesystem is touched.
        for escape in ["../..", "nested/../../..", "/tmp/../etc"] {
            let err = sup.resolve_fetch_dir(Some(escape)).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidParams, "escape: {escape}");
        }

        // The data-dir root itself is refused, so a caller cannot land a file
        // beside the identity and secret files.
        let err = sup
            .resolve_fetch_dir(Some(dir.path().to_str().unwrap()))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);
    }

    #[cfg(unix)]
    #[test]
    fn fetch_dir_refuses_a_symlink_out_of_the_downloads_tree() {
        // A symlink planted inside the tree must not redirect the write out of
        // it: the deepest existing ancestor is canonicalized before the check.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let root = std::fs::canonicalize(dir.path())
            .unwrap()
            .join(super::DOWNLOADS_DIR);
        std::fs::create_dir_all(&root).unwrap();

        let outside = tempdir().unwrap();
        let escape = root.join("escape");
        std::os::unix::fs::symlink(outside.path(), &escape).unwrap();

        let err = sup
            .resolve_fetch_dir(Some(escape.to_str().unwrap()))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);

        // And through the symlink into a subdirectory that does not exist yet.
        let err = sup
            .resolve_fetch_dir(Some(escape.join("deep").to_str().unwrap()))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);
    }

    #[cfg(unix)]
    #[test]
    fn fetch_dir_refuses_a_symlinked_default_downloads_dir() {
        // Review finding on #125: the omitted-save_dir fallback originally
        // returned the default without validating it, so a `downloads`
        // symlink pointing out of the data dir escaped the confinement on the
        // exact path the UI uses. The default must go through the same checks.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();

        let outside = tempdir().unwrap();
        let downloads = std::fs::canonicalize(dir.path())
            .unwrap()
            .join(super::DOWNLOADS_DIR);
        std::os::unix::fs::symlink(outside.path(), &downloads).unwrap();

        for arg in [None, Some("  ")] {
            let err = sup.resolve_fetch_dir(arg).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidParams, "arg: {arg:?}");
        }
    }

    #[tokio::test]
    async fn fetch_rejects_a_bad_save_dir_before_contacting_providers() {
        // Review finding on #125: an invalid save_dir was only caught after the
        // provider loop had run, so a bad request cost peer connections and a
        // full transfer, and surfaced as file_unavailable when every provider
        // was offline. The parameter error must win, and must come first.
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();
        let room_id = sup.create_room("Files").unwrap();
        sup.open_room(&room_id, &[]).await.unwrap();

        let payload = dir.path().join("payload.txt");
        std::fs::write(&payload, b"payload").unwrap();
        let shared = sup
            .share_file(&room_id, payload.to_str().unwrap(), None, None)
            .await
            .unwrap();
        let file_id = shared.file_id;

        // No other peer is online, so an unvalidated destination would return
        // FileUnavailable from the provider loop instead of the real error.
        let outside = tempdir().unwrap();
        let err = sup
            .fetch_file(
                &room_id.to_string(),
                &file_id,
                Some(outside.path().to_str().unwrap()),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidParams);

        sup.close_room(&room_id).await.unwrap();
    }
}
