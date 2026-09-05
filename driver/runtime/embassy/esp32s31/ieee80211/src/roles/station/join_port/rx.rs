use open_esp_radio_esp32s31_wifi_mac::init::MacRuntimeStopHardware;
use open_esp_radio_esp32s31_wifi_mac::rx::{RxDma, RxIngressConfig, RxSegment, extract_management};
use open_esp_radio_esp32s31_wifi_sta::join::Esp32s31StaJoinReceive;
use open_esp_radio_wifi_sta::join::{StaJoinRxDirective, StaJoinRxObserver};

use crate::{
    datapath::rx::dma::Esp32s31RxDmaStorage,
    datapath::rx::frontier::{
        Esp32s31RxFrontier, Esp32s31RxFrontierDelay, Esp32s31RxFrontierDirective,
        Esp32s31RxFrontierError, Esp32s31RxFrontierPhase,
    },
};

/// RX owner bound to the stable DMA storage used by every finite join phase.
pub struct Esp32s31StaJoinRx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Esp32s31StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: Esp32s31RxFrontier<'storage, D, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self { owner, storage }
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
> Esp32s31StaJoinReceive<H>
    for Esp32s31StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    D: Esp32s31RxFrontierDelay,
    H: RxDma + MacRuntimeStopHardware,
{
    type Error = Esp32s31RxFrontierError;

    async fn start<'a>(&'a mut self, hardware: &'a mut H) -> Result<(), Self::Error> {
        let started = if self.owner.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            self.owner.start_with_storage(hardware, self.storage).await
        };
        if started.is_ok() {
            hardware.resume_mac_runtime();
        }
        started
    }

    fn stop(&mut self, _hardware: &mut H) -> Result<(), Self::Error> {
        if self.owner.phase() == Esp32s31RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(Esp32s31RxFrontierError::OwnerUnavailable)
        }
    }

    fn service_management<O>(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: StaJoinRxObserver,
    {
        self.owner
            .service_completed(hardware, self.storage, |segment: RxSegment<'_>| {
                let management = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                )
                .ok();
                let management = management.map(|parsed| &frame[..parsed.length]);
                match observer.observe_completed(management) {
                    StaJoinRxDirective::Continue => Esp32s31RxFrontierDirective::Continue,
                    StaJoinRxDirective::Stop => Esp32s31RxFrontierDirective::Stop,
                }
            })
            .map(|_| ())
    }
}
