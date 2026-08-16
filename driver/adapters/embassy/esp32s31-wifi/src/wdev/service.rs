use super::*;

impl<
    'resources,
    'irq,
    M: RawMutex,
    N,
    B,
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
{
    pub(super) async fn service_rx(&mut self) -> Result<(), B::Error> {
        let progress = self.services.service_rx(&mut self.network_rx).await?;
        self.rx_progress = progress;
        self.network_turn_owed = progress == WdevRxProgress::ProbePending;
        // One service call owns exactly the completion frontier captured at
        // its start. Yield at that hardware epoch boundary so a separate
        // protocol task can consume staged ownership before another RX epoch.
        yield_now().await;
        if progress == WdevRxProgress::ProbePending {
            // Direct BASE publication of an exhausted list has no reload
            // interrupt. Repost only after the cooperative boundary so the
            // next service observes hardware after a distinct executor turn.
            self.irq.notify_rx_handoff();
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
    async fn prepare_ready_tx_before_completion(&mut self) -> Result<(), B::Error> {
        if !self.services.can_prepare_tx() {
            return Ok(());
        }
        let Some(frame) = self.network.try_receive_tx() else {
            return Ok(());
        };
        let network = self.network.tx_consumer();
        self.services.prepare_tx(frame, &network).await
    }

    pub(super) async fn drive_active_tx(&mut self) -> Result<(), B::Error> {
        let mut progress = WifiTxProgress::Pending;
        while progress == WifiTxProgress::Pending {
            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let service_rx_during_tx = self.services.service_rx_during_tx();
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                if !service_rx_during_tx {
                    pending().await
                } else {
                    match rx_progress {
                        WdevRxProgress::StagingBackpressured => irq.wait_rx_capacity().await,
                        WdevRxProgress::NetworkBackpressured => network_rx.wait_ready().await,
                        WdevRxProgress::Drained | WdevRxProgress::ProbePending => {
                            irq.wait_rx().await
                        }
                    }
                }
            };
            let can_prepare = self.services.can_prepare_tx();
            let wait_network = async {
                if can_prepare {
                    self.network.receive_tx().await
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
                Either4::First(()) => self.service_rx().await?,
                Either4::Second(events) => {
                    self.prepare_ready_tx_before_completion().await?;
                    progress = self
                        .services
                        .service_tx(WifiTxWake::Interrupt { events })
                        .await?;
                }
                Either4::Third(()) => {
                    progress = self.services.service_tx(WifiTxWake::Deadline).await?;
                }
                Either4::Fourth(frame) => {
                    let network = self.network.tx_consumer();
                    self.services.prepare_tx(frame, &network).await?;
                }
            }
        }
        Ok(())
    }
}
