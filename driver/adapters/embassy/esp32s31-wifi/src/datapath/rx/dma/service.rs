use core::future::ready;

use super::*;

#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::{Core0RxCyclePhase, Core0RxCycleProfile};
#[cfg(all(
    feature = "core0-rx-coarse-telemetry",
    not(feature = "task-poll-telemetry")
))]
use crate::diagnostics::core0_rx_performance::Core0PerformanceDmaProfile as Core0RxCycleProfile;

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

    /// Service one frozen RX frontier as a synchronous transaction.
    ///
    /// This is the measured per-MPDU DMA walker, staging and publication
    /// working set. It deliberately lives in the semantic hot-text class;
    /// executor scheduling, protocol fallback and role control remain in
    /// cached external code.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_rx_dma_service")
    )]
    #[inline(never)]
    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        ready((|| {
            #[cfg(feature = "task-poll-telemetry")]
            let mut core0_cycles = Core0RxCycleProfile::begin();
            #[cfg(all(
                feature = "core0-rx-coarse-telemetry",
                not(feature = "task-poll-telemetry")
            ))]
            let core0_cycles = Core0RxCycleProfile::begin();
            #[cfg(any(feature = "diagnostics", test))]
            let service_started = self
                .pipeline_observer
                .map(|observer| observer.begin_service());
            #[cfg(any(feature = "diagnostics", test))]
            let hardware_buffer_full_before = self
                .pipeline_observer
                .and_then(|_| hardware.buffer_full_count());
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_dma_entry_remaining(
                self.ring
                    .accepted_list_remaining_from_next(hardware.next_descriptor()),
            );
            // A direct BASE publication has no reload doorbell and may consume
            // its only completion edge while the previous IRQ is still being
            // serviced. Advance its durable two-turn probe before freezing the
            // next LAST-bounded completion frontier.
            self.ring.observe_exhausted_republication(hardware);
            let mut recycled_descriptors = 0_usize;
            // Upper ownership returns the exact original DMA allocations.
            // Rearm only the longest ordered released prefix before freezing
            // this service's completion frontier; at most 32 such leases can
            // exist, so ring96 retains 64 descriptors in the radio ownership
            // domain. They are not necessarily armed while a delayed masked
            // epoch accumulates completions; accepted-list pressure records
            // that separate hardware condition.
            if let Some(append) = self
                .storage
                .recycle_released_prefix::<RX_DMA_SERVICE_BUDGET_UNITS, _>(&mut self.ring, hardware)
                .map_err(RxStageTransactionError::Ring)?
            {
                recycled_descriptors = append.descriptor_count;
                self.ring
                    .complete_pending_reload(hardware)
                    .map_err(RxStageTransactionError::Ring)?;
                self.ring.observe_exhausted_republication(hardware);
            }
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
            let mut current_pool_credits = pool_credits;
            let mut current_queue_credits = queue_credits;
            let mut minimum_pool_credits = pool_credits;
            let mut minimum_queue_credits = queue_credits;
            // The radio producer is the only owner which can consume either
            // credit domain. Upper tasks can only return credits while this
            // finite service transaction runs, so the local values are safe
            // lower bounds. Refresh only at the reserve boundary instead of
            // rescanning all 32 stage slots for every completed frame.
            let reserved = if STAGE_SLOTS >= VENDOR_LARGE_RX_SLOT_COUNT
                && QUEUE_DEPTH >= VENDOR_LARGE_RX_SLOT_COUNT
            {
                self.admission.critical_reserved_credits()
            } else {
                0
            };
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
            let mut stage_capacity_blocked = false;
            let mut remaining_descriptors = frontier_snapshot.descriptor_count;

            #[cfg(feature = "task-poll-telemetry")]
            core0_cycles.switch_to(Core0RxCyclePhase::Frontier);
            for _ in 0..service_budget {
                #[cfg(feature = "task-poll-telemetry")]
                core0_cycles.switch_to(Core0RxCyclePhase::Admission);
                if current_pool_credits <= reserved || current_queue_credits <= reserved {
                    current_pool_credits = self.pool.available_slots();
                    current_queue_credits = self.frames.free_capacity();
                }
                minimum_pool_credits = minimum_pool_credits.min(current_pool_credits);
                minimum_queue_credits = minimum_queue_credits.min(current_queue_credits);
                let current_credits = current_pool_credits.min(current_queue_credits);
                let ordinary_credit_available = current_credits > reserved;
                // A free ordinary credit makes ownership transfer
                // unconditional: policy can still reject the detached unit by
                // length, but no classification result can leave it at the
                // hardware frontier. In that common case the descriptor bound
                // captured by `frontier_snapshot` is already the complete
                // frozen-cursor proof, so do not rediscover the first unit and
                // then make `take_completed_unit` validate it a second time.
                //
                // At the reserve boundary we must classify before transfer
                // because PreserveForCapacity deliberately leaves ownership in
                // DMA. Keep the explicit first-unit proof only on that slow
                // path.
                let preflight = if ordinary_credit_available {
                    None
                } else {
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
                    let unit_head_index = self
                        .storage
                        .completion_frontier_head(&self.ring)
                        .map_err(RxStageTransactionError::Ring)?
                        .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                    let unit_observation = Esp32s31RxCompletedUnit {
                        head_index: unit_head_index,
                        descriptor_count: unit_frontier.descriptor_count,
                        payload_length: self
                            .storage
                            .descriptors()
                            .get(unit_head_index)
                            .map(|descriptor| {
                                open_esp_radio_esp32s31_wifi_dma::descriptor::length(
                                    descriptor.word0(),
                                ) as usize
                            })
                            .unwrap_or(0),
                    };
                    let maximum_payload_length = self
                        .admission
                        .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                        .min(STAGE_CAPACITY);
                    Some((
                        unit_frontier.descriptor_count,
                        unit_observation,
                        maximum_payload_length,
                    ))
                };
                let length_discard = preflight.is_some_and(|(_, unit, maximum)| {
                    unit.payload_length == 0 || unit.payload_length > maximum
                });
                let (unavailable, unavailable_preview) =
                    if !length_discard && let Some((_, unit_observation, _)) = preflight {
                        // Classification only affects overload admission. An
                        // ordinary unit with a free bulk credit is staged
                        // regardless of its frame-control or VIF route, so do
                        // not repeat a frozen-cursor proof and header copy on
                        // every successful hot-path transaction.
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
                        (
                            Some(self.admission.unavailable_disposition(preview)),
                            Some(preview),
                        )
                    } else {
                        (None, None)
                    };
                let stage_with_reserved_credit = matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::PreserveForCriticalAdmission)
                ) && current_credits != 0;
                if matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::PreserveForCapacity)
                ) {
                    stage_capacity_blocked = true;
                    self.admission
                        .observe(Esp32s31RxIngressObservation::BulkAdmissionBlocked(
                            unavailable_preview.expect("unavailable bulk unit has a preview"),
                        ));
                    break;
                }
                if matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::PreserveForCriticalAdmission)
                ) && !stage_with_reserved_credit
                {
                    critical_admission_blocked = true;
                    self.admission
                        .observe(Esp32s31RxIngressObservation::CriticalAdmissionBlocked(
                            unavailable_preview.expect("unavailable critical unit has a preview"),
                        ));
                    break;
                }
                let overload_drop = matches!(
                    unavailable,
                    Some(RxStageUnavailableDisposition::DiscardAndRecycle)
                );

                #[cfg(feature = "task-poll-telemetry")]
                core0_cycles.switch_to(Core0RxCyclePhase::StageTake);
                let descriptor_limit = preflight
                    .map(|(descriptor_count, _, _)| descriptor_count)
                    .unwrap_or(remaining_descriptors);
                let unit = self
                    .storage
                    .take_completed_unit(&mut self.ring, descriptor_limit)
                    .map_err(RxStageTransactionError::Ring)?
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                let unit_descriptor_count = unit.descriptor_count();
                let unit_observation = Esp32s31RxCompletedUnit {
                    head_index: unit.head_index(),
                    descriptor_count: unit_descriptor_count,
                    payload_length: unit.total_length(),
                };
                let maximum_payload_length = preflight
                    .map(|(_, _, maximum_payload_length)| maximum_payload_length)
                    .unwrap_or_else(|| {
                        self.admission
                            .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                            .min(STAGE_CAPACITY)
                    });
                let recoverable_discard = if unit_descriptor_count != 1 {
                    Some(RxStageError::Chained)
                } else if unit_observation.payload_length == 0 {
                    Some(RxStageError::Empty)
                } else if unit_observation.payload_length > maximum_payload_length {
                    Some(RxStageError::TooLong)
                } else {
                    None
                };
                remaining_descriptors = remaining_descriptors
                    .checked_sub(unit_descriptor_count)
                    .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                admitted = admitted.saturating_add(1);
                admitted_descriptors = admitted_descriptors.saturating_add(unit_descriptor_count);

                let (frame, descriptor_replenished) = if overload_drop {
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
                        Esp32s31RxIngressObservation::OverloadDiscardedAndRecycled(
                            unavailable_preview.expect("overload discard has a preview"),
                        ),
                    );
                    (None, true)
                } else if let Some(error) = recoverable_discard {
                    unit.retain_for_deferred_recycle();
                    discarded_units = discarded_units.saturating_add(1);
                    #[cfg(any(feature = "diagnostics", test))]
                    if let Some(observer) = self.pipeline_observer {
                        let discard = match error {
                            RxStageError::Empty => RxStageDiscard::Empty,
                            RxStageError::TooLong => RxStageDiscard::TooLong,
                            RxStageError::Chained => RxStageDiscard::Chained,
                            RxStageError::Exhausted => unreachable!("capacity is not malformed"),
                        };
                        observer.observe(RxPipelineObservation::StageDiscarded(discard));
                    }
                    self.admission
                        .observe(Esp32s31RxIngressObservation::DiscardRetained {
                            unit: unit_observation,
                            reason: match error {
                                RxStageError::Empty => RxStageDiscard::Empty,
                                RxStageError::TooLong => RxStageDiscard::TooLong,
                                RxStageError::Chained => RxStageDiscard::Chained,
                                RxStageError::Exhausted => {
                                    unreachable!("capacity is not malformed")
                                }
                            },
                        });
                    (None, true)
                } else {
                    if stage_with_reserved_credit {
                        critical_reserve_admitted = critical_reserve_admitted.saturating_add(1);
                        self.admission.observe(
                            Esp32s31RxIngressObservation::CriticalReserveAdmitted(
                                unavailable_preview
                                    .expect("critical reserve admission has a preview"),
                            ),
                        );
                    }
                    #[cfg(feature = "task-poll-telemetry")]
                    core0_cycles.switch_to(Core0RxCyclePhase::StagePool);
                    match self
                        .pool
                        .stage_dma_unit_deferred_bounded(unit, maximum_payload_length)?
                    {
                        RxDmaDeferredStageUnitOutcome::Staged(frame) => (Some(frame), false),
                        RxDmaDeferredStageUnitOutcome::Discarded(_) => {
                            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                        }
                    }
                };

                if descriptor_replenished {
                    #[cfg(feature = "task-poll-telemetry")]
                    core0_cycles.switch_to(Core0RxCyclePhase::Recycle);
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
                    recycled_descriptors =
                        recycled_descriptors.saturating_add(unit_descriptor_count);
                    // The vendor append path does not yield between its reload
                    // doorbell and conditional BASE-repair suffix. Preserve
                    // that transaction boundary so a later completion cannot
                    // replace the cursor observation used to settle this
                    // append.
                    #[cfg(any(feature = "diagnostics", test))]
                    let reload_started = self.pipeline_observer.map(RxPipelineObserver::now_micros);
                    #[cfg(feature = "task-poll-telemetry")]
                    core0_cycles.switch_to(Core0RxCyclePhase::Reload);
                    self.ring
                        .complete_pending_reload(hardware)
                        .map_err(RxStageTransactionError::Ring)?;
                    #[cfg(any(feature = "diagnostics", test))]
                    if let (Some(observer), Some(started)) =
                        (self.pipeline_observer, reload_started)
                    {
                        observer.observe(RxPipelineObservation::ReloadCompleted {
                            micros: observer.elapsed_micros_since(started),
                        });
                    }
                }
                #[cfg(feature = "task-poll-telemetry")]
                core0_cycles.switch_to(Core0RxCyclePhase::Publish);
                if let Some(frame) = frame {
                    staged_units = staged_units.saturating_add(1);
                    staged_bytes = staged_bytes.saturating_add(frame.length());
                    self.frames
                        .try_send(frame)
                        .map_err(|_| RxStageTransactionError::Ring(RxRingError::Corrupt))?;
                    self.admission
                        .observe(Esp32s31RxIngressObservation::Staged(unit_observation));
                    current_pool_credits = current_pool_credits.saturating_sub(1);
                    current_queue_credits = current_queue_credits.saturating_sub(1);
                }
                minimum_pool_credits = minimum_pool_credits.min(current_pool_credits);
                minimum_queue_credits = minimum_queue_credits.min(current_queue_credits);

                // Immediate discard append changes the descriptor generation.
                // Valid packets remain in the original frozen generation
                // until the complete detached prefix is refilled below.
                if descriptor_replenished {
                    frozen_cursor = self.ring.freeze_cursor(hardware);
                }
                #[cfg(feature = "task-poll-telemetry")]
                core0_cycles.switch_to(Core0RxCyclePhase::Frontier);
            }

            #[cfg(feature = "task-poll-telemetry")]
            core0_cycles.switch_to(Core0RxCyclePhase::Tail);
            self.serviced_descriptors = self
                .serviced_descriptors
                .saturating_add(u64::try_from(admitted_descriptors).unwrap_or(u64::MAX));
            self.serviced_units = self
                .serviced_units
                .saturating_add(u64::try_from(admitted).unwrap_or(u64::MAX));
            self.serviced_bytes = self
                .serviced_bytes
                .saturating_add(u64::try_from(staged_bytes).unwrap_or(u64::MAX));

            // Every append changes descriptor generation. Retain one
            // cooperative post-append confirmation before the moderated IRQ
            // source is unmasked; a completion can consume the republished
            // unit while the current RX-success edge is still owned here.
            let completed_frontier_remaining = self
                .storage
                .completed_unit_frontier_through_cursor(
                    &self.ring,
                    frozen_cursor.last_descriptor_low(),
                    frozen_cursor.next_descriptor_low(),
                )
                .map_err(RxStageTransactionError::Ring)?
                .unit_count
                != 0;
            #[cfg(feature = "core0-rx-coarse-telemetry")]
            let recycled_probe_pending =
                recycled_descriptors != 0 && !interrupt_driven_recycled_append_for_diagnostics();
            #[cfg(not(feature = "core0-rx-coarse-telemetry"))]
            let recycled_probe_pending = recycled_descriptors != 0;
            // The terminal cursor can precede the final descriptor writeback,
            // and that writeback has no guaranteed fresh RX-success edge.
            // Retain a cooperative pass until the exhausted finite list has
            // been staged and republished.
            let exhausted_writeback_pending =
                self.ring.exhausted_terminal_writeback_pending(hardware);
            let exhausted_republication_probe_pending =
                self.ring.exhausted_republication_probe_pending();

            #[cfg(feature = "core0-rx-coarse-telemetry")]
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_dma_probe_reasons(
                recycled_descriptors != 0,
                completed_frontier_remaining,
                exhausted_writeback_pending,
                exhausted_republication_probe_pending,
            );

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
                        stage_capacity_blocked,
                        critical_admission_blocked,
                        minimum_pool_credits,
                        minimum_queue_credits,
                        micros: observer.elapsed_micros_since(started),
                        hardware_buffer_full_before,
                        hardware_buffer_full_after,
                    },
                ));
            }

            #[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
            core0_cycles.finish(admitted);

            Ok(if stage_capacity_blocked || critical_admission_blocked {
                DatapathRxProgress::StageCapacityBlocked
            } else if budget_exhausted {
                DatapathRxProgress::BudgetExhausted
            } else if completed_frontier_remaining
                || exhausted_writeback_pending
                || exhausted_republication_probe_pending
            {
                DatapathRxProgress::ProbePending
            } else if recycled_probe_pending {
                DatapathRxProgress::RecycledAppendPending
            } else if overload_discarded != 0 {
                DatapathRxProgress::UpperLayerBlockedButDroppable
            } else {
                DatapathRxProgress::Drained
            })
        })())
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters {
            completed_units: self.serviced_units,
            staged_bytes: self.serviced_bytes,
        }
    }
}
