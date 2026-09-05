//! AP-owned network TX transaction.
//!
//! DATAPATH schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;
use core::marker::PhantomData;

mod aggregate;
mod completion;
mod power_save;
mod queue;
mod service_phase;

use aggregate::aggregate_adapter_available;
#[cfg(test)]
use queue::AP_ACTIVE_FRAME_CAPACITY;
use queue::{
    ApActiveFrameQueues, ApFrameLeaseArena, ApGroupFrameQueue, ApPowerSaveFrameQueue, ApTxFlowKey,
    BufferedGroup, BufferedUnicast,
};
use service_phase::{AggregateServiceAction, AggregateServicePhase};

struct BufferedUnicastRelease<B> {
    buffered: BufferedUnicast<B>,
    release: ApBufferedUnicastRelease,
}

struct BufferedGroupRelease<B> {
    buffered: BufferedGroup<B>,
    release: ApBufferedGroupRelease,
}

struct PreparedStandby {
    admission: Esp32s31ApAggregateAdmission,
    policy: HtAmpduTxRolePolicy,
    admitted: usize,
    #[cfg(feature = "tx-phase-telemetry")]
    mismatch_claims: usize,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

pub struct Esp32s31AccessPointNetworkTx<'observer, B, N = B> {
    dma_backing: PhantomData<B>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    aggregate_phase: Option<AggregateServicePhase>,
    #[cfg(any(feature = "diagnostics", test))]
    exchange_started_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_acknowledged: Option<u8>,
    frame_arena: ApFrameLeaseArena<N>,
    active_frames: ApActiveFrameQueues,
    prepared_first: Option<N>,
    prepared_first_key: Option<ApTxFlowKey>,
    prepared_second: Option<N>,
    prepared_second_key: Option<ApTxFlowKey>,
    prepared_standby: Option<PreparedStandby>,
    buffered_unicast: ApPowerSaveFrameQueue,
    buffered_group: ApGroupFrameQueue,
    prepared_buffered_release: Option<BufferedUnicastRelease<N>>,
    active_buffered_release: Option<BufferedUnicastRelease<N>>,
    prepared_group_release: Option<BufferedGroupRelease<N>>,
    active_group_release: Option<BufferedGroupRelease<N>>,
    /// Remaining prefix authorized by one successful DTIM beacon. Frames
    /// retained after that beacon can never join this release window.
    dtim_group_release_remaining: u16,
    last_started_frames: usize,
}

impl<'observer, B, N> Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: StableDmaBacking,
{
    pub const fn new(
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
    ) -> Self {
        Self {
            dma_backing: PhantomData,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(not(any(feature = "diagnostics", test)))]
            observer_lifetime: PhantomData,
            aggregate_phase: None,
            #[cfg(any(feature = "diagnostics", test))]
            exchange_started_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_acknowledged: None,
            frame_arena: ApFrameLeaseArena::new(),
            active_frames: ApActiveFrameQueues::new(),
            prepared_first: None,
            prepared_first_key: None,
            prepared_second: None,
            prepared_second_key: None,
            prepared_standby: None,
            buffered_unicast: ApPowerSaveFrameQueue::new(),
            buffered_group: ApGroupFrameQueue::new(),
            prepared_buffered_release: None,
            active_buffered_release: None,
            prepared_group_release: None,
            active_group_release: None,
            dtim_group_release_remaining: 0,
            last_started_frames: 1,
        }
    }

    pub(super) const fn aggregate_pending(&self) -> bool {
        self.aggregate_phase.is_some()
    }

    pub(super) fn has_prepared(&self) -> bool {
        self.active_frames.len() != 0
            || self.prepared_first.is_some()
            || self.prepared_second.is_some()
            || self.prepared_standby.is_some()
            || self.prepared_buffered_release.is_some()
            || self.prepared_group_release.is_some()
    }

    pub(super) fn prepared_start_ready(&self) -> bool {
        self.has_prepared()
    }

    pub(super) fn prepared_frame_count(&self) -> usize {
        if self.prepared_group_release.is_some() || self.prepared_buffered_release.is_some() {
            return 1;
        }
        if let Some(batch) = self.prepared_standby.as_ref() {
            return batch.admitted;
        }
        if let Some(first) = self.prepared_first.as_ref() {
            let _ = first;
            return 1
                + self.active_frames.len_for(
                    self.prepared_first_key
                        .expect("prepared AP frame retains its flow key"),
                )
                + usize::from(self.prepared_second.is_some());
        }
        self.active_frames.scheduled_len()
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
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn mark_prepared_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        if !self.has_prepared() {
            return;
        }
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros });
        }
    }

    pub(super) fn can_prepare<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &self,
        aggregate: &Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        ordinary_publication_pending: bool,
    ) -> bool {
        // A retained peer-boundary frame may coexist with an ordinary MPDU
        // publication, but it cannot authorize claiming the next frame yet.
        // Encoding that pair borrows the ordinary descriptor policy adapter;
        // the adapter remains owned by the in-flight MPDU until its terminal
        // service edge. Aggregate-active preparation is still allowed because
        // an A-MPDU does not set the ordinary MAC publication owner.
        if !aggregate_adapter_available(ordinary_publication_pending) {
            return false;
        }
        if !aggregate.has_standby() {
            return false;
        }
        if self.prepared_buffered_release.is_some() {
            return false;
        }
        if self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
        {
            return false;
        }
        match self.prepared_standby.as_ref() {
            Some(batch) => {
                self.prepared_first.is_none()
                    && batch.admitted < usize::from(batch.policy.frame_limit())
            }
            None => {
                (self.aggregate_phase.is_some()
                    || self.active_frames.len() != 0
                    || self.prepared_first.is_some())
                    && self.prepared_second.is_none()
            }
        }
    }
}

#[cfg(not(any(feature = "diagnostics", test)))]
impl<'observer, B> Default for Esp32s31AccessPointNetworkTx<'observer, B, B>
where
    B: StableDmaBacking,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'observer, B, N> Esp32s31AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
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
        aggregate: &mut Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
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
        frame: N,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
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
        let _ = self.stage_dtim_group_release(control)?;
        self.retain_active_frame(control.mac.engine_mut(), frame)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        let _ = self.stage_awake_buffered_release(control)?;
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        let Some((flow_key, frame)) = self.pop_scheduled_active(control.mac.engine()) else {
            return Ok(WifiTxProgress::Complete);
        };
        let admission = control.mac.aggregate_admission(frame.as_slice());
        let mut retained_aggregate_second = None;

        // Open APs have no BlockAck owner, so they use bounded ordinary
        // A-MSDUs whenever an ordered partner is available. For WPA2+BA keep
        // saturated bursts on A-MPDU; coalesce the exact two-frame tail only
        // when the negotiated agreement echoed A-MSDU support.
        let same_flow_ready = self.active_frames.len_for(flow_key);
        let producer_ready = network.queue_len();
        if same_flow_ready.saturating_add(producer_ready) != 0
            && (admission.is_none()
                || (same_flow_ready.saturating_add(producer_ready) == 1
                    && admission.is_some_and(Esp32s31ApAggregateAdmission::amsdu)))
            && let Some(second) =
                self.take_matching_active_or_network(control.mac.engine_mut(), flow_key, network)?
        {
            match control.start_network_amsdu_pair(hardware, frame.as_slice(), second.as_slice()) {
                Ok(Some(progress)) => {
                    self.last_started_frames = 2;
                    return Ok(progress);
                }
                Ok(None) => {
                    if admission.is_some() {
                        // The pair may be too large for the ordinary AP
                        // scratch while both individual MPDUs still fit the
                        // retained A-MPDU arena. Preserve the already claimed
                        // second lease for that exact fallback.
                        retained_aggregate_second = Some(second);
                    } else {
                        self.restore_active_frame_front(flow_key, second);
                    }
                }
                Err(error) => {
                    self.restore_active_pair_front(flow_key, frame, second);
                    return Err(Esp32s31AccessPointDatapathError::Control(error));
                }
            }
        }

        if let Some(admission) = admission
            && (retained_aggregate_second.is_some()
                || self.active_frames.len_for(flow_key) != 0
                || network.queue_len() != 0)
        {
            let second = if let Some(second) = retained_aggregate_second.take() {
                second
            } else {
                let Some(second) = self.take_matching_active_or_network(
                    control.mac.engine_mut(),
                    flow_key,
                    network,
                )?
                else {
                    return control
                        .start_network_tx(hardware, frame.as_slice())
                        .map_err(Esp32s31AccessPointDatapathError::Control);
                };
                second
            };
            #[cfg(any(feature = "diagnostics", test))]
            let preparation_started = self.observer.map(AggregateTxObserver::now_micros);
            debug_assert!(admission.accepts_ethernet(second.as_slice()));

            let peer = admission.peer();
            let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                Esp32s31AccessPointDatapathError::Control(Esp32s31AccessPointControlError::Mac(
                    error,
                ))
            })?;
            ordinary
                .require_unprotected_ht_aggregate(admission.rate())
                .map_err(Esp32s31ApAmpduError::Protection)
                .map_err(Esp32s31AccessPointDatapathError::Aggregate)?;
            let (mut frame, mut second) = match network.try_materialize_pair(frame, second) {
                Ok(frames) => frames,
                Err((frame, second)) => {
                    self.restore_active_pair_front(flow_key, frame, second);
                    return Ok(WifiTxProgress::Complete);
                }
            };
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
                let Some(next) = self.take_matching_active_or_network(engine, flow_key, network)?
                else {
                    break;
                };
                debug_assert!(admission.accepts_ethernet(next.as_slice()));
                let mut next = match network.try_materialize(next) {
                    Ok(next) => next,
                    Err(next) => {
                        self.restore_active_frame_front(flow_key, next);
                        break;
                    }
                };
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
                });
                self.exchange_started_micros = Some(started);
            }
            let deadline_micros = ordinary
                .now_micros()
                .saturating_add(ordinary.publication_timeout_micros());
            self.aggregate_phase = Some(AggregateServicePhase::Published(deadline_micros));
            #[cfg(any(feature = "diagnostics", test))]
            control.observe_ht_aggregate(policy.rate());
            self.last_started_frames = admitted;
            self.prepare_ready_standby(aggregate, control, network)?;
            return Ok(WifiTxProgress::Pending);
        }

        control
            .start_network_tx(hardware, frame.as_slice())
            .map_err(Esp32s31AccessPointDatapathError::Control)
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
        aggregate: &mut Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
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
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
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
        let _ = self.stage_dtim_group_release(control)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        self.prepare_ready_standby(aggregate, control, network)?;
        #[cfg(feature = "tx-phase-telemetry")]
        self.record_partial_frontier(network);
        let Some(_batch) = self.prepared_standby.take() else {
            loop {
                let Some(frame) = self.prepared_first.take() else {
                    return Ok(WifiTxProgress::Complete);
                };
                let key = self
                    .prepared_first_key
                    .take()
                    .expect("prepared AP frame retains its flow key");
                self.prepared_first = self.prepared_second.take();
                self.prepared_first_key = self.prepared_second_key.take();
                if !key.is_current(control.mac.engine()) {
                    drop(frame);
                    continue;
                }
                let Some((readmitted_key, frame)) =
                    self.retain_power_save(control.mac.engine_mut(), frame)?
                else {
                    continue;
                };
                if readmitted_key != key {
                    drop(frame);
                    continue;
                }
                return control
                    .start_network_tx(hardware, frame.as_slice())
                    .map_err(Esp32s31AccessPointDatapathError::Control);
            }
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
        self.aggregate_phase = Some(AggregateServicePhase::Published(
            now.saturating_add(ordinary.publication_timeout_micros()),
        ));
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
            });
            observer.observe(AggregateTxObservation::StandbyPublished);
            control.observe_ht_aggregate(_batch.policy.rate());
        }
        self.prepare_ready_standby(aggregate, control, network)?;
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn cancel_prepared<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut Esp32s31AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
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
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let _ = network;
        self.rollback_prepared_buffered_release(control)?;
        self.discard_group_buffer(control)?;
        control
            .rollback_pending_buffered_releases()
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        self.prepared_first = None;
        self.prepared_first_key = None;
        self.prepared_second = None;
        self.prepared_second_key = None;
        while let Some((_, index)) = self.active_frames.pop_scheduled() {
            drop(self.frame_arena.take(index));
        }
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
}

/// Narrow bridge used by the same-channel RX owner to turn a peer's PM=0
/// edge into prepared network work without exposing frame storage to the
/// protocol processor.
pub(super) trait AccessPointPowerSaveNetworkTx<
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
{
    fn stage_awake_release(
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
    ) -> Result<bool, Esp32s31AccessPointDatapathError>;

    fn has_power_save_release(&self) -> bool;

    fn discard_group_power_save(
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>;
}

#[cfg(test)]
mod tests;
