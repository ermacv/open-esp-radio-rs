use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use embassy_time::Instant;

/// Only minimum hardware settles may spend this bounded interval in a poll.
/// Readiness retries retain their original timer cadence, regardless of size.
pub(super) const fn synchronous_settle(
    kind: oer_esp32s31_phy::executor::wait::Kind,
    micros: u64,
    limit: u64,
) -> bool {
    matches!(kind, oer_esp32s31_phy::executor::wait::Kind::Settle) && micros != 0 && micros <= limit
}

pub(super) enum HardwareDelay<F, C, S> {
    Timer(Deadline<F, C>),
    /// The target primitive owns the calibrated cycle deadline.
    Settle {
        micros: u32,
        delay: S,
    },
}

impl<F: Future<Output = ()> + Unpin, C: Fn() -> Instant + Unpin, S: FnMut(u32) + Unpin> Future
    for HardwareDelay<F, C, S>
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match self.get_mut() {
            Self::Timer(timer) => Pin::new(timer).poll(cx),
            Self::Settle { micros, delay } => {
                delay(*micros);
                Poll::Ready(())
            }
        }
    }
}

// PHY delays express a minimum settling time, not an executor yield. Embassy's
// Timer deliberately yields once even if already expired. Check the absolute
// deadline before polling it; future deadlines still use its ordinary wake
// registration. This does not spin or shorten the requested hardware delay.
pub(super) struct Deadline<F, C> {
    pub(super) deadline: Instant,
    pub(super) timer: F,
    pub(super) now: C,
}

impl<F: Future<Output = ()> + Unpin, C: Fn() -> Instant + Unpin> Future for Deadline<F, C> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if (this.now)() >= this.deadline {
            Poll::Ready(())
        } else {
            Pin::new(&mut this.timer).poll(cx)
        }
    }
}

#[cfg(test)]
mod tests;

/// Observe completion against the same start/deadline used by the timer.
/// Lateness includes timer polling and timestamp sampling, not just scheduling.
pub(super) fn measure<F, C, O>(
    mut future: F,
    start: Instant,
    requested_micros: u64,
    now: C,
    enabled: bool,
    mut observe: O,
) -> impl Future<Output = ()>
where
    F: Future<Output = ()> + Unpin,
    C: Fn() -> Instant,
    O: FnMut(oer_esp32s31_phy::executor::wait::Event),
{
    use oer_esp32s31_phy::executor::wait::Event;
    let mut started = false;
    core::future::poll_fn(move |cx| {
        // A microsecond deadline must be checked before observer work can
        // consume its settling interval and turn Pending into immediate Ready.
        let result = Pin::new(&mut future).poll(cx);
        if enabled {
            let elapsed = result
                .is_ready()
                .then(|| now().duration_since(start).as_micros());
            if !started {
                observe(Event::Started { requested_micros });
                started = true;
            }
            if let Some(elapsed_micros) = elapsed {
                observe(Event::Completed {
                    elapsed_micros,
                    lateness_micros: elapsed_micros.saturating_sub(requested_micros),
                });
            }
        }
        result
    })
}
