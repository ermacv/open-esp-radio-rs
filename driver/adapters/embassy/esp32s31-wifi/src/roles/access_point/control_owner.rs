/// Standalone AP composition of a physical RX transport and the
/// queue-independent AP protocol processor.
pub struct Esp32s31AccessPointControl<
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
> {
    receive: R,
    protocol_rx: C,
    role: AccessPointRoleRuntime<
        Esp32s31AccessPointProtocolProcessor<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        (),
        (),
        (),
    >,
}

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
> core::ops::Deref
    for Esp32s31AccessPointControl<
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
{
    type Target = Esp32s31AccessPointProtocolProcessor<
        'storage,
        'beacon,
        'slot,
        P,
        E,
        T,
        DMA_BUFFER_SIZE,
        TX_BUFFER_SIZE,
    >;

    fn deref(&self) -> &Self::Target {
        self.role.protocol()
    }
}

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
> core::ops::DerefMut
    for Esp32s31AccessPointControl<
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
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.role.protocol_mut()
    }
}

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
    pub fn new(
        receive: R,
        protocol_rx: C,
        mac: Esp32s31ApMac<'beacon, 'slot, P, E, T, TX_BUFFER_SIZE>,
        rx_frame: &'storage mut [u8],
        tx_frame: &'storage mut [u8],
        data_rx: &'storage mut Esp32s31ApRxDispatcher,
        rx_block_ack: &'storage Esp32s31StaApRxBlockAck,
        rx_reorder: &'storage mut Esp32s31AccessPointRxReorder<'storage, DMA_BUFFER_SIZE>,
        rx_reorder_storage: &'storage RxReorderFrameStorage<
            DMA_BUFFER_SIZE,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        #[cfg(feature = "diagnostics")]
        observation_storage: &'static mut AccessPointObservationStorage,
    ) -> Self {
        Self {
            receive,
            protocol_rx,
            role: AccessPointRoleRuntime::standalone(Esp32s31AccessPointProtocolProcessor::new(
                mac,
                rx_frame,
                tx_frame,
                data_rx,
                rx_block_ack,
                rx_reorder,
                rx_reorder_storage,
                #[cfg(feature = "diagnostics")]
                observation_storage,
            )),
        }
    }

    /// Attach the non-owning terminal observer to the standalone AP role.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_terminal_observer(
        mut self,
        observer: &'static dyn AccessPointTerminalObserver,
    ) -> Self {
        self.role.protocol_mut().terminal_observer = Some(observer);
        self
    }

    pub async fn start<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.start(hardware).await?;
        Ok(())
    }

    pub fn stop<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), Esp32s31AccessPointControlError>
    where
        R: AccessPointRxProducer<H, COUNT>,
    {
        self.receive.stop(hardware)?;
        Ok(())
    }

    /// Observe one RX descriptor without exposing its DMA ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.descriptor_snapshot(index)
    }

    /// Observe the live RX scheduler frontier without exposing ownership.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn rx_scheduler_snapshot(&self) -> Option<Esp32s31RxFrontierSchedulerSnapshot>
    where
        R: AccessPointRxProducerObservation<COUNT>,
    {
        self.receive.scheduler_snapshot()
    }

    /// Stage a complete finite DMA frontier before processing one AP frame.
    ///
    /// The common producer returns safe descriptors before AP parsing, WPA2,
    /// reorder, or network publication can extend the BUFFER_FULL interval.
    pub async fn service_rx<H, Q>(
        &mut self,
        hardware: &mut H,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now_micros: u64,
        publish_shared_rx: &mut Q,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<&dyn RxNetworkDeliveryObserver>,
    ) -> Result<DatapathRxProgress, Esp32s31AccessPointControlError>
    where
        H: RxDma + TxHardware + Esp32s31ApRuntimeHardware + RxBlockAckHardware,
        R: AccessPointRxProducer<H, COUNT>,
        C: AccessPointRxProtocolConsumer,
        Q: FnMut(u8),
    {
        let tx_pending = self.mac.tx_pending();
        self.apply_protocol_actions(hardware)?;
        let protocol_blocked = self.rx_batch_pending();
        let queued_frames = self.protocol_rx.queued_frames();
        let stage_progress = if should_observe_ap_rx_dma(protocol_blocked, queued_frames) {
            self.service_rx_dma(hardware).await?
        } else {
            DatapathRxProgress::ProbePending
        };

        // Vendor `wDev_ProcessFiq` services the RX-success frontier even when
        // a TX transaction is active. Preserve that ownership edge by moving
        // complete DMA units into independent staging first. Protocol work is
        // Management and EAPOL are deferred while TX owns the sole transmit
        // transaction because they may publish a response. Protected data is
        // protocol-only and must continue releasing the bounded staging pool.
        if protocol_blocked {
            return Ok(stage_progress);
        }
        if self.service_rx_reorder_expiry(now_micros)? {
            return Ok(DatapathRxProgress::ProbePending);
        }

        let staged_frame = if tx_pending {
            self.protocol_rx.try_receive_protected_data()
        } else {
            self.protocol_rx.try_receive()
        };
        let Some(staged_frame) = staged_frame else {
            return Ok(stage_progress);
        };
        self.serviced_rx_frames = self.serviced_rx_frames.saturating_add(1);
        #[cfg(feature = "diagnostics")]
        let protocol_started = Instant::now().as_micros();
        let protocol_class = self.service_staged_rx(
            if rx_protocol_consumer_has_hardware(tx_pending) {
                Some(hardware)
            } else {
                None
            },
            staged_frame,
            AccessPointRxPublication::SharedStaging,
            authenticator_nonce,
            initial_replay_counter,
            now_micros,
            publish_shared_rx,
            #[cfg(feature = "diagnostics")]
            delivery_observer,
        )?;
        // `service_staged_rx` receives no hardware capability while TX is
        // active. Only after that protocol borrow ends does this radio owner
        // translate and execute its value-only mailbox requests.
        self.apply_protocol_actions(hardware)?;
        #[cfg(not(feature = "diagnostics"))]
        let _ = protocol_class;
        #[cfg(feature = "diagnostics")]
        self.observe_rx_protocol_service(
            protocol_class,
            Instant::now().as_micros().saturating_sub(protocol_started),
        );

        Ok(
            if self.protocol_rx.queued_frames() != 0
                || self.rx_batch_pending()
                || self.mac.tx_pending()
            {
                DatapathRxProgress::ProbePending
            } else {
                stage_progress
            },
        )
    }

    /// Drain the hardware RX completion frontier into independently owned
    /// staging slots without parsing a frame or producing a control action.
    /// This is the only AP RX operation allowed to touch DMA hardware while
    /// TX owns the shared MAC transaction domain. The separate protocol
    /// consumer may parse protected data but can only publish typed actions.
    pub async fn service_rx_dma<H>(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathRxProgress, Esp32s31AccessPointControlError>
    where
        H: RxDma,
        R: AccessPointRxProducer<H, COUNT>,
    {
        #[cfg(feature = "diagnostics")]
        let started = Instant::now().as_micros();
        let progress = self.receive.stage_completed(hardware).await?;
        #[cfg(feature = "diagnostics")]
        {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            self.observer.observation.maximum_rx_dma_service_micros = self
                .observer
                .observation
                .maximum_rx_dma_service_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.total_rx_dma_service_micros = self
                .observer
                .observation
                .total_rx_dma_service_micros
                .saturating_add(u32::try_from(elapsed).unwrap_or(u32::MAX));
            self.observer.observation.rx_dma_service_calls = self
                .observer
                .observation
                .rx_dma_service_calls
                .saturating_add(1);
        }
        self.serviced_rx_descriptors = self.receive.serviced_descriptors();
        Ok(progress)
    }
}
