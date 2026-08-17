//! Embassy WDEV binding for one active AP role.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct NetworkTxPending {
    started_micros: u64,
    attempts_before: u32,
}

#[derive(Default)]
struct BoundedRxTurn<const LIMIT: usize> {
    observation_passes: usize,
    serviced_staged_frames: usize,
}

impl<const LIMIT: usize> BoundedRxTurn<LIMIT> {
    fn observe(&mut self, serviced_staged_frames: usize) {
        self.observation_passes = self.observation_passes.saturating_add(1);
        self.serviced_staged_frames = self
            .serviced_staged_frames
            .saturating_add(serviced_staged_frames);
    }

    fn has_budget(&self) -> bool {
        self.observation_passes < LIMIT && self.serviced_staged_frames < LIMIT
    }
}

const fn owned_rx_ordering_blocked(barrier: bool, copied_queue_len: usize) -> bool {
    barrier && copied_queue_len != 0
}

#[derive(Default)]
pub(super) struct BlockAckObservationState {
    operational: bool,
}

impl BlockAckObservationState {
    fn update(&mut self, operational: bool, observer: Option<&dyn AggregateTxObserver>) {
        if operational == self.operational {
            return;
        }
        if let Some(observer) = observer {
            observer.observe(AggregateTxObservation::BlockAckOperational {
                tid: 0,
                operational,
            });
        }
        self.operational = operational;
    }
}

pub(super) struct Esp32s31AccessPointWdevServices<
    'run,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    RX,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    Q,
    B: 'ampdu,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
> {
    pub(super) control: &'run mut Esp32s31AccessPointControl<
        'storage,
        'beacon,
        'slot,
        RX,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >,
    pub(super) hardware: &'run mut H,
    pub(super) network_tx:
        Esp32s31AccessPointNetworkTx<'run, 'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    pub(super) status_observer: O,
    pub(super) security_material: S,
    pub(super) set_link_state: L,
    pub(super) publish_shared_rx: Q,
    pub(super) aggregate_tx_observer: Option<&'run dyn AggregateTxObserver>,
    #[cfg(feature = "rx-delivery-observation")]
    pub(super) delivery_observer: Option<&'run dyn RxNetworkDeliveryObserver>,
    pub(super) last_status_revision: u32,
    pub(super) network_link_up: bool,
    pub(super) block_ack_observation: BlockAckObservationState,
    pub(super) network_backpressure_since_micros: Option<u64>,
    /// A cold/reordered frame was published through the copied queue. Since
    /// the network device otherwise prioritizes shared zero-copy slots, no
    /// later shared frame may be published until this queue drains.
    pub(super) owned_rx_ordering_barrier: bool,
    pub(super) tx_pending_since_micros: Option<u64>,
    pub(super) network_tx_pending: Option<NetworkTxPending>,
    pub(super) next_control_delay_millis: u32,
}

impl<
    RX,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    Q,
    B,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    Esp32s31AccessPointWdevServices<
        '_,
        '_,
        '_,
        '_,
        '_,
        RX,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
        Q,
        B,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
        AMPDU_SLOTS,
        AMPDU_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    O: FnMut(AccessPointServiceStatus),
    L: FnMut(LinkState),
    Q: FnMut(u8),
    B: StableDmaBacking,
{
    fn tx_pending(&self) -> bool {
        self.network_tx.aggregate_pending() || self.control.tx_pending()
    }

    fn observe_role_state(&mut self) {
        let (status_revision, status, link_state) = self.control.role_observation();
        if status_revision != self.last_status_revision {
            (self.status_observer)(status);
            self.last_status_revision = status_revision;
        }
        let authorized = link_state == LinkState::Up;
        if authorized != self.network_link_up {
            (self.set_link_state)(if authorized {
                LinkState::Up
            } else {
                LinkState::Down
            });
            self.network_link_up = authorized;
        }
        self.block_ack_observation.update(
            self.control.has_operational_tx_block_ack(),
            self.aggregate_tx_observer,
        );
    }

    pub(super) fn clear_block_ack_observation(&mut self) {
        self.block_ack_observation
            .update(false, self.aggregate_tx_observer);
    }

    fn observe_tx_started(&mut self) {
        if self.tx_pending() && self.tx_pending_since_micros.is_none() {
            self.tx_pending_since_micros = Some(Instant::now().as_micros());
        }
    }

    fn observe_tx_terminal(&mut self) {
        if !self.tx_pending()
            && let Some(started) = self.tx_pending_since_micros.take()
        {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            self.control.report.maximum_tx_pending_micros = self
                .control
                .report
                .maximum_tx_pending_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
    }

    fn observe_network_tx_terminal(&mut self) {
        let Some(pending) = self.network_tx_pending.take() else {
            return;
        };
        let elapsed = Instant::now()
            .as_micros()
            .saturating_sub(pending.started_micros);
        let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
        if elapsed > self.control.report.maximum_network_tx_pending_micros {
            self.control.report.maximum_network_tx_pending_micros = elapsed;
            self.control.report.network_tx_attempts_at_maximum_pending = self
                .control
                .mac_report()
                .data_tx
                .attempts
                .saturating_sub(pending.attempts_before)
                .try_into()
                .unwrap_or(u8::MAX);
        }
    }

    /// Publish the retained AP Ethernet batch without consuming its cursor on
    /// backpressure. The WDEV scheduler may service beacon/control/TX work and
    /// retry the exact same record after the network capacity edge.
    fn publish_pending_rx_batch(
        &mut self,
        network: &mut dyn WdevNetworkRx,
    ) -> Result<WdevRxProgress, Esp32s31AccessPointWdevError> {
        while let Some(record) = self
            .control
            .rx_batch_record()
            .map_err(Esp32s31AccessPointWdevError::Control)?
        {
            let frame = record.frame;
            let next_offset = record.next_offset;
            #[cfg(not(feature = "rx-delivery-observation"))]
            let result = network.try_send_parts(frame);
            #[cfg(feature = "rx-delivery-observation")]
            let result = {
                let delivery = RxNetworkDeliveryEvent { frame, raw: None };
                let observer = self.delivery_observer;
                let mut before_publish = || {
                    if let Some(observer) = observer {
                        observer.admitted(delivery);
                    }
                };
                network.try_send_parts_observed(frame, &mut before_publish)
            };

            match result {
                Ok(()) => {
                    self.owned_rx_ordering_barrier = true;
                    let protocol = ethernet_parts_protocol(frame);
                    self.control.commit_rx_batch_record(next_offset);
                    self.control.report.ethernet_frames_staged =
                        self.control.report.ethernet_frames_staged.saturating_add(1);
                    match protocol {
                        Some(EthernetProtocol::ArpRequest) => {
                            self.control.report.ethernet_arp_requests_staged = self
                                .control
                                .report
                                .ethernet_arp_requests_staged
                                .saturating_add(1);
                        }
                        Some(EthernetProtocol::Ipv4Tcp) => {
                            self.control.report.ethernet_tcp_frames_staged = self
                                .control
                                .report
                                .ethernet_tcp_frames_staged
                                .saturating_add(1);
                        }
                        _ => {}
                    }
                }
                Err(RxEnqueueError::QueueFull) => {
                    self.network_backpressure_since_micros
                        .get_or_insert_with(|| Instant::now().as_micros());
                    return Ok(WdevRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    #[cfg(feature = "rx-delivery-observation")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent { frame, raw: None },
                            RxEnqueueError::InvalidLength(error),
                        );
                    }
                    return Err(Esp32s31AccessPointWdevError::Network(error));
                }
            }
        }

        if let Some(started) = self.network_backpressure_since_micros.take() {
            let elapsed = Instant::now().as_micros().saturating_sub(started);
            self.control.report.maximum_network_backpressure_micros = self
                .control
                .report
                .maximum_network_backpressure_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
        Ok(WdevRxProgress::Drained)
    }
}

impl<
    'resources,
    M,
    RX,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
    Q,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
> WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    for Esp32s31AccessPointWdevServices<
        '_,
        '_,
        '_,
        '_,
        'resources,
        RX,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
        Q,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
        AMPDU_SLOTS,
        AMPDU_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: RxDma
        + TxHardware
        + Esp32s31ApRuntimeHardware
        + RxBlockAckHardware
        + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    RX: AccessPointRxPipeline<H, COUNT>,
    O: FnMut(AccessPointServiceStatus),
    S: FnMut() -> ([u8; 32], u64),
    L: FnMut(LinkState),
    Q: FnMut(u8),
{
    type Error = Esp32s31AccessPointWdevError;
    type Exit = Infallible;

    fn service_rx_during_tx(&self) -> bool {
        // AP RX owns protocol actions as well as DMA drainage. Processing an
        // action frame can publish a management response through the same
        // ordinary TX owner currently borrowed by A-MPDU. Preserve the IRQ
        // edge and run RX immediately after the active TX terminal edge.
        false
    }

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            if owned_rx_ordering_blocked(self.owned_rx_ordering_barrier, network_rx.queue_len()) {
                return Ok(WdevRxProgress::ProbePending);
            }
            if self.owned_rx_ordering_barrier {
                self.owned_rx_ordering_barrier = false;
            }
            let mut turn = BoundedRxTurn::<COUNT>::default();
            loop {
                let network_backpressured = self.publish_pending_rx_batch(network_rx)?
                    == WdevRxProgress::NetworkBackpressured;
                if owned_rx_ordering_blocked(self.owned_rx_ordering_barrier, network_rx.queue_len())
                {
                    return Ok(WdevRxProgress::ProbePending);
                }
                if !turn.has_budget() {
                    return Ok(if network_backpressured {
                        WdevRxProgress::NetworkBackpressured
                    } else {
                        WdevRxProgress::ProbePending
                    });
                }
                if self
                    .control
                    .beacon_publication_due(Instant::now().as_micros() as u32)
                {
                    return Ok(if network_backpressured {
                        WdevRxProgress::NetworkBackpressured
                    } else {
                        WdevRxProgress::ProbePending
                    });
                }
                let completed_before = self.control.report.completed_rx_descriptors;
                let serviced_before = self.control.report.serviced_staged_rx_frames;
                let service_started = Instant::now().as_micros();
                let (nonce, replay_counter) = (self.security_material)();
                let rx_progress = self
                    .control
                    .service_rx(
                        self.hardware,
                        nonce,
                        replay_counter,
                        Instant::now().as_micros(),
                        &mut self.publish_shared_rx,
                        #[cfg(feature = "rx-delivery-observation")]
                        self.delivery_observer,
                    )
                    .await
                    .map_err(Esp32s31AccessPointWdevError::Control)?;
                turn.observe(
                    usize::try_from(
                        self.control
                            .report
                            .serviced_staged_rx_frames
                            .saturating_sub(serviced_before),
                    )
                    .unwrap_or(usize::MAX),
                );
                let service_elapsed = Instant::now().as_micros().saturating_sub(service_started);
                self.control.report.maximum_rx_service_micros = self
                    .control
                    .report
                    .maximum_rx_service_micros
                    .max(u32::try_from(service_elapsed).unwrap_or(u32::MAX));
                self.observe_role_state();
                self.observe_tx_started();

                // A retained Ethernet record must not block the lower DMA
                // ownership frontier. `control.service_rx` stages and
                // republishes completed descriptors before it observes
                // `rx_batch_pending`, so the AP retains the same separation
                // between descriptor drainage and upper delivery as the STA
                // producer while the final network queue is full.
                if network_backpressured {
                    return Ok(WdevRxProgress::NetworkBackpressured);
                }
                if self.publish_pending_rx_batch(network_rx)?
                    == WdevRxProgress::NetworkBackpressured
                {
                    return Ok(WdevRxProgress::NetworkBackpressured);
                }
                if self.tx_pending() {
                    // This RX turn started at an idle boundary but protocol
                    // handling published a control response. Leave further
                    // queued work visible until that response reaches its
                    // terminal TX edge.
                    return Ok(rx_progress);
                }
                if rx_progress == WdevRxProgress::ProbePending {
                    continue;
                }
                if self.control.report.completed_rx_descriptors == completed_before {
                    return Ok(WdevRxProgress::Drained);
                }
            }
        }
    }

    fn has_rx_work(&self) -> bool {
        self.control.rx_work_due(Instant::now().as_micros())
    }

    fn service_control<'a>(
        &'a mut self,
        _context: WdevControlContext,
    ) -> impl Future<Output = Result<WdevControlProgress<Self::Exit>, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        async move {
            let progress = self
                .control
                .service_control(self.hardware, Instant::now().as_micros())
                .map_err(Esp32s31AccessPointWdevError::Control)?;
            if progress == WdevControlProgress::Idle {
                self.next_control_delay_millis = self
                    .control
                    .next_control_delay_millis(Instant::now().as_micros())
                    .map_err(Esp32s31AccessPointWdevError::Control)?;
            }
            self.observe_role_state();
            self.observe_tx_started();
            Ok(progress)
        }
    }

    fn service_stop(&mut self) -> Result<WdevStopProgress, Self::Error> {
        let progress = self
            .control
            .service_stop(self.hardware)
            .map_err(Esp32s31AccessPointWdevError::Control)?;
        self.observe_role_state();
        self.observe_tx_started();
        Ok(progress)
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        Timer::after_millis(u64::from(self.next_control_delay_millis))
    }

    fn start_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = self
                .network_tx
                .start(self.control, self.hardware, frame, network)
                .await?;
            if progress == WifiTxProgress::Pending {
                debug_assert!(self.network_tx_pending.is_none());
                self.network_tx_pending = Some(NetworkTxPending {
                    started_micros: Instant::now().as_micros(),
                    attempts_before: self.control.mac_report().data_tx.attempts,
                });
            }
            self.observe_tx_started();
            Ok(progress)
        }
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        async move {
            self.network_tx.wait_deadline(self.control).await;
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.network_tx.has_prepared()
    }

    fn prepared_tx_frame_count(&self) -> usize {
        self.network_tx.prepared_frame_count()
    }

    fn can_prepare_tx(&self) -> bool {
        self.network_tx.can_prepare()
    }

    fn prepare_tx<'a>(
        &'a mut self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move { self.network_tx.prepare(self.control, frame, network) }
    }

    fn start_prepared_tx<'a>(
        &'a mut self,
        network: &'a PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = self
                .network_tx
                .start_prepared(self.control, self.hardware, network)?;
            if progress == WifiTxProgress::Pending {
                debug_assert!(self.network_tx_pending.is_none());
                self.network_tx_pending = Some(NetworkTxPending {
                    started_micros: Instant::now().as_micros(),
                    attempts_before: self.control.mac_report().data_tx.attempts,
                });
            }
            self.observe_tx_started();
            Ok(progress)
        }
    }

    fn cancel_prepared_tx(&mut self) -> Result<(), Self::Error> {
        self.network_tx.cancel_prepared()
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            match wake {
                WifiTxWake::Interrupt { .. } => {
                    self.control.report.tx_interrupt_wakes =
                        self.control.report.tx_interrupt_wakes.saturating_add(1);
                }
                WifiTxWake::Deadline => {
                    self.control.report.tx_deadline_wakes =
                        self.control.report.tx_deadline_wakes.saturating_add(1);
                }
            }
            let progress = self
                .network_tx
                .service(self.control, self.hardware, wake)
                .await?;
            self.observe_role_state();
            self.observe_tx_started();
            self.observe_tx_terminal();
            if progress == WifiTxProgress::Complete {
                self.observe_network_tx_terminal();
            }
            Ok(progress)
        }
    }

    fn preferred_tx_batch_size(&self) -> usize {
        if self.control.has_operational_tx_block_ack() {
            AMPDU_SLOTS
        } else {
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingObserver(std::sync::Mutex<std::vec::Vec<AggregateTxObservation>>);

    impl AggregateTxObserver for RecordingObserver {
        fn observe(&self, observation: AggregateTxObservation) {
            self.0.lock().unwrap().push(observation);
        }
    }

    #[test]
    fn staged_dma_burst_does_not_spend_unprocessed_protocol_budget() {
        let mut turn = BoundedRxTurn::<32>::default();

        // The producer may have staged all 32 DMA descriptors, but this turn
        // accounts only the one staged owner actually consumed by AP RX.
        turn.observe(1);

        assert!(turn.has_budget());
    }

    #[test]
    fn block_ack_readiness_is_published_only_on_live_state_edges() {
        let observer = RecordingObserver::default();
        let mut state = BlockAckObservationState::default();

        state.update(false, Some(&observer));
        state.update(true, Some(&observer));
        state.update(true, Some(&observer));
        state.update(false, Some(&observer));

        assert_eq!(
            *observer.0.lock().unwrap(),
            [
                AggregateTxObservation::BlockAckOperational {
                    tid: 0,
                    operational: true,
                },
                AggregateTxObservation::BlockAckOperational {
                    tid: 0,
                    operational: false,
                },
            ]
        );
    }

    #[test]
    fn bounded_rx_turn_yields_at_or_beyond_its_staged_frame_quota() {
        let mut exact = BoundedRxTurn::<4>::default();
        exact.observe(4);
        assert!(!exact.has_budget());

        let mut overshoot = BoundedRxTurn::<4>::default();
        overshoot.observe(5);
        assert!(!overshoot.has_budget());
    }

    #[test]
    fn bounded_rx_turn_yields_after_probes_without_staged_frame_progress() {
        let mut turn = BoundedRxTurn::<2>::default();

        turn.observe(0);
        assert!(turn.has_budget());
        turn.observe(0);

        assert!(!turn.has_budget());
    }

    #[test]
    fn copied_reorder_release_blocks_later_shared_publication_until_consumed() {
        assert!(!owned_rx_ordering_blocked(false, 1));
        assert!(!owned_rx_ordering_blocked(true, 0));
        assert!(owned_rx_ordering_blocked(true, 1));
    }

    struct NetworkRx {
        capacity: usize,
        frames: std::vec::Vec<std::vec::Vec<u8>>,
    }

    impl WdevNetworkRx for NetworkRx {
        fn queue_len(&self) -> usize {
            self.frames.len()
        }

        fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
            if self.frames.len() == self.capacity {
                return Err(RxEnqueueError::QueueFull);
            }
            self.frames.push(frame.to_vec());
            Ok(())
        }

        fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
            if self.frames.len() == self.capacity {
                return Err(RxEnqueueError::QueueFull);
            }
            let mut storage = std::vec![0; frame.length()];
            frame.copy_to(&mut storage).expect("test frame fits");
            self.frames.push(storage);
            Ok(())
        }

        #[cfg(feature = "rx-delivery-observation")]
        fn try_send_observed(
            &mut self,
            frame: &[u8],
            before_publish: &mut dyn FnMut(),
        ) -> Result<(), RxEnqueueError> {
            let result = self.try_send(frame);
            if result.is_ok() {
                before_publish();
            }
            result
        }

        #[cfg(feature = "rx-delivery-observation")]
        fn try_send_parts_observed(
            &mut self,
            frame: EthernetFrameParts<'_>,
            before_publish: &mut dyn FnMut(),
        ) -> Result<(), RxEnqueueError> {
            let result = self.try_send_parts(frame);
            if result.is_ok() {
                before_publish();
            }
            result
        }
    }

    fn pack(storage: &mut [u8], payloads: &[&[u8]]) -> usize {
        let mut writer = crate::ethernet_rx::PackedEthernetWriter::new(storage);
        for (index, payload) in payloads.iter().enumerate() {
            writer
                .push(EthernetFrameParts {
                    destination: [2, 0, 0, 0, 0, 1],
                    source: [2, 0, 0, 0, 0, 2],
                    ether_type: 0x0800 + index as u16,
                    payload,
                })
                .unwrap();
        }
        writer.used()
    }

    #[test]
    fn ap_batch_publishes_every_amsdu_subframe() {
        let mut storage = [0_u8; 128];
        let used = pack(&mut storage, &[&[0; 20], &[1; 20]]);
        let mut network = NetworkRx {
            capacity: 2,
            frames: std::vec::Vec::new(),
        };
        let mut offset = 0;
        while let Some(record) = crate::ethernet_rx::record_at(&storage, used, offset).unwrap() {
            network.try_send_parts(record.frame).unwrap();
            offset = record.next_offset;
        }

        assert_eq!(offset, used);
        assert_eq!(network.frames.len(), 2);
        assert_eq!(&network.frames[0][14..], &[0; 20]);
        assert_eq!(&network.frames[1][14..], &[1; 20]);
    }

    #[test]
    fn ap_batch_retries_the_same_record_after_backpressure() {
        let mut storage = [0_u8; 128];
        let used = pack(&mut storage, &[&[0; 20], &[1; 20]]);
        let mut network = NetworkRx {
            capacity: 1,
            frames: std::vec::Vec::new(),
        };
        let first = crate::ethernet_rx::record_at(&storage, used, 0)
            .unwrap()
            .unwrap();
        network.try_send_parts(first.frame).unwrap();
        let retained_offset = first.next_offset;
        let second = crate::ethernet_rx::record_at(&storage, used, retained_offset)
            .unwrap()
            .unwrap();
        assert_eq!(
            network.try_send_parts(second.frame),
            Err(RxEnqueueError::QueueFull)
        );

        // Network consumption releases capacity. The publication cursor was
        // not advanced by QueueFull, so the exact second record is retried.
        network.frames.clear();
        let retry = crate::ethernet_rx::record_at(&storage, used, retained_offset)
            .unwrap()
            .unwrap();
        assert_eq!(retry.frame.payload, &[1; 20]);
        network.try_send_parts(retry.frame).unwrap();

        assert_eq!(network.frames.len(), 1);
        assert_eq!(&network.frames[0][14..], &[1; 20]);
        assert_eq!(retry.next_offset, used);
    }
}
