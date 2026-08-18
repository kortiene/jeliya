//! The pipe-list model with **two separately named** reachability facts (spec
//! D2; #94).
//!
//! `PipeRow.link` (the publisher device's reachability, a protocol fact) and
//! `PipeRow.connected` (whether *this* daemon holds a local connection, a
//! runtime fact) are carried as distinct fields and **never merged**. A pipe
//! that is published with nothing connected locally is **Open**; one with a held
//! local connection is **Connected**; publisher reachability is read only from
//! `link`. A revoked pipe is **absent** from the list (dropped on a
//! `pipe_revoked` push / a re-read), never rendered "Closed".

use jeliya_api::{DeviceId, Link, PipeId, PipeListOut, PipeRow, SubjectId, Timestamp, Truncated};

/// One pipe row's truthful state, built from a single [`PipeRow`].
#[derive(Clone, PartialEq, Debug)]
pub struct PipeRowView {
    /// The pipe id.
    pub pipe_id: PipeId,
    /// Who published it.
    pub published_by: SubjectId,
    /// On which device.
    pub device_id: DeviceId,
    /// When it was published.
    pub published_at: Timestamp,
    /// The publisher's reachability — a protocol fact, never merged with
    /// `connected` (spec D2).
    pub link: Link,
    /// Whether this daemon holds a local connection — a runtime fact, never
    /// merged with `link` (spec D2).
    pub connected: bool,
}

impl PipeRowView {
    /// Build the truthful view of one `pipe.list` row.
    pub fn from_row(row: PipeRow) -> Self {
        Self {
            pipe_id: row.pipe_id,
            published_by: row.published_by,
            device_id: row.device_id,
            published_at: row.published_at,
            link: row.link,
            connected: row.connected,
        }
    }

    /// Whether the **publisher** is reachable (direct or relay) — read only from
    /// `link`. A `false` here plus `connected:false` is why a connect attempt can
    /// yield the distinctive `pipe_unreachable` rather than a generic failure.
    pub fn publisher_reachable(&self) -> bool {
        matches!(self.link, Link::Direct { .. } | Link::Relay { .. })
    }

    /// Whether this row was published by `own` — the local subject, when known.
    /// Only an own pipe may be revoked; connect/release apply to any audience
    /// member. `false` when the local subject is unknown (never guessed).
    pub fn is_own(&self, own: Option<&SubjectId>) -> bool {
        own == Some(&self.published_by)
    }
}

/// The pipe-list model: the truthful rows plus the one continuation mechanism.
#[derive(Clone, PartialEq, Debug)]
pub struct PipeListModel {
    /// The rows, in served order (revoked pipes are already absent).
    pub rows: Vec<PipeRowView>,
    /// Whether more pages exist (`More{cursor}`) or the list is `Complete`.
    pub truncated: Truncated,
}

impl PipeListModel {
    /// Build the model from a `pipe.list` reply.
    pub fn from_out(out: PipeListOut) -> Self {
        Self {
            rows: out.pipes.into_iter().map(PipeRowView::from_row).collect(),
            truncated: out.truncated,
        }
    }

    /// Look up a row by id — the selected-item (`RoomDest::Pipes { item }`)
    /// detail read.
    pub fn row(&self, pipe_id: &PipeId) -> Option<&PipeRowView> {
        self.rows.iter().find(|r| &r.pipe_id == pipe_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::LinkReason;

    fn row(connected: bool, link: Link) -> PipeRow {
        PipeRow {
            pipe_id: PipeId::new("blake3:pipe"),
            published_by: SubjectId::new("blake3:owner"),
            device_id: DeviceId::new("blake3:device"),
            published_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            link,
            connected,
        }
    }

    #[test]
    fn link_and_connected_are_independent_facts() {
        // Publisher reachable, nothing connected locally → Open + reachable.
        let open = PipeRowView::from_row(row(
            false,
            Link::Direct {
                since: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            },
        ));
        assert!(!open.connected);
        assert!(open.publisher_reachable());

        // Local connection held even though the publisher link is down: the two
        // facts must not be conflated (a held connection does not imply a live
        // publisher link, nor vice versa).
        let connected_but_link_down = PipeRowView::from_row(row(
            true,
            Link::NotConnected {
                reason: LinkReason::Closed,
            },
        ));
        assert!(connected_but_link_down.connected);
        assert!(!connected_but_link_down.publisher_reachable());
    }

    #[test]
    fn is_own_is_never_guessed() {
        let view = PipeRowView::from_row(row(
            false,
            Link::NotConnected {
                reason: LinkReason::NeverDialed,
            },
        ));
        assert!(!view.is_own(None), "unknown local subject is never own");
        assert!(view.is_own(Some(&SubjectId::new("blake3:owner"))));
        assert!(!view.is_own(Some(&SubjectId::new("blake3:other"))));
    }

    #[test]
    fn pipe_list_model_builds_and_looks_up_by_id() {
        use jeliya_api::{PipeListOut, RoomId, Truncated};
        let pipe_id = PipeId::new("blake3:pipe");
        let model = PipeListModel::from_out(PipeListOut {
            room_id: RoomId::new("r"),
            pipes: vec![row(
                false,
                Link::NotConnected {
                    reason: LinkReason::NeverDialed,
                },
            )],
            truncated: Truncated::Complete,
        });
        assert_eq!(model.rows.len(), 1);
        assert!(model.row(&pipe_id).is_some(), "row lookup finds by pipe_id");
        assert!(
            model.row(&PipeId::new("blake3:other")).is_none(),
            "unknown id returns None"
        );
        assert_eq!(model.truncated, Truncated::Complete);
    }

    #[test]
    fn relay_linked_publisher_is_reachable() {
        // A Relay link counts as reachable — publisher reachability is not
        // restricted to Direct links (spec D2 / #94).
        let relay = PipeRowView::from_row(row(
            false,
            Link::Relay {
                since: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            },
        ));
        assert!(relay.publisher_reachable());
    }
}
