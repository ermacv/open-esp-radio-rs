use super::*;

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
        let context = DatapathRxServiceContext {
            maximum_protocol_frames: rx_protocol_frame_budget(
                self.rx_frame_deficit,
                self.services.has_prepared_tx() || self.network_tx_queue_len() != 0,
            ),
        };
        let serviced_before = self.services.serviced_rx_frames();
        let progress = self
            .services
            .service_rx(&mut self.network_rx, context)
            .await?;
        let serviced = self
            .services
            .serviced_rx_frames()
            .saturating_sub(serviced_before);
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_add(i64::try_from(serviced).unwrap_or(i64::MAX));
        self.complete_rx_service(progress).await;
        // A finite protocol RX turn may have staged a management response or
        // changed a role-local deadline. Re-arm control without speculating
        // about the role's protocol state.
        self.control_ready_latched = true;
        if self.services.active_tx_interface().is_some() {
            self.begin_active_tx(
                self.reported_active_tx_interface(),
                DatapathTxOrigin::Control,
            );
            self.drive_active_tx(true).await?;
        }
        Ok(())
    }

    async fn service_rx_during_tx(&mut self) -> Result<(), B::Error> {
        let serviced_before = self.services.serviced_rx_frames();
        let progress = self
            .services
            .service_rx_during_tx(&mut self.network_rx)
            .await?;
        let serviced = self
            .services
            .serviced_rx_frames()
            .saturating_sub(serviced_before);
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_add(i64::try_from(serviced).unwrap_or(i64::MAX));
        self.complete_rx_service(progress).await;
        self.control_ready_latched = true;
        Ok(())
    }

    async fn complete_rx_service(&mut self, progress: DatapathRxProgress) {
        self.rx_progress = progress;
        if matches!(
            progress,
            DatapathRxProgress::Drained | DatapathRxProgress::UpperLayerBlockedButDroppable
        ) {
            // S31 exposes a level CPU route. A completion racing the final
            // ownership probe stays latched while masked and asserts the
            // route as soon as this ordered unmask completes; adding a
            // software probe here would duplicate every idle drain edge.
            let _ = self.irq.unmask_rx_after_drain();
        } else if matches!(
            progress,
            DatapathRxProgress::ProbePending | DatapathRxProgress::BudgetExhausted
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
        yield_now().await;
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
        self.services.mark_prepared_tx_scheduler_entry();
        self.account_tx_frames(admitted);
        self.account_pair_tx_frames(interface, admitted);
        let network_tx = self.tx_consumer_for(interface);
        let progress = self.services.start_prepared_tx(&network_tx)?;
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
        let Some(frame) = self.network.try_receive_tx(interface) else {
            return Ok(());
        };
        let network = self.tx_consumer_for(interface);
        self.services.prepare_tx(frame, &network).await?;
        if self.services.has_prepared_tx() {
            self.prepared_tx_interface = Some(interface);
        }
        Ok(())
    }

    async fn service_active_tx(&mut self, wake: WifiTxWake) -> Result<WifiTxProgress, B::Error> {
        let progress = self.services.service_tx(wake).await?;
        if progress == WifiTxProgress::Complete
            && self.finish_active_tx() == DatapathTxOrigin::Control
        {
            self.control_ready_latched = true;
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
        let allow_standby = tx_lookahead_allowed(allow_standby, self.interfaces);
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
            let irq = self.irq;
            let rx_progress = self.rx_progress;
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
                        DatapathRxProgress::CriticalAdmissionBlocked => {
                            irq.wait_rx_capacity().await
                        }
                        DatapathRxProgress::NetworkBackpressured => {
                            let _ = select(
                                core::future::poll_fn(|context| network_rx.poll_any_ready(context)),
                                irq.wait_rx(),
                            )
                            .await;
                        }
                        DatapathRxProgress::Drained
                        | DatapathRxProgress::ProbePending
                        | DatapathRxProgress::BudgetExhausted
                        | DatapathRxProgress::UpperLayerBlockedButDroppable => irq.wait_rx().await,
                    }
                }
            };
            let can_prepare =
                allow_standby && self.services.can_prepare_tx() && !competing_tx_pending;
            let wait_network = async {
                if can_prepare {
                    let interface = active_tx_interface
                        .expect("active TX network preparation requires one VIF owner");
                    self.network.receive_tx(interface).await
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
                Either4::Fourth(frame) => {
                    let interface = self.tx_interface_for(&frame);
                    let active = self
                        .active_tx_interface
                        .expect("active TX preparation requires one VIF owner");
                    assert_eq!(interface, active, "prepared TX cannot cross VIFs");
                    let network = self.tx_consumer_for(interface);
                    self.services.prepare_tx(frame, &network).await?;
                    if self.services.has_prepared_tx() {
                        self.prepared_tx_interface = Some(interface);
                    }
                }
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
