//! ESP32-S31 target bindings for concrete scan RX/TX owners.
//!
//! The executor-neutral scan module owns transaction order and RX-ring
//! authority. Channel switches lease the shared radio through
//! [`RadioChannel`] once per channel.

use crate::{
    datapath::rx::frontier::RxFrontierError,
    roles::scan::{
        port::{ScanPhyPort, ScanReceivePort},
        rx::{ScanFrameObserver, ScanObservationContext, ScanRx, ScanRxProgress},
    },
};

use oer_esp32s31_hal::{owner::RadioRuntimeOwner, shared_radio::PlatformClockProvider};

use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyAsyncDelay, PhyTargetObserver};

use oer_esp32s31_ieee80211::cooperative_hardware::CooperativeRadioHardware;

use crate::roles::radio_channel::RadioChannel;

impl<'arena, P, C, O, D> ScanPhyPort<CooperativeRadioHardware<'arena>>
    for RadioChannel<'_, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = ConcurrentWifiChannelError;

    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeRadioHardware<'arena>,
        channel: u8,
    ) -> Result<(), Self::Error> {
        let access = hardware.register_access();
        self.switch_published_channel(u16::from(channel), 0, access)
            .await
    }
}

impl<P, C, O, D> ScanPhyPort<RadioRuntimeOwner> for RadioChannel<'_, P, C, O, D>
where
    C: PlatformClockProvider,
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = ConcurrentWifiChannelError;

    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRuntimeOwner,
        channel: u8,
    ) -> Result<(), Self::Error> {
        RadioChannel::switch_channel(self, u16::from(channel), 0, hardware).await
    }
}

impl<H, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    ScanReceivePort<H> for ScanRx<'_, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: oer_esp32s31_ieee80211_mac::rx::RxDma,
{
    type Error = RxFrontierError;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        self.prepare_initial_or_retry(hardware)
    }

    async fn start<'a>(&'a mut self, hardware: &'a mut H) -> Result<(), Self::Error> {
        ScanRx::start(self, hardware)
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, Self::Error>
    where
        O: ScanFrameObserver,
    {
        ScanRx::observe_management(self, hardware, context)
    }

    fn park(&mut self) -> Result<(), Self::Error> {
        ScanRx::park(self)
    }

    fn prepare_next_channel(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        ScanRx::prepare_next(self, hardware)
    }
}
