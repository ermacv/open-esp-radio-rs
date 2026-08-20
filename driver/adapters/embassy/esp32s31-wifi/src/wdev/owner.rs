use super::*;

impl<
    'resources,
    'irq,
    M: RawMutex + 'resources,
    N,
    B,
    R,
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
        R,
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
    R: WdevNetworkRxSet,
{
    pub fn new_with_rx_set(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interface: NetworkInterfaceId,
        network_rx: R,
        services: B,
    ) -> Self {
        Self::new_with_scope(
            irq,
            network,
            WdevInterfaceScope::Single(interface),
            network_rx,
            services,
        )
    }

    /// Construct one physical WDEV scheduling two permanent logical VIFs.
    pub fn new_with_scope(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interfaces: WdevInterfaceScope,
        network_rx: R,
        services: B,
    ) -> Self {
        Self {
            resources: core::marker::PhantomData,
            irq,
            network,
            interfaces,
            network_rx,
            services,
            active_tx_interface: None,
            prepared_tx_interface: None,
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

    /// Return every runner owner, including an addressed multi-VIF RX set.
    ///
    /// Standalone callers normally recreate their cheap publisher at the
    /// next epoch and use [`Self::into_parts`]. A combined owner must retain
    /// both addressed publishers across partial role transitions and uses
    /// this complete frontier instead.
    pub fn into_complete_parts(self) -> (N, R, B) {
        (self.network, self.network_rx, self.services)
    }

    pub(super) fn network_tx_queue_len(&self) -> usize {
        if let Some(interface) = self.prepared_tx_interface {
            return self.network.tx_queue_len(interface);
        }
        match self.interfaces {
            WdevInterfaceScope::Single(interface) => self.network.tx_queue_len(interface),
            WdevInterfaceScope::Pair { .. } => self.network.physical_tx_queue_len(),
        }
    }

    pub(super) fn try_receive_network_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        if let Some(interface) = self.prepared_tx_interface {
            return self.network.try_receive_tx(interface);
        }
        match self.interfaces {
            WdevInterfaceScope::Single(interface) => self.network.try_receive_tx(interface),
            WdevInterfaceScope::Pair { .. } => self.network.try_receive_physical_tx(),
        }
    }

    pub(super) fn tx_interface_for(
        &self,
        frame: &PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> NetworkInterfaceId {
        let interface = *frame.tag();
        assert!(
            self.interfaces.contains(interface),
            "tagged TX lease does not belong to this WDEV scope"
        );
        interface
    }

    pub(super) fn tx_consumer_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        assert!(self.interfaces.contains(interface));
        self.network.tx_consumer(interface)
    }

    pub(super) fn set_scope_link_state(&self, state: LinkState) {
        match self.interfaces {
            WdevInterfaceScope::Single(interface) => self.network.set_link_state(interface, state),
            WdevInterfaceScope::Pair { first, second } => {
                self.network.set_link_state(first, state);
                self.network.set_link_state(second, state);
            }
        }
    }

    pub(super) fn reported_active_tx_interface(&self) -> NetworkInterfaceId {
        let reported = self.services.active_tx_interface();
        match (self.interfaces, reported) {
            (WdevInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (WdevInterfaceScope::Pair { .. }, None) => {
                panic!("paired WDEV services must identify role-generated TX")
            }
            (_, Some(_)) => panic!("services reported TX outside the WDEV scope"),
        }
    }

    pub(super) fn retained_prepared_tx_interface(&self) -> NetworkInterfaceId {
        if let Some(interface) = self.prepared_tx_interface {
            return interface;
        }
        let reported = self.services.prepared_tx_interface();
        match (self.interfaces, reported) {
            (WdevInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (WdevInterfaceScope::Pair { .. }, None) => {
                panic!("paired WDEV services must identify prepared TX")
            }
            (_, Some(_)) => panic!("services reported prepared TX outside the WDEV scope"),
        }
    }
}

impl<
    'resources,
    'irq,
    M: RawMutex + 'resources,
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
    pub fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interface: NetworkInterfaceId,
        services: B,
    ) -> Self {
        let network_rx = network.rx_publisher(interface);
        Self::new_with_rx_set(irq, network, interface, network_rx, services)
    }
}
