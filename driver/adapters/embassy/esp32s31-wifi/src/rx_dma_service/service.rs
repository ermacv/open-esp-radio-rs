use super::*;

async fn await_completed_unit_link_release<H, D, const COUNT: usize>(
    ring: &mut RxRingLive<'_, COUNT>,
    hardware: &mut H,
    delay: &mut D,
    descriptor_count: usize,
) -> Result<(), RxRingError>
where
    H: RxDma,
    D: RxReloadDelay,
{
    let mut samples = 0_u32;
    loop {
        if ring.observe_current_completed_unit_link_release(hardware, descriptor_count) {
            break;
        }
        samples = samples.saturating_add(1);
        if samples >= RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT {
            return Err(RxRingError::Busy);
        }
        delay.after_micros(1).await;
    }
    Ok(())
}

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
> Esp32s31ConnectedRxService<H>
    for Esp32s31ConnectedRx<
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
    D: RxReloadDelay,
    P: Esp32s31RxStageAdmissionPolicy,
{
    type Error = RxStageTransactionError;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + 'a {
        async move {
            let service_started = self
                .pipeline_observer
                .map(|observer| observer.begin_service());
            let hardware_buffer_full = self
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
            let last_descriptor_low = hardware.last_descriptor_low();
            // LAST is the ownership frontier for the following descriptor
            // scan. Make the MMIO observation precede all descriptor reads.
            hardware.fence();
            let frontier_snapshot = self
                .storage
                .completed_unit_frontier_through(&self.ring, last_descriptor_low)
                .map_err(RxStageTransactionError::Ring)?;
            let frontier = frontier_snapshot.unit_count;
            let pool_credits = self.pool.available_slots();
            let queue_credits = self.frames.free_capacity();
            let credits = pool_credits.min(queue_credits);
            let admitted = frontier.min(credits);
            let mut staged_bytes = 0_usize;
            let mut remaining_descriptors = frontier_snapshot.descriptor_count;

            for _ in 0..admitted {
                let unit_frontier = self
                    .storage
                    .first_completed_unit_frontier_through(&self.ring, last_descriptor_low)
                    .map_err(RxStageTransactionError::Ring)?;
                if unit_frontier.unit_count != 1
                    || unit_frontier.descriptor_count == 0
                    || unit_frontier.descriptor_count > remaining_descriptors
                {
                    return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                }
                let unit_descriptor_count = unit_frontier.descriptor_count;
                await_completed_unit_link_release(
                    &mut self.ring,
                    hardware,
                    &mut self.delay,
                    unit_descriptor_count,
                )
                .await
                .map_err(RxStageTransactionError::Ring)?;
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
                let maximum_payload_length = self
                    .admission
                    .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                    .min(STAGE_CAPACITY);
                let pending = match self.pool.stage_dma_unit_recycle_bounded(
                    unit,
                    hardware,
                    maximum_payload_length,
                )? {
                    RxDmaStageUnitOutcome::Staged(pending) => pending,
                    RxDmaStageUnitOutcome::Discarded(error) => {
                        // Length is supplied by an untrusted receive unit. A
                        // malformed/FCS/oversize unit must not terminate the
                        // sole radio owner: the vendor path discards such a
                        // frame and immediately returns its descriptor to the
                        // DMA walker. Preserve that ownership order and the
                        // asynchronous reload edge without publishing a
                        // staging token.
                        if let Some(observer) = self.pipeline_observer {
                            let discard = match error {
                                RxStageError::Empty => RxStageDiscard::Empty,
                                RxStageError::TooLong => RxStageDiscard::TooLong,
                                _ => unreachable!("match arm admits only length discards"),
                            };
                            observer.observe(RxPipelineObservation::StageDiscarded(discard));
                        }
                        loop {
                            match self
                                .ring
                                .poll_pending_reload(hardware)
                                .map_err(RxStageTransactionError::Ring)?
                            {
                                RxReloadObservation::Pending => self.delay.after_micros(1).await,
                                RxReloadObservation::Settled => break,
                            }
                        }
                        self.admission
                            .observe(Esp32s31RxIngressObservation::DiscardReloaded {
                                unit: unit_observation,
                                reason: match error {
                                    RxStageError::Empty => RxStageDiscard::Empty,
                                    RxStageError::TooLong => RxStageDiscard::TooLong,
                                    _ => unreachable!("length discard was matched above"),
                                },
                            });
                        continue;
                    }
                };
                let frame =
                    await_staged_rx_reload(pending, hardware, &mut self.ring, &mut self.delay)
                        .await?;
                staged_bytes = staged_bytes.saturating_add(frame.length());
                self.frames.try_send(frame).map_err(|error| match error {
                    TrySendError::Full(_) => RxStageTransactionError::Ring(RxRingError::Corrupt),
                })?;
                self.admission
                    .observe(Esp32s31RxIngressObservation::Staged(unit_observation));
            }

            if let (Some(observer), Some(started)) = (self.pipeline_observer, service_started) {
                observer.observe(RxPipelineObservation::ServiceCompleted(
                    RxServiceObservation {
                        frontier,
                        pool_credits,
                        queue_credits,
                        admitted,
                        staged_bytes,
                        micros: observer.elapsed_micros_since(started),
                        hardware_buffer_full,
                    },
                ));
            }

            Ok(if admitted < frontier {
                WifiRxProgress::Backpressured
            } else if self.ring.exhausted_republication_probe_pending() {
                WifiRxProgress::ProbePending
            } else {
                WifiRxProgress::Drained
            })
        }
    }
}
