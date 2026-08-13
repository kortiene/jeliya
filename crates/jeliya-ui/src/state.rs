//! UI-facing state derived from [`jeliya_client::ClientHandle`] events (§6.1).
//!
//! [`UiState`] folds [`ClientEvent`]s into a snapshot built from **`jeliya-api`
//! view models** — never raw JSON. The seam already forbids
//! `serde_json::Value` in its public surface, so nothing here re-parses wire
//! bytes: room rows arrive as [`jeliya_api::RoomRow`], lifecycle as
//! [`jeliya_client::State`], and pushes as [`jeliya_client::ClientEvent`].

use jeliya_api::RoomRow;
use jeliya_client::{ClientEvent, State};

/// A UI-facing snapshot the application root renders.
///
/// Constructed with [`UiState::new`] at `State::Idle` and advanced by
/// [`apply_event`](Self::apply_event) (lifecycle transitions) and
/// [`set_rooms`](Self::set_rooms) (the typed `room.list` reply). It holds only
/// view models, so a component reads it without knowing anything about the
/// transport or the wire format.
#[derive(Clone, PartialEq, Debug)]
pub struct UiState {
    /// The observable client lifecycle, folded from
    /// [`ClientEvent::StateChanged`].
    pub lifecycle: State,
    /// The locally known rooms, from the typed `room.list` reply.
    pub rooms: Vec<RoomRow>,
    /// Whether the initial `room.list` reply has arrived. An empty `rooms`
    /// before then means "not answered yet", not "no rooms" — the shell must
    /// not claim an empty account while the read is still in flight.
    pub rooms_loaded: bool,
    /// The most recent local-facing notice (a wire error or a local failure),
    /// or `None`.
    pub notice: Option<String>,
    /// Whether the current [`notice`](Self::notice) is a TERMINAL failure — one
    /// this component will not retry (a wire refusal, `Timeout`, `QueueFull`, a
    /// decode failure). A retryable disconnect is `false`. The shell picks
    /// recovery-promising vs terminal copy from this, so a user is never told
    /// "we'll retry when the connection returns" for an error that never retries.
    pub notice_terminal: bool,
    /// The RAW, secret-scrubbed detail of the most recent failure — the
    /// Diagnostics "last error detail". Unlike [`notice`](Self::notice) (the
    /// primary banner, cleared when a retry recovers the shell) this is RETAINED
    /// across recovery, so opening Diagnostics after a transient disconnect still
    /// shows the failure that occurred instead of "no errors recorded".
    pub last_diagnostic: Option<String>,
}

impl UiState {
    /// A fresh snapshot before the client has started: `Idle`, no rooms, no
    /// notice.
    pub fn new() -> Self {
        Self {
            lifecycle: State::Idle,
            rooms: Vec::new(),
            rooms_loaded: false,
            notice: None,
            notice_terminal: false,
            last_diagnostic: None,
        }
    }

    /// Fold one [`ClientEvent`] into the snapshot. Only lifecycle transitions
    /// change rendered state in this foundation slice; room pushes, gaps, and
    /// resync instructions are surfaced but not yet timeline-merged (the Room
    /// Workbench port is a later M3 slice, not this one). Replies never arrive
    /// here — they are the return value of
    /// [`ClientHandle::call`](jeliya_client::ClientHandle::call).
    pub fn apply_event(&mut self, event: &ClientEvent) {
        if let ClientEvent::StateChanged { to, .. } = event {
            self.lifecycle = *to;
        }
    }

    /// Replace the room list with the typed `room.list` reply's view models.
    pub fn set_rooms(&mut self, rooms: Vec<RoomRow>) {
        self.rooms = rooms;
        self.rooms_loaded = true;
    }

    /// Record a RETRYABLE local-facing notice — a transient disconnect the read
    /// task will retry on the next recovery to `Ready`. The shell may promise
    /// recovery for this.
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        let notice = notice.into();
        self.last_diagnostic = Some(notice.clone());
        self.notice = Some(notice);
        self.notice_terminal = false;
    }

    /// Record a TERMINAL notice — a failure this component will not retry. The
    /// shell must not promise recovery for it.
    pub fn set_terminal_notice(&mut self, notice: impl Into<String>) {
        let notice = notice.into();
        self.last_diagnostic = Some(notice.clone());
        self.notice = Some(notice);
        self.notice_terminal = true;
    }

    /// Clear the primary notice (a successful read recovered the shell) — but
    /// RETAIN [`last_diagnostic`](Self::last_diagnostic), so Diagnostics still
    /// shows the failure that a now-recovered retry cleared from the banner.
    pub fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_terminal = false;
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{GapReason, GapTo, LastEvent, Role, RoomId, RoomRow, Standing};

    fn test_room(id: &str, name: &str) -> RoomRow {
        RoomRow {
            room_id: RoomId::new(id),
            name: name.to_string(),
            standing: Standing::Active,
            live: false,
            role: Role::Member,
            member_count: 1,
            last_event: LastEvent::Absent,
            capabilities: vec![],
        }
    }

    #[test]
    fn lifecycle_folds_from_state_changed() {
        let mut state = UiState::new();
        assert_eq!(state.lifecycle, State::Idle);
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Idle,
            to: State::Connecting,
            coalesced_through_problem: false,
        });
        assert_eq!(state.lifecycle, State::Connecting);
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Connecting,
            to: State::Ready,
            coalesced_through_problem: false,
        });
        assert_eq!(state.lifecycle, State::Ready);
    }

    #[test]
    fn default_equals_new() {
        assert_eq!(UiState::default(), UiState::new());
    }

    #[test]
    fn set_rooms_replaces_the_room_list() {
        let mut state = UiState::new();
        assert!(state.rooms.is_empty());
        state.set_rooms(vec![
            test_room("r-001", "Alpha"),
            test_room("r-002", "Beta"),
        ]);
        assert_eq!(state.rooms.len(), 2);
        assert_eq!(state.rooms[0].room_id, RoomId::new("r-001"));
        // A second call replaces (not appends to) the previous list.
        state.set_rooms(vec![test_room("r-003", "Gamma")]);
        assert_eq!(state.rooms.len(), 1);
        assert_eq!(state.rooms[0].room_id, RoomId::new("r-003"));
    }

    #[test]
    fn set_notice_stores_the_notice() {
        let mut state = UiState::new();
        assert_eq!(state.notice, None);
        state.set_notice("room.list: connection refused");
        assert_eq!(
            state.notice.as_deref(),
            Some("room.list: connection refused")
        );
        // A second call overwrites the first.
        state.set_notice("reconnecting");
        assert_eq!(state.notice.as_deref(), Some("reconnecting"));
    }

    #[test]
    fn a_recovered_retry_keeps_the_last_diagnostic() {
        // The Diagnostics "last error detail" must survive a recovery that clears the
        // primary banner, or opening it after a transient disconnect reports "no
        // errors recorded" even though a failure occurred.
        let mut state = UiState::new();
        state.set_notice("room.list: connection refused");
        assert_eq!(
            state.last_diagnostic.as_deref(),
            Some("room.list: connection refused")
        );
        state.clear_notice();
        assert_eq!(state.notice, None, "the primary banner clears on recovery");
        assert_eq!(
            state.last_diagnostic.as_deref(),
            Some("room.list: connection refused"),
            "the last diagnostic is RETAINED for Diagnostics after recovery"
        );
    }

    #[test]
    fn gap_event_leaves_lifecycle_unchanged() {
        let mut state = UiState::new();
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Idle,
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let before = state.lifecycle;
        state.apply_event(&ClientEvent::Gap {
            room_id: RoomId::new("r-001"),
            from_pos: 0,
            to: GapTo::Open,
            reason: GapReason::Backpressure,
        });
        assert_eq!(state.lifecycle, before);
    }

    #[test]
    fn resync_required_event_leaves_lifecycle_unchanged() {
        let mut state = UiState::new();
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Idle,
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let before = state.lifecycle;
        state.apply_event(&ClientEvent::ResyncRequired {
            room_id: RoomId::new("r-001"),
            from_pos: 42,
        });
        assert_eq!(state.lifecycle, before);
    }

    #[test]
    fn lagged_event_leaves_lifecycle_unchanged() {
        let mut state = UiState::new();
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Idle,
            to: State::Ready,
            coalesced_through_problem: false,
        });
        let before = state.lifecycle;
        state.apply_event(&ClientEvent::Lagged {
            room_id: Some(RoomId::new("r-001")),
            dropped: 7,
        });
        assert_eq!(state.lifecycle, before);
    }
}
