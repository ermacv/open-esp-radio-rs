//! Standalone ESP32-S31 normalized-monitor RX owner.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_wifi_mac::rx::{
    RxDma, RxIngressConfig, RxPhyInfo, RxRingError, RxRingHalted, view_normalized_rx_frame,
};
use open_esp_radio_wifi_softmac::{
    MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink, WifiStandaloneMonitorPlan,
    interface::{ChannelContextId, MonitorTapPoint},
};

use crate::{
    rx_dma_service::Esp32s31RxDmaStorage,
    rx_ring_owner::{Esp32s31RxRingOwner, Esp32s31RxRingOwnerError, Esp32s31RxRingPhase},
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

pub struct Esp32s31MonitorPrepareFailure<R> {
    pub error: Esp32s31MonitorConfigError,
    pub receive: R,
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
}

pub struct Esp32s31MonitorRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: Esp32s31RxRingOwner<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    channel_context: ChannelContextId,
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
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<Self, Esp32s31MonitorPrepareError> {
        let channel_context =
            monitor_channel_context(plan).map_err(Esp32s31MonitorPrepareError::Configuration)?;
        let receive = Esp32s31RxRingOwner::prepare_initial(
            hardware,
            storage,
            descriptor_base,
            buffer_addresses,
        )
        .map_err(Esp32s31MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            channel_context,
        })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        hardware: &mut H,
        storage: &'static Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<Self, Esp32s31MonitorPrepareError> {
        let channel_context =
            monitor_channel_context(plan).map_err(Esp32s31MonitorPrepareError::Configuration)?;
        let receive = Esp32s31RxRingOwner::prepare_initial(
            hardware,
            storage,
            descriptor_base,
            buffer_addresses,
        )
        .map_err(Esp32s31MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            channel_context,
        })
    }

    pub fn from_plan(
        receive: Esp32s31RxRingOwner<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        plan: WifiStandaloneMonitorPlan,
    ) -> Result<
        Self,
        Esp32s31MonitorPrepareFailure<
            Esp32s31RxRingOwner<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        >,
    > {
        match monitor_channel_context(plan) {
            Ok(channel_context) => Ok(Self {
                receive,
                channel_context,
            }),
            Err(error) => Err(Esp32s31MonitorPrepareFailure { error, receive }),
        }
    }

    pub const fn phase(&self) -> Esp32s31RxRingPhase {
        self.receive.phase()
    }

    pub const fn channel_context(&self) -> ChannelContextId {
        self.channel_context
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxRingOwnerError> {
        self.receive.start(hardware)
    }

    pub fn service<H: RxDma, S: MonitorSink<RxPhyInfo>>(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<Esp32s31MonitorRxProgress, Esp32s31RxRingOwnerError> {
        let mut progress = Esp32s31MonitorRxProgress::default();
        let ring = self.receive.service_completed(hardware, |segment| {
            match view_normalized_rx_frame(
                &segment,
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
            ) {
                Ok(frame) => match sink.try_publish(MonitorFrame {
                    tap: MonitorTapPoint::Normalized,
                    channel_context: self.channel_context,
                    bytes: frame.mpdu,
                    metadata: frame.metadata,
                    logical_length: frame.logical_length,
                }) {
                    MonitorPublishOutcome::Published => {
                        progress.published_frames = progress.published_frames.saturating_add(1)
                    }
                    MonitorPublishOutcome::Dropped(reason) => {
                        progress.dropped_frames = progress.dropped_frames.saturating_add(1);
                        let reason_count = match reason {
                            MonitorDropReason::Full => &mut progress.full_drops,
                            MonitorDropReason::TooLong => &mut progress.oversized_drops,
                            MonitorDropReason::Filtered => &mut progress.filtered_drops,
                        };
                        *reason_count = reason_count.saturating_add(1);
                    }
                },
                Err(_) => {
                    progress.malformed_frames = progress.malformed_frames.saturating_add(1);
                }
            }
        })?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxRingOwnerError> {
        self.receive.stop(hardware)
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self {
            receive,
            channel_context,
        } = self;
        receive.into_halted().map_err(|receive| Self {
            receive,
            channel_context,
        })
    }
}

fn monitor_channel_context(
    plan: WifiStandaloneMonitorPlan,
) -> Result<ChannelContextId, Esp32s31MonitorConfigError> {
    let monitor = plan.monitor();
    if monitor.tap() != MonitorTapPoint::Normalized {
        return Err(Esp32s31MonitorConfigError::UnsupportedTap(monitor.tap()));
    }
    Ok(plan.channel_context())
}
