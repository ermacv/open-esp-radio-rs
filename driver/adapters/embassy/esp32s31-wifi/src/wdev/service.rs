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
    WdevRunner<
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
    N: WdevNetwork<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    B: WdevServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    R: WdevNetworkRxSet,
{
    pub(super) async fn service_rx(&mut self) -> Result<(), B::Error> {
        let context = WdevRxServiceContext {
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
        if self.services.active_tx_interface().is_some() {
            self.active_tx_interface = Some(self.reported_active_tx_interface());
            self.drive_active_tx().await?;
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
        Ok(())
    }

    async fn complete_rx_service(&mut self, progress: WdevRxProgress) {
        self.rx_progress = progress;
        // One service call owns exactly the completion frontier captured at
        // its start. Yield at that hardware epoch boundary so a separate
        // protocol task can consume staged ownership before another RX epoch.
        yield_now().await;
        if progress == WdevRxProgress::Drained {
            // S31 exposes a level CPU route. A completion racing the final
            // ownership probe stays latched while masked and asserts the
            // route as soon as this ordered unmask completes; adding a
            // software probe here would duplicate every idle drain edge.
            let _ = self.irq.unmask_rx_after_drain();
        } else if progress == WdevRxProgress::ProbePending {
            // Direct BASE publication of an exhausted list has no reload
            // interrupt. Repost only after the cooperative boundary so the
            // next service observes hardware after a distinct executor turn.
            self.irq.notify_rx_handoff();
        }
    }

    pub(super) const fn network_turn_owed(&self) -> bool {
        self.rx_frame_deficit >= RX_TX_FAIRNESS_QUANTUM_FRAMES as i64
    }

    pub(super) fn account_tx_frames(&mut self, frames: usize) {
        self.rx_frame_deficit = self
            .rx_frame_deficit
            .saturating_sub(i64::try_from(frames.max(1)).unwrap_or(i64::MAX));
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
    async fn prepare_ready_tx_before_completion(&mut self) -> Result<(), B::Error> {
        if !self.services.can_prepare_tx() {
            return Ok(());
        }
        let interface = self
            .active_tx_interface
            .expect("active TX preparation requires one VIF owner");
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
        if progress == WifiTxProgress::Complete {
            self.active_tx_interface = None;
        }
        Ok(progress)
    }

    pub(super) async fn drive_active_tx(&mut self) -> Result<(), B::Error> {
        let mut progress = WifiTxProgress::Pending;
        let mut rx_producer_serviced = false;
        while progress == WifiTxProgress::Pending {
            // Preserve vendor RX-first ordering for the first simultaneous
            // edge, but do not let a coalesced/reposted RX level starve an
            // already terminal TX transaction. One DMA-only producer pass is
            // enough to release the captured RX frontier; the next latched
            // completion belongs to the active affine TX owner.
            if rx_producer_serviced && let Some(events) = self.irq.try_take_tx() {
                self.prepare_ready_tx_before_completion().await?;
                progress = self
                    .service_active_tx(WifiTxWake::Interrupt { events })
                    .await?;
                rx_producer_serviced = false;
                continue;
            }
            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let service_rx_during_tx = self.services.can_service_rx_during_tx();
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                if !service_rx_during_tx {
                    pending().await
                } else {
                    match rx_progress {
                        WdevRxProgress::StagingBackpressured => irq.wait_rx_capacity().await,
                        WdevRxProgress::NetworkBackpressured => {
                            let _ = select(
                                core::future::poll_fn(|context| network_rx.poll_any_ready(context)),
                                irq.wait_rx(),
                            )
                            .await;
                        }
                        WdevRxProgress::Drained | WdevRxProgress::ProbePending => {
                            irq.wait_rx().await
                        }
                    }
                }
            };
            let can_prepare = self.services.can_prepare_tx();
            let active_tx_interface = self.active_tx_interface;
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
                    self.prepare_ready_tx_before_completion().await?;
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
}
