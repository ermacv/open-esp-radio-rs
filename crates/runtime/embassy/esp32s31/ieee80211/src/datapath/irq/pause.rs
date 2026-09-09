//! Same-epoch interrupt suspension retaining hardware and executor work.

use super::*;
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_esp32s31_wifi_mac::irq::MacInterruptPauseRoute;

/// Detached CPU route and pending work, retained for the same radio epoch.
///
/// This is not cold setup: MAC/DMA quiescence and shared RF admission must be
/// established by their respective owners. The S31 backend leaves peripheral
/// masks and latched causes intact. Resume cannot select a different role mask.
/// A paused epoch is affine; pending work is restored only by consuming it.
///
/// A terminal-only route cannot silently become a resumable one:
/// ```compile_fail
/// use embassy_sync::blocking_mutex::raw::RawMutex;
/// use oer_esp32s31_wifi_mac::irq::MacInterruptRoute;
/// use oer_esp32s31_wifi_embassy::datapath::irq::InterruptEpoch;
/// fn pause<R: MacInterruptRoute, M: RawMutex>(
///     epoch: InterruptEpoch<'_, R, M>, platform: &R::Platform,
/// ) {
///     let _ = epoch.try_pause(platform);
/// }
/// ```
pub struct PausedInterruptEpoch<'runtime, R: MacInterruptPauseRoute, M: RawMutex> {
    route: R,
    paused: R::Paused,
    mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    pending: MacInterruptEpochDrain,
    moderated: bool,
}

impl<'runtime, R: MacInterruptPauseRoute, M: RawMutex> InterruptEpoch<'runtime, R, M> {
    /// Detach the route while retaining hardware causes and executor work.
    /// Failure returns the original epoch without draining notifications.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "failure returns the exact affine IRQ owner and route error"
    )]
    pub fn try_pause(
        mut self,
        platform: &R::Platform,
    ) -> Result<PausedInterruptEpoch<'runtime, R, M>, (Self, MacInterruptEpochQuiesceError<R::Error>)>
    {
        if !self.is_active() {
            return Err((self, MacInterruptEpochQuiesceError::AlreadyQuiesced));
        }
        let paused = match self
            .route
            .as_mut()
            .expect("epoch retains route")
            .pause(platform)
        {
            Ok(paused) => paused,
            Err(error) => return Err((self, MacInterruptEpochQuiesceError::Route(error))),
        };
        if self.rx_moderated {
            self.mac_runtime.end_rx_moderation();
        }
        Ok(PausedInterruptEpoch {
            route: self.route.take().expect("paused epoch retains route"),
            paused,
            mac_runtime: self.mac_runtime,
            power_runtime: self.power_runtime,
            pending: MacInterruptEpochDrain {
                mac: self.mac_runtime.drain_pending(),
                power_events: self.power_runtime.drain_pending(),
            },
            moderated: self.rx_moderated,
        })
    }
}

impl<'runtime, R: MacInterruptPauseRoute, M: RawMutex> PausedInterruptEpoch<'runtime, R, M> {
    /// Transfer the detached interrupt authority through one owned operation.
    ///
    /// The operation must return that authority before this epoch can resume.
    /// Route identity, moderation and drained work remain inside this future;
    /// neither failure nor cancellation exposes a resumable epoch. An operation
    /// which can fail must retain its hardware resources in its error value.
    /// This is an ownership handoff, not MAC/DMA or shared-RF admission.
    pub async fn try_with_authority<T, E>(
        self,
        operation: impl AsyncFnOnce(R::Paused) -> Result<(R::Paused, T), E>,
    ) -> Result<(Self, T), PausedInterruptOperationFailure<'runtime, R, M, E>> {
        let Self {
            route,
            paused,
            mac_runtime,
            power_runtime,
            pending,
            moderated,
        } = self;
        match operation(paused).await {
            Ok((paused, result)) => Ok((
                Self {
                    route,
                    paused,
                    mac_runtime,
                    power_runtime,
                    pending,
                    moderated,
                },
                result,
            )),
            Err(error) => Err(PausedInterruptOperationFailure {
                _route: route,
                _mac_runtime: mac_runtime,
                _power_runtime: power_runtime,
                _pending: pending,
                _moderated: moderated,
                error,
            }),
        }
    }

    /// Restore the original policy and merge retained work with new arrivals.
    /// Request one descriptor probe even if its old IRQ edge was coalesced.
    /// A failed resume retains both register authority and pending work.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "failure returns the exact affine IRQ owner and route error"
    )]
    pub fn try_resume(
        mut self,
        platform: &R::Platform,
    ) -> Result<InterruptEpoch<'runtime, R, M>, (Self, MacInterruptEpochActivateError<R::Error>)>
    {
        if self.moderated {
            self.mac_runtime.begin_rx_moderation();
        }
        if let Err((error, paused)) = self.route.resume(platform, self.paused) {
            self.paused = paused;
            if self.moderated {
                self.mac_runtime.end_rx_moderation();
            }
            return Err((self, MacInterruptEpochActivateError::Route(error)));
        }
        let epoch = InterruptEpoch {
            route: Some(self.route),
            setup: None,
            mac_runtime: self.mac_runtime,
            power_runtime: self.power_runtime,
            rx_moderated: self.moderated,
        };
        epoch.restore_pause_work(self.pending);
        Ok(epoch)
    }

    /// End the epoch, cleaning peripheral state without re-enabling CPU routes.
    /// Returned notifications are diagnostic facts, not replayed work.
    pub fn into_stopped(mut self) -> (InterruptEpoch<'runtime, R, M>, MacInterruptEpochDrain) {
        let setup = self.route.finish_pause(self.paused);
        (
            InterruptEpoch::new(self.route, setup, self.mac_runtime, self.power_runtime),
            self.pending,
        )
    }
}

/// Failed operation and its detached route, retained until reset.
///
/// Only shared error inspection is available: extracting the route or an
/// owned interrupt token could bypass the failed operation's restoration.
///
/// ```compile_fail
/// use embassy_sync::blocking_mutex::raw::RawMutex;
/// use oer_esp32s31_wifi_mac::irq::MacInterruptPauseRoute;
/// use oer_esp32s31_wifi_embassy::datapath::irq::PausedInterruptOperationFailure;
/// fn resume_failed<R: MacInterruptPauseRoute, M: RawMutex, E>(
///     fault: PausedInterruptOperationFailure<'_, R, M, E>, platform: &R::Platform,
/// ) {
///     fault.try_resume(platform);
/// }
/// ```
#[must_use = "failed paused operation retains its interrupt epoch until reset"]
pub struct PausedInterruptOperationFailure<'runtime, R: MacInterruptPauseRoute, M: RawMutex, E> {
    _route: R,
    _mac_runtime: &'runtime EmbassyMacIrqRuntime<M>,
    _power_runtime: &'runtime EmbassyPowerIrqRuntime<M>,
    _pending: MacInterruptEpochDrain,
    _moderated: bool,
    error: E,
}

impl<R: MacInterruptPauseRoute, M: RawMutex, E> PausedInterruptOperationFailure<'_, R, M, E> {
    pub const fn error(&self) -> &E {
        &self.error
    }
}
