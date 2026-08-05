use super::*;
/// Running-scan RX owner which retains every connected-epoch resource.
///
/// A connected teardown returns more than a halted descriptor ring: the
/// staging pool, queue sender, reload delay and telemetry binding must survive
/// candidate refresh too. This owner separates the halted ring only while the
/// finite scan runs and can then return either exact join parts or the original
/// stopped connected owner without recreating static storage.
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
    pub fn from_stopped(
        stopped: Esp32s31StoppedRx<
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
    ) -> Self {
        let (ring, resources) = stopped.into_epoch_parts();
        let scan = Esp32s31ScanRx::from_halted(ring, resources.storage());
        Self { scan, resources }
    }

    pub const fn phase(&self) -> Esp32s31ScanRxPhase {
        self.scan.phase()
    }

    /// Rebuild the first ring of this running scan from its halted connected
    /// frontier. Later channel visits use [`Self::prepare_next`] identically.
    pub fn prepare_initial<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31ScanRxError> {
        self.scan.prepare_next(hardware)
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError>
    where
        D: RxReloadDelay,
    {
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
    ) -> Result<Esp32s31ScanRxProgress, Esp32s31ScanRxError>
    where
        H: RxDma,
        O: Esp32s31ScanFrameObserver,
    {
        self.scan.observe_management(hardware, context)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        self.scan.stop(hardware)
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), Esp32s31ScanRxError> {
        self.scan.prepare_next(hardware)
    }

    /// Hand the halted ring and all peer-independent RX resources directly to
    /// Authentication/Association without manufacturing another capability.
    #[allow(clippy::type_complexity)]
    pub fn into_epoch_parts(
        self,
    ) -> Result<
        (
            RxRingHalted<'storage, COUNT>,
            Esp32s31RxEpochResources<
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
        ),
        Self,
    > {
        let Self { scan, resources } = self;
        match scan.into_halted() {
            Ok(ring) => Ok((ring, resources)),
            Err(scan) => Err(Self { scan, resources }),
        }
    }

    /// Restore the same stopped connected owner when a scan exits without
    /// crossing into a pre-connected protocol epoch.
    pub fn into_stopped(
        self,
    ) -> Result<
        Esp32s31StoppedRx<
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
        self.into_epoch_parts()
            .map(|(ring, resources)| resources.with_halted_ring(ring))
    }
}
