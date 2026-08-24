//! AP-owned network TX transaction.
//!
//! DATAPATH schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use super::*;
#[cfg(not(any(feature = "diagnostics", test)))]
use core::marker::PhantomData;

// The default pinned TX pool has 66 leases. Retaining that many handles here
// lets power-save backpressure consume the complete default producer frontier
// without allocating or copying payload bytes. Custom, larger pools still
// fail closed at this explicit bound.
const AP_POWER_SAVE_FRAME_CAPACITY: usize = 66;

struct BufferedUnicast<B> {
    peer: [u8; 6],
    order: u64,
    frame: B,
}

struct BufferedUnicastRelease<B> {
    buffered: BufferedUnicast<B>,
    release: ApBufferedUnicastRelease,
}

struct BufferedGroup<B> {
    order: u64,
    frame: B,
}

struct BufferedGroupRelease<B> {
    buffered: BufferedGroup<B>,
    release: ApBufferedGroupRelease,
}

struct ApPowerSaveFrameQueue<B> {
    slots: [Option<BufferedUnicast<B>>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl<B> ApPowerSaveFrameQueue<B> {
    const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    fn push(&mut self, peer: [u8; 6], frame: B) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedUnicast { peer, order, frame });
        self.len += 1;
        Ok(index)
    }

    fn take_at(&mut self, index: usize) -> Option<BufferedUnicast<B>> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(buffered)
    }

    fn restore(&mut self, buffered: BufferedUnicast<B>) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP power-save lease always leaves one queue slot");
        *slot = Some(buffered);
        self.len += 1;
    }

    fn oldest_index_for(&self, peer: [u8; 6]) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let entry = entry.as_ref()?;
                (entry.peer == peer).then_some((index, entry.order))
            })
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    fn oldest_releasable_peer(
        &self,
        mut releasable: impl FnMut([u8; 6]) -> bool,
    ) -> Option<[u8; 6]> {
        self.slots
            .iter()
            .flatten()
            .filter(|entry| releasable(entry.peer))
            .min_by_key(|entry| entry.order)
            .map(|entry| entry.peer)
    }

    fn retain(&mut self, mut keep: impl FnMut([u8; 6]) -> bool) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|entry| !keep(entry.peer)) {
                let _ = slot.take();
                self.len -= 1;
            }
        }
    }
}

/// Bounded caller-owned group queue. Entries are pinned network leases, not
/// payload copies; the portable AP owns only the matching advertised count.
struct ApGroupFrameQueue<B> {
    slots: [Option<BufferedGroup<B>>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl<B> ApGroupFrameQueue<B> {
    const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    fn push(&mut self, frame: B) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedGroup { order, frame });
        self.len += 1;
        Ok(index)
    }

    fn take_at(&mut self, index: usize) -> Option<BufferedGroup<B>> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(buffered)
    }

    fn restore(&mut self, buffered: BufferedGroup<B>) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP group lease always leaves one queue slot");
        *slot = Some(buffered);
        self.len += 1;
    }

    fn oldest_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry.order)))
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    fn clear(&mut self) -> usize {
        let discarded = self.len;
        for slot in &mut self.slots {
            let _ = slot.take();
        }
        self.len = 0;
        discarded
    }
}

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
    buffered_unicast: ApPowerSaveFrameQueue<B>,
    buffered_group: ApGroupFrameQueue<B>,
    prepared_buffered_release: Option<BufferedUnicastRelease<B>>,
    active_buffered_release: Option<BufferedUnicastRelease<B>>,
    prepared_group_release: Option<BufferedGroupRelease<B>>,
    active_group_release: Option<BufferedGroupRelease<B>>,
    /// Remaining prefix authorized by one successful DTIM beacon. Frames
    /// retained after that beacon can never join this release window.
    dtim_group_release_remaining: u16,
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
        self.deadline_micros.is_some()
    }

    pub(super) fn has_prepared(&self) -> bool {
        self.prepared_first.is_some()
            || self.prepared_second.is_some()
            || self.prepared_standby.is_some()
            || self.prepared_buffered_release.is_some()
            || self.prepared_group_release.is_some()
    }

    pub(super) fn prepared_frame_count(&self) -> usize {
        if self.prepared_group_release.is_some() || self.prepared_buffered_release.is_some() {
            return 1;
        }
        self.prepared_standby.as_ref().map_or(
            usize::from(self.prepared_first.is_some())
                + usize::from(self.prepared_second.is_some())
                + usize::from(self.prepared_buffered_release.is_some()),
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
    fn retain_power_save(
        &mut self,
        engine: &mut Esp32s31ApEngine<'_>,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> Result<
        Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>,
        Esp32s31AccessPointDatapathError,
    > {
        let Some(peer) = frame
            .as_slice()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(Some(frame));
        };
        if peer[0] & 1 != 0 {
            if engine.group_downlink_disposition() == ApDownlinkDisposition::TransmitNow {
                return Ok(Some(frame));
            }
            let Ok(index) = self.buffered_group.push(frame) else {
                // The caller-owned queue is deliberately bounded. Releasing
                // this excess lease applies backpressure at the producer pool
                // without claiming a TIM entry for payload we did not retain.
                return Ok(None);
            };
            if let Err(error) = engine.commit_buffered_group() {
                let _ = self
                    .buffered_group
                    .take_at(index)
                    .expect("the just-inserted AP group lease is still owned");
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::from(error),
                ));
            }
            return Ok(None);
        }
        let disposition = match engine.downlink_disposition(peer) {
            Ok(disposition) => disposition,
            // Preserve the ordinary admission path for an unknown or
            // unauthorized destination so its existing rejection accounting
            // remains authoritative.
            Err(_) => return Ok(Some(frame)),
        };
        if disposition == ApDownlinkDisposition::TransmitNow {
            return Ok(Some(frame));
        }

        let Ok(index) = self.buffered_unicast.push(peer, frame) else {
            // The bounded queue owns the complete default TX lease frontier.
            // A custom larger producer cannot force an allocation or an
            // unbounded retention path; its excess lease is released here.
            return Ok(None);
        };
        if let Err(error) = engine.commit_buffered_unicast(peer) {
            let _ = self
                .buffered_unicast
                .take_at(index)
                .expect("the just-inserted AP power-save lease is still owned");
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::from(error),
            ));
        }
        Ok(None)
    }

    /// Reserve the oldest retained frame whose peer has returned to Active.
    /// This mutates no frame bytes and leaves the TIM count unchanged until
    /// terminal TX resolves the affine release token.
    pub(super) fn stage_awake_buffered_release<
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
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.prepared_buffered_release.is_some() || self.active_buffered_release.is_some() {
            return Ok(false);
        }

        if let Some(release) = control.take_pending_buffered_release() {
            let peer = release.peer();
            if let Some(index) = self.buffered_unicast.oldest_index_for(peer) {
                let buffered = self
                    .buffered_unicast
                    .take_at(index)
                    .expect("the PS-Poll release names one retained lease");
                self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
                return Ok(true);
            }
            control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control)?;
        }

        // Peer teardown clears the portable counters. Release matching caller
        // leases at the same observation boundary instead of retaining stale
        // addresses into a later association generation.
        self.buffered_unicast.retain(|peer| {
            control
                .mac
                .engine()
                .peer_status(peer)
                .is_some_and(|status| status.phase == ApPeerPhase::Authorized)
        });
        let Some(peer) = self.buffered_unicast.oldest_releasable_peer(|peer| {
            control
                .mac
                .engine()
                .peer_status(peer)
                .is_some_and(|status| {
                    status.phase == ApPeerPhase::Authorized
                        && status.power_state == ApPeerPowerState::Active
                        && !status.buffered_release_in_flight
                })
        }) else {
            return Ok(false);
        };
        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_unicast_release(peer)
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            return Ok(false);
        };
        let Some(index) = self.buffered_unicast.oldest_index_for(peer) else {
            let _ = control
                .mac
                .engine_mut()
                .complete_buffered_unicast_release(release, false);
            return Ok(false);
        };
        let buffered = self
            .buffered_unicast
            .take_at(index)
            .expect("the selected AP power-save lease remains retained");
        self.prepared_buffered_release = Some(BufferedUnicastRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_buffered_release<
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(prepared.release, false);
        self.buffered_unicast.restore(prepared.buffered);
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    fn complete_active_buffered_release<
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
        delivered: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_buffered_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_unicast_release(active.release, delivered);
        if !delivered || result.is_err() {
            self.buffered_unicast.restore(active.buffered);
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let _ = self.stage_awake_buffered_release(control)?;
        Ok(())
    }

    fn start_prepared_buffered_release<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_buffered_release
            .take()
            .expect("checked prepared AP power-save release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_buffered_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_buffered_release = Some(prepared);
                self.rollback_prepared_buffered_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    /// Bind the exact queue prefix announced by a successfully transmitted
    /// DTIM beacon to the oldest caller-owned group lease.
    fn stage_dtim_group_release<
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
    ) -> Result<bool, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if let Some(advertised_frames) = control.take_pending_dtim_group_frames() {
            if self.dtim_group_release_remaining != 0
                || self.prepared_group_release.is_some()
                || self.active_group_release.is_some()
            {
                return Err(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::DtimGroupReleaseAlreadyPending,
                ));
            }
            self.dtim_group_release_remaining = advertised_frames;
        }
        if self.dtim_group_release_remaining == 0
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
        {
            return Ok(false);
        }

        let Some(release) = control
            .mac
            .engine_mut()
            .begin_buffered_group_release()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?
        else {
            self.dtim_group_release_remaining = 0;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let Some(index) = self.buffered_group.oldest_index() else {
            let rollback = control
                .mac
                .engine_mut()
                .complete_buffered_group_release(release, false)
                .map_err(Esp32s31AccessPointControlError::from)
                .map_err(Esp32s31AccessPointDatapathError::Control);
            self.dtim_group_release_remaining = 0;
            rollback?;
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        };
        let buffered = self
            .buffered_group
            .take_at(index)
            .expect("the selected AP group lease remains retained");
        self.prepared_group_release = Some(BufferedGroupRelease { buffered, release });
        Ok(true)
    }

    fn rollback_prepared_group_release<
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(prepared) = self.prepared_group_release.take() else {
            self.dtim_group_release_remaining = 0;
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(prepared.release, false);
        self.buffered_group.restore(prepared.buffered);
        self.dtim_group_release_remaining = 0;
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)
    }

    fn complete_active_group_release<
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
        published: bool,
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        let Some(active) = self.active_group_release.take() else {
            return Ok(());
        };
        let result = control
            .mac
            .engine_mut()
            .complete_buffered_group_release(active.release, published);
        if !published || result.is_err() {
            self.buffered_group.restore(active.buffered);
            self.dtim_group_release_remaining = 0;
        } else {
            self.dtim_group_release_remaining = self
                .dtim_group_release_remaining
                .checked_sub(1)
                .ok_or(Esp32s31AccessPointDatapathError::Control(
                    Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
                ))?;
        }
        result
            .map(|_| ())
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        if self.dtim_group_release_remaining != 0 {
            let _ = self.stage_dtim_group_release(control)?;
        }
        Ok(())
    }

    fn start_prepared_group_release<
        P,
        E,
        T,
        H,
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
        hardware: &mut H,
    ) -> Result<WifiTxProgress, Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
        H: TxHardware,
    {
        let prepared = self
            .prepared_group_release
            .take()
            .expect("checked prepared AP DTIM group release");
        let result = control.start_network_tx_with_more_data(
            hardware,
            prepared.buffered.frame.as_slice(),
            prepared.release.more_data(),
        );
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active_group_release = Some(prepared);
                Ok(WifiTxProgress::Pending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                // The control owner returns Complete without publication when
                // no authorized receiver remains. Drop both the retained
                // leases and their TIM accounting instead of advertising an
                // undeliverable queue forever.
                self.discard_group_buffer(control)?;
                Ok(WifiTxProgress::Complete)
            }
            Err(error) => {
                self.prepared_group_release = Some(prepared);
                self.rollback_prepared_group_release(control)?;
                Err(Esp32s31AccessPointDatapathError::Control(error))
            }
        }
    }

    pub(super) fn discard_group_buffer<
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        if self.active_group_release.is_some() {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        self.rollback_prepared_group_release(control)?;
        let _ = control.take_pending_dtim_group_frames();
        let portable = control
            .mac
            .engine_mut()
            .discard_buffered_groups()
            .map_err(Esp32s31AccessPointControlError::from)
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
        let retained = self.buffered_group.clear();
        self.dtim_group_release_remaining = 0;
        if usize::from(portable) != retained {
            return Err(Esp32s31AccessPointDatapathError::Control(
                Esp32s31AccessPointControlError::GroupBufferOwnershipMismatch,
            ));
        }
        Ok(())
    }

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
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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
        let _ = self.stage_dtim_group_release(control)?;
        if self.prepared_group_release.is_some() {
            if let Some(frame) = self.retain_power_save(control.mac.engine_mut(), frame)? {
                if self.prepared_first.is_none() {
                    self.prepared_first = Some(frame);
                } else if self.prepared_second.is_none() {
                    self.prepared_second = Some(frame);
                } else {
                    drop(frame);
                }
            }
            return self.start_prepared_group_release(control, hardware);
        }
        let _ = self.stage_awake_buffered_release(control)?;
        if self.prepared_buffered_release.is_some() {
            if let Some(frame) = self.retain_power_save(control.mac.engine_mut(), frame)? {
                if self.prepared_first.is_none() {
                    self.prepared_first = Some(frame);
                } else if self.prepared_second.is_none() {
                    self.prepared_second = Some(frame);
                } else {
                    // This path is reachable only for a custom scheduler that
                    // starts a fresh lease while two ordered leases are
                    // already retained. Releasing the excess lease is safer
                    // than bypassing the sleeping-peer admission decision.
                    drop(frame);
                }
            }
            return self.start_prepared_buffered_release(control, hardware);
        }
        let Some(mut frame) = self.retain_power_save(control.mac.engine_mut(), frame)? else {
            return Ok(WifiTxProgress::Complete);
        };
        let admission = control.mac.aggregate_admission(frame.as_slice());
        let mut retained_aggregate_second = None;

        // Open APs have no BlockAck owner, so they use bounded ordinary
        // A-MSDUs whenever an ordered partner is available. For WPA2+BA keep
        // saturated bursts on A-MPDU; coalesce the exact two-frame tail only
        // when the negotiated agreement echoed A-MSDU support.
        if network.queue_len() != 0
            && (admission.is_none()
                || (network.queue_len() == 1
                    && admission.is_some_and(Esp32s31ApAggregateAdmission::amsdu)))
            && let Some(second) = network.try_receive()
        {
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe_access_point_network_claim(second.as_slice());
            }
            if let Some(second) = self.retain_power_save(control.mac.engine_mut(), second)? {
                match control.start_network_amsdu_pair(
                    hardware,
                    frame.as_slice(),
                    second.as_slice(),
                ) {
                    Ok(Some(progress)) => {
                        self.last_started_frames = 2;
                        return Ok(progress);
                    }
                    Ok(None) => {
                        if admission.is_some() {
                            // The pair may be too large for the ordinary AP
                            // scratch while both individual MPDUs still fit
                            // the retained A-MPDU arena. Preserve the already
                            // claimed second lease for that exact fallback.
                            retained_aggregate_second = Some(second);
                        } else {
                            debug_assert!(self.prepared_first.is_none());
                            self.prepared_first = Some(second);
                        }
                    }
                    Err(error) => {
                        debug_assert!(self.prepared_first.is_none());
                        debug_assert!(self.prepared_second.is_none());
                        self.prepared_first = Some(frame);
                        self.prepared_second = Some(second);
                        return Err(Esp32s31AccessPointDatapathError::Control(error));
                    }
                }
            }
        }

        if let Some(admission) = admission
            && (retained_aggregate_second.is_some() || network.queue_len() != 0)
        {
            let mut second = if let Some(second) = retained_aggregate_second.take() {
                second
            } else {
                let Some(second) = network.try_receive() else {
                    unreachable!("nonempty AP network queue lost its sole consumer");
                };
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe_access_point_network_claim(second.as_slice());
                }
                let Some(second) = self.retain_power_save(control.mac.engine_mut(), second)? else {
                    return control
                        .start_network_tx(hardware, frame.as_slice())
                        .map_err(Esp32s31AccessPointDatapathError::Control);
                };
                second
            };
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
                let Some(next) = network.try_receive() else {
                    break;
                };
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    observer.observe_access_point_network_claim(next.as_slice());
                }
                let Some(mut next) = self.retain_power_save(engine, next)? else {
                    continue;
                };
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
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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

        let Some(mut frame) = self.retain_power_save(control.mac.engine_mut(), frame)? else {
            return Ok(());
        };

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
        let _ = self.stage_dtim_group_release(control)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        while self.can_prepare(aggregate) {
            let Some(frame) = network.try_receive() else {
                break;
            };
            self.prepare(aggregate, control, frame, network)?;
        }
        let Some(_batch) = self.prepared_standby.take() else {
            loop {
                let Some(frame) = self.prepared_first.take() else {
                    return Ok(WifiTxProgress::Complete);
                };
                self.prepared_first = self.prepared_second.take();
                let Some(frame) = self.retain_power_save(control.mac.engine_mut(), frame)? else {
                    continue;
                };
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
    ) -> Result<(), Esp32s31AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: WifiTxTimer,
    {
        self.rollback_prepared_buffered_release(control)?;
        self.discard_group_buffer(control)?;
        control
            .rollback_pending_buffered_releases()
            .map_err(Esp32s31AccessPointDatapathError::Control)?;
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
            let progress = match control.service_tx(hardware, wake).await {
                Ok(progress) => progress,
                Err(error) => {
                    if self.active_group_release.is_some() {
                        self.complete_active_group_release(control, false)?;
                    }
                    if self.active_buffered_release.is_some() {
                        self.complete_active_buffered_release(control, false)?;
                    }
                    return Err(Esp32s31AccessPointDatapathError::Control(error));
                }
            };
            if progress == WifiTxProgress::Complete {
                let succeeded = control.take_last_terminal_tx_succeeded().unwrap_or(false);
                if self.active_group_release.is_some() {
                    // A group MPDU has no ACK. `succeeded` is only terminal
                    // hardware publication success for the one-attempt basic-
                    // rate transaction.
                    self.complete_active_group_release(control, succeeded)?;
                }
                if self.active_buffered_release.is_some() {
                    self.complete_active_buffered_release(control, succeeded)?;
                }
                let _ = self.stage_dtim_group_release(control)?;
                if self.prepared_group_release.is_none() {
                    let _ = self.stage_awake_buffered_release(control)?;
                }
            }
            return Ok(progress);
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

impl<
    'observer,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
> AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>
    for Esp32s31AccessPointNetworkTx<
        'observer,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
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
    ) -> Result<bool, Esp32s31AccessPointDatapathError> {
        self.stage_awake_buffered_release(control)
    }

    fn has_power_save_release(&self) -> bool {
        self.prepared_buffered_release.is_some()
            || self.active_buffered_release.is_some()
            || self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
    }

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
    ) -> Result<(), Esp32s31AccessPointDatapathError> {
        self.discard_group_buffer(control)
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
