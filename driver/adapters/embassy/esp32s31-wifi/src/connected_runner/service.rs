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
                    progress = self
                        .services
                        .service_tx(WifiTxWake::Interrupt { events })
                        .await?;
                }
                Either4::Third(()) => {
                    progress = self.services.service_tx(WifiTxWake::Deadline).await?;
                }
                Either4::Fourth(frame) => {
                    // The first ready lease wakes this task. Give the network
                    // producer one cooperative poll before snapshotting the
                    // bounded queue, so one socket wake can contribute its
                    // complete ready burst without an artificial timer.
                    yield_now().await;
                    let network = self.network.tx_consumer();
                    self.services.prepare_tx(frame, &network).await?;
                }
            }
        }
        Ok(())
    }
}
