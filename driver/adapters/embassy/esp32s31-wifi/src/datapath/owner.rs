use super::*;

fn select_pair_tx_slot(pending: [bool; 2], served: [u64; 2]) -> Option<usize> {
    match pending {
        [true, true] => Some(usize::from(served[0] > served[1])),
        [true, false] => Some(0),
        [false, true] => Some(1),
        [false, false] => None,
    }
}

fn charge_pair_tx_frames(served: &mut [u64; 2], slot: usize, frames: usize) {
    served[slot] = served[slot].saturating_add(u64::try_from(frames.max(1)).unwrap_or(u64::MAX));
    let shared = served[0].min(served[1]);
    served[0] -= shared;
    served[1] -= shared;
}

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
    DatapathRunner<
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
    N: DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: DatapathServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    R: DatapathNetworkRxSet,
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
            DatapathInterfaceScope::Single(interface),
            network_rx,
            services,
        )
    }

    /// Construct one physical DATAPATH scheduling two permanent logical VIFs.
    pub fn new_with_scope(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        network: N,
        interfaces: DatapathInterfaceScope,
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
            rx_progress: DatapathRxProgress::Drained,
            rx_frame_deficit: 0,
            pair_tx_served_frames: [0; 2],
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
    /// [`DatapathRunnerExit::Role`] or [`DatapathRunnerExit::Stopped`] in order to
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
            DatapathInterfaceScope::Single(interface) => self.network.tx_queue_len(interface),
            DatapathInterfaceScope::Pair { .. } => self.network.physical_tx_queue_len(),
        }
    }

    pub(super) fn try_receive_network_tx(
        &mut self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        if let Some(interface) = self.prepared_tx_interface {
            return self.network.try_receive_tx(interface);
        }
        match self.interfaces {
            DatapathInterfaceScope::Single(interface) => self.network.try_receive_tx(interface),
            DatapathInterfaceScope::Pair { first, second } => {
                let first_pending = self.network.tx_queue_len(first) != 0;
                let second_pending = self.network.tx_queue_len(second) != 0;
                let slot = select_pair_tx_slot(
                    [first_pending, second_pending],
                    self.pair_tx_served_frames,
                );
                let interface = match slot {
                    Some(0) => {
                        if !second_pending {
                            self.pair_tx_served_frames = [0; 2];
                        }
                        first
                    }
                    Some(1) => {
                        if !first_pending {
                            self.pair_tx_served_frames = [0; 2];
                        }
                        second
                    }
                    None => return None,
                    Some(_) => unreachable!("paired TX has exactly two slots"),
                };
                self.network.try_receive_tx(interface)
            }
        }
    }

    pub(super) fn account_pair_tx_frames(&mut self, interface: NetworkInterfaceId, frames: usize) {
        let DatapathInterfaceScope::Pair { first, second } = self.interfaces else {
            return;
        };
        let slot = if interface == first {
            0
        } else {
            assert_eq!(interface, second);
            1
        };
        charge_pair_tx_frames(&mut self.pair_tx_served_frames, slot, frames);
    }

    pub(super) fn tx_interface_for(
        &self,
        frame: &PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> NetworkInterfaceId {
        let interface = *frame.tag();
        assert!(
            self.interfaces.contains(interface),
            "tagged TX lease does not belong to this DATAPATH scope"
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

    /// Whether the other VIF already has published work while `interface`
    /// owns the physical transmitter.
    ///
    /// A paired owner may build one software standby aggregate, but it must
    /// not keep extending that reservation across an already-published frame
    /// from its peer.  This is the finite ownership boundary used for VIF
    /// fairness; a single-interface runner has no competing frontier.
    pub(super) fn competing_tx_pending(&self, interface: NetworkInterfaceId) -> bool {
        match self.interfaces {
            DatapathInterfaceScope::Single(_) => false,
            DatapathInterfaceScope::Pair { first, second } => {
                let peer = if interface == first {
                    second
                } else {
                    assert_eq!(interface, second);
                    first
                };
                self.network.tx_queue_len(peer) != 0
            }
        }
    }

    pub(super) fn set_scope_link_state(&self, state: LinkState) {
        match self.interfaces {
            DatapathInterfaceScope::Single(interface) => {
                self.network.set_link_state(interface, state)
            }
            DatapathInterfaceScope::Pair { first, second } => {
                self.network.set_link_state(first, state);
                self.network.set_link_state(second, state);
            }
        }
    }

    pub(super) fn reported_active_tx_interface(&self) -> NetworkInterfaceId {
        let reported = self.services.active_tx_interface();
        match (self.interfaces, reported) {
            (DatapathInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (DatapathInterfaceScope::Pair { .. }, None) => {
                panic!("paired DATAPATH services must identify role-generated TX")
            }
            (_, Some(_)) => panic!("services reported TX outside the DATAPATH scope"),
        }
    }

    pub(super) fn retained_prepared_tx_interface(&self) -> NetworkInterfaceId {
        if let Some(interface) = self.prepared_tx_interface {
            return interface;
        }
        let reported = self.services.prepared_tx_interface();
        match (self.interfaces, reported) {
            (DatapathInterfaceScope::Single(interface), None) => interface,
            (_, Some(interface)) if self.interfaces.contains(interface) => interface,
            (DatapathInterfaceScope::Pair { .. }, None) => {
                panic!("paired DATAPATH services must identify prepared TX")
            }
            (_, Some(_)) => panic!("services reported prepared TX outside the DATAPATH scope"),
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
    DatapathRunner<
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
    N: DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: DatapathServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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

#[cfg(test)]
mod fairness_tests {
    use super::{charge_pair_tx_frames, select_pair_tx_slot};

    #[test]
    fn unequal_aggregate_sizes_receive_equal_frame_service() {
        let mut served = [0_u64; 2];
        assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
        charge_pair_tx_frames(&mut served, 0, 32);
        assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
        charge_pair_tx_frames(&mut served, 1, 16);
        assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
        charge_pair_tx_frames(&mut served, 1, 16);
        assert_eq!(served, [0, 0]);
        assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
    }
}
