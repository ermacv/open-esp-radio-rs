#![expect(
    clippy::result_large_err,
    reason = "no-alloc running-scan transitions retain the exact DMA owner on failure"
)]

use super::*;
/// Running-scan RX owner which retains every connected-epoch resource.
///
/// A connected teardown returns more than a descriptor ring: the staging
/// pool, queue sender, DMA observation delay and telemetry binding must survive
/// candidate refresh too. This owner changes only logical protocol ownership;
/// the physical RX frontier remains live for the complete scan.
pub struct Esp32s31RunningScanRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    scan: Esp32s31ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    resources: Esp32s31RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    Esp32s31RunningScanRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub fn from_parked<P>(
        parked: Esp32s31StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
        >,
    ) -> Result<
        Self,
        Esp32s31StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            P,
        >,
    > {
        match parked.try_into_live_epoch_parts() {
            Ok((ring, resources)) => {
                let scan = Esp32s31ScanRx::from_live(ring, resources.storage());
                Ok(Self { scan, resources })
            }
            Err((parked, _)) => Err(parked),
        }
    }

    pub const fn phase(&self) -> Esp32s31RxFrontierPhase {
        self.scan.phase()
    }

    /// Validate the continuously live ring before the first scan channel.
    pub fn prepare_initial<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxFrontierError> {
        self.scan.prepare_next(hardware)
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31RxFrontierError>
    where
        D: RxDmaObservationDelay,
    {
        if self.scan.phase() == Esp32s31RxFrontierPhase::Live {
            return Ok(());
        }
        self.resources
            .delay_mut()
            .after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US)
            .await;
        self.scan.start(hardware)
    }

    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31RxFrontierError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        self.scan.observe_management(hardware, context)
    }

    pub fn park(&mut self) -> Result<(), Esp32s31RxFrontierError> {
        self.scan.park()
    }

    pub fn prepare_next_channel<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31RxFrontierError> {
        self.scan.prepare_next(hardware)
    }

    pub fn into_parked(
        self,
    ) -> Result<
        Esp32s31StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        Self,
    > {
        let Self { scan, resources } = self;
        match scan.into_live() {
            Ok(ring) => Ok(resources.with_live_ring(ring)),
            Err(scan) => Err(Self { scan, resources }),
        }
    }
}
