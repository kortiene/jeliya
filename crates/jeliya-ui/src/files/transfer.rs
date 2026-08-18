//! Shared progress + cancellation glue for the file flows (spec D5).
//!
//! A user cancel must reach **both** the local copy (the `PlatformServices`
//! [`CancelToken`], which stops the bounded copy and deletes any partial staged
//! blob / uncommitted export sink) **and** the wire transfer (the client
//! stream's in-band ABORT via [`StreamCall::cancel`], or `transfer.cancel` for a
//! `file.fetch`). A cancelled flow settles as [`Settled::Cancelled`], **never**
//! `Ok` and never a fabricated "delivered". Dropping the flow future is
//! equivalent to cancel (drop-is-abort), because the platform futures honor it
//! and the `CancelToken` is fired by the component on teardown.

use futures::future::{select, Either};
use jeliya_api::Operation;
use jeliya_client::{CallError, Execution, StreamCall};
use jeliya_platform::{CancelToken, ProgressSink, StageProgress};

use super::errors::FlowStop;

/// A bounded, monotonic progress report surfaced to the UI. `total` is `None`
/// for a streamed source whose size is not known until the stream ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    /// Bytes transferred so far.
    pub transferred: u64,
    /// The total to transfer, when known up front.
    pub total: Option<u64>,
}

impl Progress {
    /// Adopt a platform [`StageProgress`], keeping `transferred` monotonic (a
    /// re-reported lower value never rewinds the bar).
    pub fn advance(self, staged: StageProgress) -> Self {
        Progress {
            transferred: self.transferred.max(staged.transferred),
            total: staged.total.or(self.total),
        }
    }

    /// The completion fraction in `0..=1`, or `None` when the total is unknown.
    pub fn fraction(self) -> Option<f64> {
        self.total.map(|total| {
            if total == 0 {
                1.0
            } else {
                (self.transferred as f64 / total as f64).clamp(0.0, 1.0)
            }
        })
    }
}

/// Build a platform [`ProgressSink`] that forwards each staging update to
/// `report`, mapped into a [`Progress`]. The callback runs synchronously on the
/// staging task, so it must not block (it only updates a UI signal).
pub fn progress_sink(report: impl Fn(Progress) + 'static) -> ProgressSink {
    ProgressSink::new(move |staged: StageProgress| {
        report(Progress::default().advance(staged));
    })
}

/// Await a streaming call to its terminal while racing an explicit cancel.
///
/// On an explicit cancel the wire is aborted in-band with
/// [`Execution::Unknown`] — once the stream has opened, bytes may have gone out,
/// so the flow can never claim the file was shared/fetched (spec D5) — and the
/// outcome is [`FlowStop::Cancelled`]. Otherwise the terminal is classified by
/// [`FlowStop::from_call`], so a delivery-classified `Cancelled` from the seam
/// is also honored as a cancel, never a failure.
pub async fn race_cancel_stream<O: Operation>(
    stream: &mut StreamCall<O>,
    ct: &CancelToken,
) -> Result<O::Output, FlowStop>
where
    O::Output: 'static,
{
    match select(ct.cancelled(), &mut *stream).await {
        Either::Left((_, _)) => {
            // In-band ABORT of the wire (spec D5): file.share / file.read cancel
            // in-band, not via transfer.cancel.
            stream.cancel(Execution::Unknown);
            Err(FlowStop::Cancelled)
        }
        Either::Right((result, _)) => result.map_err(|error| FlowStop::from_call(&error)),
    }
}

/// Await a plain call to its terminal while racing an explicit cancel, running
/// `on_cancel` (e.g. `transfer.cancel`) if the cancel wins.
///
/// Used for `file.fetch`, whose cancellation is op_id-addressed
/// (`transfer.cancel`) rather than an in-band stream ABORT (spec D5).
pub async fn race_cancel_call<T, Fut, Cancel>(
    call: Fut,
    ct: &CancelToken,
    on_cancel: Cancel,
) -> Result<T, FlowStop>
where
    Fut: std::future::Future<Output = Result<T, CallError>>,
    Cancel: FnOnce(),
{
    futures::pin_mut!(call);
    match select(ct.cancelled(), call).await {
        Either::Left((_, _)) => {
            on_cancel();
            Err(FlowStop::Cancelled)
        }
        Either::Right((result, _)) => result.map_err(|error| FlowStop::from_call(&error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_monotonic_and_fractional() {
        let p = Progress::default()
            .advance(StageProgress {
                transferred: 50,
                total: Some(100),
            })
            .advance(StageProgress {
                transferred: 25,
                total: Some(100),
            });
        assert_eq!(p.transferred, 50, "a lower re-report never rewinds");
        assert_eq!(p.fraction(), Some(0.5));

        let unknown = Progress::default().advance(StageProgress {
            transferred: 10,
            total: None,
        });
        assert_eq!(unknown.fraction(), None);
    }

    #[test]
    fn zero_total_fraction_is_complete() {
        // A zero-total known transfer is 100% complete: clamped(0/0) = 1.0.
        let p = Progress::default().advance(StageProgress {
            transferred: 0,
            total: Some(0),
        });
        assert_eq!(p.fraction(), Some(1.0));
    }

    #[test]
    fn total_is_retained_when_later_report_omits_it() {
        // Once a total is known it must survive a subsequent report that
        // omits it — "total: None" means "still unknown", not "clear it".
        let p = Progress::default()
            .advance(StageProgress {
                transferred: 10,
                total: Some(100),
            })
            .advance(StageProgress {
                transferred: 20,
                total: None,
            });
        assert_eq!(p.total, Some(100));
        assert_eq!(p.fraction(), Some(0.2));
    }
}
