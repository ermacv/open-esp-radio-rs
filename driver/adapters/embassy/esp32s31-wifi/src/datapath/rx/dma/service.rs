#![expect(
    clippy::manual_async_fn,
    reason = "the service implementation keeps the trait's explicit borrowed Future contract"
)]

use super::*;

/// Maximum completed units transferred in one masked RX poll epoch.
///
/// A later frontier remains latched and is reposted without unmasking. This
/// bounds one executor turn while keeping descriptor recycle independent from
/// protocol and network capacity.
const RX_DMA_SERVICE_BUDGET_UNITS: usize = 32;

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
> DatapathRxService<H>
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
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            #[cfg(any(feature = "diagnostics", test))]
            let service_started = self
                .pipeline_observer
                .map(|observer| observer.begin_service());
            #[cfg(any(feature = "diagnostics", test))]
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
            let mut minimum_pool_credits = pool_credits;
            let mut minimum_queue_credits = queue_credits;
            let service_budget = frontier.min(RX_DMA_SERVICE_BUDGET_UNITS);
            let mut admitted = 0_usize;
            let mut admitted_descriptors = 0_usize;
            let mut staged_bytes = 0_usize;
            let mut staged_units = 0_usize;
            let mut discarded_units = 0_usize;
            let mut overload_discarded = 0_usize;
            let mut overload_recycled_descriptors = 0_usize;
            let mut critical_reserve_admitted = 0_usize;
            let mut critical_admission_blocked = false;
            let mut remaining_descriptors = frontier_snapshot.descriptor_count;

            for _ in 0..service_budget {
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
                let unit_observation = Esp32s31RxCompletedUnit {
                    head_index: self.ring.recycle_start(),
                    descriptor_count: unit_descriptor_count,
                    payload_length: self
                        .storage
                        .descriptors()
                        .get(self.ring.recycle_start())
                        .map(|descriptor| {
                            open_esp_radio_esp32s31_wifi_dma::descriptor::length(descriptor.word0())
                                as usize
                        })
                        .unwrap_or(0),
                };
                let mut header = [0_u8; 24];
                let preview = if self
                    .storage
                    .copy_first_completed_unit_bytes_through_cursor(
                        &self.ring,
                        frozen_cursor.last_descriptor_low(),
                        frozen_cursor.next_descriptor_low(),
                        PUBLIC_HEADER_SIZE,
                        &mut header,
                    )
                    .map_err(RxStageTransactionError::Ring)?
                {
                    self.frames.preview(unit_observation, header)
                } else {
                    self.frames.unclassified_preview(unit_observation)
                };
                let current_pool_credits = self.pool.available_slots();
                let current_queue_credits = self.frames.free_capacity();
                minimum_pool_credits = minimum_pool_credits.min(current_pool_credits);
                minimum_queue_credits = minimum_queue_credits.min(current_queue_credits);
                let current_credits = current_pool_credits.min(current_queue_credits);
                let maximum_payload_length = self
                    .admission
                    .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                    .min(STAGE_CAPACITY);
                // Empty and descriptor-local oversize units need no staging
                // slot. They must reach the existing malformed discard path
                // even when upper credits are exhausted, otherwise a corrupt
                // unit could pin the hardware ring ahead of valid traffic.
                let length_discard = unit_observation.payload_length == 0
                    || unit_observation.payload_length > maximum_payload_length;
                // The reviewed large-RX production profile owns a dedicated
                // critical lane. Tiny model/scan arenas cannot reserve a slot
                // without eliminating their only useful bulk credit.
                let reserved = if STAGE_SLOTS >= VENDOR_LARGE_RX_SLOT_COUNT
                    && QUEUE_DEPTH >= VENDOR_LARGE_RX_SLOT_COUNT
                {
                    self.admission.critical_reserved_credits()
                } else {
                    0
                };
                let ordinary_credit_available = current_credits > reserved;
                let unavailable = (!length_discard && !ordinary_credit_available)
                    .then(|| self.admission.unavailable_disposition(preview));
                let stage_with_reserved_credit = matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::PreserveForCriticalAdmission)
                ) && current_credits != 0;
                if matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::PreserveForCriticalAdmission)
                ) && !stage_with_reserved_credit
                {
                    critical_admission_blocked = true;
                    self.admission
                        .observe(Esp32s31RxIngressObservation::CriticalAdmissionBlocked(
                            preview,
                        ));
                    break;
                }
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
                let overload_drop = matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::DiscardAndRecycle)
                );
                let frame = if overload_drop {
                    unit.retain_for_deferred_recycle();
                    discarded_units = discarded_units.saturating_add(1);
                    overload_discarded = overload_discarded.saturating_add(1);
                    overload_recycled_descriptors =
                        overload_recycled_descriptors.saturating_add(unit_descriptor_count);
                    #[cfg(any(feature = "diagnostics", test))]
                    if let Some(observer) = self.pipeline_observer {
                        observer.observe(RxPipelineObservation::StageDiscarded(
                            RxStageDiscard::OverloadBulk,
                        ));
                    }
                    self.admission.observe(
                        Esp32s31RxIngressObservation::OverloadDiscardedAndRecycled(preview),
                    );
                    None
                } else {
                    if stage_with_reserved_credit {
                        critical_reserve_admitted = critical_reserve_admitted.saturating_add(1);
                        self.admission.observe(
                            Esp32s31RxIngressObservation::CriticalReserveAdmitted(preview),
                        );
                    }
                    match self
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
                            #[cfg(any(feature = "diagnostics", test))]
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
                #[cfg(any(feature = "diagnostics", test))]
                let reload_started = self.pipeline_observer.map(RxPipelineObserver::now_micros);
                self.ring
                    .complete_pending_reload(hardware)
                    .map_err(RxStageTransactionError::Ring)?;
                #[cfg(any(feature = "diagnostics", test))]
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
                minimum_pool_credits = minimum_pool_credits.min(self.pool.available_slots());
                minimum_queue_credits = minimum_queue_credits.min(self.frames.free_capacity());

                // The append changed descriptor-address generation. Match the
                // vendor walker by taking a new ordered LAST/NEXT image before
                // examining the next software head.
                frozen_cursor = self.ring.freeze_cursor(hardware);
            }

            let recycled_descriptors = admitted_descriptors;

            self.serviced_descriptors = self
                .serviced_descriptors
                .saturating_add(u64::try_from(admitted_descriptors).unwrap_or(u64::MAX));

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

            let budget_exhausted = frontier > RX_DMA_SERVICE_BUDGET_UNITS;

            // Keep the two samples on opposite sides of the complete bounded
            // transaction. Diagnostics can then distinguish saturation while
            // the owner was elsewhere from saturation caused during staging
            // and descriptor republication. With only the entry sample the
            // counter increment was incorrectly paired with the following
            // service context.
            #[cfg(any(feature = "diagnostics", test))]
            let hardware_buffer_full_after = self
                .pipeline_observer
                .and_then(|_| hardware.buffer_full_count());

            #[cfg(any(feature = "diagnostics", test))]
            if let (Some(observer), Some(started)) = (self.pipeline_observer, service_started) {
                observer.observe(RxPipelineObservation::ServiceCompleted(
                    RxServiceObservation {
                        frontier,
                        pool_credits,
                        queue_credits,
                        completed_units: admitted,
                        completed_descriptors: admitted_descriptors,
                        admitted,
                        staged_units,
                        staged_bytes,
                        discarded_units,
                        recycled_descriptors,
                        overload_discarded,
                        overload_recycled_descriptors,
                        critical_reserve_admitted,
                        critical_admission_blocked,
                        minimum_pool_credits,
                        minimum_queue_credits,
                        micros: observer.elapsed_micros_since(started),
                        hardware_buffer_full_before,
                        hardware_buffer_full_after,
                    },
                ));
            }

            Ok(if critical_admission_blocked {
                DatapathRxProgress::CriticalAdmissionBlocked
            } else if budget_exhausted {
                DatapathRxProgress::BudgetExhausted
            } else if completion_frontier_remaining
                || exhausted_writeback_pending
                || self.ring.exhausted_republication_probe_pending()
            {
                DatapathRxProgress::ProbePending
            } else if overload_discarded != 0 {
                DatapathRxProgress::UpperLayerBlockedButDroppable
            } else {
                DatapathRxProgress::Drained
            })
        }
    }
}
