use super::*;

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
    >
{
    pub const fn ring(&self) -> &RxRingHalted<'storage, COUNT> {
        &self.ring
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub const fn pool(&self) -> &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY> {
        self.pool
    }

    pub const fn delay(&self) -> &D {
        &self.delay
    }

    pub fn delay_mut(&mut self) -> &mut D {
        &mut self.delay
    }

    pub const fn pipeline_observer(&self) -> Option<&'pool dyn RxPipelineObserver> {
        self.pipeline_observer
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Separate the peer-specific halted frontier from persistent connected
    /// RX resources for a finite pre-connected protocol epoch.
    pub fn into_epoch_parts(
        self,
    ) -> (
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
    ) {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
        } = self;
        (
            ring,
            Esp32s31RxEpochResources {
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            },
        )
    }

    /// Rebuild descriptor and buffer state for a fresh association epoch.
    ///
    /// Hardware is already confirmed stopped by this type. On every failure
    /// the complete halted owner is reconstructed, including its queue sender
    /// and delay implementation.
    #[allow(clippy::result_large_err)]
    pub fn prepare<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31PreparedRx<
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
        (Self, RxRingError),
    > {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err((self, RxRingError::Size));
        }
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
        } = self;
        match storage.prepare_halted(ring, hardware) {
            Ok(ring) => Ok(Esp32s31PreparedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                },
                error,
            )),
        }
    }
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
    >
{
    /// Bind board-allocated DMA/staging resources before the first connected
    /// epoch. Later epochs recover this same owner from [`Esp32s31StoppedRx`].
    pub fn new(
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
        delay: D,
    ) -> Self {
        Self {
            storage,
            pool,
            frames,
            delay,
            pipeline_observer: None,
        }
    }

    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [Esp32s31RxDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub fn delay_mut(&mut self) -> &mut D {
        &mut self.delay
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Reassemble the stopped production owner after a finite join attempt.
    pub fn with_halted_ring(
        self,
        ring: RxRingHalted<'storage, COUNT>,
    ) -> Esp32s31StoppedRx<
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
    > {
        Esp32s31StoppedRx {
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            pipeline_observer: self.pipeline_observer,
        }
    }

    /// Promote the same persistent resources into a connected RX service
    /// after Association/WPA2 returns the live ring frontier.
    pub fn with_live_ring(
        self,
        ring: RxRingLive<'storage, COUNT>,
    ) -> Esp32s31ConnectedRx<
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
    > {
        Esp32s31ConnectedRx {
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            pipeline_observer: self.pipeline_observer,
            admission: FullRxStageAdmission,
        }
    }
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
    Esp32s31PreparedRx<
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
    pub const fn ring(&self) -> &RxRingStopped<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Observe the required settle delay and open a fresh live RX epoch.
    ///
    /// A rejected walker-enable readback returns this prepared owner intact,
    /// so a higher-level reset/retry policy never loses static resources.
    #[allow(clippy::result_large_err)]
    pub async fn start<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31ConnectedRx<
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
        (Self, RxRingError),
    >
    where
        D: RxDmaObservationDelay,
    {
        let Self {
            ring,
            storage,
            pool,
            frames,
            mut delay,
            pipeline_observer,
        } = self;
        delay
            .after_micros(ESP32S31_RX_WALKER_ENABLE_SETTLE_US)
            .await;
        match ring.try_start(hardware) {
            Ok(ring) => Ok(Esp32s31ConnectedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
                admission: FullRxStageAdmission,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                },
                error,
            )),
        }
    }
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
    Esp32s31ConnectedRx<
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
    pub fn new(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'storage Esp32s31RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        delay: D,
        frames: Sender<
            'queue,
            M,
            Esp32s31StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
            QUEUE_DEPTH,
        >,
    ) -> Self {
        Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer: None,
            admission: FullRxStageAdmission,
        }
    }

    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    /// Install a statically dispatched ingress admission policy.
    ///
    /// The default policy is zero-sized and admits the complete physical
    /// staging slot. Changing it consumes the owner so a policy cannot be
    /// swapped while a DMA transaction is in progress.
    pub fn with_stage_admission_policy<P>(
        self,
        admission: P,
    ) -> Esp32s31ConnectedRx<
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
    > {
        Esp32s31ConnectedRx {
            ring: self.ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            pipeline_observer: self.pipeline_observer,
            admission,
        }
    }
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
    P,
>
    Esp32s31ConnectedRx<
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
    >
{
    pub const fn ring(&self) -> &RxRingLive<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Confirm that DMA released the ring and return a stopped RX owner.
    ///
    /// On failure the complete live owner is returned together with the
    /// hardware error; no staging, queue or delay capability is lost.
    #[allow(clippy::result_large_err)]
    pub fn try_stop<H: RxDma>(
        self,
        hardware: &mut H,
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
        (Self, RxRingError),
    > {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            pipeline_observer,
            admission,
        } = self;
        match ring.try_stop(hardware) {
            Ok(ring) => Ok(Esp32s31StoppedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    pipeline_observer,
                    admission,
                },
                error,
            )),
        }
    }
}
