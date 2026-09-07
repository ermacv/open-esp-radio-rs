use super::*;
impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    H,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> ScanReceivePort<H>
    for RunningScanRx<
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
where
    D: RxDmaObservationDelay,
    H: RxDma,
{
    type Error = RxFrontierError;

    fn prepare_initial(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_initial(self, hardware)
    }

    fn start<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        Self::start(self, hardware)
    }

    fn observe_management<O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, Self::Error>
    where
        O: ScanFrameObserver,
    {
        Self::observe_management(self, hardware, context)
    }

    fn park(&mut self) -> Result<(), Self::Error> {
        Self::park(self)
    }

    fn prepare_next_channel(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_next_channel(self, hardware)
    }
}

impl<'slot, P, E, W, H, const BUFFER_SIZE: usize> ScanTransmitPort<H>
    for RunningScanTx<'slot, P, E, W, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    W: WifiTxTimer,
    H: TxHardware,
{
    type Error = ControlTxError;

    fn begin_scan(&mut self) {
        Self::begin_scan(self);
    }

    fn transmit_probe_request<'a>(
        &'a mut self,
        hardware: &'a mut H,
        request: ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<ScanProbeReport, Self::Error>> + 'a {
        Self::transmit_probe_request(self, hardware, request)
    }
}
