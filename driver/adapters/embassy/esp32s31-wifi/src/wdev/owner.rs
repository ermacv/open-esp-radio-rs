use super::*;

impl<
    'resources,
    'irq,
    M: RawMutex,
    N,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    WdevRunner<
        'resources,
        'irq,
        M,
        N,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    N: WdevNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    pub fn new(irq: &'irq EmbassyMacIrqRuntime<M>, network: N, services: B) -> Self {
        let network_rx = network.rx_publisher();
        Self {
            irq,
            network,
            network_rx,
            services,
            rx_progress: WdevRxProgress::Drained,
            rx_frame_deficit: 0,
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
    /// The outer role lifecycle must be able to reclaim these values after
    /// [`WdevRunnerExit::Role`] or [`WdevRunnerExit::Stopped`] in order to
    /// stop DMA, clear role state and construct a later epoch. Keeping them
    /// recoverable also makes it impossible for `run` to hide teardown behind
    /// task-local globals.
    pub fn into_parts(self) -> (N, B) {
        (self.network, self.services)
    }
}
