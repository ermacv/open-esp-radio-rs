//! Embassy WDEV binding for one active AP role.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct NetworkTxPending {
    started_micros: u64,
    attempts_before: u32,
}

pub(super) struct Esp32s31AccessPointWdevServices<
    'run,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    D,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
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
        D,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >,
    pub(super) hardware: &'run mut H,
    pub(super) aggregate:
        &'run mut Esp32s31AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    pub(super) aggregate_deadline_micros: Option<u64>,
    pub(super) status_observer: O,
    pub(super) security_material: S,
    pub(super) set_link_state: L,
    #[cfg(feature = "rx-delivery-observation")]
    pub(super) delivery_observer: Option<&'run dyn RxNetworkDeliveryObserver>,
    pub(super) last_status_revision: u32,
    pub(super) network_link_up: bool,
    pub(super) pending_network_rx: Option<usize>,
    pub(super) network_backpressure_since_micros: Option<u64>,
    pub(super) tx_pending_since_micros: Option<u64>,
    pub(super) network_tx_pending: Option<NetworkTxPending>,
    pub(super) next_control_delay_millis: u32,
}

impl<
    D,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
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
        D,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
        B,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
        AMPDU_SLOTS,
        AMPDU_BUFFER_SIZE,
    >
where
    D: Esp32s31RxFrontierDelay,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    O: FnMut(AccessPointServiceStatus),
    L: FnMut(LinkState),
    B: StableDmaBacking,
{
    fn tx_pending(&self) -> bool {
        self.aggregate_deadline_micros.is_some() || self.control.tx_pending()
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

    fn publish_network_rx(
        &self,
        network_rx: &mut dyn WdevNetworkRx,
        frame: &[u8],
    ) -> Result<(), RxEnqueueError> {
        #[cfg(not(feature = "rx-delivery-observation"))]
        {
            network_rx.try_send(frame)
        }
        #[cfg(feature = "rx-delivery-observation")]
        {
            let delivery = network_delivery_event(frame);
            let mut before_publish = || {
                if let (Some(observer), Some(event)) = (self.delivery_observer, delivery) {
                    observer.admitted(event);
                }
            };
            let result = network_rx.try_send_observed(frame, &mut before_publish);
            if let Err(error) = result
                && let (Some(observer), Some(event)) = (self.delivery_observer, delivery)
            {
                observer.dropped(event, error);
            }
            result
        }
    }
}

impl<
    'resources,
    M,
    D,
    P,
    E,
    T,
    H,
    O,
    S,
    L,
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
        D,
        P,
        E,
        T,
        H,
        O,
        S,
        L,
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
    D: Esp32s31RxFrontierDelay,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
    H: RxDma
        + TxHardware
        + Esp32s31ApRuntimeHardware
        + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    O: FnMut(AccessPointServiceStatus),
    S: FnMut() -> ([u8; 32], u64),
    L: FnMut(LinkState),
{
    type Error = Esp32s31AccessPointWdevError;
    type Exit = Infallible;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn WdevNetworkRx,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            if let Some(length) = self.pending_network_rx {
                match self.publish_network_rx(network_rx, &self.control.rx_frame[..length]) {
                    Ok(()) => {
                        self.pending_network_rx = None;
                        if let Some(started) = self.network_backpressure_since_micros.take() {
                            let elapsed = Instant::now().as_micros().saturating_sub(started);
                            self.control.report.maximum_network_backpressure_micros = self
                                .control
                                .report
                                .maximum_network_backpressure_micros
                                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
                        }
                    }
                    Err(RxEnqueueError::QueueFull) => {
                        return Ok(WdevRxProgress::NetworkBackpressured);
                    }
                    Err(RxEnqueueError::InvalidLength(error)) => {
                        return Err(Esp32s31AccessPointWdevError::Network(error));
                    }
                }
            }

            let mut serviced_descriptors = 0_usize;
            loop {
                if serviced_descriptors == COUNT {
                    return Ok(WdevRxProgress::ProbePending);
                }
                if self
                    .control
                    .beacon_publication_due(Instant::now().as_micros() as u32)
                {
                    return Ok(WdevRxProgress::ProbePending);
                }
                if self
                    .control
                    .receive
                    .service_continuation(self.hardware)
                    .map_err(|error| {
                        Esp32s31AccessPointWdevError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?
                    == Esp32s31RxFrontierContinuation::ProbePending
                {
                    Timer::after_micros(1).await;
                }
                let completed_before = self.control.report.completed_rx_descriptors;
                let service_started = Instant::now().as_micros();
                let (nonce, replay_counter) = (self.security_material)();
                let ethernet_length = self
                    .control
                    .service_rx(
                        self.hardware,
                        nonce,
                        replay_counter,
                        Instant::now().as_micros(),
                    )
                    .await
                    .map_err(Esp32s31AccessPointWdevError::Control)?;
                serviced_descriptors = serviced_descriptors.saturating_add(
                    usize::try_from(
                        self.control
                            .report
                            .completed_rx_descriptors
                            .saturating_sub(completed_before),
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

                if let Some(length) = ethernet_length {
                    match self.publish_network_rx(network_rx, &self.control.rx_frame[..length]) {
                        Ok(()) => {}
                        Err(RxEnqueueError::QueueFull) => {
                            self.pending_network_rx = Some(length);
                            self.network_backpressure_since_micros =
                                Some(Instant::now().as_micros());
                            return Ok(WdevRxProgress::NetworkBackpressured);
                        }
                        Err(RxEnqueueError::InvalidLength(error)) => {
                            return Err(Esp32s31AccessPointWdevError::Network(error));
                        }
                    }
                }
                if self.tx_pending() {
                    return Ok(WdevRxProgress::ProbePending);
                }
                if self
                    .control
                    .receive
                    .service_continuation(self.hardware)
                    .map_err(|error| {
                        Esp32s31AccessPointWdevError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?
                    == Esp32s31RxFrontierContinuation::ProbePending
                {
                    return Ok(WdevRxProgress::ProbePending);
                }
                if self.control.report.completed_rx_descriptors == completed_before {
                    return Ok(WdevRxProgress::Drained);
                }
            }
        }
    }

    fn service_rx_during_tx(&self) -> bool {
        false
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
        self.pending_network_rx = None;
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
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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
            let destination = frame
                .as_slice()
                .get(..6)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
            let agreement = destination
                .filter(|peer| peer[0] & 1 == 0)
                .and_then(|peer| {
                    self.control
                        .mac
                        .engine()
                        .tx_block_ack_agreement(peer)
                        .map(|agreement| (peer, agreement))
                });

            let progress = if let Some((peer, agreement)) = agreement
                && network.queue_len() != 0
                && let Some(mut second) = network.try_receive()
            {
                let second_peer = second
                    .as_slice()
                    .get(..6)
                    .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
                if second_peer != Some(peer) {
                    network.requeue(second);
                    self.control
                        .start_network_tx(self.hardware, frame.as_slice())
                        .map_err(Esp32s31AccessPointWdevError::Control)?
                } else {
                    let (engine, ordinary) =
                        self.control.mac.try_aggregate_adapter().map_err(|error| {
                            Esp32s31AccessPointWdevError::Control(
                                Esp32s31AccessPointControlError::Mac(error),
                            )
                        })?;
                    let rate = HtRate::new(
                        HtMcs::Mcs7,
                        HtGuardInterval::Long800Ns,
                        HtChannelWidth::Mhz20,
                    );
                    let first_offset = frame.ethernet_offset();
                    let first_length = frame.ethernet_length();
                    let first_encoded = engine
                        .encode_aggregate_ethernet_in_place(
                            peer,
                            frame.storage_mut(),
                            first_offset,
                            first_length,
                        )
                        .map_err(|error| {
                            Esp32s31AccessPointWdevError::Control(
                                Esp32s31AccessPointControlError::from(error),
                            )
                        })?;
                    let aggregate = self.aggregate.active_mut();
                    aggregate
                        .begin(
                            peer,
                            rate,
                            first_encoded.sequence_number,
                            first_encoded.hardware_key_selector,
                        )
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                    aggregate
                        .push(peer, frame, first_encoded)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?;

                    let second_offset = second.ethernet_offset();
                    let second_length = second.ethernet_length();
                    let second_encoded = engine
                        .encode_aggregate_ethernet_in_place(
                            peer,
                            second.storage_mut(),
                            second_offset,
                            second_length,
                        )
                        .map_err(|error| {
                            Esp32s31AccessPointWdevError::Control(
                                Esp32s31AccessPointControlError::from(error),
                            )
                        })?;
                    aggregate
                        .push(peer, second, second_encoded)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?;

                    let target = usize::from(agreement.window).min(AMPDU_SLOTS);
                    let mut admitted = 2_usize;
                    while admitted < target {
                        let Some(mut next) = network.try_receive() else {
                            break;
                        };
                        let next_peer = next
                            .as_slice()
                            .get(..6)
                            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok());
                        if next_peer != Some(peer) {
                            network.requeue(next);
                            break;
                        }
                        let offset = next.ethernet_offset();
                        let length = next.ethernet_length();
                        let encoded = engine
                            .encode_aggregate_ethernet_in_place(
                                peer,
                                next.storage_mut(),
                                offset,
                                length,
                            )
                            .map_err(|error| {
                                Esp32s31AccessPointWdevError::Control(
                                    Esp32s31AccessPointControlError::from(error),
                                )
                            })?;
                        aggregate
                            .push(peer, next, encoded)
                            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                        admitted += 1;
                    }
                    aggregate
                        .publish(ordinary, self.hardware)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                    self.aggregate_deadline_micros = Some(
                        ordinary
                            .now_micros()
                            .saturating_add(ordinary.publication_timeout_micros()),
                    );
                    WifiTxProgress::Pending
                }
            } else {
                self.control
                    .start_network_tx(self.hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointWdevError::Control)?
            };
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
            if let Some(deadline) = self.aggregate_deadline_micros {
                let (_, ordinary) = self
                    .control
                    .mac
                    .try_aggregate_adapter()
                    .expect("aggregate publication leaves ordinary AP TX idle");
                ordinary.wait_until(deadline).await;
            } else {
                self.control.wait_tx_deadline().await;
            }
        }
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
            let progress = if self.aggregate_deadline_micros.is_some() {
                let events = match wake {
                    WifiTxWake::Interrupt { events } => events,
                    WifiTxWake::Deadline => 0,
                };
                let tx_events =
                    events & (MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION);
                if tx_events.count_ones() > 1 {
                    return Err(Esp32s31AccessPointWdevError::Aggregate(
                        Esp32s31ApAmpduError::ConflictingInterruptEvents(tx_events),
                    ));
                }
                if tx_events == MAC_INT_COLLISION {
                    if !self
                        .aggregate
                        .active_mut()
                        .abort_collision(self.hardware)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?
                    {
                        return Err(Esp32s31AccessPointWdevError::Aggregate(
                            Esp32s31ApAmpduError::HardwareDidNotDetach,
                        ));
                    }
                    self.aggregate_deadline_micros = None;
                    WifiTxProgress::Complete
                } else if tx_events == MAC_INT_TX_TIMEOUT || matches!(wake, WifiTxWake::Deadline) {
                    if !self
                        .aggregate
                        .active_mut()
                        .begin_timeout_abort(self.hardware)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?
                    {
                        return Err(Esp32s31AccessPointWdevError::Aggregate(
                            Esp32s31ApAmpduError::HardwareDidNotDetach,
                        ));
                    }
                    let (_, ordinary) =
                        self.control.mac.try_aggregate_adapter().map_err(|error| {
                            Esp32s31AccessPointWdevError::Control(
                                Esp32s31AccessPointControlError::Mac(error),
                            )
                        })?;
                    ordinary.after_micros(16).await;
                    self.aggregate
                        .active_mut()
                        .finish_timeout_abort(self.hardware)
                        .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                    self.aggregate_deadline_micros = None;
                    WifiTxProgress::Complete
                } else {
                    let aggregate_progress = {
                        let (_, ordinary) =
                            self.control.mac.try_aggregate_adapter().map_err(|error| {
                                Esp32s31AccessPointWdevError::Control(
                                    Esp32s31AccessPointControlError::Mac(error),
                                )
                            })?;
                        self.aggregate
                            .active_mut()
                            .service_completion(ordinary, self.hardware)
                            .map_err(Esp32s31AccessPointWdevError::Aggregate)?
                    };
                    match aggregate_progress {
                        Esp32s31ApAmpduProgress::Complete => {
                            self.aggregate_deadline_micros = None;
                            WifiTxProgress::Complete
                        }
                        Esp32s31ApAmpduProgress::Republished => {
                            let (_, ordinary) =
                                self.control.mac.try_aggregate_adapter().map_err(|error| {
                                    Esp32s31AccessPointWdevError::Control(
                                        Esp32s31AccessPointControlError::Mac(error),
                                    )
                                })?;
                            self.aggregate_deadline_micros = Some(
                                ordinary
                                    .now_micros()
                                    .saturating_add(ordinary.publication_timeout_micros()),
                            );
                            WifiTxProgress::Pending
                        }
                        Esp32s31ApAmpduProgress::Pending => {
                            if tx_events == MAC_INT_TX_COMPLETE {
                                return Err(Esp32s31AccessPointWdevError::Aggregate(
                                    Esp32s31ApAmpduError::CompletionInterruptWithoutState,
                                ));
                            }
                            WifiTxProgress::Pending
                        }
                    }
                }
            } else {
                self.control
                    .service_tx(self.hardware, wake)
                    .await
                    .map_err(Esp32s31AccessPointWdevError::Control)?
            };
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
        AMPDU_SLOTS
    }
}
