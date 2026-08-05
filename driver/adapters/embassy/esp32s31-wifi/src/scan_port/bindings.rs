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
> Esp32s31ScanReceivePort<H>
    for Esp32s31RunningScanRx<
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
    D: RxReloadDelay,
    H: RxDma,
{
    type Error = Esp32s31ScanRxError;

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
        context: &mut Esp32s31ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<Esp32s31ScanRxProgress, Self::Error>
    where
        O: Esp32s31ScanFrameObserver,
    {
        Self::observe_management(self, hardware, context)
    }

    fn stop(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::stop(self, hardware)
    }

    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_next(self, hardware)
    }
}

impl<'slot, 'interrupt, P, E, W, H, const BUFFER_SIZE: usize> Esp32s31ScanTransmitPort<H>
    for Esp32s31RunningScanTx<'slot, 'interrupt, P, E, W, BUFFER_SIZE>
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
        request: Esp32s31ScanProbeRequest<'a>,
    ) -> impl Future<Output = Result<Esp32s31ScanProbeReport, Self::Error>> + 'a {
        Self::transmit_probe_request(self, hardware, request)
    }
}
