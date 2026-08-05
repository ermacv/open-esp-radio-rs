use super::*;

impl<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    ConnectedRunner<
        'resources,
        'irq,
        M,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    B: ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    pub const fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: SplitPinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        services: B,
    ) -> Self {
        Self {
            irq,
            network,
            services,
            rx_backpressured: false,
        }
    }

    pub const fn services(&self) -> &B {
        &self.services
    }

    pub fn services_mut(&mut self) -> &mut B {
        &mut self.services
    }

    /// Return the network and hardware owners after the runner exits.
    ///
    /// A station lifecycle must be able to reclaim these values after
    /// [`ConnectedRunnerExit::Disconnected`] in order to stop DMA, clear keys and
    /// construct a later association epoch. Keeping them recoverable also
    /// makes it impossible for `run` to hide teardown behind task-local
    /// globals.
    pub fn into_parts(
        self,
    ) -> (
        SplitPinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        B,
    ) {
        (self.network, self.services)
    }
}
