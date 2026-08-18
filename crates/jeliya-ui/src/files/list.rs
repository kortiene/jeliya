//! The file-list model with **truthful, non-inferred** provider availability
//! (spec D2; contract invariant 4; #50/#79/#94).
//!
//! Three facts are kept distinct and each read from its own protocol source:
//! membership/presence (`room.peers`, roster) is **never** consulted here;
//! file-provider availability comes **only** from `FileRow.providers[].link`,
//! `FileRow.fetchable`, and `FileRow.self_hosted`. A reachable provider serves a
//! file regardless of a stale roster display (#50); a `fetchable:false` row is
//! "not currently fetchable", carrying the provider evidence — it is **not**
//! rendered as "the sharer left". Construction takes only a `FileRow`, so no
//! code path can derive availability from presence (the type makes the
//! cross-inference the contract forbids unreachable).
//!
//! Peer-supplied names cross as [`FileName`], parsed fail-closed at ingest (spec
//! D4): a hostile `../../etc/passwd` / `a/b` / control-char name is rejected
//! here (its parsed form becomes `None`), before it can reach an export sink's
//! artifact naming — and the rejection carries no payload.

use jeliya_api::{DeviceId, FileId, FileListOut, FileRow, Link, SubjectId, Truncated};
use jeliya_platform::FileName;

/// One provider device carrying evidence for a file, rendered **verbatim** from
/// `FileRow.providers[].link`. This per-device link is the availability fact
/// (spec D2); it is never merged with roster presence.
#[derive(Clone, PartialEq, Debug)]
pub struct ProviderView {
    /// The provider's subject.
    pub subject_id: SubjectId,
    /// The provider's device.
    pub device_id: DeviceId,
    /// This daemon's link to the provider, or why there is none — the evidence.
    pub link: Link,
}

impl ProviderView {
    /// Whether this provider is reachable over a live link (direct or relay).
    /// Read only from the link fact; a stale roster cannot change it (#50).
    pub fn reachable(&self) -> bool {
        matches!(self.link, Link::Direct { .. } | Link::Relay { .. })
    }
}

/// A file row's truthful state, built from a single [`FileRow`] and nothing else.
#[derive(Clone, PartialEq, Debug)]
pub struct FileRowView {
    /// The file id.
    pub file_id: FileId,
    /// The declared name, parsed fail-closed. `None` when the peer-supplied name
    /// carried path syntax or control characters (spec D4) — the pane then shows
    /// a safe placeholder and suppresses export (a sink needs a valid name).
    pub name: Option<FileName>,
    /// The byte count.
    pub bytes: u64,
    /// A short, non-reversible form of the digest for display (never the full
    /// digest — spec D7).
    pub digest_short: String,
    /// The peer-declared, **untrusted** content type. Always shown *labeled as
    /// declared* and never used to authorize an inline render (spec D6).
    pub declared_content_type: String,
    /// Provider devices carrying evidence, each with its own `link`.
    pub providers: Vec<ProviderView>,
    /// Whether a fetch can be served now — read from the field, never inferred.
    pub fetchable: bool,
    /// Whether this daemon holds the bytes.
    pub self_hosted: bool,
}

/// A short, non-reversible display form of a content digest: the essence after
/// any `algo:` prefix, truncated. Never the full digest (spec D7).
fn short_digest(digest: &str) -> String {
    let essence = digest.rsplit(':').next().unwrap_or(digest);
    essence.chars().take(12).collect()
}

impl FileRowView {
    /// Build the truthful view of one `file.list` row. The name is parsed
    /// fail-closed; providers/fetchable/self_hosted are carried verbatim.
    pub fn from_row(row: FileRow) -> Self {
        Self {
            file_id: row.file_id,
            name: FileName::parse(row.name).ok(),
            bytes: row.bytes,
            digest_short: short_digest(&row.digest),
            declared_content_type: row.declared_content_type,
            providers: row
                .providers
                .into_iter()
                .map(|p| ProviderView {
                    subject_id: p.subject_id,
                    device_id: p.device_id,
                    link: p.link,
                })
                .collect(),
            fetchable: row.fetchable,
            self_hosted: row.self_hosted,
        }
    }

    /// Whether this file can be fetched now, read **only** from `fetchable`
    /// (and, redundantly, whether any provider is reachable). Never derived from
    /// membership/presence (spec D2).
    pub fn can_fetch(&self) -> bool {
        self.fetchable
    }

    /// Whether at least one provider is currently reachable — availability
    /// evidence, computed only from provider links, so a `fetchable:false` row
    /// can still explain *which* providers were tried and why (spec D2).
    pub fn any_provider_reachable(&self) -> bool {
        self.providers.iter().any(ProviderView::reachable)
    }
}

/// The file-list model: the truthful rows plus the one continuation mechanism.
#[derive(Clone, PartialEq, Debug)]
pub struct FileListModel {
    /// The rows, in served order.
    pub rows: Vec<FileRowView>,
    /// Whether more pages exist (`More{cursor}`) or the list is `Complete`.
    pub truncated: Truncated,
}

impl FileListModel {
    /// Build the model from a `file.list` reply.
    pub fn from_out(out: FileListOut) -> Self {
        Self {
            rows: out.files.into_iter().map(FileRowView::from_row).collect(),
            truncated: out.truncated,
        }
    }

    /// Look up a row by id — the selected-item (`RoomDest::Files { item }`)
    /// detail read.
    pub fn row(&self, file_id: &FileId) -> Option<&FileRowView> {
        self.rows.iter().find(|r| &r.file_id == file_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jeliya_api::{LinkReason, PeerRow, Timestamp};

    fn peer(link: Link) -> PeerRow {
        PeerRow {
            subject_id: SubjectId::new("blake3:subject"),
            device_id: DeviceId::new("blake3:device"),
            link,
        }
    }

    fn row(name: &str, fetchable: bool, providers: Vec<PeerRow>) -> FileRow {
        FileRow {
            file_id: FileId::new("blake3:file"),
            name: name.to_string(),
            bytes: 10,
            digest: "blake3:deadbeefcafefeed".to_string(),
            declared_content_type: "text/plain".to_string(),
            shared_by: SubjectId::new("blake3:sharer"),
            shared_at: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            providers,
            fetchable,
            self_hosted: false,
        }
    }

    #[test]
    fn a_hostile_name_fails_closed_at_ingest() {
        let view = FileRowView::from_row(row("../../etc/passwd", true, vec![]));
        assert!(view.name.is_none(), "a path-syntax name must not parse");
        let ok = FileRowView::from_row(row("report.pdf", true, vec![]));
        assert_eq!(ok.name.as_ref().map(FileName::as_str), Some("report.pdf"));
    }

    #[test]
    fn availability_reads_only_the_file_list_fields() {
        // A reachable provider + fetchable → can fetch and is reachable.
        let live = FileRowView::from_row(row(
            "a.txt",
            true,
            vec![peer(Link::Direct {
                since: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            })],
        ));
        assert!(live.can_fetch());
        assert!(live.any_provider_reachable());

        // fetchable:false is honored even with a reachable provider present — the
        // field is authoritative, "not currently fetchable" carries the evidence.
        let not_fetchable = FileRowView::from_row(row(
            "a.txt",
            false,
            vec![peer(Link::Relay {
                since: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            })],
        ));
        assert!(!not_fetchable.can_fetch());
        assert!(not_fetchable.any_provider_reachable());

        // A provider with no link is not reachable evidence.
        let offline = FileRowView::from_row(row(
            "a.txt",
            false,
            vec![peer(Link::NotConnected {
                reason: LinkReason::NoRoute,
            })],
        ));
        assert!(!offline.any_provider_reachable());
    }

    #[test]
    fn digest_short_form_is_truncated_and_prefix_free() {
        let view = FileRowView::from_row(row("a.txt", true, vec![]));
        assert_eq!(view.digest_short, "deadbeefcafe");
        assert!(!view.digest_short.contains(':'));
    }

    #[test]
    fn file_list_model_builds_and_looks_up_by_id() {
        use jeliya_api::{FileListOut, RoomId, Truncated};
        let file_id = FileId::new("blake3:file");
        let model = FileListModel::from_out(FileListOut {
            room_id: RoomId::new("r"),
            files: vec![row("notes.txt", true, vec![])],
            truncated: Truncated::Complete,
        });
        assert_eq!(model.rows.len(), 1);
        assert!(model.row(&file_id).is_some(), "row lookup finds by file_id");
        assert!(
            model.row(&FileId::new("blake3:other")).is_none(),
            "unknown id returns None"
        );
        assert_eq!(model.truncated, Truncated::Complete);
    }

    #[test]
    fn relay_provider_link_is_reachable() {
        // A provider over a Relay link is reachable — availability is not
        // restricted to Direct links (#50; spec D2).
        let relay = FileRowView::from_row(row(
            "a.txt",
            true,
            vec![peer(Link::Relay {
                since: Timestamp::new(time::OffsetDateTime::UNIX_EPOCH),
            })],
        ));
        assert!(relay.any_provider_reachable());
    }

    #[test]
    fn self_hosted_field_is_carried_verbatim() {
        // self_hosted comes only from the protocol field; it is NEVER derived
        // from presence or membership (spec D2).
        let mut hosted = row("a.txt", true, vec![]);
        hosted.self_hosted = true;
        let view = FileRowView::from_row(hosted);
        assert!(view.self_hosted);
        assert!(!FileRowView::from_row(row("b.txt", true, vec![])).self_hosted);
    }
}
