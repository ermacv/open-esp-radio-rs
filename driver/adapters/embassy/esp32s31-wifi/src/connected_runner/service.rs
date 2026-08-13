use super::*;

impl<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    ConnectedRunner<
        'resources,
        'irq,
        M,
        B,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
where
    B: ConnectedRunnerServices<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
{
    pub(super) async fn service_rx(&mut self) -> Result<(), B::Error> {
        self.rx_backpressured = self.services.service_rx().await? == WifiRxProgress::Backpressured;
        // One service call owns exactly the completion frontier captured at
        // its start. Yield at that hardware epoch boundary so a separate
        // protocol task can consume staged ownership before another RX epoch.
        yield_now().await;
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
            let rx_backpressured = self.rx_backpressured;
            let wait_rx = async move {
                if rx_backpressured {
                    irq.wait_rx_capacity().await;
                } else {
                    irq.wait_rx().await;
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
