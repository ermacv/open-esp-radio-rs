//! AP binding for the role-neutral receive BlockAck reorder machines.
//!
//! The AP control and RX paths execute in one WDEV owner, so no mailbox is
//! needed. Out-of-order MPDUs still move into the same independent cold frame
//! storage used by connected STA; in-order frames remain zero-copy views of
//! the current DMA descriptor.

use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    rx::RxSegment,
    rx_ampdu::{
        RX_BLOCK_ACK_BANK_COUNT, RxAmpduError, RxAmpduMpdu, RxAmpduRelease, RxBlockAckIdentity,
        RxBlockAckMpduKey, RxBlockAckReorderBanks, RxBlockAckSnapshot,
    },
};

use crate::rx_reorder::{
    RX_REORDER_BACKING_SLOT_COUNT, RX_REORDER_CURRENT_SLOT, RX_REORDER_GAP_TIMEOUT_MICROS,
    RX_REORDER_SLOT_DOMAIN, RxReorderFrame, RxReorderFrameStorage, RxReorderStorageError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31AccessPointRxReorderError {
    Sequence(RxAmpduError),
    Storage(RxReorderStorageError),
}

impl From<RxAmpduError> for Esp32s31AccessPointRxReorderError {
    fn from(error: RxAmpduError) -> Self {
        Self::Sequence(error)
    }
}

impl From<RxReorderStorageError> for Esp32s31AccessPointRxReorderError {
    fn from(error: RxReorderStorageError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Esp32s31AccessPointRxReorderProgress {
    pub active: bool,
    pub buffered: bool,
    pub duplicate: bool,
    pub dropped: bool,
    pub dispatched: u8,
    pub hardware_window_reset: Option<Esp32s31AccessPointRxWindowReset>,
}

/// One vendor-equivalent synchronization edge for a stale first A-MPDU on a
/// newly activated extra-SoftAP receive BlockAck bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Esp32s31AccessPointRxWindowReset {
    pub hardware_index: u8,
    pub starting_sequence: u16,
}

struct PendingReleasedFrame<'storage, const CAPACITY: usize> {
    frame: RxReorderFrame<'storage, CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>,
    identity: RxBlockAckIdentity,
}

/// Static AP reorder arena. The frame bytes belong to the separate cold
/// storage; this value owns only their affine leases and sequence state.
pub struct Esp32s31AccessPointRxReorder<'storage, const CAPACITY: usize> {
    banks: RxBlockAckReorderBanks<RX_REORDER_SLOT_DOMAIN>,
    pending_hardware_window_reset: [bool; RX_BLOCK_ACK_BANK_COUNT],
    deadlines: [Option<u64>; RX_BLOCK_ACK_BANK_COUNT],
    retained: [Option<RxReorderFrame<'storage, CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>>;
        RX_REORDER_BACKING_SLOT_COUNT],
    pending_released:
        [Option<PendingReleasedFrame<'storage, CAPACITY>>; RX_REORDER_BACKING_SLOT_COUNT],
    pending_released_head: usize,
    pending_released_count: usize,
}

impl<'storage, const CAPACITY: usize> Esp32s31AccessPointRxReorder<'storage, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            banks: RxBlockAckReorderBanks::new(),
            pending_hardware_window_reset: [false; RX_BLOCK_ACK_BANK_COUNT],
            deadlines: [None; RX_BLOCK_ACK_BANK_COUNT],
            retained: [const { None }; RX_REORDER_BACKING_SLOT_COUNT],
            pending_released: [const { None }; RX_REORDER_BACKING_SLOT_COUNT],
            pending_released_head: 0,
            pending_released_count: 0,
        }
    }

    pub(super) fn start(
        &mut self,
        agreement: RxBlockAckSnapshot,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> Result<(), Esp32s31AccessPointRxReorderError> {
        let bank = usize::from(agreement.hardware_index);
        let replaced = self.banks.identity(bank);
        let released = self.banks.start(agreement)?;
        // SOURCE: complete `ht_recv_action_ba_addba_request` sets agreement
        // flag 0x40 after publishing the hardware entry. Complete
        // `ieee80211_ampdu_reorder` consumes that flag when it observes the
        // first physical A-MPDU. It reloads the hardware entry only when that
        // MPDU is behind the current software sequence frontier.
        self.pending_hardware_window_reset[bank] = true;
        self.deadlines[bank] = None;
        if let Some(release) = released {
            self.dispatch_retained_release(
                release,
                replaced.expect("a replaced reorder bank owns one identity"),
                &mut dispatch,
            );
        }
        Ok(())
    }

    pub(super) fn stop(
        &mut self,
        identity: RxBlockAckIdentity,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> bool {
        let bank = usize::from(identity.hardware_index);
        let Some(release) = self.banks.stop(identity) else {
            return false;
        };
        self.pending_hardware_window_reset[bank] = false;
        self.deadlines[bank] = None;
        self.dispatch_retained_release(release, identity, &mut dispatch);
        true
    }

    pub(super) fn stop_discard(&mut self, identity: RxBlockAckIdentity) -> u8 {
        let mut discarded = self.discard_pending_releases(identity);
        let bank = usize::from(identity.hardware_index);
        let Some(release) = self.banks.stop(identity) else {
            return discarded;
        };
        self.pending_hardware_window_reset[bank] = false;
        self.deadlines[bank] = None;
        for released in release.iter() {
            if self.retained[usize::from(released.slot)].take().is_some() {
                discarded = discarded.saturating_add(1);
            }
        }
        discarded
    }

    pub(super) fn ingest(
        &mut self,
        storage: &'storage RxReorderFrameStorage<CAPACITY, RX_REORDER_BACKING_SLOT_COUNT>,
        segment: RxSegment<'_>,
        key: RxBlockAckMpduKey,
        ampdu_baseband_format: Option<u8>,
        now_micros: u64,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> Result<Esp32s31AccessPointRxReorderProgress, Esp32s31AccessPointRxReorderError> {
        let Some(bank) = self
            .banks
            .find(MacInterface::AccessPoint, key.peer, key.tid)
        else {
            dispatch(segment);
            return Ok(Esp32s31AccessPointRxReorderProgress {
                dispatched: 1,
                ..Default::default()
            });
        };
        let identity = RxBlockAckIdentity {
            hardware_index: bank as u8,
            interface: MacInterface::AccessPoint,
            peer: key.peer,
            tid: key.tid,
        };
        let mut hardware_window_reset = None;
        let mut resync_dispatched = 0_u8;
        if let Some(baseband_format) = ampdu_baseband_format
            && self.pending_hardware_window_reset[bank]
        {
            self.pending_hardware_window_reset[bank] = false;
            let resync = self
                .banks
                .state_mut(bank)
                .expect("one bank identity owns one reorder state")
                .resynchronize_stale_initial_ampdu(key.sequence, baseband_format > 3)?;
            if let Some((release, starting_sequence)) = resync {
                resync_dispatched =
                    self.dispatch_retained_release(release, identity, &mut dispatch);
                self.deadlines[bank] = None;
                hardware_window_reset = Some(Esp32s31AccessPointRxWindowReset {
                    hardware_index: bank as u8,
                    starting_sequence,
                });
            }
        }
        let retain = self
            .banks
            .state(bank)
            .expect("one bank identity owns one reorder state")
            .retains_on_ingest(key.sequence)?;
        let retained = if retain {
            let reservation = match storage.try_reserve() {
                Ok(reservation) => reservation,
                Err(RxReorderStorageError::Exhausted) => {
                    return Ok(Esp32s31AccessPointRxReorderProgress {
                        active: true,
                        dropped: true,
                        ..Default::default()
                    });
                }
                Err(error) => return Err(error.into()),
            };
            match reservation.copy_from(segment) {
                Ok(frame) => Some(frame),
                Err((RxReorderStorageError::TooLong(_), _reservation)) => {
                    return Ok(Esp32s31AccessPointRxReorderProgress {
                        active: true,
                        dropped: true,
                        ..Default::default()
                    });
                }
                Err((error, _reservation)) => return Err(error.into()),
            }
        } else {
            None
        };
        let slot = retained
            .as_ref()
            .map_or(RX_REORDER_CURRENT_SLOT, RxReorderFrame::slot);
        let release = self
            .banks
            .state_mut(bank)
            .expect("one bank identity owns one reorder state")
            .ingest(RxAmpduMpdu {
                sequence: key.sequence,
                slot: slot as u8,
            })?;
        self.update_deadline(bank, now_micros);

        let mut progress = Esp32s31AccessPointRxReorderProgress {
            active: true,
            buffered: release.buffered,
            duplicate: release.rejected.is_some(),
            dropped: false,
            dispatched: resync_dispatched,
            hardware_window_reset,
        };
        if let Some(retained) = retained {
            debug_assert!(self.retained[slot].is_none());
            self.retained[slot] = Some(retained);
            // A frame predicted to require backing can still be released by
            // this same ingest when a window advance closes a complete run.
            // The release token names the retained slot, so keep that exact
            // owner in the slot domain until dispatch consumes it instead of
            // deriving ownership from the final `buffered` state.
            progress.dispatched = progress
                .dispatched
                .saturating_add(self.dispatch_retained_release(release, identity, &mut dispatch));
        } else {
            progress.dispatched =
                progress
                    .dispatched
                    .saturating_add(self.dispatch_release_with_current(
                        release,
                        identity,
                        RX_REORDER_CURRENT_SLOT,
                        Some(segment),
                        &mut dispatch,
                    ));
        }
        Ok(progress)
    }

    /// Release at most one due gap per finite WDEV turn.
    pub(super) fn expire_due(
        &mut self,
        now_micros: u64,
        mut dispatch: impl FnMut(RxSegment<'_>),
    ) -> u8 {
        let Some(bank) = self
            .deadlines
            .iter()
            .position(|deadline| deadline.is_some_and(|deadline| deadline <= now_micros))
        else {
            return 0;
        };
        self.deadlines[bank] = None;
        let identity = self
            .banks
            .identity(bank)
            .expect("a live gap deadline owns one identity");
        let release = self
            .banks
            .state_mut(bank)
            .expect("a live gap deadline owns one reorder state")
            .expire_gap();
        self.update_deadline(bank, now_micros);
        self.dispatch_retained_release(release, identity, &mut dispatch)
    }

    /// Publish at most one frame already released from sequence ownership.
    pub(super) fn dispatch_pending(&mut self, mut dispatch: impl FnMut(RxSegment<'_>)) -> bool {
        let Some(pending) = self.pop_pending_release() else {
            return false;
        };
        let frame = pending.frame;
        let view = frame.segment();
        dispatch(view.as_segment());
        drop(view);
        drop(frame);
        true
    }

    pub(super) const fn has_pending_release(&self) -> bool {
        self.pending_released_count != 0
    }

    pub(super) fn next_deadline(&self) -> Option<u64> {
        self.deadlines.iter().copied().flatten().min()
    }

    /// Whether a hardware-rejected public MPDU identity is already outside
    /// the software agreement frontier. This does not interpret the private
    /// hardware `rx_state`; it proves independently that publishing this
    /// sequence would be a duplicate or stale delivery.
    pub(super) fn is_duplicate_or_stale(&self, key: RxBlockAckMpduKey) -> bool {
        let Some(bank) = self
            .banks
            .find(MacInterface::AccessPoint, key.peer, key.tid)
        else {
            return false;
        };
        let state = self
            .banks
            .state(bank)
            .expect("active AP reorder identity owns one state");
        if key.sequence == state.next_sequence() {
            return false;
        }
        matches!(
            state.retains_on_ingest(key.sequence),
            Ok(false) | Err(RxAmpduError::DuplicateSequence(_))
        )
    }

    pub(super) fn discard_all(&mut self) -> u8 {
        let mut discarded = 0_u8;
        for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
            self.deadlines[bank] = None;
            self.pending_hardware_window_reset[bank] = false;
            let _ = self.banks.stop_bank(bank);
        }
        for retained in &mut self.retained {
            if retained.take().is_some() {
                discarded = discarded.saturating_add(1);
            }
        }
        while let Some(pending) = self.pop_pending_release() {
            drop(pending);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    fn update_deadline(&mut self, bank: usize, now_micros: u64) {
        if self
            .banks
            .state(bank)
            .is_some_and(|state| state.occupied() != 0)
        {
            self.deadlines[bank]
                .get_or_insert(now_micros.saturating_add(RX_REORDER_GAP_TIMEOUT_MICROS));
        } else {
            self.deadlines[bank] = None;
        }
    }

    #[inline(always)]
    fn dispatch_retained_release(
        &mut self,
        release: RxAmpduRelease,
        identity: RxBlockAckIdentity,
        dispatch: &mut impl FnMut(RxSegment<'_>),
    ) -> u8 {
        self.dispatch_release_with_current(release, identity, usize::MAX, None, dispatch)
    }

    #[inline(always)]
    fn dispatch_release_with_current(
        &mut self,
        release: RxAmpduRelease,
        identity: RxBlockAckIdentity,
        current_slot: usize,
        current: Option<RxSegment<'_>>,
        dispatch: &mut impl FnMut(RxSegment<'_>),
    ) -> u8 {
        let mut current = current;
        let releases_current = release
            .iter()
            .any(|released| usize::from(released.slot) == current_slot);
        let mut dispatched = false;
        for (position, released) in release.iter().enumerate() {
            let slot = usize::from(released.slot);
            if slot == current_slot {
                debug_assert_eq!(position, 0, "current gap closer must release first");
                dispatch(
                    current
                        .take()
                        .expect("current AP reorder release is unique"),
                );
                dispatched = true;
            } else if !releases_current && !dispatched {
                let frame = self.retained[slot]
                    .take()
                    .unwrap_or_else(|| {
                        panic!(
                            "AP direct release lost backing slot={slot} current={current_slot} pending={} release={release:?}",
                            self.pending_released_count,
                        )
                    });
                let view = frame.segment();
                dispatch(view.as_segment());
                drop(view);
                drop(frame);
                dispatched = true;
            } else {
                let frame = self.retained[slot]
                    .take()
                    .unwrap_or_else(|| {
                        panic!(
                            "AP queued release lost backing slot={slot} current={current_slot} pending={} release={release:?}",
                            self.pending_released_count,
                        )
                    });
                self.push_pending_release(PendingReleasedFrame { frame, identity });
            }
        }
        if release.rejected.is_some() {
            let _ = current.take();
        }
        debug_assert!(current.is_none());
        u8::from(dispatched)
    }

    fn push_pending_release(&mut self, pending: PendingReleasedFrame<'storage, CAPACITY>) {
        assert!(
            self.pending_released_count < self.pending_released.len(),
            "AP released-frame queue cannot exceed retained backing"
        );
        let tail = (self.pending_released_head + self.pending_released_count)
            % self.pending_released.len();
        debug_assert!(self.pending_released[tail].is_none());
        self.pending_released[tail] = Some(pending);
        self.pending_released_count += 1;
    }

    fn pop_pending_release(&mut self) -> Option<PendingReleasedFrame<'storage, CAPACITY>> {
        if self.pending_released_count == 0 {
            return None;
        }
        let pending = self.pending_released[self.pending_released_head]
            .take()
            .expect("non-empty AP release queue owns its head");
        self.pending_released_head = (self.pending_released_head + 1) % self.pending_released.len();
        self.pending_released_count -= 1;
        Some(pending)
    }

    fn discard_pending_releases(&mut self, identity: RxBlockAckIdentity) -> u8 {
        let count = self.pending_released_count;
        let mut discarded = 0_u8;
        for _ in 0..count {
            let pending = self
                .pop_pending_release()
                .expect("snapshotted AP release count remains exact");
            if pending.identity == identity {
                drop(pending);
                discarded = discarded.saturating_add(1);
            } else {
                self.push_pending_release(pending);
            }
        }
        discarded
    }
}

impl<const CAPACITY: usize> Default for Esp32s31AccessPointRxReorder<'_, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER_A: [u8; 6] = [2, 0, 0, 0, 0, 1];
    const PEER_B: [u8; 6] = [2, 0, 0, 0, 0, 2];

    fn agreement(hardware_index: u8, peer: [u8; 6], starting_sequence: u16) -> RxBlockAckSnapshot {
        RxBlockAckSnapshot {
            hardware_index,
            interface: MacInterface::AccessPoint,
            peer,
            tid: 6,
            window: 8,
            starting_sequence,
        }
    }

    fn segment(address: u32, bytes: &[u8]) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: address,
            descriptor_word0: 0,
            buffer: bytes,
            next_descriptor_address: 0,
        }
    }

    #[test]
    fn one_gap_releases_current_then_retained_frame_in_sequence_order() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
        let bytes_10 = [10];
        let bytes_11 = [11];
        let mut released = std::vec::Vec::new();

        let buffered = reorder
            .ingest(
                &storage,
                segment(11, &bytes_11),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 11,
                    retry: false,
                },
                None,
                1_000,
                |segment| released.push(segment.descriptor_address),
            )
            .unwrap();
        assert!(buffered.buffered);
        assert!(released.is_empty());
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT - 1);

        let current = reorder
            .ingest(
                &storage,
                segment(10, &bytes_10),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 10,
                    retry: false,
                },
                None,
                1_001,
                |segment| released.push(segment.descriptor_address),
            )
            .unwrap();
        assert_eq!(current.dispatched, 1);
        assert_eq!(released, [10]);
        assert!(reorder.has_pending_release());
        assert!(
            reorder.retained.iter().all(Option::is_none),
            "the pending queue must own the released backing, not a slot alias"
        );
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT - 1);
        assert!(reorder.dispatch_pending(|segment| released.push(segment.descriptor_address)));
        assert_eq!(released, [10, 11]);
        assert!(!reorder.has_pending_release());
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
        assert_eq!(reorder.next_deadline(), None);
    }

    #[test]
    fn out_of_window_mpdu_advances_only_the_software_reorder_frontier() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
        let bytes = [20];

        let progress = reorder
            .ingest(
                &storage,
                segment(20, &bytes),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 20,
                    retry: false,
                },
                None,
                1_000,
                |_| panic!("far successor remains buffered"),
            )
            .unwrap();

        assert!(progress.active);
        assert!(progress.buffered);
        assert_eq!(progress.dispatched, 0);
    }

    #[test]
    fn window_advance_that_closes_a_full_run_retains_release_ownership() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(0, PEER_A, 0), |_| {}).unwrap();
        let bytes = [0];
        let mut released = std::vec::Vec::new();

        for sequence in 1..8 {
            let progress = reorder
                .ingest(
                    &storage,
                    segment(sequence, &bytes),
                    RxBlockAckMpduKey {
                        peer: PEER_A,
                        tid: 6,
                        sequence: sequence as u16,
                        retry: false,
                    },
                    None,
                    sequence as u64,
                    |_| panic!("the leading gap retains the partial run"),
                )
                .unwrap();
            assert!(progress.buffered);
        }

        let progress = reorder
            .ingest(
                &storage,
                segment(8, &bytes),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 8,
                    retry: false,
                },
                None,
                8,
                |segment| released.push(segment.descriptor_address),
            )
            .unwrap();
        assert!(!progress.buffered);
        assert_eq!(progress.dispatched, 1);
        assert_eq!(released, [1]);

        while reorder.dispatch_pending(|segment| released.push(segment.descriptor_address)) {}
        assert_eq!(released, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    }

    #[test]
    fn aligned_first_physical_ampdu_does_not_reset_the_hardware_window() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
        let bytes = [0];
        let key = |sequence| RxBlockAckMpduKey {
            peer: PEER_A,
            tid: 6,
            sequence,
            retry: false,
        };

        let standalone = reorder
            .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
            .unwrap();
        assert_eq!(standalone.hardware_window_reset, None);

        let first_ampdu = reorder
            .ingest(&storage, segment(11, &bytes), key(11), Some(2), 2, |_| {})
            .unwrap();
        assert_eq!(first_ampdu.hardware_window_reset, None);

        let next_ampdu = reorder
            .ingest(&storage, segment(12, &bytes), key(12), Some(2), 3, |_| {})
            .unwrap();
        assert_eq!(next_ampdu.hardware_window_reset, None);
    }

    #[test]
    fn stale_first_ht_ampdu_rebases_to_the_negotiated_sequence() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(3, PEER_A, 10), |_| {}).unwrap();
        let bytes = [0];
        let key = |sequence| RxBlockAckMpduKey {
            peer: PEER_A,
            tid: 6,
            sequence,
            retry: false,
        };

        reorder
            .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
            .unwrap();
        let stale_first_ampdu = reorder
            .ingest(&storage, segment(10, &bytes), key(10), Some(2), 2, |_| {})
            .unwrap();
        assert_eq!(
            stale_first_ampdu.hardware_window_reset,
            Some(Esp32s31AccessPointRxWindowReset {
                hardware_index: 3,
                starting_sequence: 10,
            })
        );
        assert!(!stale_first_ampdu.duplicate);
        assert_eq!(stale_first_ampdu.dispatched, 1);
    }

    #[test]
    fn peer_banks_keep_equal_tid_sequence_spaces_independent() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(0, PEER_A, 20), |_| {}).unwrap();
        reorder.start(agreement(1, PEER_B, 40), |_| {}).unwrap();
        let bytes = [0];

        for (peer, sequence, address) in [(PEER_A, 21, 21), (PEER_B, 41, 41)] {
            reorder
                .ingest(
                    &storage,
                    segment(address, &bytes),
                    RxBlockAckMpduKey {
                        peer,
                        tid: 6,
                        sequence,
                        retry: false,
                    },
                    None,
                    5_000,
                    |_| panic!("gap successor must remain retained"),
                )
                .unwrap();
        }
        let mut released = std::vec::Vec::new();
        assert_eq!(
            reorder.expire_due(5_000 + RX_REORDER_GAP_TIMEOUT_MICROS - 1, |segment| {
                released.push(segment.descriptor_address)
            },),
            0
        );
        assert_eq!(
            reorder.expire_due(5_000 + RX_REORDER_GAP_TIMEOUT_MICROS, |segment| released
                .push(segment.descriptor_address),),
            1
        );
        assert_eq!(released, [21]);
        assert_eq!(reorder.next_deadline(), Some(305_000));
        assert_eq!(
            reorder.expire_due(305_000, |segment| released.push(segment.descriptor_address)),
            1
        );
        assert_eq!(released, [21, 41]);
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    }

    #[test]
    fn peer_teardown_discards_retained_frames_and_releases_backing() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        let agreement = agreement(3, PEER_A, 100);
        reorder.start(agreement, |_| {}).unwrap();
        let bytes = [101];
        reorder
            .ingest(
                &storage,
                segment(101, &bytes),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 101,
                    retry: false,
                },
                None,
                0,
                |_| panic!("gap successor must remain retained"),
            )
            .unwrap();
        let current = [100];
        reorder
            .ingest(
                &storage,
                segment(100, &current),
                RxBlockAckMpduKey {
                    peer: PEER_A,
                    tid: 6,
                    sequence: 100,
                    retry: false,
                },
                None,
                1,
                |segment| assert_eq!(segment.descriptor_address, 100),
            )
            .unwrap();
        assert!(reorder.has_pending_release());

        assert_eq!(reorder.stop_discard(agreement.identity()), 1);
        assert!(!reorder.has_pending_release());
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
        assert_eq!(reorder.next_deadline(), None);
    }

    #[test]
    fn hardware_rejection_is_safe_only_for_independently_stale_or_owned_sequences() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        reorder.start(agreement(0, PEER_A, 10), |_| {}).unwrap();
        let key = |sequence| RxBlockAckMpduKey {
            peer: PEER_A,
            tid: 6,
            sequence,
            retry: true,
        };
        assert!(!reorder.is_duplicate_or_stale(key(10)));
        assert!(!reorder.is_duplicate_or_stale(key(11)));

        let bytes = [11];
        reorder
            .ingest(&storage, segment(11, &bytes), key(11), None, 0, |_| {})
            .unwrap();
        assert!(reorder.is_duplicate_or_stale(key(11)));

        let bytes = [10];
        reorder
            .ingest(&storage, segment(10, &bytes), key(10), None, 1, |_| {})
            .unwrap();
        assert!(reorder.is_duplicate_or_stale(key(10)));
        assert!(!reorder.is_duplicate_or_stale(key(12)));
        assert!(!reorder.is_duplicate_or_stale(RxBlockAckMpduKey {
            peer: PEER_B,
            ..key(10)
        }));
    }

    #[test]
    fn full_shared_backing_drops_one_frame_without_advancing_sequence_state() {
        let storage = RxReorderFrameStorage::<32>::new();
        let mut reorder = Esp32s31AccessPointRxReorder::<32>::new();
        let bytes = [0];
        for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
            let peer = [2, 0, 0, 0, 1, bank as u8];
            reorder
                .start(
                    RxBlockAckSnapshot {
                        hardware_index: bank as u8,
                        interface: MacInterface::AccessPoint,
                        peer,
                        tid: 0,
                        window: 64,
                        starting_sequence: 0,
                    },
                    |_| {},
                )
                .unwrap();
            for sequence in 1..=8 {
                let progress = reorder
                    .ingest(
                        &storage,
                        segment((bank * 16 + sequence) as u32, &bytes),
                        RxBlockAckMpduKey {
                            peer,
                            tid: 0,
                            sequence: sequence as u16,
                            retry: false,
                        },
                        None,
                        0,
                        |_| panic!("a leading gap retains every successor"),
                    )
                    .unwrap();
                assert!(progress.buffered);
            }
        }
        assert_eq!(storage.available_slots(), 0);

        let progress = reorder
            .ingest(
                &storage,
                segment(999, &bytes),
                RxBlockAckMpduKey {
                    peer: [2, 0, 0, 0, 1, 0],
                    tid: 0,
                    sequence: 9,
                    retry: false,
                },
                None,
                1,
                |_| panic!("exhausted backing cannot publish out of order"),
            )
            .unwrap();
        assert!(progress.dropped);
        assert!(!progress.buffered);
        assert_eq!(reorder.discard_all(), RX_REORDER_BACKING_SLOT_COUNT as u8);
        assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
    }
}
