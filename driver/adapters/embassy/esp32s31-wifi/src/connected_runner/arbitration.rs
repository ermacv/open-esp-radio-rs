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
    /// Run the production radio event loop until connected policy proves the
    /// peer is unreachable.
    pub async fn run(&mut self) -> Result<ConnectedRunnerExit, B::Error> {
        self.run_until(pending()).await
    }

    /// Run the production radio event loop until disconnect or caller stop.
    ///
    /// RX is the first future in both selects. Embassy's ordered `select`
    /// therefore preserves the recovered `wDev_ProcessFiq` priority when RX
    /// and TX become ready together. A pinned network lease stays live until
    /// `service_tx` proves that hardware ownership has ended; dropping the
    /// lease then returns that slot to `embassy-net`.
    ///
    /// `stop` is observed only at transaction boundaries. If it becomes ready
    /// during TX, the normal IRQ/deadline path first releases hardware; the
    /// next idle boundary returns [`ConnectedRunnerExit::Stopped`]. This makes
    /// cancellation bounded without inventing an unsafe descriptor abort.
    pub async fn run_until<S>(&mut self, stop: S) -> Result<ConnectedRunnerExit, B::Error>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        loop {
            // Poll the caller edge before servicing control. `ready(())`
            // makes this a non-blocking ordered probe, with stop winning an
            // exact tie before another transaction can begin.
            if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                self.services.cancel_prepared_tx()?;
                self.network
                    .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                return Ok(ConnectedRunnerExit::Stopped);
            }
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            let control_context = WifiControlContext {
                network_tx_pending: self.network.tx_queue_len() != 0,
            };
            match self.services.service_control(control_context).await? {
                WifiControlProgress::More => continue,
                WifiControlProgress::TxPending => {
                    self.drive_active_tx().await?;
                    continue;
                }
                WifiControlProgress::Disconnected(reason) => {
                    self.services.cancel_prepared_tx()?;
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(ConnectedRunnerExit::Disconnected(reason));
                }
                WifiControlProgress::Idle => {}
            }

            // A prepared network batch owns only software-reserved
            // descriptors and pinned leases. Give control the transaction
            // boundary above, then admit the batch before claiming newer
            // frames from the network queue.
            if self.services.has_prepared_tx() {
                let network_tx = self.network.tx_consumer();
                let progress = self.services.start_prepared_tx(&network_tx).await?;
                if progress == WifiTxProgress::Pending {
                    self.drive_active_tx().await?;
                }
                continue;
            }

            let irq = self.irq;
            let rx_backpressured = self.rx_backpressured;
            let wait_rx = async move {
                if rx_backpressured {
                    irq.wait_rx_capacity().await;
                } else {
                    irq.wait_rx().await;
                }
            };
            match select(
                stop.as_mut(),
                select3(
                    wait_rx,
                    self.services.wait_control_ready(),
                    self.network.receive_tx(),
                ),
            )
            .await
            {
                Either::First(()) => {
                    self.services.cancel_prepared_tx()?;
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(ConnectedRunnerExit::Stopped);
                }
                Either::Second(Either3::First(())) => self.service_rx().await?,
                Either::Second(Either3::Second(())) => {}
                Either::Second(Either3::Third(frame)) => {
                    // `receive_tx` may have consumed the first lease after
                    // the context at the top of the loop was sampled. Hold
                    // that lease while control restores PM=0 (if needed),
                    // then publish the data frame only after the AP-visible
                    // state is coherent again.
                    loop {
                        if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                            drop(frame);
                            self.services.cancel_prepared_tx()?;
                            self.network
                                .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                            return Ok(ConnectedRunnerExit::Stopped);
                        }
                        match self
                            .services
                            .service_control(WifiControlContext {
                                network_tx_pending: true,
                            })
                            .await?
                        {
                            WifiControlProgress::More => continue,
                            WifiControlProgress::TxPending => {
                                self.drive_active_tx().await?;
                            }
                            WifiControlProgress::Disconnected(reason) => {
                                drop(frame);
                                self.services.cancel_prepared_tx()?;
                                self.network
                                    .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                                return Ok(ConnectedRunnerExit::Disconnected(reason));
                            }
                            WifiControlProgress::Idle => break,
                        }
                    }
                    let network_tx = self.network.tx_consumer();
                    let progress = self.services.start_tx(frame, &network_tx).await?;
                    if progress == WifiTxProgress::Pending {
                        self.drive_active_tx().await?;
                    }
                }
            }
        }
    }
}
