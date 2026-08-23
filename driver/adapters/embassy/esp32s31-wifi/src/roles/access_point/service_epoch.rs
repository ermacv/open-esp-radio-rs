impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    /// Run the AP control plane until the caller publishes stop.
    ///
    /// A pending TX descriptor is always driven to a terminal edge before IRQ
    /// routing is masked. RX is then stopped cooperatively; `Busy` means the
    /// walker has not yet acknowledged the request and is retried without
    /// weakening ownership.
    pub async fn run_until_stopped<
        'resources,
        IR,
        NM,
        H,
        F,
        N,
        NR,
        Q,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const RX_QUEUE_DEPTH: usize,
        const TX_QUEUE_DEPTH: usize,
        const AMPDU_SLOTS: usize,
        const AMPDU_BUFFER_SIZE: usize,
    >(
        &mut self,
        hardware: &mut H,
        interrupts: &mut Esp32s31MacInterruptEpoch<'_, IR, NM>,
        platform: &IR::Platform,
        network: &NR,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            'resources,
            PinnedTxFrame<'resources, NM, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
        #[cfg(any(feature = "diagnostics", test))] aggregate_tx_observer: Option<
            &dyn AggregateTxObserver,
        >,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
        publish_shared_rx: Q,
        stop: F,
        mut status_observer: impl FnMut(AccessPointServiceStatus),
        security_material: N,
    ) -> Result<Esp32s31AccessPointRunObservation, Esp32s31AccessPointRunError<IR::Error>>
    where
        IR: MacInterruptRoute,
        NM: RawMutex,
        NR: crate::datapath::network::DatapathNetwork<
                'resources,
                NM,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                RX_QUEUE_DEPTH,
                TX_QUEUE_DEPTH,
            >,
        H: RxDma
            + TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
        F: Future<Output = ()>,
        N: FnMut() -> ([u8; 32], u64),
        Q: FnMut(u8),
    {
        network.set_link_state(
            crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            LinkState::Down,
        );
        self.start(hardware)
            .await
            .map_err(Esp32s31AccessPointRunError::Control)?;
        interrupts.mac_runtime().begin_rx_moderation();
        if let Err(error) = interrupts.activate(platform, MAC_COLD_RX_INTERRUPT_MASK) {
            interrupts.mac_runtime().end_rx_moderation();
            return Err(Esp32s31AccessPointRunError::InterruptActivate(error));
        }
        interrupts.mac_runtime().notify_rx_handoff();
        self.publish_beacon(hardware, Instant::now().as_micros())
            .map_err(Esp32s31AccessPointRunError::Control)?;
        let (last_status_revision, status, _) = self.role_observation();
        status_observer(status);
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = aggregate_tx_observer {
            observer.observe(AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational: false,
            });
        }
        #[cfg(any(feature = "diagnostics", test))]
        let network_tx = Esp32s31AccessPointNetworkTx::new(aggregate_tx_observer);
        #[cfg(not(any(feature = "diagnostics", test)))]
        let network_tx = Esp32s31AccessPointNetworkTx::new();
        let services = Esp32s31AccessPointDatapathServices {
            control: self,
            hardware,
            aggregate,
            network_tx,
            status_observer,
            security_material,
            set_link_state: |state| {
                network.set_link_state(crate::roles::concurrent::AP_NETWORK_INTERFACE_ID, state)
            },
            publish_shared_rx,
            #[cfg(any(feature = "diagnostics", test))]
            aggregate_tx_observer,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
            last_status_revision,
            network_link_up: false,
            #[cfg(any(feature = "diagnostics", test))]
            block_ack_observation: BlockAckObservationState::default(),
            #[cfg(feature = "diagnostics")]
            network_backpressure_since_micros: None,
            #[cfg(feature = "diagnostics")]
            tx_pending_since_micros: Some(Instant::now().as_micros()),
            #[cfg(feature = "diagnostics")]
            network_tx_pending: None,
            next_control_delay_millis: 1,
        };
        let mut runner = DatapathRunner::new(
            interrupts.mac_runtime(),
            network,
            crate::roles::concurrent::AP_NETWORK_INTERFACE_ID,
            services,
        );
        let exit = await_stack_boundary!(runner.run_until(stop)).map_err(|error| match error {
            Esp32s31AccessPointDatapathError::Control(error) => {
                Esp32s31AccessPointRunError::Control(error)
            }
            Esp32s31AccessPointDatapathError::Network(error) => {
                Esp32s31AccessPointRunError::Network(error)
            }
            Esp32s31AccessPointDatapathError::Aggregate(error) => {
                Esp32s31AccessPointRunError::Aggregate(error)
            }
        })?;
        let (_, mut services) = runner.into_parts();
        match exit {
            crate::datapath::DatapathRunnerExit::Stopped => {}
            crate::datapath::DatapathRunnerExit::Role(exit) => match exit {},
        }
        services.clear_block_ack_observation();
        drop(services);
        let _discarded_staged = self.protocol_rx.discard_queued();
        #[cfg(any(feature = "diagnostics", test))]
        let rx_scheduler = self.receive.scheduler_snapshot();
        observe_access_point!(self, observation, {
            observation.ignored_rx_frames = observation
                .ignored_rx_frames
                .saturating_add(u32::try_from(_discarded_staged).unwrap_or(u32::MAX));
            observation.retained_rx_descriptors = rx_scheduler
                .map(|snapshot| snapshot.observed_mask.count_ones())
                .unwrap_or(0);
        });
        let interrupt_drain = interrupts.quiesce(platform);
        interrupts.mac_runtime().end_rx_moderation();
        let _interrupt_drain =
            interrupt_drain.map_err(Esp32s31AccessPointRunError::InterruptQuiesce)?;
        loop {
            match self.stop(hardware) {
                Ok(()) => break,
                Err(Esp32s31AccessPointControlError::Receive(
                    open_esp_radio_esp32s31_wifi_mac::rx_pool::RxStageTransactionError::Ring(
                        open_esp_radio_esp32s31_wifi_mac::rx::RxRingError::Busy,
                    ),
                )) => yield_now().await,
                Err(error) => return Err(Esp32s31AccessPointRunError::Control(error)),
            }
        }
        Ok(Esp32s31AccessPointRunObservation {
            #[cfg(any(feature = "diagnostics", test))]
            interrupt_drain: _interrupt_drain,
            #[cfg(any(feature = "diagnostics", test))]
            rx_scheduler,
        })
    }

    /// Consume a quiescent AP service and return every reusable capability.
    /// Failure returns the exact service; no caller can manufacture stopped
    /// Wi-Fi while RX or TX remains active.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_finish<H>(
        self,
        hardware: &mut H,
    ) -> Result<
        Esp32s31AccessPointStopped<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            R,
            C,
            COUNT,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
            TX_BUFFER_SIZE,
        >,
        Self,
    >
    where
        H: Esp32s31ApRuntimeHardware + RxDma,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
    {
        if self.protocol_rx.queued_frames() != 0 {
            return Err(self);
        }
        let Self {
            receive,
            protocol_rx,
            role,
        } = self;
        let (processor, (), (), ()) = role.into_parts();
        let Esp32s31AccessPointProtocolStopped {
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        } = match processor.try_finish_paired(hardware) {
            Ok(stopped) => stopped,
            Err(processor) => {
                return Err(Self {
                    receive,
                    protocol_rx,
                    role: AccessPointRoleRuntime::standalone(processor),
                });
            }
        };
        Ok(Esp32s31AccessPointStopped {
            receive,
            protocol_rx,
            transmit,
            rx_frame,
            tx_frame,
            data_rx,
            rx_block_ack,
            rx_reorder,
            rx_reorder_storage,
            #[cfg(feature = "diagnostics")]
            observation_storage,
            engine,
        })
    }
}
