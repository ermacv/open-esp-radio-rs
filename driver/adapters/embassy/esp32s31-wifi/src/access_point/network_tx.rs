//! AP-owned network TX transaction.
//!
//! WDEV schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;

struct PreparedStandby {
    admission: Esp32s31ApAggregateAdmission,
    policy: HtAmpduTxRolePolicy,
    admitted: usize,
    preparation_micros: u64,
}

pub struct Esp32s31AccessPointNetworkTx<'observer, B> {
    observer: Option<&'observer dyn AggregateTxObserver>,
    deadline_micros: Option<u64>,
    exchange_started_micros: Option<u64>,
    prepared_first: Option<B>,
    prepared_second: Option<B>,
    prepared_standby: Option<PreparedStandby>,
    last_started_frames: usize,
}

impl<'observer, B> Esp32s31AccessPointNetworkTx<'observer, B>
where
    B: StableDmaBacking,
{
    pub const fn new(observer: Option<&'observer dyn AggregateTxObserver>) -> Self {
        Self {
            observer,
            deadline_micros: None,
            exchange_started_micros: None,
            prepared_first: None,
            prepared_second: None,
            prepared_standby: None,
            last_started_frames: 1,
        }
    }

    pub(super) const fn aggregate_pending(&self) -> bool {
        self.deadline_micros.is_some()
    }

    pub(super) fn has_prepared(&self) -> bool {
        self.prepared_first.is_some()
            || self.prepared_second.is_some()
            || self.prepared_standby.is_some()
    }

    pub(super) fn prepared_frame_count(&self) -> usize {
        self.prepared_standby.as_ref().map_or(
            usize::from(self.prepared_first.is_some())
                + usize::from(self.prepared_second.is_some()),
            |batch| batch.admitted,
        )
    }

    pub(super) const fn last_started_frame_count(&self) -> usize {
        self.last_started_frames
    }

    pub(super) fn can_prepare<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &self,
        aggregate: &Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
    ) -> bool {
        if !aggregate.has_standby() {
            return false;
        }
        match self.prepared_standby.as_ref() {
            Some(batch) => {
                self.prepared_first.is_none()
                    && batch.admitted < usize::from(batch.policy.frame_limit())
            }
            None => {
                (self.deadline_micros.is_some() || self.prepared_first.is_some())
                    && self.prepared_second.is_none()
            }
        }
    }
}

impl<
    'observer,
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
>
    Esp32s31AccessPointNetworkTx<
        'observer,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >
where
    M: RawMutex,
{
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointWdevError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        self.last_started_frames = 1;
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }
        let admission = control.mac.aggregate_admission(frame.as_slice());

        if let Some(admission) = admission
            && network.queue_len() != 0
            && let Some(mut second) = network.try_receive()
        {
            if let Some(observer) = self.observer {
                observer.observe_access_point_network_claim(second.as_slice());
            }
            let preparation_started = self.observer.map(|_| Instant::now().as_micros());
            if !admission.accepts_ethernet(second.as_slice()) {
                // This lease was older than every frame still in the network
                // queue. Retain it locally for the next transaction; putting
                // it on the channel tail would reorder one VIF's UDP stream.
                debug_assert!(self.prepared_first.is_none());
                self.prepared_first = Some(second);
                return control
                    .start_network_tx(hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointWdevError::Control);
            }

            let peer = admission.peer();
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
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
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(
                        error,
                    ))
                })?;
            let policy = admission
                .bind_policy(first_encoded.hardware_key_selector, SLOTS)
                .map_err(Esp32s31ApAmpduError::from)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            let active = aggregate.active_mut();
            active
                .begin(
                    peer,
                    policy.rate(),
                    first_encoded.sequence_number,
                    policy.role().hardware_key_selector,
                )
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            active
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
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(
                        error,
                    ))
                })?;
            active
                .push(peer, second, second_encoded)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;

            let target = usize::from(policy.frame_limit());
            let mut admitted = 2_usize;
            while admitted < target {
                let Some(mut next) = network.try_receive() else {
                    break;
                };
                if let Some(observer) = self.observer {
                    observer.observe_access_point_network_claim(next.as_slice());
                }
                if !admission.accepts_ethernet(next.as_slice()) {
                    debug_assert!(self.prepared_first.is_none());
                    self.prepared_first = Some(next);
                    break;
                }
                let offset = next.ethernet_offset();
                let length = next.ethernet_length();
                let encoded = engine
                    .encode_aggregate_ethernet_in_place(peer, next.storage_mut(), offset, length)
                    .map_err(|error| {
                        Esp32s31AccessPointWdevError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?;
                active
                    .push(peer, next, encoded)
                    .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
                admitted += 1;
            }
            if let Some(observer) = self.observer {
                observe_aggregate_rate(observer, policy.rate());
                observer.observe(AggregateTxObservation::Prepared {
                    subframes: u8::try_from(admitted).unwrap_or(u8::MAX),
                    stop: if admitted == target {
                        AggregateBuildStop::FrameLimit
                    } else {
                        AggregateBuildStop::QueueEmpty
                    },
                });
                observer.observe(AggregateTxObservation::PreparationCompleted {
                    micros: Instant::now()
                        .as_micros()
                        .saturating_sub(preparation_started.unwrap_or(0)),
                });
            }
            let publication_started = self.observer.map(|_| Instant::now().as_micros());
            active
                .publish(ordinary, hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            if let Some(observer) = self.observer {
                let finished = Instant::now().as_micros();
                let started = publication_started.unwrap_or(finished);
                observer.observe(AggregateTxObservation::Published {
                    at_micros: started,
                    program_micros: finished.saturating_sub(started),
                });
                self.exchange_started_micros = Some(started);
            }
            let deadline_micros = ordinary
                .now_micros()
                .saturating_add(ordinary.publication_timeout_micros());
            self.deadline_micros = Some(deadline_micros);
            control.observe_ht_aggregate(policy.rate());
            self.last_started_frames = admitted;
            return Ok(WifiTxProgress::Pending);
        }

        control
            .start_network_tx(hardware, frame.as_slice())
            .map_err(Esp32s31AccessPointWdevError::Control)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        mut frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        _network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<(), Esp32s31AccessPointWdevError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        assert!(
            (self.aggregate_pending() || self.has_prepared()) && aggregate.has_standby(),
            "WDEV must check AP standby ownership before claiming another ordered lease"
        );
        let started = self.observer.map(|_| Instant::now().as_micros());
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }

        if let Some(batch) = self.prepared_standby.as_mut() {
            if !batch.admission.accepts_ethernet(frame.as_slice()) {
                debug_assert!(self.prepared_first.is_none());
                self.prepared_first = Some(frame);
                return Ok(());
            }
            let peer = batch.admission.peer();
            let offset = frame.ethernet_offset();
            let length = frame.ethernet_length();
            let encoded = control
                .mac
                .engine_mut()
                .encode_aggregate_ethernet_in_place(peer, frame.storage_mut(), offset, length)
                .map_err(|error| {
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(
                        error,
                    ))
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            batch.admitted += 1;
            batch.preparation_micros = batch.preparation_micros.saturating_add(
                self.observer
                    .map(|_| {
                        Instant::now()
                            .as_micros()
                            .saturating_sub(started.unwrap_or(0))
                    })
                    .unwrap_or(0),
            );
            return Ok(());
        }

        let Some(mut first) = self.prepared_first.take() else {
            self.prepared_first = Some(frame);
            return Ok(());
        };
        let admission = control.mac.aggregate_admission(first.as_slice());
        let Some(admission) =
            admission.filter(|admission| admission.accepts_ethernet(frame.as_slice()))
        else {
            debug_assert!(self.prepared_second.is_none());
            self.prepared_first = Some(first);
            self.prepared_second = Some(frame);
            return Ok(());
        };
        let peer = admission.peer();
        let first_offset = first.ethernet_offset();
        let first_length = first.ethernet_length();
        let first_encoded = control
            .mac
            .engine_mut()
            .encode_aggregate_ethernet_in_place(
                peer,
                first.storage_mut(),
                first_offset,
                first_length,
            )
            .map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(error))
            })?;
        let policy = admission
            .bind_policy(first_encoded.hardware_key_selector, SLOTS)
            .map_err(Esp32s31ApAmpduError::from)
            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
        let standby = aggregate.standby_mut().expect("checked standby arena");
        standby
            .begin(
                peer,
                policy.rate(),
                first_encoded.sequence_number,
                policy.role().hardware_key_selector,
            )
            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
        standby
            .push(peer, first, first_encoded)
            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
        let offset = frame.ethernet_offset();
        let length = frame.ethernet_length();
        let encoded = control
            .mac
            .engine_mut()
            .encode_aggregate_ethernet_in_place(peer, frame.storage_mut(), offset, length)
            .map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::from(error))
            })?;
        aggregate
            .standby_mut()
            .expect("checked standby arena")
            .push(peer, frame, encoded)
            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
        self.prepared_standby = Some(PreparedStandby {
            admission,
            policy,
            admitted: 2,
            preparation_micros: self
                .observer
                .map(|_| {
                    Instant::now()
                        .as_micros()
                        .saturating_sub(started.unwrap_or(0))
                })
                .unwrap_or(0),
        });
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::StandbyPrepared);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_prepared<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        network: &PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointWdevError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        while self.can_prepare(aggregate) {
            let Some(frame) = network.try_receive() else {
                break;
            };
            self.prepare(aggregate, control, frame, network)?;
        }
        let Some(batch) = self.prepared_standby.take() else {
            let Some(frame) = self.prepared_first.take() else {
                return Ok(WifiTxProgress::Complete);
            };
            self.prepared_first = self.prepared_second.take();
            return control
                .start_network_tx(hardware, frame.as_slice())
                .map_err(Esp32s31AccessPointWdevError::Control);
        };
        if let Some(observer) = self.observer {
            observe_aggregate_rate(observer, batch.policy.rate());
            observer.observe(AggregateTxObservation::Prepared {
                subframes: u8::try_from(batch.admitted).unwrap_or(u8::MAX),
                stop: if batch.admitted == usize::from(batch.policy.frame_limit()) {
                    AggregateBuildStop::FrameLimit
                } else {
                    AggregateBuildStop::QueueEmpty
                },
            });
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: batch.preparation_micros,
            });
        }
        let publication_started = self.observer.map(|_| Instant::now().as_micros());
        let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
        })?;
        aggregate
            .publish_standby(ordinary, hardware)
            .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
        let now = ordinary.now_micros();
        self.deadline_micros = Some(now.saturating_add(ordinary.publication_timeout_micros()));
        self.exchange_started_micros = publication_started;
        control.observe_ht_aggregate(batch.policy.rate());
        if let Some(observer) = self.observer {
            let finished = Instant::now().as_micros();
            let started = publication_started.unwrap_or(finished);
            observer.observe(AggregateTxObservation::Published {
                at_micros: started,
                program_micros: finished.saturating_sub(started),
            });
            observer.observe(AggregateTxObservation::StandbyPublished);
        }
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn cancel_prepared<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
    ) -> Result<(), Esp32s31AccessPointWdevError> {
        self.prepared_first = None;
        self.prepared_second = None;
        if self.prepared_standby.take().is_some() {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyCancelled);
            }
        }
        Ok(())
    }

    pub(super) async fn wait_deadline<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
    >(
        &mut self,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(deadline) = self.deadline_micros {
            let (_, ordinary) = control
                .mac
                .try_aggregate_adapter()
                .expect("aggregate publication leaves ordinary AP TX idle");
            ordinary.wait_until(deadline).await;
        } else {
            control.wait_tx_deadline().await;
        }
    }

    pub(super) async fn service<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<
            '_,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
            SLOTS,
            BUFFER_SIZE,
        >,
        control: &mut Esp32s31AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointWdevError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        if self.deadline_micros.is_none() {
            return control
                .service_tx(hardware, wake)
                .await
                .map_err(Esp32s31AccessPointWdevError::Control);
        }

        let service_event = AggregateTxServiceEvent::classify(wake).map_err(|error| {
            Esp32s31AccessPointWdevError::Aggregate(
                Esp32s31ApAmpduError::ConflictingInterruptEvents(error.events),
            )
        })?;
        if service_event == AggregateTxServiceEvent::Collision {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            if !aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?
            {
                return Err(Esp32s31AccessPointWdevError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            self.exchange_started_micros = None;
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::Collision);
            }
            return Ok(WifiTxProgress::Complete);
        }
        if matches!(
            service_event,
            AggregateTxServiceEvent::HardwareTimeout | AggregateTxServiceEvent::ExecutorDeadline
        ) {
            if !aggregate
                .active_mut()
                .begin_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?
            {
                return Err(Esp32s31AccessPointWdevError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            ordinary.after_micros(16).await;
            aggregate
                .active_mut()
                .finish_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            self.exchange_started_micros = None;
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::HardwareTimeout);
            }
            return Ok(WifiTxProgress::Complete);
        }

        let aggregate_progress = {
            let completion_started = self.observer.map(|_| Instant::now().as_micros());
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(error))
            })?;
            let progress = aggregate
                .active_mut()
                .service_completion(ordinary, hardware)
                .map_err(Esp32s31AccessPointWdevError::Aggregate)?;
            if let Esp32s31ApAmpduProgress::Republished(_) = progress
                && let Some(observer) = self.observer
            {
                let finished = Instant::now().as_micros();
                let started = completion_started.unwrap_or(finished);
                observer.observe(AggregateTxObservation::Published {
                    at_micros: started,
                    program_micros: finished.saturating_sub(started),
                });
            }
            progress
        };
        match aggregate_progress {
            Esp32s31ApAmpduProgress::Complete(completion) => {
                self.observe_completion(completion, false);
                self.deadline_micros = None;
                self.exchange_started_micros = None;
                Ok(WifiTxProgress::Complete)
            }
            Esp32s31ApAmpduProgress::Republished(completion) => {
                self.observe_completion(completion, true);
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    Esp32s31AccessPointWdevError::Control(Esp32s31AccessPointControlError::Mac(
                        error,
                    ))
                })?;
                self.deadline_micros = Some(
                    ordinary
                        .now_micros()
                        .saturating_add(ordinary.publication_timeout_micros()),
                );
                Ok(WifiTxProgress::Pending)
            }
            Esp32s31ApAmpduProgress::Pending => {
                if service_event == AggregateTxServiceEvent::Completion {
                    return Err(Esp32s31AccessPointWdevError::Aggregate(
                        Esp32s31ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    fn observe_completion(&self, completion: Esp32s31ApAmpduCompletion, republished: bool) {
        let Some(observer) = self.observer else {
            return;
        };
        observer.observe(AggregateTxObservation::BlockAckProcessed {
            tx_status: completion.tx_status,
            block_ack_received: completion.block_ack_received,
            control: completion.block_ack_control,
            first_sequence: completion.first_sequence,
            starting_sequence: completion.starting_sequence,
            subframes: completion.subframes,
            missing: completion.missing,
        });
        if !republished {
            observer.observe(AggregateTxObservation::Completed {
                acknowledged: completion.acknowledged,
                individual_retry: false,
            });
            if let Some(started) = self.exchange_started_micros {
                observer.observe(AggregateTxObservation::ExchangeCompleted {
                    micros: Instant::now().as_micros().saturating_sub(started),
                    publications: completion.aggregate_attempts,
                });
            }
        }
    }
}
