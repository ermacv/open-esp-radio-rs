use super::*;

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
>
    Esp32s31ConnectedRxProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    S: ConnectedRxProtocolSink,
{
    /// Wait for and dispatch one independently owned staged frame.
    pub async fn dispatch_next(&mut self) -> ConnectedRxDispatch {
        loop {
            if let Some(command) = self
                .reorder_commands
                .as_ref()
                .and_then(try_receive_rx_reorder_command)
            {
                if let Some(result) = self.apply_reorder_command(command).await {
                    return result;
                }
                continue;
            }

            let next_gap = self.next_gap_deadline();
            let frame = if let Some(commands) = &self.reorder_commands {
                if let Some((tid, deadline)) = next_gap {
                    match select(
                        select(commands.receive(), self.frames.receive()),
                        Timer::at(deadline),
                    )
                    .await
                    {
                        Either::First(Either::First(command)) => {
                            if let Some(result) = self.apply_reorder_command(command).await {
                                return result;
                            }
                            continue;
                        }
                        Either::First(Either::Second(frame)) => frame,
                        Either::Second(()) => {
                            if let Some(result) = self.expire_reorder_gap(tid).await {
                                return result;
                            }
                            continue;
                        }
                    }
                } else {
                    match select(commands.receive(), self.frames.receive()).await {
                        Either::First(command) => {
                            if let Some(result) = self.apply_reorder_command(command).await {
                                return result;
                            }
                            continue;
                        }
                        Either::Second(frame) => frame,
                    }
                }
            } else if let Some((tid, deadline)) = next_gap {
                match select(self.frames.receive(), Timer::at(deadline)).await {
                    Either::First(frame) => frame,
                    Either::Second(()) => {
                        if let Some(result) = self.expire_reorder_gap(tid).await {
                            return result;
                        }
                        continue;
                    }
                }
            } else {
                self.frames.receive().await
            };
            if let Some(result) = self.accept_frame(frame).await {
                return result;
            }
        }
    }

    /// Run protocol processing independently from the PAC/DMA owner.
    pub async fn run(&mut self) -> ! {
        loop {
            self.dispatch_next().await;
        }
    }

    /// Run until an outer connected-epoch owner requests teardown.
    ///
    /// The stop future is polled first, so a simultaneous frame/stop edge does
    /// not publish new network input after disconnect. Cancelling the current
    /// `dispatch_next` future drops its local staging lease; the explicit
    /// shutdown then drains queued and retained ownership before returning.
    pub async fn run_until<F: Future<Output = ()>>(
        &mut self,
        stop: F,
    ) -> ConnectedRxProtocolShutdown {
        let mut stop = core::pin::pin!(stop);
        loop {
            match select(stop.as_mut(), self.dispatch_next()).await {
                Either::First(()) => return self.shutdown_discard(),
                Either::Second(_) => {}
            }
        }
    }

    /// Consume one protocol epoch and return its scratch only after shutdown.
    pub async fn run_until_stopped<F: Future<Output = ()>>(
        mut self,
        stop: F,
    ) -> ConnectedRxProtocolStopped<'scratch> {
        let shutdown = self.run_until(stop).await;
        let (mpdu, ethernet) = self.into_scratch();
        ConnectedRxProtocolStopped {
            shutdown,
            mpdu,
            ethernet,
        }
    }
}
