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
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    /// Wait for and dispatch one independently owned staged frame.
    pub async fn dispatch_next(&mut self) -> ConnectedRxDispatch {
        loop {
            if let Some(command) = self
                .processor
                .reorder_commands
                .as_ref()
                .and_then(try_receive_rx_reorder_command)
            {
                if let Some(result) = self.processor.apply_reorder_command(command).await {
                    return result;
                }
                continue;
            }

            let next_gap = self.processor.next_gap_deadline();
            let frame = if let Some(commands) = &self.processor.reorder_commands {
                if let Some((tid, deadline)) = next_gap {
                    match select(
                        select(commands.receive(), self.frames.receive()),
                        Timer::at(deadline),
                    )
                    .await
                    {
                        Either::First(Either::First(command)) => {
                            if let Some(result) =
                                self.processor.apply_reorder_command(command).await
                            {
                                return result;
                            }
                            continue;
                        }
                        Either::First(Either::Second(frame)) => frame,
                        Either::Second(()) => {
                            if let Some(result) = self.processor.expire_reorder_gap(tid).await {
                                return result;
                            }
                            continue;
                        }
                    }
                } else {
                    match select(commands.receive(), self.frames.receive()).await {
                        Either::First(command) => {
                            if let Some(result) =
                                self.processor.apply_reorder_command(command).await
                            {
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
                        if let Some(result) = self.processor.expire_reorder_gap(tid).await {
                            return result;
                        }
                        continue;
                    }
                }
            } else {
                self.frames.receive().await
            };
            if let Some(result) = self.processor.dispatch_frame(frame).await {
                return result;
            }
        }
    }

    /// Run protocol processing independently from the PAC/DMA owner.
    pub async fn run(&mut self) -> ! {
        let mut completed_in_turn = 0;
        loop {
            self.dispatch_next().await;
            checkpoint_protocol_turn(&mut completed_in_turn).await;
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
        let mut completed_in_turn = 0;
        loop {
            match select(stop.as_mut(), self.dispatch_next()).await {
                Either::First(()) => return self.shutdown_discard(),
                Either::Second(_) => checkpoint_protocol_turn(&mut completed_in_turn).await,
            }
        }
    }

    /// Consume one protocol epoch and return its scratch only after shutdown.
    pub async fn run_until_stopped<F: Future<Output = ()>>(
        mut self,
        stop: F,
    ) -> Esp32s31ConnectedRxProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS> {
        let shutdown = self.run_until(stop).await;
        let (mpdu, ethernet, runtime) = self.into_stopped_parts();
        ConnectedRxProtocolStopped {
            shutdown,
            mpdu,
            ethernet,
            runtime,
        }
    }

    /// Run one executor-owned protocol epoch and return its exact scratch
    /// through the shared task-control capability.
    ///
    /// The observation receives value-only shutdown evidence before the task
    /// endpoint publishes completion. Dropping this future instead poisons the
    /// endpoint, so a caller cannot confuse cancellation with owner return.
    pub async fn run_controlled_task<O>(
        self,
        endpoint: ConnectedTaskEndpoint<
            '_,
            M,
            Esp32s31ConnectedRxProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
        >,
        observe_shutdown: O,
    ) where
        O: FnOnce(ConnectedRxProtocolShutdown),
    {
        let stopped = self.run_until_stopped(endpoint.wait_for_stop()).await;
        observe_shutdown(stopped.shutdown());
        endpoint.complete(stopped);
    }
}

async fn checkpoint_protocol_turn(completed: &mut usize) {
    if advance_protocol_turn(completed) {
        embassy_futures::yield_now().await;
    }
}

fn advance_protocol_turn(completed: &mut usize) -> bool {
    *completed += 1;
    if *completed < RX_PROTOCOL_DISPATCH_BUDGET {
        return false;
    }
    *completed = 0;
    true
}

#[cfg(test)]
mod tests {
    use super::{RX_PROTOCOL_DISPATCH_BUDGET, advance_protocol_turn};

    #[test]
    fn protocol_turn_yields_at_the_configured_budget() {
        let mut completed = 0;
        for _ in 1..RX_PROTOCOL_DISPATCH_BUDGET {
            assert!(!advance_protocol_turn(&mut completed));
        }
        assert!(advance_protocol_turn(&mut completed));
        assert_eq!(completed, 0);
    }
}
