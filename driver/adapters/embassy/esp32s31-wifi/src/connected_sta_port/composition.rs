use super::*;

impl Esp32s31ConnectedStaPort {
    /// Bind the selected connected peer to the allocation-free staged RX
    /// protocol. The caller chooses the network/HIL sink but cannot replace
    /// the station identity or dispatcher policy.
    pub fn build_rx_protocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        const DEPTH: usize,
        const CAPACITY: usize,
        const SLOTS: usize,
        const REORDER_SLOTS: usize,
    >(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaRxProtocolResources<
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
        >,
    ) -> Esp32s31ConnectedRxProtocol<
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
        M: RawMutex,
        S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    {
        let mut protocol = Esp32s31ConnectedRxProtocol::new_with_reorder_slots(
            resources.frames,
            resources.irq,
            ConnectedRxDispatcher::new(plan.rx_config()),
            resources.sink,
            resources.mpdu,
            resources.ethernet,
            resources.runtime,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        if let Some(counters) = resources.pipeline_observer {
            protocol = protocol.with_pipeline_observer(counters);
        }
        match resources.reorder_scratch {
            Some(scratch) => protocol.with_rx_reorder_scratch(scratch),
            None => protocol,
        }
    }

    /// Move a quiescent control TX owner into its connected ordinary/A-MPDU
    /// owner. A busy control owner returns every authority unchanged.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn build_tx<
        'slot,
        'resources,
        M,
        P,
        E,
        T,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
        const AGGREGATE_SLOTS: usize,
        const AGGREGATE_BUFFER_SIZE: usize,
        const ORDINARY_BUFFER_SIZE: usize,
    >(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<
        Esp32s31ConnectedTx<
            'slot,
            'resources,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Esp32s31ConnectedStaTxHandoffFailure<
            'slot,
            'resources,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            P,
            E,
            T,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >
    where
        M: RawMutex,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        assert_eq!(
            resources.aggregate.primary().state(),
            TxSlotState::Free,
            "a connected epoch requires returned idle aggregate storage"
        );
        if let Some(standby) = resources.aggregate.standby() {
            assert_eq!(
                standby.state(),
                TxSlotState::Free,
                "a connected epoch requires returned idle standby aggregate storage"
            );
        }
        let handoff = ConnectedTxHandoff {
            key: resources.pairwise_key,
            sequences: resources.sequences,
            config: plan.single_mpdu_tx_config(),
        };
        let ordinary = match resources.control.try_into_connected(handoff) {
            Ok(ordinary) => ordinary,
            Err((control, handoff)) => {
                return Err(Esp32s31ConnectedStaTxHandoffFailure {
                    control,
                    handoff,
                    aggregate: resources.aggregate,
                    aggregate_tx_observer: resources.aggregate_tx_observer,
                });
            }
        };
        let mut tx =
            Esp32s31ConnectedTx::new(ordinary, resources.aggregate, plan.aggregate_tx_config())
                .expect(
                    "connected STA config and idle aggregate storage were validated before handoff",
                );
        if let Some(observer) = resources.aggregate_tx_observer {
            tx = tx.with_observer(observer);
        }
        Ok(tx)
    }

    /// Construct BlockAck, beacon-loss and RX-reorder control from the same
    /// connected plan used by RX and TX.
    pub fn build_control<'resources, M: RawMutex, const CAPACITY: usize>(
        plan: &Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaControlResources<'resources, M, CAPACITY>,
    ) -> Esp32s31ConnectedControl<'resources, M, CAPACITY> {
        let tx_block_ack = StaTxBlockAckSessions::new(
            plan.config.block_ack.tx_block_ack_window,
            plan.config.block_ack.tx_block_ack_negotiation_timeout_us,
            plan.config.block_ack.tid0_amsdu,
        )
        .expect("connected STA plan validated TX BlockAck policy");
        let mut control = Esp32s31ConnectedControl::new(
            resources.receiver,
            plan.link.bssid,
            plan.link.association_phy == StaAssociationPhy::He20,
            tx_block_ack,
        )
        .with_rx_block_ack_maximum_window(plan.config.block_ack.rx_block_ack_maximum_window)
        .expect("connected STA plan validated RX BlockAck policy")
        .with_rx_reorder_commands(resources.reorder_commands);
        control.enable_beacon_loss(plan.beacon_loss);
        if plan.config.block_ack.request_initial_tx_block_ack
            && matches!(plan.aggregate_tx_rate, TxPhyRate::Ht(_) | TxPhyRate::He(_))
        {
            control.queue_initial_tx_block_ack(
                plan.config.block_ack.tx_block_ack_negotiation_attempt_limit,
            );
        }
        control
    }

    /// Atomically compose the complete connected driver graph.
    ///
    /// TX is acquired first because it is the only fallible owner handoff
    /// after plan validation. If it is still active, every untouched
    /// hardware/RX/protocol/control owner is returned alongside the exact TX
    /// frontier. Only a successful TX handoff may consume scratch and mailbox
    /// resources into the long-running graph.
    #[allow(
        clippy::too_many_arguments,
        clippy::type_complexity,
        clippy::result_large_err
    )]
    pub fn compose<
        'slot,
        'resources,
        'queue,
        'pool,
        'scratch,
        'irq,
        'control,
        M,
        S,
        P,
        E,
        T,
        H,
        R,
        const RX_DEPTH: usize,
        const RX_CAPACITY: usize,
        const RX_SLOTS: usize,
        const REORDER_SLOTS: usize,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const TX_QUEUE_DEPTH: usize,
        const AGGREGATE_SLOTS: usize,
        const AGGREGATE_BUFFER_SIZE: usize,
        const ORDINARY_BUFFER_SIZE: usize,
        const CONTROL_CAPACITY: usize,
    >(
        plan: Esp32s31ConnectedStaPlan,
        hardware: H,
        rx: R,
        protocol: Esp32s31ConnectedStaRxProtocolResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            RX_DEPTH,
            RX_CAPACITY,
            RX_SLOTS,
            REORDER_SLOTS,
        >,
        tx: Esp32s31ConnectedStaTxResources<
            'slot,
            'resources,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
            AGGREGATE_SLOTS,
            AGGREGATE_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        control: Esp32s31ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
    ) -> Result<
        Esp32s31ConnectedStaDrivers<
            H,
            R,
            Esp32s31ConnectedTx<
                'slot,
                'resources,
                'resources,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                TX_QUEUE_DEPTH,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Esp32s31ConnectedControl<'control, M, CONTROL_CAPACITY>,
            Esp32s31ConnectedRxProtocol<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
        >,
        Esp32s31ConnectedStaCompositionFailure<
            H,
            R,
            Esp32s31ConnectedStaRxProtocolResources<
                'queue,
                'pool,
                'scratch,
                'irq,
                M,
                S,
                RX_DEPTH,
                RX_CAPACITY,
                RX_SLOTS,
                REORDER_SLOTS,
            >,
            Esp32s31ConnectedStaControlResources<'control, M, CONTROL_CAPACITY>,
            Esp32s31ConnectedStaTxHandoffFailure<
                'slot,
                'resources,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
                P,
                E,
                T,
                AGGREGATE_SLOTS,
                AGGREGATE_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
        >,
    >
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink<RX_CAPACITY, RX_SLOTS>,
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let tx = match Self::build_tx(&plan, tx) {
            Ok(tx) => tx,
            Err(tx) => {
                return Err(Esp32s31ConnectedStaCompositionFailure {
                    plan,
                    hardware,
                    rx,
                    protocol,
                    control,
                    tx,
                });
            }
        };
        let protocol = Self::build_rx_protocol(&plan, protocol);
        let control = Self::build_control(&plan, control);
        Ok(Self::assemble(
            plan,
            Esp32s31ConnectedStaDriverParts {
                hardware,
                rx,
                tx,
                control,
                protocol,
            },
        ))
    }

    /// Join the already prepared hardware/RX/TX/control owners into the only
    /// services accepted by [`crate::connected_runner::ConnectedRunner`].
    pub fn assemble<H, R, X, C, P>(
        plan: Esp32s31ConnectedStaPlan,
        parts: Esp32s31ConnectedStaDriverParts<H, R, X, C, P>,
    ) -> Esp32s31ConnectedStaDrivers<H, R, X, C, P> {
        Esp32s31ConnectedStaDrivers {
            services: Esp32s31ConnectedServices::with_control(
                parts.hardware,
                parts.rx,
                parts.tx,
                parts.control,
            ),
            protocol: parts.protocol,
            report: Esp32s31ConnectedStaReport {
                link: plan.link,
                data_tx_rate: plan.data_tx_rate,
                aggregate_tx_rate: plan.aggregate_tx_rate,
            },
        }
    }
}
