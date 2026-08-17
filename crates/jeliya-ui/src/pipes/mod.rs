//! Host-testable **Pipes** orchestration (spec §5.C): the pure decisions that
//! drive the `ClientHandle` pipe operations. Target-agnostic and renderer-free,
//! reusing the shared [`crate::files::errors`] failure taxonomy so a pipe
//! failure classifies the same way a file failure does.
//!
//! - [`list`] — the pipe-list model keeping `link` (publisher reachability) and
//!   `connected` (local connection) as separate facts (D2/#94).
//! - [`publish`] — loopback target validation client-side before `pipe.publish`
//!   (D6).
//! - [`connect`] — connect / release / revoke, with `pipe_unreachable` vs
//!   `pipe_unknown` vs `pipe_revoked` kept distinct.

pub mod connect;
pub mod list;
pub mod publish;

pub use connect::{run_connect, run_release, run_revoke, ConnectDone};
pub use list::{PipeListModel, PipeRowView};
pub use publish::{run_publish, validate_loopback_target, PublishDone};
