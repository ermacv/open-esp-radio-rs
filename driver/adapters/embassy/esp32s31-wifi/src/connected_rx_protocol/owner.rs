use super::*;

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
>
    Esp32s31ConnectedRxProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub fn new_with_reorder_slots(
        frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        dispatcher: ConnectedRxDispatcher,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut Esp32s31ConnectedRxProtocolStorage<
            'pool,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> Self {
        assert!(
            CAPACITY <= usize::from(u16::MAX),
            "staged RX capacity must fit the deferred record length"
        );
        assert!(
            ethernet.len() >= CAPACITY,
            "A-MSDU output scratch must cover one complete staged RX unit"
        );
        assert!(SLOTS != 0, "staged RX pool must not be empty");
        assert!(
            REORDER_SLOTS != 0,
            "RX reorder slot domain must not be empty"
        );
        assert!(
            REORDER_SLOTS <= RX_REORDER_BACKING_SLOT_COUNT,
            "RX reorder slot domain exceeds the MAC maximum"
        );
        assert!(
            SLOTS <= usize::from(u8::MAX) + 1,
            "reorder slot identity must fit the MAC token"
        );
        Self {
            frames,
            irq,
            dispatcher,
            sink,
            mpdu,
            ethernet,
            pipeline_observer: None,
            reorder_commands: None,
            reorder_storage: None,
            reorder_scratch: None,
            runtime,
        }
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandReceiver<'queue, M>,
    ) -> Self {
        self.reorder_commands = Some(commands);
        self
    }

    /// Install cold backing for the MPDUs that actually cross a sequence gap.
    /// In-order frames continue directly from the SRAM staging lease.
    pub fn with_rx_reorder_storage(
        mut self,
        storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    ) -> Self {
        self.reorder_storage = Some(storage);
        self
    }

    /// Install one internal-SRAM readback scratch for a retained ordinary
    /// MPDU. This avoids repeatedly parsing the cold PSRAM backing in place.
    /// A-MSDU keeps its distinct output-scratch path.
    pub fn with_rx_reorder_scratch(mut self, scratch: &'scratch mut [u8]) -> Self {
        assert!(
            scratch.len() >= CAPACITY,
            "reorder readback scratch must cover one complete staged RX unit"
        );
        self.reorder_scratch = Some(scratch);
        self
    }

    pub fn with_pipeline_observer(mut self, observer: &'queue dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn dispatcher(&self) -> &ConnectedRxDispatcher {
        &self.dispatcher
    }

    pub const fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    pub fn queue_len(&self) -> usize {
        self.frames.len()
    }

    /// Discard all ownership retained by the completed connected epoch.
    ///
    /// The connected control producer must already be stopped, otherwise it
    /// could publish a new reorder command after the mailbox is drained. This
    /// operation performs no PAC access and no sink publication: reconnect
    /// teardown must not block on a full network queue merely to return RX
    /// staging and cold-reorder leases.
    pub fn shutdown_discard(&mut self) -> ConnectedRxProtocolShutdown {
        let mut shutdown = ConnectedRxProtocolShutdown::default();
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            shutdown.queued_frames = shutdown.queued_frames.saturating_add(1);
        }
        if let Some(commands) = &self.reorder_commands {
            while try_receive_rx_reorder_command(commands).is_some() {
                shutdown.reorder_commands = shutdown.reorder_commands.saturating_add(1);
            }
        }
        for reorder in &mut self.runtime.reorders {
            if reorder.take().is_some() {
                shutdown.active_reorders = shutdown.active_reorders.saturating_add(1);
            }
        }
        self.runtime.reorder_first_starts.fill(None);
        self.runtime.gap_deadlines.fill(None);
        for retained in &mut self.runtime.retained {
            if retained.take().is_some() {
                shutdown.retained_frames = shutdown.retained_frames.saturating_add(1);
            }
        }
        if shutdown.queued_frames != 0 || shutdown.retained_frames != 0 {
            self.irq.notify_rx_capacity();
        }
        shutdown
    }

    pub(super) fn into_stopped_parts(
        self,
    ) -> (
        &'scratch mut [u8],
        &'scratch mut [u8],
        &'pool mut Esp32s31ConnectedRxProtocolStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    ) {
        (self.mpdu, self.ethernet, self.runtime)
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
>
    Esp32s31ConnectedRxProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        DEPTH,
        CAPACITY,
        SLOTS,
        RX_REORDER_BACKING_SLOT_COUNT,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    /// Construct the vendor-maximum 64-slot reorder profile.
    pub fn new(
        frames: Receiver<'queue, M, Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>, DEPTH>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        dispatcher: ConnectedRxDispatcher,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut Esp32s31ConnectedRxProtocolStorage<
            'pool,
            CAPACITY,
            SLOTS,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
    ) -> Self {
        Self::new_with_reorder_slots(frames, irq, dispatcher, sink, mpdu, ethernet, runtime)
    }
}
