use super::*;

impl<
    'storage,
    'pool,
    'queue,
    H,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
> WdevRxService<H>
    for Esp32s31StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
    >
where
    H: RxDma,
    D: RxDmaObservationDelay,
    P: Esp32s31RxStageAdmissionPolicy,
{
    type Error = RxStageTransactionError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WdevRxProgress, Self::Error>> + 'a {
        async move {
            let service_started = self
                .pipeline_observer
                .map(|observer| observer.begin_service());
            let hardware_buffer_full_before = self
                .pipeline_observer
                .and_then(|_| hardware.buffer_full_count());
            // A direct BASE publication has no reload doorbell and may consume
            // its only completion edge while the previous IRQ is still being
            // serviced. Advance its durable two-turn probe before freezing the
            // next LAST-bounded completion frontier.
            self.ring.observe_exhausted_republication(hardware);
            // Freeze the completion frontier before any descriptor is rearmed.
            // A saturated producer can therefore only create a later epoch; it
            // cannot make this service call unbounded by refilling the ring.
            let mut frozen_cursor = self.ring.freeze_cursor(hardware);
            let frontier_snapshot = self
                .storage
                .completed_unit_frontier_through_cursor(
                    &self.ring,
                    frozen_cursor.last_descriptor_low(),
                    frozen_cursor.next_descriptor_low(),
                )
                .map_err(RxStageTransactionError::Ring)?;
            let frontier = frontier_snapshot.unit_count;
            let pool_credits = self.pool.available_slots();
            let queue_credits = self.frames.free_capacity();
            let credits = pool_credits.min(queue_credits);
            let admission_budget = frontier.min(credits);
            let mut admitted = 0_usize;
            let mut admitted_descriptors = 0_usize;
            let mut staged_bytes = 0_usize;
            let mut staged_units = 0_usize;
            let mut discarded_units = 0_usize;
            let mut remaining_descriptors = frontier_snapshot.descriptor_count;

            for _ in 0..admission_budget {
                let unit_frontier = self
                    .storage
                    .first_completed_unit_frontier_through_cursor(
                        &self.ring,
                        frozen_cursor.last_descriptor_low(),
                        frozen_cursor.next_descriptor_low(),
                    )
                    .map_err(RxStageTransactionError::Ring)?;
                if unit_frontier.unit_count != 1
                    || unit_frontier.descriptor_count == 0
                    || unit_frontier.descriptor_count > remaining_descriptors
                {
                    return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                }
                let unit_descriptor_count = unit_frontier.descriptor_count;
                let unit = self
                    .storage
                    .take_completed_unit(&mut self.ring, unit_descriptor_count)
                    .map_err(RxStageTransactionError::Ring)?
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                let unit_descriptor_count = unit.descriptor_count();
                let unit_observation = Esp32s31RxCompletedUnit {
                    head_index: unit.head_index(),
                    descriptor_count: unit_descriptor_count,
                    payload_length: unit.total_length(),
                };
                remaining_descriptors = remaining_descriptors
                    .checked_sub(unit_descriptor_count)
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                admitted = admitted.saturating_add(1);
                admitted_descriptors = admitted_descriptors.saturating_add(unit_descriptor_count);
                let maximum_payload_length = self
                    .admission
                    .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                    .min(STAGE_CAPACITY);
                let frame = match self
                    .pool
                    .stage_dma_unit_deferred_bounded(unit, maximum_payload_length)?
                {
                    RxDmaDeferredStageUnitOutcome::Staged(frame) => Some(frame),
                    RxDmaDeferredStageUnitOutcome::Discarded(error) => {
                        discarded_units = discarded_units.saturating_add(1);
                        // Length is supplied by an untrusted receive unit. A
                        // malformed/FCS/oversize unit must not terminate the
                        // sole radio owner. It remains part of the same
                        // observed descriptor epoch and is reclaimed with the
                        // other copied/discarded units at the terminal tail.
                        if let Some(observer) = self.pipeline_observer {
                            let discard = match error {
                                RxStageError::Empty => RxStageDiscard::Empty,
                                RxStageError::TooLong => RxStageDiscard::TooLong,
                                _ => unreachable!("match arm admits only length discards"),
                            };
                            observer.observe(RxPipelineObservation::StageDiscarded(discard));
                        }
                        self.admission
                            .observe(Esp32s31RxIngressObservation::DiscardRetained {
                                unit: unit_observation,
                                reason: match error {
                                    RxStageError::Empty => RxStageDiscard::Empty,
                                    RxStageError::TooLong => RxStageDiscard::TooLong,
                                    _ => unreachable!("length discard was matched above"),
                                },
                            });
                        None
                    }
                };

                let append = self
                    .storage
                    .recycle_completed_unit_through_frozen_last(
                        &mut self.ring,
                        hardware,
                        frozen_cursor,
                        unit_descriptor_count,
                    )
                    .map_err(RxStageTransactionError::Ring)?
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
                if append.descriptor_count != unit_descriptor_count {
                    return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                }
                // The vendor append path does not yield between its reload
                // doorbell and conditional BASE-repair suffix. Preserve that
                // transaction boundary so a later completion cannot replace
                // the cursor observation used to settle this append.
                let reload_started = self.pipeline_observer.map(RxPipelineObserver::now_micros);
                self.ring
                    .complete_pending_reload(hardware)
                    .map_err(RxStageTransactionError::Ring)?;
                if let (Some(observer), Some(started)) = (self.pipeline_observer, reload_started) {
                    observer.observe(RxPipelineObservation::ReloadCompleted {
                        micros: observer.elapsed_micros_since(started),
                    });
                }
                if let Some(frame) = frame {
                    staged_units = staged_units.saturating_add(1);
                    staged_bytes = staged_bytes.saturating_add(frame.length());
                    self.frames
                        .try_send(frame)
                        .map_err(|_| RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                    self.admission
                        .observe(Esp32s31RxIngressObservation::Staged(unit_observation));
                }

                // The append changed descriptor-address generation. Match the
                // vendor walker by taking a new ordered LAST/NEXT image before
                // examining the next software head.
                frozen_cursor = self.ring.freeze_cursor(hardware);
            }

            let recycled_descriptors = admitted_descriptors;

            self.report.completed_units = self
                .report
                .completed_units
                .saturating_add(u32::try_from(admitted).unwrap_or(u32::MAX));
            self.report.completed_descriptors = self
                .report
                .completed_descriptors
                .saturating_add(u32::try_from(admitted_descriptors).unwrap_or(u32::MAX));
            self.report.staged_units = self
                .report
                .staged_units
                .saturating_add(u32::try_from(staged_units).unwrap_or(u32::MAX));
            self.report.staged_bytes = self
                .report
                .staged_bytes
                .saturating_add(u32::try_from(staged_bytes).unwrap_or(u32::MAX));
            self.report.discarded_units = self
                .report
                .discarded_units
                .saturating_add(u32::try_from(discarded_units).unwrap_or(u32::MAX));
            self.report.recycled_descriptors = self
                .report
                .recycled_descriptors
                .saturating_add(u32::try_from(recycled_descriptors).unwrap_or(u32::MAX));

            // Every append changes descriptor generation. Retain one
            // cooperative post-append confirmation before the moderated IRQ
            // source is unmasked; a completion can consume the republished
            // unit while the current RX-success edge is still owned here.
            let completion_frontier_remaining = recycled_descriptors != 0
                || self
                    .storage
                    .completed_unit_frontier_through_cursor(
                        &self.ring,
                        frozen_cursor.last_descriptor_low(),
                        frozen_cursor.next_descriptor_low(),
                    )
                    .map_err(RxStageTransactionError::Ring)?
                    .unit_count
                    != 0;
            // The terminal cursor can precede the final descriptor writeback,
            // and that writeback has no guaranteed fresh RX-success edge.
            // Retain a cooperative pass until the exhausted finite list has
            // been staged and republished.
            let exhausted_writeback_pending =
                self.ring.exhausted_terminal_writeback_pending(hardware);

            let credit_limited = credits < frontier;

            // Keep the two samples on opposite sides of the complete bounded
            // transaction. Diagnostics can then distinguish saturation while
            // the owner was elsewhere from saturation caused during staging
            // and descriptor republication. With only the entry sample the
            // counter increment was incorrectly paired with the following
            // service context.
            let hardware_buffer_full_after = self
                .pipeline_observer
                .and_then(|_| hardware.buffer_full_count());

            if let (Some(observer), Some(started)) = (self.pipeline_observer, service_started) {
                observer.observe(RxPipelineObservation::ServiceCompleted(
                    RxServiceObservation {
                        frontier,
                        pool_credits,
                        queue_credits,
                        admitted,
                        staged_bytes,
                        micros: observer.elapsed_micros_since(started),
                        hardware_buffer_full_before,
                        hardware_buffer_full_after,
                    },
                ));
            }

            Ok(if credit_limited && recycled_descriptors == 0 {
                WdevRxProgress::StagingBackpressured
            } else if completion_frontier_remaining
                || exhausted_writeback_pending
                || self.ring.exhausted_republication_probe_pending()
            {
                WdevRxProgress::ProbePending
            } else {
                WdevRxProgress::Drained
            })
        }
    }
}
