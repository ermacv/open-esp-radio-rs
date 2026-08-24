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
        plan: &mut Esp32s31ConnectedStaPlan,
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
        let Esp32s31ConnectedStaRxProtocolResources {
            frames,
            irq,
            sink,
            mpdu,
            ethernet,
            reorder_commands,
            reorder_storage,
            runtime,
            reorder_scratch,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            #[cfg(any(feature = "diagnostics", test))]
            reorder_observer,
        } = resources;
        let processor = Self::build_rx_processor(
            plan,
            Esp32s31ConnectedStaRxProcessorResources {
                irq,
                sink,
                mpdu,
                ethernet,
                reorder_commands,
                reorder_storage,
                runtime,
                reorder_scratch,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
                #[cfg(any(feature = "diagnostics", test))]
                reorder_observer,
            },
        );
        Esp32s31ConnectedRxProtocol::from_processor(frames, processor)
    }

    /// Build connected-station RX policy without choosing a source queue.
    ///
    /// This is the shared composition point for standalone STA and paired
    /// STA+AP. Both receive the same dispatcher, reorder, control mailbox and
    /// evidence bindings; only the outer physical producer differs.
    pub fn build_rx_processor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        const CAPACITY: usize,
        const SLOTS: usize,
        const REORDER_SLOTS: usize,
    >(
        plan: &mut Esp32s31ConnectedStaPlan,
        resources: Esp32s31ConnectedStaRxProcessorResources<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> Esp32s31ConnectedRxProcessor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
    where
        M: RawMutex,
        S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    {
        let dispatcher = match plan.disable_esp_now_rx() {
            Some(epoch) => {
                ConnectedRxDispatcher::new(plan.rx_config()).with_esp_now_rx_epoch(epoch)
            }
            None => ConnectedRxDispatcher::new(plan.rx_config()),
        };
        #[cfg(any(feature = "diagnostics", test))]
        let mut processor = Esp32s31ConnectedRxProcessor::new_with_reorder_slots(
            resources.irq,
            dispatcher,
            resources.sink,
            resources.mpdu,
            resources.ethernet,
            resources.runtime,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        #[cfg(not(any(feature = "diagnostics", test)))]
        let processor = Esp32s31ConnectedRxProcessor::new_with_reorder_slots(
            resources.irq,
            dispatcher,
            resources.sink,
            resources.mpdu,
            resources.ethernet,
            resources.runtime,
        )
        .with_rx_reorder_commands(resources.reorder_commands)
        .with_rx_reorder_storage(resources.reorder_storage);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(counters) = resources.pipeline_observer {
            processor = processor.with_pipeline_observer(counters);
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = resources.reorder_observer {
            processor = processor.with_reorder_observer(observer);
        }
        match resources.reorder_scratch {
            Some(scratch) => processor.with_rx_reorder_scratch(scratch),
            None => processor,
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
        plan: &mut Esp32s31ConnectedStaPlan,
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
                    #[cfg(any(feature = "diagnostics", test))]
                    aggregate_tx_observer: resources.aggregate_tx_observer,
                    tx_block_ack_status_sink: resources.tx_block_ack_status_sink,
                });
            }
        };
        let he_trigger_based = plan.config.tx.he_trigger_based;
        let rate_control = plan
            .rate_control
            .take()
            .expect("a connected STA plan transfers rate control exactly once");
        let mut tx = Esp32s31ConnectedTx::new(
            ordinary,
            resources.aggregate,
            plan.aggregate_tx_config(),
            rate_control,
            plan.aggregate_rate_policy,
        )
        .expect("connected STA config and idle aggregate storage were validated before handoff")
        .with_he_trigger_based(he_trigger_based);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = resources.aggregate_tx_observer {
            tx = tx.with_observer(observer);
        }
        if let Some(sink) = resources.tx_block_ack_status_sink {
            tx = tx.with_block_ack_status_sink(sink);
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
        let mut control = Esp32s31ConnectedControl::new_shared(
            resources.receiver,
            plan.link.bssid,
            plan.link.association_phy == StaAssociationPhy::He20,
            tx_block_ack,
            resources.rx_block_ack,
        )
        .with_rx_block_ack_maximum_window(plan.config.block_ack.rx_block_ack_maximum_window)
        .expect("connected STA plan validated RX BlockAck policy")
        .with_rx_reorder_commands(resources.reorder_commands);
        control.enable_beacon_loss(plan.beacon_loss);
        if let Some(policy) = plan.power_save {
            control.enable_power_save(policy);
        }
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
        mut plan: Esp32s31ConnectedStaPlan,
        mut hardware: H,
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
        H: StaEspNowRxPolicyHardware,
    {
        let tx = match Self::build_tx(&mut plan, tx) {
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
        if plan.esp_now_rx_enabled() {
            configure_sta_esp_now_receive_policy(&mut hardware, plan.link.bssid);
        }
        let protocol = Self::build_rx_protocol(&mut plan, protocol);
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
    /// services accepted by [`crate::datapath::DatapathRunner`].
    pub fn assemble<H, R, X, C, P>(
        plan: Esp32s31ConnectedStaPlan,
        parts: Esp32s31ConnectedStaDriverParts<H, R, X, C, P>,
    ) -> Esp32s31ConnectedStaDrivers<H, R, X, C, P> {
        Esp32s31ConnectedStaDrivers {
            services: SingleRoleServices::with_control(
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
