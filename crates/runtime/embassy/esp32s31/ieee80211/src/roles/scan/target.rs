//! ESP32-S31 target bindings for concrete scan RX/TX owners.
//!
//! The executor-neutral scan module owns transaction order and RX-ring
//! authority. This target-only owner keeps the persistent PHY state, platform
//! controls, delay and observer together so initial and reconnect scan ports do not
//! reconstruct the five-argument channel-switch boundary in application or
//! HIL code.

use crate::{
    datapath::rx::frontier::RxFrontierError,
    roles::scan::{
        port::{ScanPhyPort, ScanReceivePort},
        rx::{ScanFrameObserver, ScanObservationContext, ScanRx, ScanRxProgress},
    },
};

use oer_esp32s31_hal::owner::RadioRuntimeOwner;

use oer_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};

use oer_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;

use oer_esp32s31_wifi_sta::hardware::channel::ScanPhy;

impl<'state, 'arena, P, O, D> ScanPhyPort<CooperativeRadioHardware<'arena>>
    for ScanPhy<'state, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

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

impl<P, O, D> ScanPhyPort<RadioRuntimeOwner> for ScanPhy<'_, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRuntimeOwner,
        channel: u8,
    ) -> Result<(), Self::Error> {
        self.switch_channel(u16::from(channel), 0, hardware).await
    }
}

impl<H, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    ScanReceivePort<H> for ScanRx<'_, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: oer_esp32s31_wifi_mac::rx::RxDma,
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
