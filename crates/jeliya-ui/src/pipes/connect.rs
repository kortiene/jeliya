//! Connect / release / revoke (spec §5.C).
//!
//! A reachable authorized owner connects (**Connected**); a genuinely offline
//! owner yields the distinctive `pipe_unreachable` (#94); an out-of-audience or
//! absent pipe yields `pipe_unknown` (indistinguishable — not an oracle); a
//! revoked pipe yields `pipe_revoked`. `pipe.release` releases the **local**
//! connection (by connection id), while `pipe.revoke` withdraws a **published**
//! pipe (owner-only; `pipe_not_publisher` otherwise). All three are signed
//! mutations issued without replay (`Dedup::None`) — never auto-replayed.

use jeliya_api::{PipeConnect, PipeId, PipeRelease, PipeRevoke, RoomId, Target};
use jeliya_client::{CallError, ClientHandle, Dedup};
use jeliya_platform::CancelToken;

use crate::files::errors::{FlowStop, Settled};

/// The success payload of a connect: the local connection identity and endpoint.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectDone {
    /// The local connection id (names the connection for a later `pipe.release`).
    pub connection_id: String,
    /// The local endpoint the connection is bound to.
    pub local: Target,
}

/// Await a plain call to its terminal while racing an explicit cancel, mapping
/// the terminal through [`FlowStop::from_call`]. Shared by the pipe mutations,
/// which have no in-band stream to abort — a cancel simply drops the call.
pub async fn race_or<T, Fut>(call: Fut, ct: &CancelToken) -> Result<T, FlowStop>
where
    Fut: std::future::Future<Output = Result<T, CallError>>,
{
    use futures::future::{select, Either};
    futures::pin_mut!(call);
    match select(ct.cancelled(), call).await {
        Either::Left((_, _)) => Err(FlowStop::Cancelled),
        Either::Right((result, _)) => result.map_err(|error| FlowStop::from_call(&error)),
    }
}

/// Run one `pipe.connect` to settlement. The distinctive `pipe_unreachable`
/// (offline publisher) is preserved as [`crate::files::FlowFailure::PipeUnreachable`],
/// distinct from `pipe_unknown` (spec D2/#94).
pub async fn run_connect(
    handle: &ClientHandle,
    room_id: RoomId,
    pipe_id: PipeId,
    ct: &CancelToken,
) -> Settled<ConnectDone> {
    let call = handle.call::<PipeConnect>(PipeConnect { room_id, pipe_id }, Dedup::None);
    match race_or(call, ct).await {
        Ok(out) => Settled::Ok(ConnectDone {
            connection_id: out.connection_id,
            local: out.local,
        }),
        Err(stop) => Settled::from_stop(stop),
    }
}

/// Run one `pipe.release` to settlement, releasing the **local** connection by
/// its id (never by pipe id — release is per-connection).
pub async fn run_release(
    handle: &ClientHandle,
    connection_id: String,
    ct: &CancelToken,
) -> Settled<()> {
    let call = handle.call::<PipeRelease>(PipeRelease { connection_id }, Dedup::None);
    match race_or(call, ct).await {
        Ok(_) => Settled::Ok(()),
        Err(stop) => Settled::from_stop(stop),
    }
}

/// Run one `pipe.revoke` to settlement, withdrawing a **published** pipe by pipe
/// id (owner-only; a non-publisher gets `pipe_not_publisher`). A re-revoke
/// returns the original withdrawal, so it is idempotent, not an error.
pub async fn run_revoke(
    handle: &ClientHandle,
    room_id: RoomId,
    pipe_id: PipeId,
    ct: &CancelToken,
) -> Settled<()> {
    let call = handle.call::<PipeRevoke>(PipeRevoke { room_id, pipe_id }, Dedup::None);
    match race_or(call, ct).await {
        Ok(_) => Settled::Ok(()),
        Err(stop) => Settled::from_stop(stop),
    }
}
