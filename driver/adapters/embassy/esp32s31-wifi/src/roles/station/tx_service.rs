#![expect(
    clippy::manual_async_fn,
    reason = "TX service keeps the role-neutral borrowed Future contract explicit"
)]

//! Station ordinary-TX binding for the role-neutral DATAPATH service port.

use core::future::Future;

use open_esp_radio_embassy_net::{PinnedTxFrame, PinnedTxInterfaceConsumer, RawMutex};
use open_esp_radio_esp32s31_wifi_mac::tx::TxHardware;
use open_esp_radio_esp32s31_wifi_sta::single_mpdu_tx::{
    Esp32s31SingleMpduTx, SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer,
};

use crate::datapath::{WifiTxProgress, WifiTxWake, services::DatapathNetworkTxService};

impl<
    'resources,
    'slot,
    M,
    H,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const TX_BUFFER_SIZE: usize,
> DatapathNetworkTxService<'resources, M, H, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    for Esp32s31SingleMpduTx<'slot, P, E, T, TX_BUFFER_SIZE>
where
    M: RawMutex,
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    type Error = SingleMpduTxError;

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        _network: &'a PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = Esp32s31SingleMpduTx::start(self, hardware, frame.ethernet())?;
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
