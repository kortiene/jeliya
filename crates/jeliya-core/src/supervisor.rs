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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;

use iroh::TransportAddr;
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
use iroh_rooms::experimental::session::{
    Admission, AdmissionView, AllowlistAdmission, BlobServeConfig, ConnEvent, EndpointAddr,
    EndpointId, JoinBootstrapAdmission, NetConfig, NetMode, Node, PeerConnState, SecretKey,
    SnapshotAdmission, TracingAudit, DEFAULT_TICK,
};
use iroh_rooms::experimental::store::{EventStore, StoreOptions, StoredEvent};
use iroh_rooms::experimental::sync::{SyncConfig, SyncEngine};
use iroh_rooms::files::build_file_shared;
use iroh_rooms::identity::{DeviceBinding, DeviceKey, IdentityKey};
use iroh_rooms::room::{
    build_member_invited, build_member_joined, build_member_left, build_room_created,
    derive_room_id, Ingest, MembershipSnapshot, Role, RoomId, RoomInviteTicket, RoomMembership,
    Status,
};

use crate::error::{CoreError, CoreResult, ErrorKind};
use crate::fleet::{self, Liveness};
use crate::identity::SecretKeys;
use crate::materializer::{self, bare_event_hex, file_handle, role_label};
use crate::{localstate, now_ms};

/// The single event-store database file under the data dir (mirrors the CLI).
pub const DB_FILE: &str = "rooms.db";
/// Root for the per-room durable blob stores.
const BLOBS_DIR: &str = "blobs";
/// Maximum number of bytes accepted for one shared file, exposed so the daemon's
/// browser-upload endpoint can reject over-limit bodies before staging them.
pub const FILE_UPLOAD_MAX_BYTES: u64 = MAX_SHARED_FILE_BYTES;
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
pub struct LocalFile {
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
    pub bytes: u64,
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
pub struct RoomSession {
    node: Node,
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
    forwarders: StdMutex<HashMap<[u8; SHORT_ID_LEN], PipeForwarder>>,
    seen: StdMutex<BTreeSet<EventId>>,
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
pub struct RoomSupervisor {
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

impl RoomSupervisor {
    /// Create the supervisor (and the data dir, owner-only).
    pub fn new(data_dir: PathBuf, loopback: bool) -> CoreResult<Self> {
        crate::identity::ensure_dir(&data_dir)?;
        Ok(Self {
            data_dir,
            loopback,
            sessions: StdMutex::new(HashMap::new()),
            structural: TokioMutex::new(()),
            snapshot_cache: StdMutex::new(HashMap::new()),
        })
    }

    /// Brief lock over the session map. Held only for a map operation, never
    /// across an `.await`.
    fn sessions(&self) -> MutexGuard<'_, HashMap<RoomId, Arc<RoomSession>>> {
        self.sessions.lock().expect("sessions mutex poisoned")
    }

    /// The resolved data directory.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The daemon network mode string for `daemon.status`.
    #[must_use]
    pub fn mode(&self) -> &'static str {
        if self.loopback {
            "loopback"
        } else {
            "real"
        }
    }

    /// Room ids of all open sessions (protocol string form).
    #[must_use]
    pub fn open_rooms(&self) -> Vec<String> {
        self.sessions().keys().map(ToString::to_string).collect()
    }

    /// Room ids of all open sessions (typed, for the push loop).
    #[must_use]
    pub fn open_room_ids(&self) -> Vec<RoomId> {
        self.sessions().keys().copied().collect()
    }

    /// The `daemon.status` endpoint object (first open session), or `None`
    /// when no room is open — the daemon has no live node of its own.
    #[must_use]
    pub fn status_endpoint(&self) -> Option<Value> {
        let session = self.sessions().values().next().cloned()?;
        let node = &session.node;
        Some(json!({
            "endpoint_id": node.id().to_string(),
            "addr": dialable_addr(node),
            "relay_url": node.relay_url(),
        }))
    }

    // ------------------------------------------------------------------
    // Shared plumbing
    // ------------------------------------------------------------------

    fn db_path(&self) -> PathBuf {
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

    fn open_store(&self) -> CoreResult<EventStore> {
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

    fn secrets(&self) -> CoreResult<SecretKeys> {
        SecretKeys::load(&self.data_dir)
    }

    /// A cloned handle to an open room's session (an `Arc`), or `RoomNotOpen`.
    /// The map lock is released before the caller does any network work.
    fn session(&self, room_id: &RoomId) -> CoreResult<Arc<RoomSession>> {
        self.sessions().get(room_id).cloned().ok_or_else(|| {
            CoreError::new(
                ErrorKind::RoomNotOpen,
                format!("room {room_id} is not open"),
            )
        })
    }

    /// A cloned handle to an open room's session, if any.
    fn session_opt(&self, room_id: &RoomId) -> Option<Arc<RoomSession>> {
        self.sessions().get(room_id).cloned()
    }

    /// Whether a room currently has an open session.
    fn is_open(&self, room_id: &RoomId) -> bool {
        self.sessions().contains_key(room_id)
    }

    /// Re-fold a room's persisted log (re-validating every stored event), the
    /// same projection the reference CLI's `fold_room` builds.
    fn fold(
        &self,
        store: &EventStore,
        room_id: &RoomId,
    ) -> CoreResult<(RoomMembership, MembershipSnapshot)> {
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
    async fn snapshot_for(&self, room_id: &RoomId) -> CoreResult<MembershipSnapshot> {
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

    /// Spawn the managed room session node (the CLI `room tail` pattern):
    /// live `SnapshotAdmission` refreshed by the pump, join bootstrap hosted
    /// while we are the room owner, blob serving from the room's store, and
    /// the room's persisted peer hints as the dial set.
    async fn spawn_node(&self, room_id: &RoomId) -> CoreResult<(Node, Arc<AtomicBool>, bool)> {
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let store = self.open_store()?;
        let (_, snapshot) = self.fold(&store, room_id)?;
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
        let secret_key = SecretKey::from_bytes(&secret.device.to_seed());
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
    /// open time).
    fn make_session(
        node: Node,
        accept_joins: Arc<AtomicBool>,
        is_owner: bool,
        seen: BTreeSet<EventId>,
    ) -> Arc<RoomSession> {
        let conn_rx = node.conn_events();
        let room_rx = node.room_events();
        Arc::new(RoomSession {
            node,
            conn_rx: StdMutex::new(conn_rx),
            room_rx: Arc::new(TokioMutex::new(room_rx)),
            forwarders: StdMutex::new(HashMap::new()),
            seen: StdMutex::new(seen),
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
        for (_, forwarder) in session
            .forwarders
            .lock()
            .expect("forwarders poisoned")
            .drain()
        {
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
    pub fn create_room(&self, name: &str) -> CoreResult<String> {
        validate_room_name(name)?;
        let secret = self.secrets()?;

        let mut room_nonce = [0u8; ROOM_NONCE_LEN];
        getrandom::fill(&mut room_nonce).map_err(|e| internal("OS CSPRNG unavailable", e))?;
        let created_at = now_ms();
        let sender_id = secret.identity.identity_key();
        let room_id = derive_room_id(&sender_id, &room_nonce, created_at);

        let wire = build_room_created(
            &secret.identity,
            &secret.device,
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

        let mut store = self.open_store()?;
        store
            .insert(&validated)
            .map_err(|e| internal("could not persist the room genesis", e))?;
        localstate::remember_room(&self.data_dir, &room_id.to_string(), Some(name))?;
        Ok(room_id.to_string())
    }

    /// `room.list`: every locally known room with name/role/member count/open.
    pub async fn list_rooms(&self) -> CoreResult<Vec<Value>> {
        if !self.db_path().exists() {
            return Ok(Vec::new());
        }
        let self_key = crate::identity::load_profile(&self.data_dir)?
            .map(|p| p.identity_id)
            .and_then(|id| id.parse::<IdentityKey>().ok());
        // Sync scope: gather each room's id + display name, then DROP the store
        // before any `.await` (the `!Sync` store must not be held across the
        // `snapshot_for` awaits below, or this future would not be `Send`).
        let room_meta: Vec<(RoomId, Option<String>)> = {
            let store = self.open_store()?;
            let room_ids = store
                .room_ids()
                .map_err(|e| internal("could not enumerate rooms", e))?;
            room_ids
                .into_iter()
                .map(|room_id| {
                    let name = genesis_name(&store, &room_id)
                        .or_else(|| localstate::local_name(&self.data_dir, &room_id.to_string()));
                    (room_id, name)
                })
                .collect()
        };
        let mut rooms = Vec::with_capacity(room_meta.len());
        for (room_id, name) in room_meta {
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
            let self_member = self_key
                .as_ref()
                .and_then(|key| snapshot.members().find(|member| &member.identity == key));
            if self_key.is_some() && self_member.is_none() {
                continue;
            }
            let role = self_key
                .as_ref()
                .and_then(|key| snapshot.role(key))
                .map(role_label);
            let status = if let Some(key) = self_key.as_ref() {
                let store = self.open_store()?;
                let (removed_ids, left_ids) = departure_sets(&store, &room_id)?;
                self_member.map(|member| status_label(member.status, key, &removed_ids, &left_ids))
            } else {
                None
            };
            rooms.push(json!({
                "room_id": room_id.to_string(),
                "name": name,
                "role": role,
                "status": status,
                "member_count": snapshot.members().count(),
                "open": self.is_open(&room_id),
            }));
        }
        Ok(rooms)
    }

    /// `room.open`: spawn the room's node session and return the endpoint the
    /// inviter shares, the member roster, and the folded timeline. Optional
    /// `peers` (`"<endpoint_id>@<ip:port>"`) merge into the room's persisted
    /// dial hints before the spawn (loopback mode has no discovery).
    pub async fn open_room(&self, room_id_str: &str, peers: &[String]) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        if !peers.is_empty() {
            parse_peers(peers)?; // validate before persisting
            localstate::add_peer_hints(&self.data_dir, &room_id.to_string(), peers)?;
        }
        // Serialize node spawn/teardown so two structural flows never race the
        // room's exclusive blob-store lock.
        let _structural = self.structural.lock().await;
        if !self.is_open(&room_id) {
            let (node, accept_joins, is_owner) = self.spawn_node(&room_id).await?;
            // Seed the push dedupe set with the full history the caller receives
            // here, BEFORE the session is visible to the push loop, so it never
            // re-emits history and never races an un-seeded window.
            let rows = node
                .room_tail(u32::MAX)
                .await
                .map_err(|e| internal("could not read the timeline", e))?;
            let seen: BTreeSet<EventId> = rows.iter().map(|se| se.event_id).collect();
            let session = Self::make_session(node, accept_joins, is_owner, seen);
            self.sessions().insert(room_id, session);
        }

        // Keep `_structural` held through the reads below so a concurrent
        // close/share cannot pull the session we just resolved. These reads are
        // all fast and bounded; the message/fetch/pipe/push paths never take
        // this lock, so nothing daemon-wide is blocked.
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
    pub async fn close_room(&self, room_id_str: &str) -> CoreResult<()> {
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

    /// `room.leave`: publish a signed `member.left` for this identity, then
    /// close this daemon's local live session if one is open. The immutable room
    /// owner cannot leave yet: the protocol has no ownership transfer, and an
    /// owner-authored `member.left` would not remove the genesis admin anyway.
    pub async fn leave_room(&self, room_id_str: &str) -> CoreResult<String> {
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
                &secret.device,
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
            let admin_identity = snapshot
                .admin()
                .copied()
                .ok_or_else(|| CoreError::internal("room snapshot has no admin"))?;
            let heads = Self::authorization_class_heads(&store, &room_id, &admin_identity)?;
            let wire = build_member_left(
                &secret.identity,
                &secret.device,
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

    /// `room.timeline`: chronological `TimelineEvent`s from the local log
    /// (an offline read — works whether or not the room is open; a second
    /// read handle on the WAL-mode store sees the engine's committed writes).
    pub async fn timeline(&self, room_id_str: &str, limit: Option<u32>) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        // Sender roles come from the fast membership snapshot (live snapshot for
        // an open room, cached fold for a closed one) — NOT a full O(history)
        // re-fold of the whole log on every timeline read. This also yields
        // `RoomUnknown` for a room with no stored events, exactly like `fold`.
        let snapshot = self.snapshot_for(&room_id).await?;
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
    pub async fn members(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        let snapshot = self.snapshot_for(&room_id).await?;
        let store = self.open_store()?;
        let (removed_ids, left_ids) = departure_sets(&store, &room_id)?;
        Ok(snapshot
            .members()
            .map(|m| {
                json!({
                    "identity_id": m.identity.to_string(),
                    "role": role_label(m.role),
                    "status": status_label(m.status, &m.identity, &removed_ids, &left_ids),
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
    pub async fn create_invite(
        &self,
        room_id_str: &str,
        invitee_hex: &str,
        role: &str,
        expiry: Option<&str>,
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
        let expires_at = expiry
            .map(|spec| parse_expiry(spec, created_at))
            .transpose()?;

        let is_open = self.is_open(&room_id);
        // The whole store-backed authoring path lives in one sync scope so no
        // !Sync store borrow crosses the publish await below.
        let wire = {
            let mut store = self.open_store()?;
            let (mut membership, snapshot) = self.fold(&store, &room_id)?;
            if snapshot.admin() != Some(&admin_identity) {
                return Err(CoreError::new(
                    ErrorKind::NotAMember,
                    format!("only the room owner can issue invites for {room_id}"),
                ));
            }
            let heads = Self::authorization_class_heads(&store, &room_id, &admin_identity)?;

            let wire = build_member_invited(
                &secret.identity,
                &secret.device,
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
            wire
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
            discovery: vec![secret.device.device_key()],
        };
        Ok(ticket.to_string())
    }

    /// `room.join`: redeem a ticket — bootstrap the membership sub-DAG from
    /// the admin over an ephemeral node, author + fold-check + publish the
    /// `member.joined`, and record the room locally (mirrors the CLI join).
    pub async fn join_room(
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
        let secret_key = SecretKey::from_bytes(&secret.device.to_seed());
        let node = Node::spawn(
            secret_key,
            Arc::new(admission),
            Arc::new(TracingAudit),
            engine,
            self.net_config(),
            DEFAULT_TICK,
        )
        .await
        .map_err(|e| internal("could not bring up the join node", e))?;
        for addr in dial_set {
            node.connect_to(addr);
        }

        let outcome = self
            .bootstrap_and_join(&node, &secret, &ticket, display_name)
            .await;
        let shutdown = node.shutdown().await;
        let joined = outcome?;
        shutdown.map_err(|e| internal("could not shut the join node down", e))?;

        // Local room index: the join-time name override wins; else the pulled
        // genesis name resolves on read. The supplied peer addresses persist
        // as the room's dial hints so `room.open` can reach the inviter.
        localstate::remember_room(&self.data_dir, &room_id.to_string(), display_name)?;
        localstate::add_peer_hints(&self.data_dir, &room_id.to_string(), peers)?;
        Ok(joined.to_string())
    }

    /// The post-bring-up half of the join (split so the node always shuts
    /// down): wait invited, build + fold-check + publish, confirm active.
    async fn bootstrap_and_join(
        &self,
        node: &Node,
        secret: &SecretKeys,
        ticket: &RoomInviteTicket,
        display_name: Option<&str>,
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
        let binding = DeviceBinding::create(&room_id, &secret.identity, secret.device.device_key());
        let wire = build_member_joined(
            &secret.identity,
            &secret.device,
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
    pub async fn send_message(&self, room_id_str: &str, body: &str) -> CoreResult<String> {
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
        let heads = Self::node_heads(&session.node).await?;
        let wire = build_message_text(
            &secret.identity,
            &secret.device,
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
    pub async fn post_status(
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
        let heads = Self::node_heads(&session.node).await?;
        let wire = build_agent_status(
            &secret.identity,
            &secret.device,
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
    pub async fn share_file(
        &self,
        room_id_str: &str,
        path_str: &str,
        name: Option<&str>,
        mime: Option<&str>,
    ) -> CoreResult<Value> {
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

        // Import into the room's durable blob store on the LIVE session (issue
        // #84): the node reuses the store handle it already owns, so there is no
        // second FsStore open (no lock contention), no session cycle, and no
        // endpoint rebind — the dial address stays valid and peers see no churn.
        let import = session
            .node
            .blob_import(&import_path)
            .await
            .map_err(|e| internal("could not import the file into the blob store", e))?;

        let mut file_id = [0u8; SHORT_ID_LEN];
        getrandom::fill(&mut file_id).map_err(|e| internal("OS CSPRNG unavailable", e))?;
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

        let heads = Self::node_heads(&session.node).await?;
        let wire = build_file_shared(
            &secret.identity,
            &secret.device,
            &room_id,
            file_id,
            &display_name,
            &mime_type,
            import.size_bytes,
            iroh_rooms::files::HashRef::from_bytes(import.hash),
            Some("raw"),
            &[secret.device.device_key()],
            &heads,
            now_ms(),
        );
        let event_id = Self::publish_authored(&session.node, &room_id, &wire).await?;
        Ok(json!({
            "file_id": file_handle(&file_id),
            "event_id": bare_event_hex(&event_id),
        }))
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
    pub async fn list_files(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        let store = self.open_store()?;
        // Existence check without the O(history) re-fold (the membership
        // snapshot is unused here — file rows are read directly by type).
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
    pub async fn fetch_file(
        &self,
        room_id_str: &str,
        file_id_str: &str,
        save_dir: Option<&str>,
    ) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        let file_id = parse_file_id(file_id_str)?;
        let session = self.session(&room_id)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();
        let self_device = endpoint_id_of(secret.device.device_key())?;

        // Access check from the fast membership snapshot (live for an open
        // session, cached fold for a closed room) — not an O(history) re-fold.
        let snapshot = self.snapshot_for(&room_id).await?;
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

        let provider_devices: Vec<DeviceKey> = match &shared.providers {
            Some(list) if !list.is_empty() => list.clone(),
            _ => vec![author_device],
        };
        let provider_ids: Vec<EndpointId> = provider_devices
            .iter()
            .filter_map(|dev| endpoint_id_of(*dev).ok())
            .filter(|id| *id != self_device)
            .collect();
        let mut providers: Vec<EndpointAddr> = Vec::with_capacity(provider_ids.len());
        for id in provider_ids {
            providers.push(self.enriched_addr(&session.node, &room_id, id).await);
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
        let mut fetched: Option<Vec<u8>> = None;
        let (mut denied_at_connect, mut attempted) = (0usize, 0usize);
        for provider in &providers {
            let (outcome, data) = session
                .node
                .fetch_file(provider.clone(), declared, declared, FETCH_TIMEOUT)
                .await;
            // The outcome enum is matched by name via its Debug string-free
            // variants (the facade re-exports FetchOutcome).
            use iroh_rooms::experimental::blob::FetchOutcome as O;
            match outcome {
                O::Fetched => {
                    fetched = data.map(|b| b.to_vec());
                    break;
                }
                O::HashMismatch => {
                    return Err(CoreError::new(
                        ErrorKind::HashMismatch,
                        format!(
                            "integrity check FAILED: fetched bytes do not hash to the declared \
                             {}; refusing to save",
                            shared.blob_hash
                        ),
                    ));
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
        let Some(data) = fetched else {
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

        // Save atomically under save_dir (default <data-dir>/downloads),
        // never overwriting an existing file.
        let dir = save_dir.map_or_else(|| self.data_dir.join(DOWNLOADS_DIR), PathBuf::from);
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

        Ok(json!({
            "path": target.display().to_string(),
            "bytes": data.len(),
            "verified": true,
        }))
    }

    /// A previously verified local copy addressed by protocol identifiers, never
    /// by a browser-supplied filesystem path.
    pub fn local_file(&self, room_id_str: &str, file_id_str: &str) -> CoreResult<LocalFile> {
        let room_id = parse_room_id(room_id_str)?;
        let file_id = parse_file_id(file_id_str)?;
        let store = self.open_store()?;
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
        let Some((shared, _)) = find_file_shared(&events, file_id) else {
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!("no such file {file_id_str} in room {room_id}"),
            ));
        };
        let file_id_handle = file_handle(&file_id);
        let room_id_key = room_id.to_string();
        let Some(local) = localstate::fetched_file(&self.data_dir, &room_id_key, &file_id_handle)
            .or_else(|| self.downloaded_file_meta(&file_id, &shared.name, shared.size_bytes))
        else {
            return Err(CoreError::new(
                ErrorKind::FileUnavailable,
                format!("file {file_id_str} has not been fetched on this daemon"),
            )
            .with_hint("fetch the file first, then open the local copy"));
        };
        Ok(LocalFile {
            path: local.path,
            name: shared.name,
            mime: shared.mime_type,
            bytes: local.bytes,
        })
    }

    // ------------------------------------------------------------------
    // Pipes
    // ------------------------------------------------------------------

    /// `pipe.expose`: announce + serve a loopback TCP target to exactly one
    /// authorized peer (the runtime rule) through the open session's node.
    pub async fn pipe_expose(
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
        let pipe_id = session
            .node
            .pipe_expose(
                &secret.identity,
                &secret.device,
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

    /// `pipe.list`: the room's pipes from the local log, with open/closed
    /// state and whether this daemon currently forwards or serves them.
    pub fn pipe_list(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        let store = self.open_store()?;
        // Existence check without the O(history) re-fold (the membership
        // snapshot is unused here — pipe rows are read directly from the log).
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
                    .contains_key(&p.pipe_id)
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

    /// `pipe.connect`: bind a local loopback forwarder toward the pipe owner
    /// and keep it alive inside the session. Returns the local address.
    pub async fn pipe_connect(&self, room_id_str: &str, pipe_id_hex: &str) -> CoreResult<String> {
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
        if let Some(old) = session
            .forwarders
            .lock()
            .expect("forwarders poisoned")
            .insert(pipe_id, forwarder)
        {
            old.shutdown();
        }
        Ok(local_addr)
    }

    /// `pipe.close`: publish a signed `pipe.closed` (owner or room owner) and
    /// tear down any local forwarder.
    pub async fn pipe_close(&self, room_id_str: &str, pipe_id_hex: &str) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        let pipe_id = parse_pipe_id(pipe_id_hex)?;
        let secret = self.secrets()?;
        let self_id = secret.identity.identity_key();

        // Access check from the fast membership snapshot (live for an open
        // session, cached fold for a closed room) — not an O(history) re-fold.
        let snapshot = self.snapshot_for(&room_id).await?;
        // Sync scope: no !Sync store borrow crosses the pipe_close await.
        {
            let store = self.open_store()?;
            let opened = open_pipe(&store, &room_id, pipe_id)?;
            let is_admin = snapshot.admin() == Some(&self_id);
            let is_owner = opened.as_ref().is_some_and(|o| o.owner_id == self_id);
            if opened.is_none() {
                return Err(CoreError::invalid(format!(
                    "no pipe {pipe_id_hex} known in room {room_id}"
                )));
            }
            if !is_admin && !is_owner {
                return Err(CoreError::new(
                    ErrorKind::PipeDenied,
                    "only the pipe owner or the room owner can close a pipe",
                ));
            }
        }

        let session = self.session(&room_id)?;
        session
            .node
            .pipe_close(
                &secret.identity,
                &secret.device,
                &room_id,
                pipe_id,
                Some("closed"),
                now_ms(),
            )
            .await
            .map_err(|e| internal("could not publish pipe.closed", e))?;

        if let Some(forwarder) = session
            .forwarders
            .lock()
            .expect("forwarders poisoned")
            .remove(&pipe_id)
        {
            forwarder.shutdown();
        }
        let event_id = self
            .find_pipe_event(&room_id, EventType::PipeClosed, pipe_id)
            .await?;
        Ok(json!({ "event_id": event_id }))
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
    pub async fn peers_status(&self, room_id_str: &str) -> CoreResult<Vec<Value>> {
        let room_id = parse_room_id(room_id_str)?;
        let session = self.session(&room_id)?;
        Ok(Self::peers_of(&session.node).await)
    }

    /// The `PeerStatus` list for one live node.
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
    /// Since #83 the primary, sub-second push path is [`Self::recv_room_events`]
    /// (the node's `room_events` broadcast); this poll stays as the reconcile
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
    pub async fn poll_new_events(&self, room_id: &RoomId) -> CoreResult<Vec<Value>> {
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

    /// Primary push path (issue #83): await the next batch of live room events
    /// from the node's `room_events` broadcast, materialized for the daemon to
    /// fan out as `room.event` with sub-second latency.
    ///
    /// Blocks on the broadcast until at least one event arrives, then drains
    /// every immediately-ready event. Each is deduped against the shared `seen`
    /// set (exactly-once — the broadcast can coincide with the open-time
    /// snapshot and with the reconcile poll, which share this set). The
    /// broadcast is LOSSY: on `Lagged` — the receiver fell behind and the SDK
    /// dropped events — this resyncs from the full causal tail exactly as the
    /// reconcile poll does, so no ingested event is ever missed. Returns
    /// `RoomNotOpen` once the session's broadcast closes (`room.close`), so the
    /// caller's per-room pump loop exits cleanly.
    pub async fn recv_room_events(&self, room_id: &RoomId) -> CoreResult<Vec<Value>> {
        // Clone ONLY the broadcast-receiver handle, not the session. Parking on
        // `recv().await` below must never pin the session `Arc`: `room.close`'s
        // `reclaim_session` waits for `Arc::try_unwrap` of the session, and a
        // quiet room emits no event to unpark us — so a session clone held here
        // would deadlock close daemon-wide. This independent `Arc` keeps the
        // receiver's cursor across pump iterations without keeping the node
        // alive; when close shuts the node down, the broadcast closes and the
        // parked `recv` wakes with `Closed` (handled below as `RoomNotOpen`).
        let room_rx = self.session(room_id)?.room_rx.clone();

        // Await the first event; a lagged/closed receiver short-circuits.
        let mut lagged;
        let mut batch: Vec<StoredEvent> = Vec::new();
        {
            let mut rx = room_rx.lock().await;
            match rx.recv().await {
                Ok(ev) => {
                    lagged = false;
                    batch.push(ev);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => lagged = true,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CoreError::new(
                        ErrorKind::RoomNotOpen,
                        format!("room {room_id} closed"),
                    ));
                }
            }
            // Drain any further already-ready events in the same wake-up.
            loop {
                match rx.try_recv() {
                    Ok(ev) => batch.push(ev),
                    Err(broadcast::error::TryRecvError::Lagged(_)) => lagged = true,
                    Err(broadcast::error::TryRecvError::Empty)
                    | Err(broadcast::error::TryRecvError::Closed) => break,
                }
            }
        }

        // Re-resolve the session now that we have events to materialize. If the
        // room was closed while we were parked, this returns `RoomNotOpen` and
        // the pump exits cleanly — the same signal a `Closed` broadcast gives.
        // This clone is short-lived and bounded (a snapshot/tail read), so it
        // never blocks `reclaim_session` the way a clone held across the park
        // would.
        let session = self.session(room_id)?;
        let snapshot = session
            .node
            .snapshot()
            .await
            .map_err(|e| internal("could not read the membership snapshot", e))?;

        // On lag the batch is incomplete, so recover from the authoritative
        // full tail — a superset of anything the batch held — deduped against
        // `seen`, exactly as the reconcile poll recovers.
        if lagged {
            let rows = session
                .node
                .room_tail(u32::MAX)
                .await
                .map_err(|e| internal("could not read the timeline", e))?;
            batch = rows;
        }

        let mut seen = session.seen.lock().expect("seen poisoned");
        let mut out = Vec::new();
        for se in &batch {
            if seen.insert(se.event_id) {
                if let Some(v) = materializer::materialize(se, &snapshot) {
                    out.push(v);
                }
            }
        }
        Ok(out)
    }

    /// Drain the session's `conn_events` broadcast; `true` if any peer
    /// connection transition happened since the last drain.
    pub fn drain_conn_changes(&self, room_id: &RoomId) -> bool {
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
    /// room (the local store's rooms ∪ the localstate index), open or not.
    ///
    /// A **pure read**: it authors nothing, opens no room, and invents no
    /// count. Every number derives from folded stored events plus live
    /// `PeerConnState` on rooms this daemon has open. Liveness follows the
    /// §1.2 decision table via [`fleet::derive_liveness`] — in particular a
    /// `working` latest status with no connected peer reports `stale`, never
    /// `working`, and a room without an open session can never read
    /// `online-idle`/`working` (no live peer state exists to support it).
    pub async fn agents_fleet(&self) -> CoreResult<Value> {
        let now = crate::now_ms();
        // Known rooms: everything in the event store plus the localstate
        // index (a room remembered before/without any synced events still
        // counts toward rooms_total — it is honestly known, just unreadable).
        let mut known: BTreeSet<String> = localstate::load(&self.data_dir)?
            .rooms
            .keys()
            .cloned()
            .collect();

        // Phase 1 (sync): enumerate the store's rooms and read each room's
        // display name. The `!Sync` store is opened, fully used, and DROPPED
        // inside this scope — it must not be held across the `snapshot_for`
        // awaits in phase 2, or this future would not be `Send`. Neither the
        // membership snapshots nor the timeline rows are taken here: for open
        // rooms the snapshot comes from the live engine (async), and taking it
        // via `snapshot_for` in phase 2 is exactly what retires the
        // O(full-history) re-fold. The rows are likewise deferred to phase 2 so
        // they are read *after* the snapshot (see the read-order note there).
        let scans: Vec<RoomScan> = if self.db_path().exists() {
            let store = self.open_store()?;
            for id in store
                .room_ids()
                .map_err(|e| internal("could not enumerate rooms", e))?
            {
                known.insert(id.to_string());
            }
            let mut scans = Vec::new();
            for room_str in &known {
                let Ok(room_id) = room_str.parse::<RoomId>() else {
                    continue;
                };
                let room_name = genesis_name(&store, &room_id)
                    .or_else(|| localstate::local_name(&self.data_dir, room_str));
                scans.push(RoomScan {
                    room_id,
                    room_str: room_str.clone(),
                    room_name,
                });
            }
            scans
        } else {
            Vec::new()
        };
        let rooms_total = known.len();
        let mut rooms_covered = 0usize;
        let mut agents: BTreeMap<String, FleetAgentAgg> = BTreeMap::new();

        // Phase 2 (async): fold-free membership via `snapshot_for` (O(1) for
        // open rooms, cached for closed rooms), aggregated over each room's
        // timeline rows. A room whose log will not fold (no readable membership)
        // contributes nothing beyond its rooms_total slot — never a guess.
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
                let agent_ids: BTreeSet<IdentityKey> = snapshot
                    .members()
                    .filter(|m| m.role == Role::Agent)
                    .map(|m| m.identity)
                    .collect();
                if agent_ids.is_empty() {
                    continue;
                }
                rooms_covered += 1;
                let room_name = scan.room_name.clone();
                let rows = {
                    let store = self.open_store()?;
                    store
                        .room_tail(&room_id, u32::MAX)
                        .map_err(|e| internal("could not read the timeline", e))?
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
    pub fn agent_history(
        &self,
        room_id_str: &str,
        identity_hex: &str,
        limit: Option<u32>,
    ) -> CoreResult<Value> {
        let room_id = parse_room_id(room_id_str)?;
        let identity: IdentityKey = identity_hex.trim().parse().map_err(|e| {
            CoreError::invalid(format!("invalid identity_id (expected 64-char hex): {e}"))
        })?;
        let store = self.open_store()?;
        // Existence check only — this read needs no folded membership (it
        // decodes each row independently below), so surface RoomUnknown via the
        // cheap `count` (a single `SELECT COUNT(*)`, no crypto) rather than a
        // full O(full-history) signature-verifying `fold`. `fold` returns
        // RoomUnknown exactly when the room has no stored events, i.e. count 0.
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

/// A closed-over, store-free view of one room for `agents.fleet`'s async
/// aggregation phase: its id, its protocol-string key, and its display name.
/// Collected while the `!Sync` store is briefly open (phase 1) so the store is
/// dropped before any `snapshot_for` await (phase 2). Deliberately does NOT
/// carry the timeline rows: those are read per room in phase 2, *after* the
/// membership snapshot, so the rows are never older than the snapshot they are
/// aggregated against (see the read-order note in `agents_fleet`).
struct RoomScan {
    room_id: RoomId,
    room_str: String,
    room_name: Option<String>,
}

/// One agent's per-room evidence for the fleet read: its known device keys
/// (from `member.joined` bindings and authored events), its newest
/// `agent_status`, and the ts of its newest event of any kind. All fields
/// derive from stored events — nothing is synthesized.
#[derive(Default)]
struct AgentRoomSignals {
    devices: BTreeSet<DeviceKey>,
    latest: Option<LatestStatus>,
    last_seen_ts: Option<u64>,
}

/// The newest `agent_status` posted by an agent (per room).
struct LatestStatus {
    ts: u64,
    label: String,
    message: Option<String>,
    progress: Option<u64>,
}

/// One agent's cross-room aggregate for `agents.fleet`.
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
fn endpoint_id_of(dev: DeviceKey) -> CoreResult<EndpointId> {
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
fn genesis_name(store: &EventStore, room_id: &RoomId) -> Option<String> {
    let genesis = store.by_type(room_id, EventType::RoomCreated).ok()?;
    let stored = genesis.into_iter().next()?;
    let event = SignedEvent::decode(&stored.wire.signed).ok()?;
    match event.content {
        Content::RoomCreated(c) => Some(c.room_name),
        _ => None,
    }
}

/// Log-derived departure sets (`member.removed` subjects, `member.left`
/// subjects) backing the display-status refinement.
fn departure_sets(
    store: &EventStore,
    room_id: &RoomId,
) -> CoreResult<(BTreeSet<IdentityKey>, BTreeSet<IdentityKey>)> {
    let mut removed_ids = BTreeSet::new();
    for se in store
        .by_type(room_id, EventType::MemberRemoved)
        .map_err(|e| internal("could not read member.removed events", e))?
    {
        if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
            if let Content::MemberRemoved(c) = ev.content {
                removed_ids.insert(c.member_id);
            }
        }
    }
    let mut left_ids = BTreeSet::new();
    for se in store
        .by_type(room_id, EventType::MemberLeft)
        .map_err(|e| internal("could not read member.left events", e))?
    {
        if let Ok(ev) = SignedEvent::decode(&se.wire.signed) {
            if let Content::MemberLeft(c) = ev.content {
                left_ids.insert(c.member_id);
            }
        }
    }
    Ok((removed_ids, left_ids))
}

/// `active | invited | removed | left` (the CLI's D5 display refinement: an
/// admin removal dominates a concurrent self-leave).
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
    use super::{
        parse_expiry, parse_file_id, parse_pipe_id, sanitize_name, validate_room_name, Content,
        EventType, RoomSupervisor,
    };
    use crate::error::ErrorKind;
    use iroh_rooms::events::{validate_wire_bytes, ValidationContext, WireEvent};
    use iroh_rooms::identity::{DeviceBinding, SigningKey};
    use iroh_rooms::room::{RoomId, RoomInviteTicket};
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
        let history = sup.agent_history(&room_id, &agent_hex, None).unwrap();
        let points = history["points"].as_array().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["label"], "working");
        assert_eq!(points[0]["progress"], 40);
        assert_eq!(points[0]["ts"], t1);
        assert_eq!(points[1]["label"], "idle");
        assert!(points[1]["progress"].is_null());
        let limited = sup.agent_history(&room_id, &agent_hex, Some(1)).unwrap();
        assert_eq!(limited["points"].as_array().unwrap().len(), 1);
        assert_eq!(limited["points"][0]["label"], "idle");

        // A member with no statuses returns an empty (not fabricated) series.
        let owner_hex = crate::identity::load_profile(dir.path())
            .unwrap()
            .unwrap()
            .identity_id;
        let empty = sup.agent_history(&room_id, &owner_hex, None).unwrap();
        assert_eq!(empty["points"].as_array().unwrap().len(), 0);

        // Error taxonomy: unknown room, malformed identity.
        let unknown = format!("blake3:{}", "ee".repeat(32));
        let err = sup.agent_history(&unknown, &agent_hex, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::RoomUnknown);
        let err = sup.agent_history(&room_id, "not-hex", None).unwrap_err();
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

        let timeline = sup.timeline(&room_id, None).await.unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0]["kind"], "room_created");

        let members = sup.members(&room_id).await.unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["role"], "owner");
        assert_eq!(members[0]["status"], "active");
    }

    #[tokio::test]
    async fn list_rooms_excludes_rooms_this_identity_is_not_a_member_of() {
        // Regression: a foreign room's membership sub-DAG can be backfilled into
        // our store by a shared peer's sync (that peer is in a room WITH us and
        // also in this OTHER room), even though we were never invited. Such a
        // room must not appear in `room.list` — listing it leaks a room we are
        // not in and hands the UI a room whose every `room.open` returns
        // `not_a_member` (and then `message.send` returns `room_not_open`).
        let dir = tempdir().unwrap();
        crate::identity::create(dir.path()).unwrap();
        let sup = RoomSupervisor::new(dir.path().to_path_buf(), true).unwrap();

        // A room WE own — the control that must still list.
        let mine = sup.create_room("Mine").unwrap();

        // A room authored entirely by a STRANGER, persisted straight into our
        // store the way a sync backfill lands it. We author no event in it.
        let stranger_id = SigningKey::generate();
        let stranger_dev = SigningKey::generate();
        let nonce = [0x11; super::ROOM_NONCE_LEN];
        let created_at = crate::now_ms();
        let foreign_room_id =
            super::derive_room_id(&stranger_id.identity_key(), &nonce, created_at);
        let genesis =
            super::build_room_created(&stranger_id, &stranger_dev, "Not Yours", &nonce, created_at);
        insert_wire(&sup, &foreign_room_id, &genesis);

        // Sanity: the foreign room's genesis really is in our store.
        {
            let store = sup.open_store().unwrap();
            assert!(store.count(&foreign_room_id).unwrap() >= 1);
        }

        let rooms = sup.list_rooms().await.unwrap();
        let ids: Vec<&str> = rooms.iter().filter_map(|r| r["room_id"].as_str()).collect();
        assert!(ids.contains(&mine.as_str()), "our own room must list");
        assert!(
            !ids.contains(&foreign_room_id.to_string().as_str()),
            "a room we are not a member of must be excluded from room.list"
        );
        assert_eq!(rooms.len(), 1, "only our own room lists; got {rooms:?}");

        // And the honest failure still stands if the UI somehow targets it.
        let err = sup
            .open_room(&foreign_room_id.to_string(), &[])
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotAMember);
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
        let file_id = shared["file_id"].as_str().unwrap().to_owned();

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
        let file_id = shared["file_id"].as_str().unwrap().to_owned();

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
        assert_eq!(fetched["verified"], true);

        member.close_room(&room_id).await.unwrap();
        let member_restarted = RoomSupervisor::new(member_dir.path().to_path_buf(), true).unwrap();
        let files = member_restarted.list_files(&room_id).await.unwrap();
        let row = files
            .iter()
            .find(|f| f["file_id"].as_str() == Some(file_id.as_str()))
            .expect("shared file remains listed after restart");
        assert_eq!(row["fetched"], true);
        assert_eq!(row["local_bytes"], 14);
        assert_eq!(row["local_path"], fetched["path"]);

        let local = member_restarted.local_file(&room_id, &file_id).unwrap();
        assert_eq!(
            local.path.display().to_string(),
            fetched["path"].as_str().unwrap()
        );
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
}
