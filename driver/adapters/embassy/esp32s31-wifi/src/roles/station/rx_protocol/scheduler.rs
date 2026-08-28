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
    /// Whether this owner can make protocol progress without a fresh MAC IRQ.
    pub fn has_ready_work(&self) -> bool {
        !self.frames.is_empty()
            || self
                .processor
                .reorder_commands
                .as_ref()
                .is_some_and(|commands| !commands.is_empty())
            || self
                .processor
                .next_gap_deadline()
                .is_some_and(|(_, deadline)| deadline <= Instant::now())
    }

    /// Wait only for the next finite reorder deadline.
    ///
    /// Staging publications already carry the physical RX wake and reorder
    /// commands are emitted by the connected control owner. Keeping this
    /// future timer-only avoids consuming a queue item merely to wake the
    /// common DATAPATH scheduler.
    pub fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        let deadline = self
            .processor
            .next_gap_deadline()
            .map(|(_, deadline)| deadline);
        async move {
            match deadline {
                Some(deadline) => Timer::at(deadline).await,
                None => pending().await,
            }
        }
    }

    /// Drain ready commands/deadlines and at most `maximum_frames` staging
    /// leases without waiting for another producer edge.
    ///
    /// Command and gap processing stays ahead of every frame exactly as in the
    /// former task loop. The independent action bound prevents a continuously
    /// publishing control plane from turning one radio poll into an unbounded
    /// protocol loop.
    pub async fn service_bounded(&mut self, maximum_frames: usize) -> ConnectedRxProtocolTurn {
        assert!(
            self.processor.runtime.dispatcher_configured(),
            "stopped connected RX protocol cannot service another turn"
        );
        let maximum_frames = maximum_frames.max(1);
        let maximum_actions = maximum_frames
            .saturating_add(crate::datapath::rx::reorder::RX_REORDER_COMMAND_CAPACITY)
            .saturating_add(RX_BLOCK_ACK_BANK_COUNT);
        let mut actions = 0_usize;
        let mut consumed_frames = 0_usize;

        while consumed_frames < maximum_frames && actions < maximum_actions {
            if let Some(command) = self
                .processor
                .reorder_commands
                .as_ref()
                .and_then(try_receive_rx_reorder_command)
            {
                actions = actions.saturating_add(1);
                let _ = self.processor.apply_reorder_command(command).await;
                continue;
            }

            if let Some((bank, deadline)) = self.processor.next_gap_deadline()
                && deadline <= Instant::now()
            {
                actions = actions.saturating_add(1);
                let _ = self.processor.expire_reorder_gap(bank).await;
                continue;
            }

            let Ok(frame) = self.frames.try_receive() else {
                break;
            };
            #[cfg(feature = "task-poll-telemetry")]
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .record_protocol_frame_dequeued(crate::diagnostics::core0_rx_cycles::cycle_count());
            actions = actions.saturating_add(1);
            consumed_frames = consumed_frames.saturating_add(1);
            let _ = self.processor.dispatch_frame(frame).await;
        }

        ConnectedRxProtocolTurn {
            consumed_frames,
            work_remaining: self.has_ready_work(),
        }
    }
}
