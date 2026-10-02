//! Waits on the image's monotonic time.

use oer_time::{Duration, Timer};

/// Wait for `duration` on `timer`. A deadline past the timer's range is
/// never reached, so the wait does not end early.
pub(crate) async fn wait_for(timer: &impl Timer, duration: Duration) {
    if timer.wait_for(duration).await.is_err() {
        core::future::pending::<()>().await;
    }
}
