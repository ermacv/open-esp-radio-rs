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
    pub(super) async fn accept_frame(
        &mut self,
        frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        let Some(key) = self.dispatcher.reorder_key(frame.segment()) else {
            return Some(self.dispatch_owned_frame(frame).await);
        };
        let bank = self.runtime.reorder_banks.find(key.peer, key.tid);
        let active = bank.is_some();
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderIngress {
                active,
                retry: key.retry,
            });
        }
        if !active {
            return Some(self.dispatch_owned_frame(frame).await);
        }
        let bank = bank.expect("active reorder has one hardware-bank identity");
        if let Some(start) = self.runtime.reorder_first_starts[bank].take()
            && let Some(observer) = self.pipeline_observer
        {
            observer.observe(RxPipelineObservation::ReorderFirst {
                tid: key.tid,
                start,
                sequence: key.sequence,
            });
        }

        let retain = match self
            .runtime
            .reorder_banks
            .state(bank)
            .expect("active TID was checked above")
            .retains_on_ingest(key.sequence)
        {
            Ok(retain) => retain,
            Err(error) => {
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        // A 64-slot hot pool can retain the maximum 31 out-of-order frames,
        // admit the next 32-descriptor hardware burst and still own the
        // current frontier frame. Smaller compositions keep the independent
        // cold backing so one sequence gap cannot exhaust DMA staging.
        let retain_hot = retain && SLOTS == RX_REORDER_BACKING_SLOT_COUNT;
        let reservation = if retain && !retain_hot {
            let Some(storage) = self.reorder_storage else {
                // An agreement must never retain the finite hot staging pool
                // when its independent backing was omitted by the composition.
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(ConnectedRxDispatch::Ignored);
            };
            match storage.try_reserve() {
                Ok(reservation) => Some(reservation),
                Err(_) => {
                    if let Some(observer) = self.pipeline_observer {
                        observer.observe(RxPipelineObservation::ReorderDiscarded);
                    }
                    drop(frame);
                    self.irq.notify_rx_capacity();
                    return Some(ConnectedRxDispatch::Ignored);
                }
            }
        } else {
            None
        };
        let slot = reservation.as_ref().map_or_else(
            || {
                if retain_hot {
                    frame.slot()
                } else {
                    RX_REORDER_CURRENT_SLOT
                }
            },
            |reservation| reservation.slot(),
        );
        let mpdu = RxAmpduMpdu {
            sequence: key.sequence,
            slot: slot as u8,
        };
        let release = match self
            .runtime
            .reorder_banks
            .state_mut(bank)
            .expect("active TID was checked above")
            .ingest(mpdu)
        {
            Ok(release) => release,
            Err(error) => {
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderDiscarded);
                }
                drop(reservation);
                drop(frame);
                self.irq.notify_rx_capacity();
                return Some(if matches!(error, RxAmpduError::DuplicateSequence(_)) {
                    ConnectedRxDispatch::Duplicate
                } else {
                    ConnectedRxDispatch::Ignored
                });
            }
        };
        self.update_gap_deadline(bank);
        self.record_reorder_occupied();
        if release.buffered {
            if retain_hot {
                debug_assert!(slot < self.runtime.retained.len());
                debug_assert!(self.runtime.retained[slot].is_none());
                self.runtime.retained[slot] = Some(RetainedRxFrame::Hot(frame));
                return self.dispatch_release(release).await;
            }
            let reservation = reservation.expect("predicted retained frame owns backing");
            let retained = match reservation.copy_from(frame.segment()) {
                Ok(retained) => retained,
                Err((_error, reservation)) => {
                    if let Some(observer) = self.pipeline_observer {
                        observer.observe(RxPipelineObservation::ReorderDiscarded);
                    }
                    let rollback = self
                        .runtime
                        .reorder_banks
                        .stop_bank(bank)
                        .expect("active reorder owns the failed retained copy");
                    self.runtime.gap_deadlines[bank] = None;
                    self.runtime.reorder_first_starts[bank] = None;
                    drop(reservation);
                    return self
                        .dispatch_release_with_current(rollback, slot, frame)
                        .await
                        .or(Some(ConnectedRxDispatch::Ignored));
                }
            };
            debug_assert_eq!(retained.slot(), slot);
            debug_assert!(self.runtime.retained[slot].is_none());
            self.runtime.retained[slot] = Some(RetainedRxFrame::Cold(retained));
            drop(frame);
            self.irq.notify_rx_capacity();
            self.dispatch_release(release).await
        } else {
            drop(reservation);
            self.dispatch_release_with_current(release, slot, frame)
                .await
        }
    }

    pub(super) async fn apply_reorder_command(
        &mut self,
        command: RxReorderCommand,
    ) -> Option<ConnectedRxDispatch> {
        match command {
            RxReorderCommand::Start(agreement) => {
                let bank = usize::from(agreement.hardware_index);
                let released = self.runtime.reorder_banks.start(agreement).ok().flatten();
                if self.runtime.reorder_banks.identity(bank) != Some(agreement.identity()) {
                    return None;
                }
                self.runtime.reorder_first_starts[bank] = Some(agreement.starting_sequence);
                self.runtime.gap_deadlines[bank] = None;
                if let Some(observer) = self.pipeline_observer {
                    observer.observe(RxPipelineObservation::ReorderStarted {
                        tid: agreement.tid,
                        starting_sequence: agreement.starting_sequence,
                        window: agreement.window,
                    });
                }
                match released {
                    Some(release) => self.dispatch_release(release).await,
                    None => None,
                }
            }
            RxReorderCommand::Stop(identity) => {
                let bank = usize::from(identity.hardware_index);
                if bank >= RX_BLOCK_ACK_BANK_COUNT
                    || self.runtime.reorder_banks.identity(bank) != Some(identity)
                {
                    None
                } else {
                    self.stop_reorder(bank).await
                }
            }
            RxReorderCommand::StopAll => {
                let mut result = None;
                for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
                    if let Some(released) = self.stop_reorder(bank).await {
                        result = Some(released);
                    }
                }
                result
            }
        }
    }

    async fn stop_reorder(&mut self, bank: usize) -> Option<ConnectedRxDispatch> {
        self.runtime.gap_deadlines[bank] = None;
        self.runtime.reorder_first_starts[bank] = None;
        let release = self.runtime.reorder_banks.stop_bank(bank)?;
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderStopped);
        }
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    pub(super) fn next_gap_deadline(&self) -> Option<(usize, Instant)> {
        self.runtime
            .gap_deadlines
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(tid, deadline)| deadline.map(|deadline| (tid, deadline)))
            .min_by_key(|(_, deadline)| *deadline)
    }

    fn update_gap_deadline(&mut self, tid: usize) {
        if self
            .runtime
            .reorder_banks
            .state(tid)
            .is_some_and(|reorder| reorder.occupied() != 0)
        {
            self.runtime.gap_deadlines[tid].get_or_insert_with(|| {
                Instant::now() + Duration::from_micros(RX_REORDER_GAP_TIMEOUT_MICROS)
            });
        } else {
            self.runtime.gap_deadlines[tid] = None;
        }
    }

    pub(super) async fn expire_reorder_gap(&mut self, tid: usize) -> Option<ConnectedRxDispatch> {
        self.runtime.gap_deadlines[tid] = None;
        let release = self.runtime.reorder_banks.state_mut(tid)?.expire_gap();
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderGapExpired);
        }
        self.update_gap_deadline(tid);
        self.record_reorder_occupied();
        self.dispatch_release(release).await
    }

    fn record_reorder_occupied(&self) {
        let Some(observer) = self.pipeline_observer else {
            return;
        };
        let occupied = self.runtime.reorder_banks.occupied();
        observer.observe(RxPipelineObservation::ReorderOccupied { occupied });
    }

    async fn dispatch_release(&mut self, release: RxAmpduRelease) -> Option<ConnectedRxDispatch> {
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderReleased {
                buffered: release.buffered,
                released: release.count,
                missing: release.missing,
                stale: release.rejected.is_some(),
            });
        }
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            let frame = self.runtime.retained[slot]
                .take()
                .expect("reorder release must reference one retained frame lease");
            result = Some(self.dispatch_retained_frame(frame).await);
        }
        if let Some(rejected) = release.rejected {
            self.release_retained_slot(usize::from(rejected.slot));
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        result
    }

    async fn dispatch_release_with_current(
        &mut self,
        release: RxAmpduRelease,
        current_slot: usize,
        current_frame: Esp32s31StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> Option<ConnectedRxDispatch> {
        if let Some(observer) = self.pipeline_observer {
            observer.observe(RxPipelineObservation::ReorderReleased {
                buffered: release.buffered,
                released: release.count,
                missing: release.missing,
                stale: release.rejected.is_some(),
            });
        }
        let mut current_frame = Some(current_frame);
        let mut result = None;
        for released in release.iter() {
            let slot = usize::from(released.slot);
            result = Some(if slot == current_slot {
                self.dispatch_owned_frame(
                    current_frame
                        .take()
                        .expect("current reorder release is unique"),
                )
                .await
            } else {
                let frame = self.runtime.retained[slot]
                    .take()
                    .expect("reorder release references retained cold backing");
                self.dispatch_retained_frame(frame).await
            });
        }
        if let Some(rejected) = release.rejected {
            let slot = usize::from(rejected.slot);
            if slot == current_slot {
                drop(current_frame.take());
                self.irq.notify_rx_capacity();
            } else {
                self.release_retained_slot(slot);
            }
            result = Some(ConnectedRxDispatch::Duplicate);
        }
        debug_assert!(current_frame.is_none());
        result
    }

    fn release_retained_slot(&mut self, slot: usize) {
        if let Some(frame) = self.runtime.retained[slot].take() {
            let hot = matches!(&frame, RetainedRxFrame::Hot(_));
            drop(frame);
            if hot {
                self.irq.notify_rx_capacity();
            }
        }
    }
}
