//! AP-owned network TX transaction.
//!
//! DATAPATH schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;
#[cfg(not(any(feature = "diagnostics", test)))]
use core::marker::PhantomData;

struct PreparedStandby {
    admission: Esp32s31ApAggregateAdmission,
    policy: HtAmpduTxRolePolicy,
    admitted: usize,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Default)]
struct PreparedSchedulerTraceBuilder {
    active_service_returned_micros: Option<u64>,
    scheduler_loop_resumed_micros: Option<u64>,
    stop_poll_completed_micros: Option<u64>,
    control_readiness_checked_micros: Option<u64>,
    prepared_entry_micros: Option<u64>,
    scheduler_passes: u8,
    control_ready_passes: u8,
}

#[cfg(any(feature = "diagnostics", test))]
impl PreparedSchedulerTraceBuilder {
    fn mark(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        match phase {
            PreparedTxSchedulerPhase::ActiveServiceReturned => {
                self.active_service_returned_micros.get_or_insert(at_micros);
            }
            PreparedTxSchedulerPhase::SchedulerLoopResumed => {
                self.scheduler_loop_resumed_micros = Some(at_micros);
                self.scheduler_passes = self.scheduler_passes.saturating_add(1);
            }
            PreparedTxSchedulerPhase::StopPollCompleted => {
                self.stop_poll_completed_micros = Some(at_micros);
            }
            PreparedTxSchedulerPhase::ControlReadinessChecked { ready } => {
                self.control_readiness_checked_micros = Some(at_micros);
                self.control_ready_passes =
                    self.control_ready_passes.saturating_add(u8::from(ready));
            }
            PreparedTxSchedulerPhase::PreparedEntry => {
                self.prepared_entry_micros = Some(at_micros);
            }
        }
    }

    fn complete(self) -> Option<PreparedTxSchedulerTrace> {
        Some(PreparedTxSchedulerTrace {
            active_service_returned_micros: self.active_service_returned_micros?,
            scheduler_loop_resumed_micros: self.scheduler_loop_resumed_micros?,
            stop_poll_completed_micros: self.stop_poll_completed_micros?,
            control_readiness_checked_micros: self.control_readiness_checked_micros?,
            prepared_entry_micros: self.prepared_entry_micros?,
            scheduler_passes: self.scheduler_passes,
            control_ready_passes: self.control_ready_passes,
        })
    }
}

pub struct Esp32s31AccessPointNetworkTx<'observer, B> {
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    deadline_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    exchange_started_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_acknowledged: Option<u8>,
    #[cfg(any(feature = "diagnostics", test))]
    prepared_scheduler_trace: Option<PreparedSchedulerTraceBuilder>,
    prepared_first: Option<B>,
    prepared_second: Option<B>,
    prepared_standby: Option<PreparedStandby>,
    last_started_frames: usize,
}

impl<'observer, B> Esp32s31AccessPointNetworkTx<'observer, B>
where
    B: StableDmaBacking,
{
    pub const fn new(
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
    ) -> Self {
        Self {
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(not(any(feature = "diagnostics", test)))]
            observer_lifetime: PhantomData,
            deadline_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            exchange_started_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_acknowledged: None,
            #[cfg(any(feature = "diagnostics", test))]
            prepared_scheduler_trace: None,
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

    /// Publish the terminal aggregate observation at the outer role-service
    /// boundary, after role diagnostics have consumed the completed state.
    /// This keeps completion-to-publication focused on DATAPATH scheduling
    /// instead of charging unrelated observer bookkeeping to the scheduler.
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn observe_service_boundary(&mut self) {
        let Some(acknowledged) = self.terminal_acknowledged.take() else {
            return;
        };
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::Completed {
                acknowledged,
                individual_retry: false,
            });
            self.prepared_scheduler_trace = self
                .has_prepared()
                .then(PreparedSchedulerTraceBuilder::default);
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn mark_prepared_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        let Some(trace) = self.prepared_scheduler_trace.as_mut() else {
            return;
        };
        trace.mark(phase, at_micros);
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

#[cfg(not(any(feature = "diagnostics", test)))]
impl<'observer, B> Default for Esp32s31AccessPointNetworkTx<'observer, B>
where
    B: StableDmaBacking,
{
    fn default() -> Self {
        Self::new()
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
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
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
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }
        let admission = control.mac.aggregate_admission(frame.as_slice());

        if let Some(admission) = admission
            && network.queue_len() != 0
            && let Some(mut second) = network.try_receive()
        {
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe_access_point_network_claim(second.as_slice());
            }
            #[cfg(any(feature = "diagnostics", test))]
            let preparation_started = self.observer.map(AggregateTxObserver::now_micros);
            if !admission.accepts_ethernet(second.as_slice()) {
                // This lease was older than every frame still in the network
                // queue. Retain it locally for the next transaction; putting
                // it on the channel tail would reorder one VIF's UDP stream.
                debug_assert!(self.prepared_first.is_none());
                self.prepared_first = Some(second);
                return control
                    .start_network_tx(hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointDatapathError::Control);
            }

            let peer = admission.peer();
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            let first_offset = frame.ethernet_offset();
            let first_length = frame.ethernet_length();
            let first_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    frame.storage_mut(),
                    first_offset,
                    first_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            let policy = admission
                .bind_policy(first_encoded.hardware_key_selector, SLOTS)
                .map_err(Esp32s31ApAmpduError::from)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let active = aggregate.active_mut();
            active
                .begin(
                    peer,
                    policy.rate(),
                    first_encoded.sequence_number,
                    policy.role().hardware_key_selector,
                )
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            active
                .push(peer, frame, first_encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;

            let second_offset = second.ethernet_offset();
            let second_length = second.ethernet_length();
            let second_encoded = engine
                .encode_aggregate_ethernet_in_place(
                    admission.binding(),
                    second.storage_mut(),
                    second_offset,
                    second_length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            active
                .push(peer, second, second_encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;

            let target = usize::from(policy.frame_limit());
            let mut admitted = 2_usize;
            while admitted < target {
                let Some(mut next) = network.try_receive() else {
                    break;
                };
                #[cfg(any(feature = "diagnostics", test))]
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
                    .encode_aggregate_ethernet_in_place(
                        admission.binding(),
                        next.storage_mut(),
                        offset,
                        length,
                    )
                    .map_err(|error| {
                        Esp32s31AccessPointDatapathError::Control(
                            Esp32s31AccessPointControlError::from(error),
                        )
                    })?;
                active
                    .push(peer, next, encoded)
                    .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
                admitted += 1;
            }
            #[cfg(any(feature = "diagnostics", test))]
            let publication_started = self.observer.map(AggregateTxObserver::now_micros);
            active
                .publish(ordinary, hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = publication_started.unwrap_or(finished);
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
                    micros: started.saturating_sub(preparation_started.unwrap_or(started)),
                });
                observer.observe(AggregateTxObservation::Published {
                    at_micros: started,
                    program_micros: finished.saturating_sub(started),
                    prepared_scheduler: None,
                });
                self.exchange_started_micros = Some(started);
            }
            let deadline_micros = ordinary
                .now_micros()
                .saturating_add(ordinary.publication_timeout_micros());
            self.deadline_micros = Some(deadline_micros);
            #[cfg(any(feature = "diagnostics", test))]
            control.observe_ht_aggregate(policy.rate());
            self.last_started_frames = admitted;
            return Ok(WifiTxProgress::Pending);
        }

        control
            .start_network_tx(hardware, frame.as_slice())
            .map_err(Esp32s31AccessPointDatapathError::Control)
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        assert!(
            (self.aggregate_pending() || self.has_prepared()) && aggregate.has_standby(),
            "DATAPATH must check AP standby ownership before claiming another ordered lease"
        );
        #[cfg(any(feature = "diagnostics", test))]
        let started = self.observer.map(AggregateTxObserver::now_micros);
        #[cfg(any(feature = "diagnostics", test))]
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
                .encode_aggregate_ethernet_in_place(
                    batch.admission.binding(),
                    frame.storage_mut(),
                    offset,
                    length,
                )
                .map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(
                        Esp32s31AccessPointControlError::from(error),
                    )
                })?;
            aggregate
                .standby_mut()
                .expect("checked standby arena")
                .push(peer, frame, encoded)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            batch.admitted += 1;
            #[cfg(any(feature = "diagnostics", test))]
            {
                batch.preparation_micros = batch.preparation_micros.saturating_add(
                    self.observer
                        .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                        .unwrap_or(0),
                );
            }
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
                admission.binding(),
                first.storage_mut(),
                first_offset,
                first_length,
            )
            .map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::from(
                    error,
                ))
            })?;
        let policy = admission
            .bind_policy(first_encoded.hardware_key_selector, SLOTS)
            .map_err(Esp32s31ApAmpduError::from)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let standby = aggregate.standby_mut().expect("checked standby arena");
        standby
            .begin(
                peer,
                policy.rate(),
                first_encoded.sequence_number,
                policy.role().hardware_key_selector,
            )
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        standby
            .push(peer, first, first_encoded)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let offset = frame.ethernet_offset();
        let length = frame.ethernet_length();
        let encoded = control
            .mac
            .engine_mut()
            .encode_aggregate_ethernet_in_place(
                admission.binding(),
                frame.storage_mut(),
                offset,
                length,
            )
            .map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::from(
                    error,
                ))
            })?;
        aggregate
            .standby_mut()
            .expect("checked standby arena")
            .push(peer, frame, encoded)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        self.prepared_standby = Some(PreparedStandby {
            admission,
            policy,
            admitted: 2,
            #[cfg(any(feature = "diagnostics", test))]
            preparation_micros: self
                .observer
                .map(|observer| observer.now_micros().saturating_sub(started.unwrap_or(0)))
                .unwrap_or(0),
        });
        #[cfg(any(feature = "diagnostics", test))]
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
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware
            + Esp32s31ApRuntimeHardware
            + RxBlockAckHardware
            + open_esp_radio_esp32s31_wifi_mac::tx_ampdu::HtAmpduHardware,
    {
        #[cfg(any(feature = "diagnostics", test))]
        let prepared_scheduler = self
            .prepared_scheduler_trace
            .take()
            .and_then(PreparedSchedulerTraceBuilder::complete);
        while self.can_prepare(aggregate) {
            let Some(frame) = network.try_receive() else {
                break;
            };
            self.prepare(aggregate, control, frame, network)?;
        }
        let Some(_batch) = self.prepared_standby.take() else {
            let Some(frame) = self.prepared_first.take() else {
                return Ok(WifiTxProgress::Complete);
            };
            self.prepared_first = self.prepared_second.take();
            return control
                .start_network_tx(hardware, frame.as_slice())
                .map_err(Esp32s31AccessPointDatapathError::Control);
        };
        #[cfg(any(feature = "diagnostics", test))]
        let publication_started = self.observer.map(AggregateTxObserver::now_micros);
        let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(error))
        })?;
        aggregate
            .publish_standby(ordinary, hardware)
            .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
        let now = ordinary.now_micros();
        self.deadline_micros = Some(now.saturating_add(ordinary.publication_timeout_micros()));
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.exchange_started_micros = publication_started;
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            let finished = observer.now_micros();
            let started = publication_started.unwrap_or(finished);
            observe_aggregate_rate(observer, _batch.policy.rate());
            observer.observe(AggregateTxObservation::Prepared {
                subframes: u8::try_from(_batch.admitted).unwrap_or(u8::MAX),
                stop: if _batch.admitted == usize::from(_batch.policy.frame_limit()) {
                    AggregateBuildStop::FrameLimit
                } else {
                    AggregateBuildStop::QueueEmpty
                },
            });
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: _batch.preparation_micros,
            });
            observer.observe(AggregateTxObservation::Published {
                at_micros: started,
                program_micros: finished.saturating_sub(started),
                prepared_scheduler,
            });
            observer.observe(AggregateTxObservation::StandbyPublished);
            control.observe_ht_aggregate(_batch.policy.rate());
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
    ) -> Result<(), Esp32s31AccessPointDatapathError> {
        self.prepared_first = None;
        self.prepared_second = None;
        if self.prepared_standby.take().is_some() {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
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
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
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
                .map_err(Esp32s31AccessPointDatapathError::Control);
        }

        let service_event = AggregateTxServiceEvent::classify(wake).map_err(|error| {
            Esp32s31AccessPointDatapathError::Aggregate(
                Esp32s31ApAmpduError::ConflictingInterruptEvents(error.events),
            )
        })?;
        if service_event == AggregateTxServiceEvent::Collision {
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            if !aggregate
                .active_mut()
                .abort_collision(hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?
            {
                return Err(Esp32s31AccessPointDatapathError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
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
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?
            {
                return Err(Esp32s31AccessPointDatapathError::Aggregate(
                    Esp32s31ApAmpduError::HardwareDidNotDetach,
                ));
            }
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            ordinary.after_micros(16).await;
            aggregate
                .active_mut()
                .finish_timeout_abort(hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            ordinary.reset_aggregate_contention();
            self.deadline_micros = None;
            #[cfg(any(feature = "diagnostics", test))]
            {
                self.exchange_started_micros = None;
            }
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::HardwareTimeout);
            }
            return Ok(WifiTxProgress::Complete);
        }

        let aggregate_progress = {
            #[cfg(any(feature = "diagnostics", test))]
            let completion_started = self.observer.map(AggregateTxObserver::now_micros);
            let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            let progress = aggregate
                .active_mut()
                .service_completion(ordinary, hardware)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                let finished = observer.now_micros();
                let started = completion_started.unwrap_or(finished);
                match progress {
                    Esp32s31ApAmpduProgress::Republished(_) => {
                        observer.observe(AggregateTxObservation::Published {
                            at_micros: started,
                            program_micros: finished.saturating_sub(started),
                            prepared_scheduler: None,
                        });
                    }
                    Esp32s31ApAmpduProgress::CompletionReady(_) => {
                        observer.observe(AggregateTxObservation::CompletionCoreCompleted {
                            micros: finished.saturating_sub(started),
                        });
                    }
                    Esp32s31ApAmpduProgress::Pending => {}
                }
            }
            progress
        };
        match aggregate_progress {
            Esp32s31ApAmpduProgress::CompletionReady(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, false);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                #[cfg(any(feature = "diagnostics", test))]
                let release_started = self.observer.map(AggregateTxObserver::now_micros);
                aggregate
                    .active_mut()
                    .release_completed()
                    .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    let finished = observer.now_micros();
                    observer.observe(AggregateTxObservation::BackingReleaseCompleted {
                        micros: finished.saturating_sub(release_started.unwrap_or(finished)),
                    });
                }
                #[cfg(any(feature = "diagnostics", test))]
                {
                    debug_assert!(self.terminal_acknowledged.is_none());
                    self.terminal_acknowledged = Some(completion.acknowledged);
                }
                self.deadline_micros = None;
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.exchange_started_micros = None;
                }
                Ok(WifiTxProgress::Complete)
            }
            Esp32s31ApAmpduProgress::Republished(completion) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe_completion_details(completion, true);
                #[cfg(not(any(feature = "diagnostics", test)))]
                let _ = completion;
                let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
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
                    return Err(Esp32s31AccessPointDatapathError::Aggregate(
                        Esp32s31ApAmpduError::CompletionInterruptWithoutState,
                    ));
                }
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn observe_completion_details(&self, completion: Esp32s31ApAmpduCompletion, republished: bool) {
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
        if !republished && let Some(started) = self.exchange_started_micros {
            observer.observe(AggregateTxObservation::ExchangeCompleted {
                micros: observer.now_micros().saturating_sub(started),
                publications: completion.aggregate_attempts,
            });
        }
    }
}

#[cfg(test)]
mod scheduler_trace_tests {
    use super::*;

    #[test]
    fn trace_preserves_adjacent_scheduler_boundaries_and_detour_counts() {
        let mut trace = PreparedSchedulerTraceBuilder::default();
        trace.mark(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
        trace.mark(PreparedTxSchedulerPhase::SchedulerLoopResumed, 20);
        trace.mark(
            PreparedTxSchedulerPhase::ControlReadinessChecked { ready: true },
            25,
        );
        trace.mark(PreparedTxSchedulerPhase::SchedulerLoopResumed, 30);
        trace.mark(PreparedTxSchedulerPhase::StopPollCompleted, 35);
        trace.mark(
            PreparedTxSchedulerPhase::ControlReadinessChecked { ready: false },
            40,
        );
        trace.mark(PreparedTxSchedulerPhase::PreparedEntry, 45);

        assert_eq!(
            trace.complete(),
            Some(PreparedTxSchedulerTrace {
                active_service_returned_micros: 10,
                scheduler_loop_resumed_micros: 30,
                stop_poll_completed_micros: 35,
                control_readiness_checked_micros: 40,
                prepared_entry_micros: 45,
                scheduler_passes: 2,
                control_ready_passes: 1,
            })
        );
    }

    #[test]
    fn incomplete_trace_cannot_be_reported_as_a_scheduler_measurement() {
        let mut trace = PreparedSchedulerTraceBuilder::default();
        trace.mark(PreparedTxSchedulerPhase::ActiveServiceReturned, 10);
        trace.mark(PreparedTxSchedulerPhase::PreparedEntry, 45);

        assert_eq!(trace.complete(), None);
    }
}
