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
    /// The most recent local-facing notice (a wire error or a local failure),
    /// or `None`.
    pub notice: Option<String>,
}

impl UiState {
    /// A fresh snapshot before the client has started: `Idle`, no rooms, no
    /// notice.
    pub fn new() -> Self {
        Self {
            lifecycle: State::Idle,
            rooms: Vec::new(),
            notice: None,
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
    }

    /// Record a local-facing notice (a failed read or a wire error).
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
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
        });
        assert_eq!(state.lifecycle, State::Connecting);
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Connecting,
            to: State::Ready,
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
    fn gap_event_leaves_lifecycle_unchanged() {
        let mut state = UiState::new();
        state.apply_event(&ClientEvent::StateChanged {
            from: State::Idle,
            to: State::Ready,
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
        });
        let before = state.lifecycle;
        state.apply_event(&ClientEvent::Lagged {
            room_id: Some(RoomId::new("r-001")),
            dropped: 7,
        });
        assert_eq!(state.lifecycle, before);
    }
}
