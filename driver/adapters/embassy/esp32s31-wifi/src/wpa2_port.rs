//! ESP32-S31 WPA2 bindings for the Embassy RX and ordinary-TX owners.
//!
//! The executor-independent handshake/key ports live in the chip STA crate.
//! This module only adapts the concrete retained DMA frontier and control-TX
//! owner used by the Embassy integration.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::{
    rx::{RxDma, RxIngressConfig, extract_data},
    tx::{LegacyTxQueue, TxCompletion, TxHardware, TxPhyRate},
};
use open_esp_radio_ieee80211::station::{StaDataFrame, StaProtectedDataFrame};
use open_esp_radio_wpa2::runner::Wpa2RxProgress;

pub use open_esp_radio_esp32s31_wifi_sta::wpa2::*;

use crate::{
    control_tx::{ControlTxError, Esp32s31ControlTx},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    preconnected_rx::{
        Esp32s31PreconnectedRx, Esp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxDirective,
        Esp32s31PreconnectedRxError,
    },
    rx_backend::Esp32s31RxDmaStorage,
};

impl<'slot, P, E, W, H, const BUFFER_SIZE: usize> Esp32s31Wpa2Transmit<H>
    for Esp32s31ControlTx<'slot, P, E, W, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    W: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;

    fn transmit_unprotected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaDataFrame<'a>,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        self.transmit_unprotected_data(hardware, frame)
    }

    fn transmit_protected<'a>(
        &'a mut self,
        hardware: &'a mut H,
        frame: StaProtectedDataFrame<'a>,
        queue: LegacyTxQueue,
        rate: TxPhyRate,
        hardware_key_selector: u8,
    ) -> impl Future<Output = Result<TxCompletion, Self::Error>> + 'a {
        self.transmit_protected_data(hardware, frame, queue, rate, hardware_key_selector)
    }
}

/// Retained RX owner bound to its stable DMA allocation for WPA2.
pub struct Esp32s31Wpa2Rx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    station: Esp32s31Wpa2Station,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31Wpa2Rx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        station: Esp32s31Wpa2Station,
    ) -> Self {
        Self {
            owner,
            storage,
            station,
        }
    }

    pub fn into_owner(self) -> Esp32s31PreconnectedRx<'storage, D, COUNT, DMA_BUFFER_SIZE> {
        self.owner
    }
}

impl<
    'storage,
    D,
    H,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Esp32s31Wpa2Receive<H> for Esp32s31Wpa2Rx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    D: Esp32s31PreconnectedRxDelay,
    H: RxDma,
{
    type Error = Esp32s31PreconnectedRxError;

    fn service(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
    ) -> Result<Wpa2RxProgress, Self::Error> {
        let mut eapol = None;
        let progress = self
            .owner
            .service_completed(hardware, self.storage, |segment| {
                let candidate = extract_data(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                )
                .ok()
                .and_then(|data| {
                    copy_station_eapol(frame, data.mpdu.length, data.payload_offset, self.station)
                });
                if let Some(candidate) = candidate {
                    eapol = Some(candidate);
                    Esp32s31PreconnectedRxDirective::Stop
                } else {
                    Esp32s31PreconnectedRxDirective::Continue
                }
            })?;
        Ok(match eapol {
            Some(eapol) => Wpa2RxProgress::eapol(progress.completed, eapol),
            None => Wpa2RxProgress::drained(progress.completed),
        })
    }

    fn restart<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            self.owner.stop(hardware)?;
            self.owner.start_with_storage(hardware, self.storage).await
        }
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        self.owner.stop(hardware)
    }
}
