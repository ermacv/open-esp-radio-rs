#![expect(
    clippy::manual_async_fn,
    reason = "TX service keeps the role-neutral borrowed Future contract explicit"
)]

//! Station ordinary-TX binding for the role-neutral DATAPATH service port.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::tx::TxHardware;
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    Esp32s31SingleMpduTx, SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
};

use crate::datapath::{
    MaterializedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame, WifiTxProgress, WifiTxWake,
    services::DatapathNetworkTxService,
};

impl<'slot, H, P, E, T, SoftwareFrame, PhysicalFrame, const TX_BUFFER_SIZE: usize>
    DatapathNetworkTxService<H, SoftwareFrame, PhysicalFrame>
    for Esp32s31SingleMpduTx<'slot, P, E, T, TX_BUFFER_SIZE>
where
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: MaterializedTxFrame,
{
    type Error = SingleMpduTxError;

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        frame: SoftwareFrame,
        _network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        async move {
            let progress = Esp32s31SingleMpduTx::start(self, hardware, frame.as_slice())?;
            // Ordinary TX copied the complete Ethernet body into its private
            // pinned slot before publishing DMA, so the network lease is no
            // longer hardware-visible at this boundary.
            drop(frame);
            Ok(progress)
        }
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        Esp32s31SingleMpduTx::wait_deadline(self)
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        Esp32s31SingleMpduTx::service(self, hardware, wake)
    }
}
