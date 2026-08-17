//! Pure, host-testable room logic for the departed-room read-only archive (#91).
//!
//! This module is deliberately **renderer-free** (no `dioxus::prelude`),
//! mirroring [`crate::shell`] and [`crate::prefs`]: the archive projection, the
//! historical-roster fold, and the capability gate are decided here and
//! host-tested without a renderer, while [`crate::components::room_archive`] is a
//! thin RSX view over them. This keeps the decisions on the MSRV/host jobs and
//! the renderer optional (architecture Decision 3).
//!
//! Structure (#91 §3):
//!
//! - [`archive`] — the [`ArchiveView`] projection and the paged
//!   `room.archive` → `ArchiveView` fold.
//! - [`roster`] — the historical-roster fold over signed membership events
//!   ([`HistoricalMember`]), the roster the departed caller cannot read from
//!   `room.members` (#91 D3).
//! - [`capability`] — the [`grants`](capability::grants) predicate and the
//!   [`FORBIDDEN_IN_ARCHIVE`](capability::FORBIDDEN_IN_ARCHIVE) set that prove
//!   the served capabilities and the archive composition agree (#91 D2).

pub mod archive;
pub mod capability;
pub mod roster;

pub use archive::{ArchiveView, DepartureFact, ARCHIVE_PAGE_LIMIT};
pub use roster::HistoricalMember;
