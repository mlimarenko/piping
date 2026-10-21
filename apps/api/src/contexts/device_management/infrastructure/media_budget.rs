//! Process-wide memory budget shared by every CCTV media-plane adapter.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::contexts::device_management::domain::DriverError;

/// A reserved slice of the shared [`MediaBudget`], embedded alongside the
/// plane's own concurrency permit for the operation's full lifetime; dropping
/// it returns the units to the pool.
#[derive(Debug)]
pub struct MediaBudgetPermit {
    _permit: OwnedSemaphorePermit,
}

/// Process-wide memory budget shared by every CCTV media-plane adapter
/// (still/live/clip/download).
///
/// Each plane already caps its OWN concurrency with an independent
/// `tokio::sync::Semaphore` (see `rtsp_still`/`rtsp_live`/`rtsp_clip`/`remux`),
/// but nothing today bounds the SUM those four independent ceilings can hold
/// at once — each can independently hit its own ceiling and the host must
/// absorb the sum of all four. This wraps a SECOND, shared `Semaphore` sized
/// in fixed `unit_bytes` chunks; every plane reserves `media_units` chunks
/// from it before admitting an operation, so the total memory footprint
/// across all four planes is bounded by one process-wide number.
///
/// # Never blocks
///
/// [`Self::try_reserve`] is ALWAYS non-blocking (`try_acquire_many_owned`,
/// never awaited). The LIVE plane holds its permit for a long session
/// duration, so a blocking wait anywhere against this shared budget could
/// starve behind a long-lived LIVE stream. The only blocking wait anywhere
/// stays the STILL plane's existing LOCAL `acquire()` call (bounded — still
/// grabs are short-lived); every global-budget check here is fail-fast.
#[derive(Clone)]
pub struct MediaBudget {
    slots: Arc<Semaphore>,
}

impl MediaBudget {
    /// `total_bytes` split into `unit_bytes`-sized chunks (at least 1 unit —
    /// a misconfigured 0 must never wedge the gate permanently shut).
    #[must_use]
    pub fn new(total_bytes: usize, unit_bytes: usize) -> Self {
        let units = (total_bytes / unit_bytes.max(1)).max(1);
        Self {
            slots: Arc::new(Semaphore::new(units)),
        }
    }

    /// Reserve `units` chunks for `plane` (used only in the error string).
    ///
    /// # Errors
    ///
    /// [`DriverError::Busy`] when the shared budget has fewer than `units`
    /// chunks free. Never blocks — see the "Never blocks" section on
    /// [`MediaBudget`]'s type-level docs.
    pub fn try_reserve(&self, units: u32, plane: &str) -> Result<MediaBudgetPermit, DriverError> {
        self.slots
            .clone()
            .try_acquire_many_owned(units)
            .map(|permit| MediaBudgetPermit { _permit: permit })
            .map_err(|_| {
                DriverError::Busy(format!("shared media memory budget exhausted ({plane})"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_unit_bytes_does_not_panic_and_yields_at_least_one_unit() {
        let budget = MediaBudget::new(0, 0);
        // A misconfigured 0/0 must still open the gate for exactly one unit,
        // never wedge it permanently shut.
        let permit = budget.try_reserve(1, "test").expect("one unit available");
        let error = budget
            .try_reserve(1, "test")
            .expect_err("only one unit exists, and it is already held");
        assert!(matches!(error, DriverError::Busy(_)));
        drop(permit);
    }

    #[test]
    fn reserve_within_budget_succeeds() {
        let budget = MediaBudget::new(100, 10); // 10 units
        let permit = budget.try_reserve(5, "still");
        assert!(permit.is_ok());
    }

    #[test]
    fn reserve_beyond_budget_fails_busy() {
        let budget = MediaBudget::new(100, 10); // 10 units
        let error = budget
            .try_reserve(11, "live")
            .expect_err("11 units exceeds the 10-unit budget");
        match error {
            DriverError::Busy(message) => {
                assert!(
                    message.contains("live"),
                    "message should name the plane: {message}"
                );
            }
            other => panic!("expected Busy, got {other:?}"),
        }
    }

    #[test]
    fn permit_is_released_on_drop_freeing_units_for_the_next_caller() {
        let budget = MediaBudget::new(100, 10); // 10 units
        let permit = budget
            .try_reserve(10, "clip")
            .expect("full budget reserved");
        assert!(
            budget.try_reserve(1, "clip").is_err(),
            "budget is fully reserved"
        );
        drop(permit);
        assert!(
            budget.try_reserve(10, "clip").is_ok(),
            "units must be returned to the pool on drop"
        );
    }
}
