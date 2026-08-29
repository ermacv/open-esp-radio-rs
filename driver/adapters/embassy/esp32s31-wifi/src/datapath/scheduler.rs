use super::*;

#[cfg(feature = "tx-phase-telemetry")]
use crate::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0TxPhase,
};

#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::{
    CORE0_RX_CYCLES, Core0ControlOutcome, Core0RxSchedulerCycleProfile, Core0RxSchedulerPath,
    cycle_count,
};

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
    /// Run the production radio event loop until role policy reaches its
    /// terminal edge.
    pub async fn run(&mut self) -> Result<DatapathRunnerExit<B::Exit>, B::Error> {
        self.run_until(pending()).await
    }

    /// Run the production radio event loop until a role exit or caller stop.
    ///
    /// RX is the first future in both selects. Embassy's ordered `select`
    /// therefore preserves the recovered `wDev_ProcessFiq` priority when RX
    /// and TX become ready together. A pinned network lease stays live until
    /// `service_tx` proves that hardware ownership has ended; dropping the
    /// lease then returns that slot to `embassy-net`.
    ///
    /// `stop` is observed only at transaction boundaries. If it becomes ready
    /// during TX, the normal IRQ/deadline path first releases hardware; the
    /// next idle boundary returns [`DatapathRunnerExit::Stopped`]. This makes
    /// cancellation bounded without inventing an unsafe descriptor abort.
    pub async fn run_until<S>(&mut self, stop: S) -> Result<DatapathRunnerExit<B::Exit>, B::Error>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        let mut stopping = false;
        #[cfg(feature = "diagnostics")]
        let mut stop_iterations = 0_u8;
        let mut tx_batch_states = [TxBatchState::new(); 2];
        loop {
            #[cfg(feature = "task-poll-telemetry")]
            let mut core0_scheduler_cycles = Core0RxSchedulerCycleProfile::begin();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::SchedulerLoopResumed,
                Instant::now().as_micros(),
            );
            // Poll the caller edge before servicing control. `ready(())`
            // makes this a non-blocking ordered probe, with stop winning an
            // exact tie before another transaction can begin.
            if !stopping && matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                stopping = true;
                #[cfg(feature = "diagnostics")]
                log::info!(
                    "open-radio: DATAPATH stop observed active_tx={} prepared_tx={}",
                    self.active_tx_interface.is_some(),
                    self.prepared_tx_interface.is_some(),
                );
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.stop_completed();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::StopPollCompleted,
                Instant::now().as_micros(),
            );
            if stopping {
                #[cfg(feature = "diagnostics")]
                {
                    if stop_iterations < 16 {
                        log::info!(
                            "open-radio: DATAPATH stop turn={} active_tx={} prepared_tx={} control_before_stop={}",
                            stop_iterations,
                            self.active_tx_interface.is_some(),
                            self.prepared_tx_interface.is_some(),
                            self.services.control_required_before_stop(),
                        );
                    }
                    stop_iterations = stop_iterations.saturating_add(1);
                }
                // An error returned from RX-during-TX retains the exact live
                // transaction. Resume it to its terminal IRQ/deadline edge
                // before asking either paired role to acquire physical TX
                // for shutdown control.
                if self.active_tx_interface.is_some() {
                    self.drive_active_tx_for_stop().await?;
                    continue;
                }
                self.services.cancel_prepared_tx()?;
                self.prepared_tx_interface = None;
                if self.services.control_required_before_stop() {
                    match self
                        .services
                        .service_control(DatapathControlContext::STOPPING)
                        .await?
                    {
                        DatapathControlProgress::More => continue,
                        DatapathControlProgress::TxPending => {
                            self.begin_active_tx(
                                self.reported_active_tx_interface(),
                                DatapathTxOrigin::Control,
                            );
                            self.drive_active_tx(false).await?;
                            continue;
                        }
                        DatapathControlProgress::Exit(exit) => {
                            self.set_scope_link_state(open_esp_radio_embassy_net::LinkState::Down);
                            return Ok(DatapathRunnerExit::Role(exit));
                        }
                        DatapathControlProgress::Idle => {}
                    }
                }
                match self.services.service_stop()? {
                    DatapathStopProgress::More => continue,
                    DatapathStopProgress::TxPending => {
                        self.begin_active_tx(
                            self.reported_active_tx_interface(),
                            DatapathTxOrigin::Control,
                        );
                        self.drive_active_tx(false).await?;
                        continue;
                    }
                    DatapathStopProgress::Stopped => {
                        self.set_scope_link_state(open_esp_radio_embassy_net::LinkState::Down);
                        return Ok(DatapathRunnerExit::Stopped);
                    }
                }
            }
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.discard_wakes_completed();
            let network_tx_pending = self.network_tx_queue_len() != 0;
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.first_network_queue_completed();
            let control_ready = self.control_ready_latched
                || self.services.control_ready(Instant::now().as_micros())
                || (network_tx_pending && self.services.control_required_before_network_tx());
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.control_ready_completed();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::ControlReadinessChecked {
                    ready: control_ready,
                },
                Instant::now().as_micros(),
            );
            if control_ready {
                self.control_ready_latched = false;
                let control_context = DatapathControlContext {
                    network_tx_pending,
                    stop_pending: false,
                };
                #[cfg(feature = "task-poll-telemetry")]
                let core0_control_started = cycle_count();
                let control_progress = self.services.service_control(control_context).await?;
                #[cfg(feature = "task-poll-telemetry")]
                CORE0_RX_CYCLES.record_control(
                    cycle_count().wrapping_sub(core0_control_started),
                    match &control_progress {
                        DatapathControlProgress::Idle => Core0ControlOutcome::Idle,
                        DatapathControlProgress::More => Core0ControlOutcome::More,
                        DatapathControlProgress::TxPending => Core0ControlOutcome::TxPending,
                        DatapathControlProgress::Exit(_) => Core0ControlOutcome::Exit,
                    },
                );
                match control_progress {
                    DatapathControlProgress::More => {
                        self.control_ready_latched = true;
                        continue;
                    }
                    DatapathControlProgress::TxPending => {
                        self.begin_active_tx(
                            self.reported_active_tx_interface(),
                            DatapathTxOrigin::Control,
                        );
                        self.drive_active_tx(true).await?;
                        continue;
                    }
                    DatapathControlProgress::Exit(exit) => {
                        self.services.cancel_prepared_tx()?;
                        self.prepared_tx_interface = None;
                        self.set_scope_link_state(open_esp_radio_embassy_net::LinkState::Down);
                        return Ok(DatapathRunnerExit::Role(exit));
                    }
                    DatapathControlProgress::Idle => {}
                }
            }
            // The active transaction can finish with a complete standby
            // aggregate already owned by software. Once stop, control and RX
            // priority have been established above, publish it directly.
            // Re-running physical queue discovery, burst classification and
            // collection-deadline calculation here creates an avoidable air
            // gap on every saturated BA transaction.
            if self.services.has_prepared_tx()
                && !self.services.has_rx_work()
                && !self.irq.rx_signaled()
            {
                let interface = self.retained_prepared_tx_interface();
                let admitted = self.services.prepared_tx_frame_count().max(1);
                let preferred = self.services.preferred_tx_batch_size_for(interface).max(1);
                if admitted >= preferred {
                    self.start_prepared_network_tx(interface, admitted, &mut tx_batch_states)
                        .await?;
                    continue;
                }
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.prepared_completed();
            let network_tx_pending =
                self.services.has_prepared_tx() || self.network_tx_queue_len() != 0;
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.network_pending_completed();
            let now = Instant::now();
            match self.interfaces {
                DatapathInterfaceScope::Single(interface) => {
                    if !self.network_tx_pending_for(interface) {
                        tx_batch_states[0].note_idle(now);
                    }
                }
                DatapathInterfaceScope::Pair { first, second } => {
                    if !self.network_tx_pending_for(first) {
                        tx_batch_states[0].note_idle(now);
                    }
                    if !self.network_tx_pending_for(second) {
                        tx_batch_states[1].note_idle(now);
                    }
                }
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.tx_checks_completed();

            // A staged protocol owner or reorder timeout has no new MAC IRQ
            // edge, but it is still charged to the same frame deficit as an
            // IRQ-originated RX turn. Under saturation it must yield one TX
            // transaction at the configured quantum; otherwise this early
            // software-work branch bypasses the fairness gate below.
            if self.services.has_rx_work() && !(network_tx_pending && self.network_turn_owed()) {
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Software);
                self.service_rx().await?;
                continue;
            }

            // A delayed recycled-only continuation retains the masked RX
            // epoch without keeping this task continuously runnable. Once its
            // deadline is due it has the same priority as the software repost
            // it replaces, including the existing RX/TX fairness gate.
            if self.recycled_rx_probe_due(Instant::now())
                && !(network_tx_pending && self.network_turn_owed())
            {
                self.clear_recycled_rx_probe_deadline();
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Software);
                self.service_rx().await?;
                continue;
            }

            // Consume a coalesced RX frontier before admitting a fresh
            // network transaction, matching the recovered FIQ priority. If a
            // previous finite RX pass reported a continuation while network
            // TX is queued, admit exactly one network transaction first; the
            // reposted RX signal remains pending for the following boundary.
            let rx_can_run = matches!(
                self.rx_progress,
                DatapathRxProgress::Drained
                    | DatapathRxProgress::ProbePending
                    | DatapathRxProgress::ProtocolBlockedByTx
                    | DatapathRxProgress::RecycledAppendPending
                    | DatapathRxProgress::BudgetExhausted
                    | DatapathRxProgress::UpperLayerBlockedButDroppable
            );
            if rx_can_run
                && self.irq.rx_signaled()
                && !(network_tx_pending && self.network_turn_owed())
            {
                self.irq.wait_rx().await;
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Irq);
                self.service_rx().await?;
                continue;
            }

            let mut wait_for_batch_until = None;
            if network_tx_pending {
                let interface = self
                    .next_network_tx_interface()
                    .expect("pending network TX has one VIF owner");
                let preferred = self.services.preferred_tx_batch_size_for(interface);
                let available = self
                    .services
                    .prepared_tx_frame_count()
                    .saturating_add(self.network.tx_queue_len(interface));
                let slot = self.tx_batch_state_slot(interface);
                wait_for_batch_until =
                    tx_batch_states[slot].collection_deadline(preferred, available, Instant::now());

                if wait_for_batch_until.is_none() {
                    // A partial standby arena and newly queued frames form
                    // one batch. Extend it once; the aggregate owner drains
                    // every immediately ready lease up to the negotiated
                    // target.
                    if self.services.has_prepared_tx() {
                        let interface = self.retained_prepared_tx_interface();
                        if self.services.can_prepare_tx()
                            && let Some(frame) = self.network.try_receive_tx(interface)
                        {
                            assert_eq!(self.tx_interface_for(&frame), interface);
                            let network_tx = self.tx_consumer_for(interface);
                            #[cfg(feature = "tx-phase-telemetry")]
                            let tx_phase_started = Core0PerformanceSample::read();
                            self.services.prepare_tx(frame, &network_tx).await?;
                            #[cfg(feature = "tx-phase-telemetry")]
                            CORE0_PERFORMANCE.record_tx_phase(
                                Core0TxPhase::Prepare,
                                tx_phase_started,
                                Core0PerformanceSample::read(),
                            );
                        }
                        let admitted = self.services.prepared_tx_frame_count().max(1);
                        self.start_prepared_network_tx(interface, admitted, &mut tx_batch_states)
                            .await?;
                        continue;
                    }

                    let Some(frame) = self.try_receive_network_tx() else {
                        continue;
                    };
                    let interface = self.tx_interface_for(&frame);
                    let network_tx = self.tx_consumer_for(interface);
                    #[cfg(feature = "tx-phase-telemetry")]
                    let tx_phase_started = Core0PerformanceSample::read();
                    let progress = self.services.start_tx(frame, &network_tx).await?;
                    #[cfg(feature = "tx-phase-telemetry")]
                    CORE0_PERFORMANCE.record_tx_phase(
                        Core0TxPhase::Start,
                        tx_phase_started,
                        Core0PerformanceSample::read(),
                    );
                    let admitted = self.services.last_started_tx_frame_count().max(1);
                    self.account_tx_frames(admitted);
                    self.account_pair_tx_frames(interface, admitted);
                    let slot = self.tx_batch_state_slot(interface);
                    tx_batch_states[slot].note_started(admitted);
                    if progress == WifiTxProgress::Pending {
                        self.begin_active_tx(interface, DatapathTxOrigin::Network);
                        self.drive_active_tx(true).await?;
                    }
                    continue;
                }
            }

            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let recycled_rx_probe_deadline = self.recycled_rx_probe_deadline();
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                match rx_progress {
                    DatapathRxProgress::StageCapacityBlocked => irq.wait_rx_capacity().await,
                    // Network ownership can remain unavailable for multiple
                    // milliseconds. RX DMA is an independent lower frontier:
                    // wake on either capacity or a new completion so a role
                    // can keep copying and republishing descriptors into its
                    // bounded staging reserve.
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
            };
            let network = &self.network;
            let interfaces = self.interfaces;
            let prepared_tx_interface = self.prepared_tx_interface;
            match select(
                stop.as_mut(),
                select3(wait_rx, self.services.wait_control_ready(), async {
                    if let Some(deadline) = wait_for_batch_until {
                        let _ =
                            select(self.network.wait_tx_publication(), Timer::at(deadline)).await;
                    } else {
                        if let Some(interface) = prepared_tx_interface {
                            network.wait_tx_ready(interface).await;
                        } else {
                            match interfaces {
                                DatapathInterfaceScope::Single(interface) => {
                                    network.wait_tx_ready(interface).await
                                }
                                DatapathInterfaceScope::Pair { .. } => {
                                    network.wait_tx_publication().await
                                }
                            }
                        }
                    }
                }),
            )
            .await
            {
                Either::First(()) => {
                    stopping = true;
                }
                Either::Second(Either3::First(())) => {
                    #[cfg(feature = "task-poll-telemetry")]
                    core0_scheduler_cycles.finish(Core0RxSchedulerPath::Select);
                    self.service_rx().await?
                }
                Either::Second(Either3::Second(())) => {
                    self.control_ready_latched = true;
                }
                Either::Second(Either3::Third(())) => {}
            }
        }
    }
}
