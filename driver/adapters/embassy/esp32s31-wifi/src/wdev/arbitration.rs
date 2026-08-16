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
    /// Run the production radio event loop until role policy reaches its
    /// terminal edge.
    pub async fn run(&mut self) -> Result<WdevRunnerExit<B::Exit>, B::Error> {
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
    /// next idle boundary returns [`WdevRunnerExit::Stopped`]. This makes
    /// cancellation bounded without inventing an unsafe descriptor abort.
    pub async fn run_until<S>(&mut self, stop: S) -> Result<WdevRunnerExit<B::Exit>, B::Error>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        let mut stopping = false;
        let mut tx_batch_deadline = None;
        loop {
            // Poll the caller edge before servicing control. `ready(())`
            // makes this a non-blocking ordered probe, with stop winning an
            // exact tie before another transaction can begin.
            if !stopping && matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                stopping = true;
            }
            if stopping {
                self.services.cancel_prepared_tx()?;
                match self.services.service_stop()? {
                    WdevStopProgress::More => continue,
                    WdevStopProgress::TxPending => {
                        self.drive_active_tx().await?;
                        continue;
                    }
                    WdevStopProgress::Stopped => {
                        self.network
                            .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                        return Ok(WdevRunnerExit::Stopped);
                    }
                }
            }
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            let control_context = WdevControlContext {
                network_tx_pending: self.network.tx_queue_len() != 0,
            };
            match self.services.service_control(control_context).await? {
                WdevControlProgress::More => continue,
                WdevControlProgress::TxPending => {
                    self.drive_active_tx().await?;
                    continue;
                }
                WdevControlProgress::Exit(exit) => {
                    self.services.cancel_prepared_tx()?;
                    self.network
                        .set_link_state(open_esp_radio_embassy_net::LinkState::Down);
                    return Ok(WdevRunnerExit::Role(exit));
                }
                WdevControlProgress::Idle => {}
            }

            let network_tx_pending =
                self.services.has_prepared_tx() || self.network.tx_queue_len() != 0;

            // Consume a coalesced RX frontier before admitting a fresh
            // network transaction, matching the recovered FIQ priority. If a
            // previous finite RX pass reported a continuation while network
            // TX is queued, admit exactly one network transaction first; the
            // reposted RX signal remains pending for the following boundary.
            let rx_can_run = matches!(
                self.rx_progress,
                WdevRxProgress::Drained | WdevRxProgress::ProbePending
            );
            if rx_can_run
                && self.irq.rx_signaled()
                && !(network_tx_pending && self.network_turn_owed)
            {
                self.irq.wait_rx().await;
                self.service_rx().await?;
                continue;
            }

            let mut wait_for_batch_until = None;
            if network_tx_pending {
                let preferred = self.services.preferred_tx_batch_size();
                let available = self
                    .services
                    .prepared_tx_frame_count()
                    .saturating_add(self.network.tx_queue_len());
                // A lone frame is not an aggregate and must retain the
                // immediate low-latency path. Once a real multi-frame burst
                // exists, collect only up to the negotiated target/deadline.
                if preferred > 1 && available >= TX_BATCH_MIN_FRAMES && available < preferred {
                    let deadline = *tx_batch_deadline
                        .get_or_insert_with(|| Instant::now() + TX_BATCH_MAX_WAIT);
                    if Instant::now() < deadline {
                        wait_for_batch_until = Some(deadline);
                    }
                }

                if wait_for_batch_until.is_none() {
                    tx_batch_deadline = None;
                    // A partial standby arena and newly queued frames form
                    // one batch. Extend it once; the aggregate owner drains
                    // every immediately ready lease up to the negotiated
                    // target.
                    if self.services.has_prepared_tx() {
                        if self.services.can_prepare_tx()
                            && let Some(frame) = self.network.try_receive_tx()
                        {
                            let network_tx = self.network.tx_consumer();
                            self.services.prepare_tx(frame, &network_tx).await?;
                        }
                        let network_tx = self.network.tx_consumer();
                        self.network_turn_owed = false;
                        let progress = self.services.start_prepared_tx(&network_tx).await?;
                        if progress == WifiTxProgress::Pending {
                            self.drive_active_tx().await?;
                        }
                        continue;
                    }

                    let Some(frame) = self.network.try_receive_tx() else {
                        continue;
                    };
                    let network_tx = self.network.tx_consumer();
                    self.network_turn_owed = false;
                    let progress = self.services.start_tx(frame, &network_tx).await?;
                    if progress == WifiTxProgress::Pending {
                        self.drive_active_tx().await?;
                    }
                    continue;
                }
            } else {
                tx_batch_deadline = None;
            }

            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                match rx_progress {
                    WdevRxProgress::StagingBackpressured => irq.wait_rx_capacity().await,
                    WdevRxProgress::NetworkBackpressured => network_rx.wait_ready().await,
                    WdevRxProgress::Drained | WdevRxProgress::ProbePending => irq.wait_rx().await,
                }
            };
            match select(
                stop.as_mut(),
                select3(wait_rx, self.services.wait_control_ready(), async {
                    if let Some(deadline) = wait_for_batch_until {
                        let _ =
                            select(self.network.wait_tx_publication(), Timer::at(deadline)).await;
                    } else {
                        self.network.wait_tx_ready().await;
                    }
                }),
            )
            .await
            {
                Either::First(()) => {
                    stopping = true;
                }
                Either::Second(Either3::First(())) => self.service_rx().await?,
                Either::Second(Either3::Second(())) => {}
                Either::Second(Either3::Third(())) => {}
            }
        }
    }
}
