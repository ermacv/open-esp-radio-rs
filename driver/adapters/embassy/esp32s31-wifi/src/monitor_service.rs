//! Finite standalone-monitor owner over RX DMA and one MAC interrupt epoch.

#![forbid(unsafe_code)]

use core::{future::Future, pin::pin};

use embassy_futures::select::{Either, select};
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{RxDma, RxPhyInfo},
};
use open_esp_radio_wifi_softmac::MonitorSink;

use crate::{
    embassy_irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
    },
    monitor_rx::{Esp32s31MonitorRx, Esp32s31MonitorRxProgress},
    rx_ring_owner::{Esp32s31RxRingOwnerError, Esp32s31RxRingPhase},
};

/// Qualified interrupt mask retained by a standalone normalized monitor.
///
/// This is deliberately the complete recovered cold-RX mask rather than only
/// `RX_SUCCESS`. It retains acknowledgement of status which accompanies RX
/// on sustained traffic and whose independent semantics are not qualified.
pub const ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK: u32 = MAC_COLD_RX_INTERRUPT_MASK;

/// Aggregate progress for one finite monitor run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31MonitorRunReport {
    /// Bottom-half service epochs, including the initial handoff probe.
    pub rx_service_wakes: u32,
    /// Actual hard-IRQ RX work posts observed during this run.
    pub rx_interrupt_posts: u32,
    pub receive: Esp32s31MonitorRxProgress,
    pub interrupt_drain: Esp32s31MacInterruptEpochDrain,
}

impl Esp32s31MonitorRunReport {
    fn record(&mut self, progress: Esp32s31MonitorRxProgress) {
        self.receive.completed_descriptors = self
            .receive
            .completed_descriptors
            .saturating_add(progress.completed_descriptors);
        self.receive.published_frames = self
            .receive
            .published_frames
            .saturating_add(progress.published_frames);
        self.receive.dropped_frames = self
            .receive
            .dropped_frames
            .saturating_add(progress.dropped_frames);
        self.receive.full_drops = self.receive.full_drops.saturating_add(progress.full_drops);
        self.receive.oversized_drops = self
            .receive
            .oversized_drops
            .saturating_add(progress.oversized_drops);
        self.receive.filtered_drops = self
            .receive
            .filtered_drops
            .saturating_add(progress.filtered_drops);
        self.receive.malformed_frames = self
            .receive
            .malformed_frames
            .saturating_add(progress.malformed_frames);
        self.receive.recycled_descriptors = self
            .receive
            .recycled_descriptors
            .saturating_add(progress.recycled_descriptors);
        self.receive.reload_pending = progress.reload_pending;
    }

    fn record_interrupt_posts(&mut self, start: u32, current: u32) {
        self.rx_interrupt_posts = current.wrapping_sub(start);
    }
}

/// Failure while closing an interrupt/RX ownership epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorCleanupError<E> {
    /// CPU/peripheral routing could not be quiesced. RX remains live because
    /// stopping it while a handler may still own the register bank is unsafe.
    Interrupt(Esp32s31MacInterruptEpochQuiesceError<E>),
    /// The interrupt route is quiesced, but the DMA walker did not confirm its
    /// stop. The drain is retained as evidence for reset/recovery policy.
    Receive {
        error: Esp32s31RxRingOwnerError,
        interrupt_drain: Esp32s31MacInterruptEpochDrain,
    },
}

/// Terminal reason for a failed standalone monitor transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorRunError<E> {
    Start(Esp32s31RxRingOwnerError),
    Activate(Esp32s31MacInterruptEpochActivateError<E>),
    ActivateCleanup {
        activation: Esp32s31MacInterruptEpochActivateError<E>,
        cleanup: Esp32s31MonitorCleanupError<E>,
    },
    Service {
        error: Esp32s31RxRingOwnerError,
        cleanup: Option<Esp32s31MonitorCleanupError<E>>,
    },
    Shutdown(Esp32s31MonitorCleanupError<E>),
}

/// Failed run report. The borrowed service retains every unique owner at its
/// exact current phase.
pub struct Esp32s31MonitorRunFailure<E> {
    pub error: Esp32s31MonitorRunError<E>,
    pub report: Esp32s31MonitorRunReport,
}

/// Complete standalone monitor owner.
///
/// The hardware value is not borrowed by another task. The capture sink may
/// retain only independent pool leases, never RX DMA storage.
pub struct Esp32s31MonitorService<
    'storage,
    'runtime,
    'platform,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    H: RxDma,
    R: MacInterruptRoute,
    S: MonitorSink<RxPhyInfo>,
{
    hardware: Option<H>,
    receive: Option<Esp32s31MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>>,
    sink: Option<S>,
    interrupts: Option<Esp32s31MacInterruptEpoch<'runtime, R, M>>,
    platform: &'platform R::Platform,
}

impl<
    'storage,
    'runtime,
    'platform,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31MonitorService<
        'storage,
        'runtime,
        'platform,
        H,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    H: RxDma,
    R: MacInterruptRoute,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn new(
        hardware: H,
        receive: Esp32s31MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        sink: S,
        interrupts: Esp32s31MacInterruptEpoch<'runtime, R, M>,
        platform: &'platform R::Platform,
    ) -> Self {
        Self {
            hardware: Some(hardware),
            receive: Some(receive),
            sink: Some(sink),
            interrupts: Some(interrupts),
            platform,
        }
    }

    pub fn receive_phase(&self) -> Esp32s31RxRingPhase {
        self.receive
            .as_ref()
            .expect("monitor owner was already extracted")
            .phase()
    }

    pub fn interrupt_active(&self) -> bool {
        self.interrupts
            .as_ref()
            .expect("monitor owner was already extracted")
            .is_active()
    }

    /// Extract owners only after both asynchronous hardware actors are known
    /// inactive. Failure returns this complete service unchanged.
    #[allow(clippy::type_complexity)]
    pub fn try_into_parts(
        mut self,
    ) -> Result<
        (
            H,
            Esp32s31MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
            S,
            Esp32s31MacInterruptEpoch<'runtime, R, M>,
        ),
        Self,
    > {
        if self.interrupt_active() || self.receive_phase() == Esp32s31RxRingPhase::Live {
            return Err(self);
        }
        Ok((
            self.hardware
                .take()
                .expect("checked monitor hardware owner"),
            self.receive.take().expect("checked monitor RX owner"),
            self.sink.take().expect("checked monitor sink owner"),
            self.interrupts
                .take()
                .expect("checked monitor interrupt owner"),
        ))
    }
}

impl<
    'storage,
    'runtime,
    'platform,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31MonitorService<
        'storage,
        'runtime,
        'platform,
        H,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    H: RxDma,
    R: MacInterruptRoute,
    S: MonitorSink<RxPhyInfo>,
{
    /// Run until `stop` resolves, leaving this owner in halted state.
    ///
    /// The future only borrows `self`. Cancelling it cannot drop the hardware
    /// owner, and dropping the service itself performs the same fail-closed
    /// shutdown before any owner field can be destroyed.
    pub async fn run_until_stopped<F>(
        &mut self,
        stop: F,
    ) -> Result<Esp32s31MonitorRunReport, Esp32s31MonitorRunFailure<R::Error>>
    where
        F: Future<Output = ()>,
    {
        let mut report = Esp32s31MonitorRunReport::default();
        let interrupt_posts_at_start = self.interrupts().mac_runtime().rx_post_count();
        let start = {
            let receive = self.receive.as_mut().expect("monitor RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("monitor hardware owner exists");
            receive.start(hardware)
        };
        if let Err(error) = start {
            report.record_interrupt_posts(
                interrupt_posts_at_start,
                self.interrupts().mac_runtime().rx_post_count(),
            );
            return Err(Esp32s31MonitorRunFailure {
                error: Esp32s31MonitorRunError::Start(error),
                report,
            });
        }
        let platform = self.platform;
        if let Err(activation) = self
            .interrupts_mut()
            .activate(platform, ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK)
        {
            let error = match self.shutdown() {
                Ok(drain) => {
                    report.interrupt_drain = drain;
                    Esp32s31MonitorRunError::Activate(activation)
                }
                Err(cleanup) => Esp32s31MonitorRunError::ActivateCleanup {
                    activation,
                    cleanup,
                },
            };
            report.record_interrupt_posts(
                interrupt_posts_at_start,
                self.interrupts().mac_runtime().rx_post_count(),
            );
            return Err(Esp32s31MonitorRunFailure { error, report });
        }

        // A descriptor may have completed before route activation. The ring,
        // not this synthetic wake, remains the source of multiplicity.
        self.interrupts().mac_runtime().notify_rx_handoff();
        let mut stop = pin!(stop);
        loop {
            match select(stop.as_mut(), self.interrupts().mac_runtime().wait_rx()).await {
                Either::First(()) => break,
                Either::Second(()) => {
                    report.rx_service_wakes = report.rx_service_wakes.saturating_add(1);
                    let service = {
                        let receive = self.receive.as_mut().expect("monitor RX owner exists");
                        let hardware = self
                            .hardware
                            .as_mut()
                            .expect("monitor hardware owner exists");
                        let sink = self.sink.as_mut().expect("monitor sink owner exists");
                        receive.service(hardware, sink)
                    };
                    match service {
                        Ok(progress) => report.record(progress),
                        Err(error) => {
                            let cleanup = self.shutdown().err();
                            report.record_interrupt_posts(
                                interrupt_posts_at_start,
                                self.interrupts().mac_runtime().rx_post_count(),
                            );
                            return Err(Esp32s31MonitorRunFailure {
                                error: Esp32s31MonitorRunError::Service { error, cleanup },
                                report,
                            });
                        }
                    }
                }
            }
        }

        match self.shutdown() {
            Ok(drain) => {
                report.interrupt_drain = drain;
                report.record_interrupt_posts(
                    interrupt_posts_at_start,
                    self.interrupts().mac_runtime().rx_post_count(),
                );
                Ok(report)
            }
            Err(error) => {
                report.record_interrupt_posts(
                    interrupt_posts_at_start,
                    self.interrupts().mac_runtime().rx_post_count(),
                );
                Err(Esp32s31MonitorRunFailure {
                    error: Esp32s31MonitorRunError::Shutdown(error),
                    report,
                })
            }
        }
    }

    /// Close a live or partially started epoch after normal completion,
    /// failure, or cancellation of [`Self::run_until_stopped`].
    pub fn try_shutdown(
        &mut self,
    ) -> Result<Esp32s31MacInterruptEpochDrain, Esp32s31MonitorCleanupError<R::Error>> {
        let platform = self.platform;
        let interrupt_drain = if self.interrupt_active() {
            self.interrupts_mut()
                .quiesce(platform)
                .map_err(Esp32s31MonitorCleanupError::Interrupt)?
        } else {
            Esp32s31MacInterruptEpochDrain::default()
        };
        if self.receive_phase() == Esp32s31RxRingPhase::Live {
            let receive = self.receive.as_mut().expect("monitor RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("monitor hardware owner exists");
            receive
                .stop(hardware)
                .map_err(|error| Esp32s31MonitorCleanupError::Receive {
                    error,
                    interrupt_drain,
                })?;
        }
        Ok(interrupt_drain)
    }

    fn shutdown(
        &mut self,
    ) -> Result<Esp32s31MacInterruptEpochDrain, Esp32s31MonitorCleanupError<R::Error>> {
        self.try_shutdown()
    }

    fn interrupts(&self) -> &Esp32s31MacInterruptEpoch<'runtime, R, M> {
        self.interrupts
            .as_ref()
            .expect("monitor interrupt owner exists")
    }

    fn interrupts_mut(&mut self) -> &mut Esp32s31MacInterruptEpoch<'runtime, R, M> {
        self.interrupts
            .as_mut()
            .expect("monitor interrupt owner exists")
    }
}

impl<
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Drop for Esp32s31MonitorService<'_, '_, '_, H, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: RxDma,
    R: MacInterruptRoute,
    S: MonitorSink<RxPhyInfo>,
{
    fn drop(&mut self) {
        if self.hardware.is_none() {
            return;
        }
        if (self.interrupt_active() || self.receive_phase() == Esp32s31RxRingPhase::Live)
            && self.try_shutdown().is_err()
        {
            panic!("live ESP32-S31 monitor owner could not stop; platform reset required");
        }
    }
}

#[cfg(test)]
mod tests;
