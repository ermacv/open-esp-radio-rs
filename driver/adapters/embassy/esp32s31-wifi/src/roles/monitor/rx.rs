#![expect(
    clippy::result_large_err,
    reason = "no-alloc monitor shutdown returns the complete RX frontier"
)]

//! Standalone ESP32-S31 normalized-monitor RX owner.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_wifi_mac::rx::RxRingHalted;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxDmaBufferAddresses, RxIngressConfig, RxPhyInfo, RxRingError, view_normalized_rx_frame,
};
use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFilter, MonitorFrame, MonitorPublishOutcome, MonitorSink,
    WifiStandaloneMonitorPlan,
    interface::{ChannelContextId, MonitorTapPoint},
};

use crate::{
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{
        EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontier, Esp32s31RxFrontierError,
        Esp32s31RxFrontierPhase,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorConfigError {
    UnsupportedTap(MonitorTapPoint),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31MonitorPrepareError {
    Configuration(Esp32s31MonitorConfigError),
    Ring(RxRingError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31MonitorRxProgress {
    pub completed_descriptors: u32,
    pub published_frames: u32,
    pub dropped_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub filtered_drops: u32,
    pub malformed_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

pub struct Esp32s31MonitorRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: Esp32s31RxFrontier<'storage, EmbassyEsp32s31RxFrontierDelay, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    channel_context: ChannelContextId,
    filter: MonitorFilter,
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        hardware: &mut H,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, Esp32s31MonitorPrepareError> {
        let (channel_context, filter) =
            monitor_context(plan).map_err(Esp32s31MonitorPrepareError::Configuration)?;
        let receive = Esp32s31RxFrontier::prepare_initial(
            hardware,
            storage,
            descriptor_base,
            buffer_addresses,
        )
        .map_err(Esp32s31MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }

    /// Rebind the physical RX ring returned by a previous role without
    /// acquiring the shared DMA arena a second time.
    pub fn prepare_halted<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        ring: RxRingHalted<'storage, COUNT>,
        hardware: &mut H,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<Self, (RxRingHalted<'storage, COUNT>, Esp32s31MonitorPrepareError)> {
        let (channel_context, filter) = match monitor_context(plan) {
            Ok(context) => context,
            Err(error) => {
                return Err((ring, Esp32s31MonitorPrepareError::Configuration(error)));
            }
        };
        let prepared = storage
            .prepare_halted(ring, hardware)
            .map_err(|(ring, error)| (ring, Esp32s31MonitorPrepareError::Ring(error)))?;
        Ok(Self {
            receive: Esp32s31RxFrontier::from_prepared(prepared),
            storage,
            channel_context,
            filter,
        })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        hardware: &mut H,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, Esp32s31MonitorPrepareError> {
        let (channel_context, filter) =
            monitor_context(plan).map_err(Esp32s31MonitorPrepareError::Configuration)?;
        let receive = Esp32s31RxFrontier::prepare_initial(
            hardware,
            storage,
            descriptor_base,
            buffer_addresses,
        )
        .map_err(Esp32s31MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }

    pub const fn phase(&self) -> Esp32s31RxFrontierPhase {
        self.receive.phase()
    }

    #[cfg(test)]
    pub const fn channel_context(&self) -> ChannelContextId {
        self.channel_context
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxFrontierError> {
        self.receive.start_prepared(hardware)
    }

    pub fn service<H: RxDma, S: MonitorSink<RxPhyInfo>>(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<Esp32s31MonitorRxProgress, Esp32s31RxFrontierError> {
        let mut progress = Esp32s31MonitorRxProgress::default();
        let channel_context = self.channel_context;
        let filter = self.filter;
        let ring = self
            .receive
            .service_completed_frontier(hardware, self.storage, |segment| {
                match view_normalized_rx_frame(
                    &segment,
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                ) {
                    Ok(frame) => {
                        let observation = MonitorFrame {
                            tap: MonitorTapPoint::Normalized,
                            channel_context,
                            bytes: frame.mpdu,
                            metadata: frame.metadata,
                            logical_length: frame.logical_length,
                        };
                        if !filter.accepts(&observation) {
                            progress.dropped_frames = progress.dropped_frames.saturating_add(1);
                            progress.filtered_drops = progress.filtered_drops.saturating_add(1);
                        } else {
                            match sink.try_publish(observation) {
                                MonitorPublishOutcome::Published => {
                                    progress.published_frames =
                                        progress.published_frames.saturating_add(1)
                                }
                                MonitorPublishOutcome::Dropped(reason) => {
                                    progress.dropped_frames =
                                        progress.dropped_frames.saturating_add(1);
                                    let reason_count = match reason {
                                        MonitorDropReason::Full => &mut progress.full_drops,
                                        MonitorDropReason::TooLong => &mut progress.oversized_drops,
                                        MonitorDropReason::Filtered => &mut progress.filtered_drops,
                                    };
                                    *reason_count = reason_count.saturating_add(1);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        progress.malformed_frames = progress.malformed_frames.saturating_add(1);
                    }
                }
            })?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        progress.service_probe_pending = ring.service_probe_pending;
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxFrontierError> {
        self.receive.stop(hardware)
    }

    /// Rebuild a halted ring before publishing another capture epoch.
    pub fn prepare_next<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxFrontierError> {
        self.receive.prepare_next(hardware, self.storage)
    }

    pub(crate) fn require_reset(&mut self) {
        self.receive.require_reset();
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self {
            receive,
            storage,
            channel_context,
            filter,
        } = self;
        receive.try_into_halted().map_err(|receive| Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }
}

fn monitor_context(
    plan: WifiStandaloneMonitorPlan,
) -> Result<(ChannelContextId, MonitorFilter), Esp32s31MonitorConfigError> {
    let monitor = plan.monitor();
    if monitor.tap() != MonitorTapPoint::Normalized {
        return Err(Esp32s31MonitorConfigError::UnsupportedTap(monitor.tap()));
    }
    Ok((plan.channel_context(), monitor.filter()))
}
