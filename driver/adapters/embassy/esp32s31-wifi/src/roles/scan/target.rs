//! ESP32-S31 target bindings for concrete scan RX/TX owners.
//!
//! The executor-neutral scan module owns transaction order and RX-ring
//! authority. This target-only owner keeps the persistent PHY state, platform
//! controls, delay and observer together so initial and reconnect scan ports do not
//! reconstruct the five-argument channel-switch boundary in application or
//! HIL code.

use crate::{
    datapath::rx::frontier::Esp32s31RxFrontierError,
    roles::scan::port::{Esp32s31ScanPhyPort, Esp32s31ScanReceivePort},
    roles::scan::rx::{
        Esp32s31ScanFrameObserver, Esp32s31ScanObservationContext, Esp32s31ScanRx,
        Esp32s31ScanRxProgress,
    },
};
use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
use open_esp_radio_esp32s31_phy::{PhyAsyncDelay, PhyTargetObserver, PhyTargetPortError};
use open_esp_radio_esp32s31_wifi::cooperative_hardware::CooperativeRadioHardware;
use open_esp_radio_esp32s31_wifi_sta::channel::Esp32s31ScanPhy;

impl<'state, 'arena, P, O, D> Esp32s31ScanPhyPort<CooperativeRadioHardware<'arena>>
    for Esp32s31ScanPhy<'state, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut CooperativeRadioHardware<'arena>,
        channel: u8,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let access = hardware.register_access();
            self.switch_published_channel(u16::from(channel), 0, access)
                .await
        }
    }
}

impl<P, O, D> Esp32s31ScanPhyPort<RadioRuntimeOwner> for Esp32s31ScanPhy<'_, P, O, D>
where
    O: PhyTargetObserver,
    D: PhyAsyncDelay,
{
    type Error = PhyTargetPortError;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut RadioRuntimeOwner,
        channel: u8,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move { self.switch_channel(u16::from(channel), 0, hardware).await }
    }
}

impl<H, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31ScanReceivePort<H> for Esp32s31ScanRx<'_, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: open_esp_radio_esp32s31_wifi_mac::rx::RxDma,
{
    type Error = Esp32s31RxFrontierError;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        self.prepare_initial_or_retry(hardware)
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl core::future::Future<Output = Result<(), Self::Error>> + 'a {
        async move { Esp32s31ScanRx::start(self, hardware) }
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver,
    {
        Esp32s31ScanRx::observe_management(self, hardware, context)
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Esp32s31ScanRx::stop(self, hardware)
    }

    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Esp32s31ScanRx::prepare_next(self, hardware)
    }
}
