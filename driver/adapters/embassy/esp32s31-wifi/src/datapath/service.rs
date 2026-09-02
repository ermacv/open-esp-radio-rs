use super::*;

#[cfg(feature = "tx-phase-telemetry")]
use crate::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0TxPhase,
};

#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::Core0RxRunnerCycleProfile;
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
use crate::diagnostics::core0_rx_performance::Core0PerformanceRunnerProfile as Core0RxRunnerCycleProfile;

impl<
    'resources,
    'irq,
    M: RawMutex + 'resources,
    N,
    B,
    R,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    DatapathRunner<
        'resources,
        'irq,
        M,
        N,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        R,
    >
where
    N: DatapathNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: DatapathServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    R: DatapathNetworkRxSet,
{
    pub(super) async fn service_rx(&mut self) -> Result<(), B::Error> {
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        let mut core0_cycles = Core0RxRunnerCycleProfile::begin();
        let context = DatapathRxServiceContext {
            maximum_protocol_frames: rx_protocol_frame_budget(
                self.rx_frame_deficit,
                self.services.has_prepared_tx() || self.network_tx_queue_len() != 0,
            ),
        };
        let serviced_before = self.services.serviced_rx_frames();
        let work_before = self.services.rx_work_counters();
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles.begin_driver();
        let progress = self
            .services
            .service_rx(&mut self.network_rx, context)
            .await?;
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles.end_driver();
        let serviced = self
            .services
            .serviced_rx_frames()
            .saturating_sub(serviced_before);
        let work = self.services.rx_work_counters().saturating_sub(work_before);
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_add(i64::try_from(serviced).unwrap_or(i64::MAX));
        self.complete_rx_service(
            progress,
            work,
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            core0_cycles,
        )
        .await;
        // Control work has its own O(1) readiness predicate and wake future.
        // An ordinary data-only DMA pass must not force the complete control
        // machine to run once per RX frontier. If a role-specific RX turn
        // stages a response, `active_tx_interface` below owns it immediately;
        // mailbox and deadline changes are exposed by `control_ready` /
        // `wait_control_ready` at the next scheduler boundary.
        if self.services.has_active_tx() {
            self.begin_active_tx(
                self.reported_active_tx_interface(),
                DatapathTxOrigin::Control,
            );
            self.drive_active_tx(true).await?;
        }
        Ok(())
    }

    async fn service_rx_during_tx(&mut self) -> Result<(), B::Error> {
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        let mut core0_cycles = Core0RxRunnerCycleProfile::begin();
        let serviced_before = self.services.serviced_rx_frames();
        let work_before = self.services.rx_work_counters();
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles.begin_driver();
        let progress = self
            .services
            .service_rx_during_tx(&mut self.network_rx)
            .await?;
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles.end_driver();
        let serviced = self
            .services
            .serviced_rx_frames()
            .saturating_sub(serviced_before);
        let work = self.services.rx_work_counters().saturating_sub(work_before);
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_add(i64::try_from(serviced).unwrap_or(i64::MAX));
        self.complete_rx_service(
            progress,
            work,
            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            core0_cycles,
        )
        .await;
        Ok(())
    }

    async fn complete_rx_service(
        &mut self,
        progress: DatapathRxProgress,
        work: DatapathRxWorkCounters,
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles: Core0RxRunnerCycleProfile,
    ) {
        self.rx_progress = progress;
        if progress != DatapathRxProgress::RecycledAppendPending {
            self.recycled_rx_probe_coalescing_level = 0;
        }
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_rx_progress(progress);
        self.clear_recycled_rx_probe_deadline();
        if matches!(
            progress,
            DatapathRxProgress::Drained
                | DatapathRxProgress::ProtocolBlockedByTx
                | DatapathRxProgress::UpperLayerBlockedButDroppable
        ) {
            // S31 exposes a level CPU route. A completion racing the final
            // ownership probe stays latched while masked and asserts the
            // route as soon as this ordered unmask completes; adding a
            // software probe here would duplicate every idle drain edge.
            let _ = self.irq.unmask_rx_after_drain();
        } else if progress == DatapathRxProgress::RecycledAppendPending
            && self.defer_recycled_rx_probe(work)
        {
            // The initial hardware edge has already been serviced and remains
            // masked. Keep the drain epoch owned by this runner, but allow the
            // executor to service control/network work while a short bounded
            // window accumulates the next DMA frontier.
        } else if matches!(
            progress,
            DatapathRxProgress::ProbePending
                | DatapathRxProgress::RecycledAppendPending
                | DatapathRxProgress::BudgetExhausted
        ) {
            // Direct BASE publication of an exhausted list has no reload
            // interrupt. Repost before surrendering the executor so the next
            // service remains runnable while another task consumes a turn.
            self.irq.notify_rx_handoff();
        }
        // One service call owns exactly the completion frontier captured at
        // its start. Publish the terminal IRQ ownership edge before yielding:
        // otherwise an unrelated long executor poll can leave RX masked for
        // milliseconds after the durable frontier was already drained.
        #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
        core0_cycles.finish_before_yield();
        yield_now().await;
    }

    pub(super) fn clear_recycled_rx_probe_deadline(&mut self) {
        self.recycled_rx_probe_deadline = None;
    }

    fn defer_recycled_rx_probe(&mut self, work: DatapathRxWorkCounters) -> bool {
        let delay = if adaptive_recycled_rx_probe_enabled() {
            let (delay, level) =
                adaptive_recycled_rx_probe_delay(work, self.recycled_rx_probe_coalescing_level);
            self.recycled_rx_probe_coalescing_level = level;
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .record_adaptive_probe_selection(delay.as_micros(), work);
            Some(delay)
        } else {
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            {
                recycled_rx_probe_delay_for_diagnostics()
            }
            #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
            None
        };
        if let Some(delay) = delay {
            self.recycled_rx_probe_deadline = Some(Instant::now() + delay);
            return true;
        }
        false
    }

    pub(super) fn recycled_rx_probe_deadline(&self) -> Option<Instant> {
        self.recycled_rx_probe_deadline
    }

    pub(super) fn recycled_rx_probe_due(&self, now: Instant) -> bool {
        self.recycled_rx_probe_deadline()
            .is_some_and(|deadline| now >= deadline)
    }

    /// Return a complete software-owned successor only after every higher
    /// priority RX edge has been excluded at this transaction boundary.
    ///
    /// Both the ordinary outer loop and the saturated single-owner chain use
    /// this exact admission check. Keeping it here prevents the fast chain
    /// from bypassing a due recycle-only continuation merely because no fresh
    /// MAC IRQ was posted for it.
    pub(super) fn prepared_network_tx_candidate(
        &mut self,
    ) -> Result<Option<(NetworkInterfaceId, usize)>, B::Error> {
        let prepared = self.services.has_prepared_tx();
        let rx_blocked = prepared
            && (self.services.has_rx_work()
                || self.irq.rx_signaled()
                || self.recycled_rx_probe_due(Instant::now()));
        #[cfg(any(feature = "diagnostics", test))]
        self.services.mark_prepared_tx_scheduler_phase(
            PreparedTxSchedulerPhase::PreparedReadinessChecked,
            Instant::now().as_micros(),
        );
        if !prepared || rx_blocked {
            return Ok(None);
        }

        let interface = self.retained_prepared_tx_interface();
        let network = self.tx_consumer_for(interface);
        #[cfg(feature = "tx-phase-telemetry")]
        let tx_phase_started = Core0PerformanceSample::read();
        self.services.advance_prepared_tx(&network)?;
        #[cfg(feature = "tx-phase-telemetry")]
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Prepare,
            tx_phase_started,
            Core0PerformanceSample::read(),
        );
        if !self.services.prepared_tx_start_ready() {
            return Ok(None);
        }
        let admitted = self.services.prepared_tx_frame_count().max(1);
        let preferred = self.services.preferred_tx_batch_size_for(interface).max(1);
        #[cfg(any(feature = "diagnostics", test))]
        self.services.mark_prepared_tx_scheduler_phase(
            PreparedTxSchedulerPhase::PreparedBatchChecked,
            Instant::now().as_micros(),
        );
        Ok((admitted >= preferred).then_some((interface, admitted)))
    }

    pub(super) const fn network_turn_owed(&self) -> bool {
        self.rx_frame_deficit >= RX_TX_FAIRNESS_QUANTUM_FRAMES as i64
    }

    pub(super) fn account_tx_frames(&mut self, frames: usize) {
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_sub(i64::try_from(frames.max(1)).unwrap_or(i64::MAX));
    }

    /// Publish one complete software-owned standby transaction.
    ///
    /// Keeping this boundary in one helper lets the saturated scheduler use
    /// the same accounting and ownership transition without re-entering the
    /// generic queue/batch discovery path. The caller must still establish
    /// stop, control and RX priority before invoking it.
    pub(super) async fn start_prepared_network_tx(
        &mut self,
        interface: NetworkInterfaceId,
        admitted: usize,
        tx_batch_states: &mut [TxBatchState; 2],
    ) -> Result<(), B::Error> {
        #[cfg(feature = "tx-phase-telemetry")]
        if let Some(completed) = self.prepared_tx_completion.take() {
            CORE0_PERFORMANCE.record_tx_prepared_gap(completed, Core0PerformanceSample::read());
        }
        #[cfg(any(feature = "diagnostics", test))]
        self.services.mark_prepared_tx_scheduler_phase(
            PreparedTxSchedulerPhase::PreparedEntry,
            Instant::now().as_micros(),
        );
        self.account_tx_frames(admitted);
        self.account_pair_tx_frames(interface, admitted);
        let network_tx = self.tx_consumer_for(interface);
        #[cfg(feature = "tx-phase-telemetry")]
        let tx_phase_started = Core0PerformanceSample::read();
        let start = self.services.start_prepared_tx(&network_tx)?;
        let progress = start.progress();
        #[cfg(feature = "tx-phase-telemetry")]
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Publish,
            tx_phase_started,
            Core0PerformanceSample::read(),
        );
        let slot = self.tx_batch_state_slot(interface);
        tx_batch_states[slot].note_started(admitted);
        self.prepared_tx_interface = self.services.has_prepared_tx().then_some(interface);
        if progress == WifiTxProgress::Pending {
            self.begin_active_tx(interface, DatapathTxOrigin::Network);
            // Standalone operation keeps the double-buffered pipeline live.
            // A paired owner returns the physical owner at every transaction
            // boundary and `drive_active_tx` disables look-ahead for it.
            self.drive_active_tx(true).await?;
        }
        Ok(())
    }

    pub(super) fn discard_stale_tx_wakes(&self) {
        while self.irq.try_take_tx().is_some() {}
    }

    /// Extend the software-owned standby batch before completing the active
    /// hardware transaction when both edges are ready together.
    ///
    /// `select4` deliberately gives the TX interrupt priority over the
    /// network future. Without this bounded non-blocking probe, completion
    /// immediately publishes a standby batch which may contain only its first
    /// frame, even though the rest of the producer burst is already queued.
    async fn prepare_ready_tx_before_completion(
        &mut self,
        allow_standby: bool,
    ) -> Result<(), B::Error> {
        if !allow_standby || !self.services.can_prepare_tx() {
            return Ok(());
        }
        let interface = self
            .active_tx_interface
            .expect("active TX preparation requires one VIF owner");
        if self.competing_tx_pending(interface) {
            return Ok(());
        }
        let network = self.tx_consumer_for(interface);
        self.services.advance_prepared_tx(&network)?;
        if self.services.prepared_tx_start_ready() {
            self.prepared_tx_interface = Some(interface);
            return Ok(());
        }
        let starts_new_batch = !self.services.has_prepared_tx();
        let frame = if starts_new_batch {
            self.try_receive_egress_head_for(interface)
        } else {
            self.network.try_receive_tx(interface)
        };
        let Some(frame) = frame else {
            return Ok(());
        };
        #[cfg(feature = "tx-phase-telemetry")]
        let tx_phase_started = Core0PerformanceSample::read();
        self.services.prepare_tx(frame, &network).await?;
        #[cfg(feature = "tx-phase-telemetry")]
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Prepare,
            tx_phase_started,
            Core0PerformanceSample::read(),
        );
        if self.services.has_prepared_tx() {
            self.prepared_tx_interface = Some(interface);
        }
        Ok(())
    }

    async fn service_active_tx(&mut self, wake: WifiTxWake) -> Result<WifiTxProgress, B::Error> {
        #[cfg(feature = "tx-phase-telemetry")]
        let tx_phase_started = Core0PerformanceSample::read();
        let progress = self.services.service_tx(wake).await?;
        #[cfg(feature = "tx-phase-telemetry")]
        CORE0_PERFORMANCE.record_tx_phase(
            Core0TxPhase::Service,
            tx_phase_started,
            Core0PerformanceSample::read(),
        );
        if progress == WifiTxProgress::Complete {
            let origin = self.finish_active_tx();
            #[cfg(feature = "tx-phase-telemetry")]
            {
                if origin == DatapathTxOrigin::Network {
                    let prepared = self.services.has_prepared_tx();
                    let prepared_frames = self.services.prepared_tx_frame_count();
                    let preferred_frames = self
                        .prepared_tx_interface
                        .map(|interface| self.services.preferred_tx_batch_size_for(interface))
                        .unwrap_or(1);
                    CORE0_PERFORMANCE.record_tx_network_completion(
                        prepared_frames,
                        preferred_frames,
                        self.network_tx_queue_len() != 0,
                    );
                    self.prepared_tx_completion = prepared.then(Core0PerformanceSample::read);
                } else {
                    self.prepared_tx_completion = None;
                }
            }
            if origin == DatapathTxOrigin::Control {
                self.control_ready_latched = true;
            }
        }
        Ok(progress)
    }

    pub(super) async fn drive_active_tx(&mut self, allow_standby: bool) -> Result<(), B::Error> {
        // A same-channel pair must expose a finite physical ownership edge
        // after every transaction. Both role encoders already drain all
        // immediately queued leases into the current negotiated aggregate;
        // retaining a second DMA arena across completion would postpone the
        // other VIF's protocol/control work and can exhaust the shared RX
        // staging pool. Single-VIF runners keep the throughput look-ahead.
        let active_tx_origin = self
            .active_tx_origin
            .expect("active TX drive retains its semantic origin");
        let allow_standby = tx_lookahead_allowed(allow_standby, self.interfaces, active_tx_origin);
        let mut progress = WifiTxProgress::Pending;
        let mut rx_producer_serviced = false;
        while progress == WifiTxProgress::Pending {
            // Preserve vendor RX-first ordering for the first simultaneous
            // edge, but do not let a coalesced/reposted RX level starve an
            // already terminal TX transaction. One DMA-only producer pass is
            // enough to release the captured RX frontier; the next latched
            // completion belongs to the active affine TX owner.
            if rx_producer_serviced && let Some(events) = self.irq.try_take_tx() {
                self.prepare_ready_tx_before_completion(allow_standby)
                    .await?;
                progress = self
                    .service_active_tx(WifiTxWake::Interrupt { events })
                    .await?;
                rx_producer_serviced = false;
                continue;
            }
            // A continuously reasserted RX level can remain the first ready
            // branch in the ordered select below. Once one producer pass has
            // released that durable RX frontier, also probe the transaction's
            // executor deadline without blocking. Otherwise RX traffic can
            // starve the sole fallback which polls a physically completed TX
            // whose interrupt edge was coalesced with RX.
            if rx_producer_serviced
                && matches!(
                    select(self.services.wait_tx_deadline(), ready(())).await,
                    Either::First(())
                )
            {
                progress = self.service_active_tx(WifiTxWake::Deadline).await?;
                rx_producer_serviced = false;
                continue;
            }
            rx_producer_serviced = false;
            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let recycled_rx_probe_deadline = self.recycled_rx_probe_deadline();
            let service_rx_during_tx = self.services.can_service_rx_during_tx();
            let active_tx_interface = self.active_tx_interface;
            let competing_tx_pending =
                active_tx_interface.is_some_and(|interface| self.competing_tx_pending(interface));
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                if !service_rx_during_tx {
                    pending().await
                } else {
                    match rx_progress {
                        DatapathRxProgress::StageCapacityBlocked => irq.wait_rx_capacity().await,
                        DatapathRxProgress::NetworkBackpressured => {
                            let _ = select(
                                core::future::poll_fn(|context| network_rx.poll_any_ready(context)),
                                irq.wait_rx(),
                            )
                            .await;
                        }
                        DatapathRxProgress::RecycledAppendPending
                            if recycled_rx_probe_deadline.is_some() =>
                        {
                            if let Some(deadline) = recycled_rx_probe_deadline {
                                Timer::at(deadline).await;
                            } else {
                                irq.wait_rx().await;
                            }
                        }
                        DatapathRxProgress::ProbePending
                        | DatapathRxProgress::RecycledAppendPending => irq.wait_rx().await,
                        DatapathRxProgress::Drained
                        | DatapathRxProgress::ProtocolBlockedByTx
                        | DatapathRxProgress::BudgetExhausted
                        | DatapathRxProgress::UpperLayerBlockedButDroppable => irq.wait_rx().await,
                    }
                }
            };
            let can_prepare =
                allow_standby && self.services.can_prepare_tx() && !competing_tx_pending;
            let preparation_threshold = can_prepare.then(|| {
                let interface = active_tx_interface
                    .expect("active TX network preparation requires one VIF owner");
                self.services
                    .preferred_tx_batch_size_for(interface)
                    .max(1)
                    .saturating_sub(self.services.prepared_tx_frame_count())
                    .max(1)
            });
            let network = &self.network;
            let wait_network = async {
                if let Some(minimum) = preparation_threshold {
                    let interface = active_tx_interface
                        .expect("active TX network preparation requires one VIF owner");
                    network.wait_tx_queue_len_at_least(interface, minimum).await;
                } else {
                    pending().await
                }
            };
            let wake = select4(
                wait_rx,
                self.irq.wait_tx(),
                self.services.wait_tx_deadline(),
                wait_network,
            )
            .await;
            match wake {
                Either4::First(()) => {
                    self.service_rx_during_tx().await?;
                    rx_producer_serviced = true;
                }
                Either4::Second(events) => {
                    self.prepare_ready_tx_before_completion(allow_standby)
                        .await?;
                    progress = self
                        .service_active_tx(WifiTxWake::Interrupt { events })
                        .await?;
                    rx_producer_serviced = false;
                }
                Either4::Third(()) => {
                    progress = self.service_active_tx(WifiTxWake::Deadline).await?;
                    rx_producer_serviced = false;
                }
                Either4::Fourth(()) => {
                    let interface =
                        active_tx_interface.expect("active TX preparation requires one VIF owner");
                    let network = self.tx_consumer_for(interface);
                    self.services.advance_prepared_tx(&network)?;
                    if self.services.prepared_tx_start_ready() {
                        self.prepared_tx_interface = Some(interface);
                        continue;
                    }
                    let starts_new_batch = !self.services.has_prepared_tx();
                    let frame = if starts_new_batch {
                        self.try_receive_egress_head_for(interface)
                    } else {
                        self.network.try_receive_tx(interface)
                    };
                    let Some(frame) = frame else {
                        // `advance_prepared_tx` may consume the readiness edge
                        // itself: an out-of-core completion can be encoded and
                        // immediately replaced by the next affine batch. The
                        // old synchronous queue path could not do that, so its
                        // `expect` encoded an invariant which no longer holds.
                        continue;
                    };
                    let interface = self.tx_interface_for(&frame);
                    let active = self
                        .active_tx_interface
                        .expect("active TX preparation requires one VIF owner");
                    assert_eq!(interface, active, "prepared TX cannot cross VIFs");
                    #[cfg(feature = "tx-phase-telemetry")]
                    let tx_phase_started = Core0PerformanceSample::read();
                    self.services.prepare_tx(frame, &network).await?;
                    #[cfg(feature = "tx-phase-telemetry")]
                    CORE0_PERFORMANCE.record_tx_phase(
                        Core0TxPhase::Prepare,
                        tx_phase_started,
                        Core0PerformanceSample::read(),
                    );
                    if self.services.has_prepared_tx() {
                        self.prepared_tx_interface = Some(interface);
                    }
                }
            }
            #[cfg(any(feature = "diagnostics", test))]
            if progress == WifiTxProgress::Complete {
                self.services.mark_prepared_tx_scheduler_phase(
                    PreparedTxSchedulerPhase::ActiveServiceReturned,
                    Instant::now().as_micros(),
                );
            }
        }
        Ok(())
    }

    /// Finish an already-live physical TX while rolling back a failed role.
    ///
    /// A protocol/DMA service error may be reported while `drive_active_tx`
    /// still retains the affine TX owner. A subsequent stop transaction must
    /// not attempt to activate another role first. RX remains masked and is
    /// stopped by the outer rollback after this terminal TX edge.
    pub(super) async fn drive_active_tx_for_stop(&mut self) -> Result<(), B::Error> {
        let mut progress = WifiTxProgress::Pending;
        while progress == WifiTxProgress::Pending {
            match select(self.irq.wait_tx(), self.services.wait_tx_deadline()).await {
                Either::First(events) => {
                    progress = self
                        .service_active_tx(WifiTxWake::Interrupt { events })
                        .await?;
                }
                Either::Second(()) => {
                    progress = self.service_active_tx(WifiTxWake::Deadline).await?;
                }
            }
        }
        Ok(())
    }
}
