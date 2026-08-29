//! ESP32-S31 WPA2 bindings for the Embassy RX and ordinary-TX owners.
//!
//! The executor-independent handshake/key ports live in the chip STA crate.
//! This module only adapts the concrete retained DMA frontier and control-TX
//! owner used by the Embassy integration.

use core::future::Future;

use open_esp_radio_esp32s31_wifi_mac::rx::{RxDma, RxIngressConfig, extract_data};
use open_esp_radio_esp32s31_wifi_sta::wpa2::{
    Esp32s31Wpa2Receive, Esp32s31Wpa2Station, copy_station_eapol,
};
use open_esp_radio_wpa2::runner::Wpa2RxProgress;

use crate::{
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{
        Esp32s31RxFrontier, Esp32s31RxFrontierDelay, Esp32s31RxFrontierDirective,
        Esp32s31RxFrontierError,
    },
};

/// Retained RX owner bound to its stable DMA allocation for WPA2.
pub struct Esp32s31Wpa2Rx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    station: Esp32s31Wpa2Station,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31Wpa2Rx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        station: Esp32s31Wpa2Station,
    ) -> Self {
        Self {
            owner,
            storage,
            station,
        }
    }

    pub fn into_owner(self) -> Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE> {
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
    D: Esp32s31RxFrontierDelay,
    H: RxDma,
{
    type Error = Esp32s31RxFrontierError;

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
                    Esp32s31RxFrontierDirective::Stop
                } else {
                    Esp32s31RxFrontierDirective::Continue
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
            if self.owner.phase() == crate::datapath::rx::frontier::Esp32s31RxFrontierPhase::Live {
                Ok(())
            } else {
                self.owner.start_with_storage(hardware, self.storage).await
            }
        }
    }

    fn stop(&mut self, _hardware: &mut H) -> Result<(), Self::Error> {
        if self.owner.phase() == crate::datapath::rx::frontier::Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(Esp32s31RxFrontierError::OwnerUnavailable)
        }
    }
}
