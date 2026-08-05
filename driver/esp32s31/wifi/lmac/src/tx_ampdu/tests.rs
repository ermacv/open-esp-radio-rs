//! Host/oracle tests for A-MPDU protocol, formatting and ownership.

use super::*;
use open_esp_radio_dma::{HardwareOwnedTxDma, PinnedDmaTxPool, PreparedTxDma};
use open_esp_radio_esp32s31_registers::{
    MacHeTxVectorSnapshot, MacLegacyTxProgram, MacTxCompletionRegisters, MacTxQueueDetached,
};

struct CompletionHardware {
    completion: Option<MacHtAmpduCompletionRegisters>,
    cleared: Option<MacHeTbLinkReservation>,
    trigger_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
}

impl TxHardware for CompletionHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _: &dyn PreparedTxDma,
        _: u8,
        _: MacLegacyTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_legacy_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8, _: u32) {}

    fn prepare_bound_ht_tx(&mut self, _: &dyn PreparedTxDma, _: u8, _: MacHtTxProgram) -> bool {
        true
    }

    fn start_bound_ht_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8, _: u32) {}

    fn he_tx_vector_snapshot(&self, _: u8) -> Option<MacHeTxVectorSnapshot> {
        None
    }

    fn take_tx_completion(&mut self, _: u8) -> Option<MacTxCompletionRegisters> {
        None
    }

    fn begin_tx_timeout_abort(&mut self, _: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: u8,
        _: u32,
        _: MacTxDetachReason,
        _: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        MacTxDetachOutcome::NoEvent
    }
}

impl HtAmpduHardware for CompletionHardware {
    fn take_ht_ampdu_completion(&mut self, _: u8) -> Option<MacHtAmpduCompletionRegisters> {
        self.completion.take()
    }

    fn prepare_he_trigger_based_queue(
        &mut self,
        _: MacHeTbTidLimit,
        _: MacHeTbLinkReservation,
        _: MacHeTid,
        _: &[u16],
        _: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        self.trigger_snapshot
            .ok_or(MacHeTbProgramError::LengthCountMismatch)
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        self.cleared = Some(reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        _: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        self.trigger_snapshot
    }
}

fn frame_layout(
    dma_offset: usize,
    mpdu_length: usize,
    hardware_mic_length: u8,
) -> AmpduFrameLayout {
    AmpduFrameLayout::new(
        dma_offset,
        AmpduFrameSize::new(mpdu_length, hardware_mic_length),
    )
    .unwrap()
}

fn ht_frame_request(
    dma_offset: usize,
    mpdu_length: usize,
    hardware_mic_length: u8,
    empty_delimiters: u8,
    rate: HtRate,
) -> HtAmpduFrameRequest {
    HtAmpduFrameRequest::new(
        frame_layout(dma_offset, mpdu_length, hardware_mic_length),
        empty_delimiters,
        rate,
    )
}

#[test]
fn ampdu_frame_layout_rejects_unaligned_dma_prefix() {
    let frame_size = AmpduFrameSize::new(32, 8);
    assert_eq!(AmpduFrameLayout::new(1, frame_size), None);
    assert_eq!(
        AmpduFrameLayout::new(4, frame_size).unwrap().dma_offset(),
        4
    );
}

#[test]
fn retained_dma_owner_cancels_reserved_storage_before_releasing_backing() {
    let storage = HtAmpduTxStorage::<2, 0>::new();
    let mut storage = core::pin::pin!(storage);
    let pool = PinnedDmaTxPool::<256, 0, 0, 1>::new();
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |bytes| {
        bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
    });
    let backing = pool.claim_radio(index);

    {
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut()).unwrap();
        let cookie = owner.begin().unwrap();
        owner
            .commit_ht(
                cookie,
                backing,
                ht_frame_request(
                    0,
                    32,
                    8,
                    0,
                    HtRate::new(
                        crate::tx::HtMcs::Mcs0,
                        crate::tx::HtGuardInterval::Long800Ns,
                        crate::tx::HtChannelWidth::Mhz20,
                    ),
                ),
            )
            .unwrap();
        assert_eq!(owner.held_backing_count(), 1);
    }

    assert_eq!(storage.state(), TxSlotState::Free);
    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn rejected_referenced_commit_rolls_back_the_lower_lease() {
    let storage = HtAmpduTxStorage::<2, 0>::new();
    let mut storage = core::pin::pin!(storage);
    let pool = PinnedDmaTxPool::<256, 0, 0, 1>::new();
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |_| {});
    let backing = pool.claim_radio(index);

    {
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut()).unwrap();
        let cookie = owner.begin().unwrap();
        assert_eq!(
            owner.commit_ht(
                cookie,
                backing,
                ht_frame_request(
                    256,
                    32,
                    8,
                    0,
                    HtRate::new(
                        crate::tx::HtMcs::Mcs0,
                        crate::tx::HtGuardInterval::Long800Ns,
                        crate::tx::HtChannelWidth::Mhz20,
                    ),
                ),
            ),
            Err(HtAmpduTxError::FrameTooLong)
        );
        assert_eq!(owner.held_backing_count(), 0);
        owner.cancel(cookie).unwrap();
    }

    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn retained_dma_owner_quarantines_hardware_owned_backing_on_drop() {
    let storage = HtAmpduTxStorage::<2, 0>::new();
    let mut storage = core::pin::pin!(storage);
    let pool = PinnedDmaTxPool::<256, 0, 0, 1>::new();
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |bytes| {
        bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
    });
    let backing = pool.claim_radio(index);

    {
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut()).unwrap();
        let cookie = owner.begin().unwrap();
        let rate = HtRate::new(
            crate::tx::HtMcs::Mcs0,
            crate::tx::HtGuardInterval::Long800Ns,
            crate::tx::HtChannelWidth::Mhz20,
        );
        owner
            .commit_ht(cookie, backing, ht_frame_request(0, 32, 8, 0, rate))
            .unwrap();
        let aggregate = owner.prepared_aggregate(cookie).unwrap();
        owner
            .submit(
                &mut CompletionHardware {
                    completion: None,
                    cleared: None,
                    trigger_snapshot: None,
                },
                cookie,
                LegacyTxQueue::BestEffort,
                HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap(),
            )
            .unwrap();
    }

    assert_eq!(storage.state(), TxSlotState::ResetRequired);
    assert_eq!(pool.claimed_slots(), 1);
}

const CONFIG: TxBlockAckConfig = TxBlockAckConfig {
    tid: 7,
    window: 32,
    timeout_tu: 0,
    negotiation_timeout_us: 100_000,
    amsdu: true,
};

#[test]
fn owned_dma_pool_builds_two_mpdu_length_without_publishing_hardware() {
    let storage = HtAmpduTxStorage::<4, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();

    storage.as_mut().next_frame_buffer(cookie).unwrap()[..100].fill(0xa5);
    storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..101].fill(0x5a);
    storage.as_mut().commit_frame(cookie, 101, 8, 0).unwrap();

    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            // First PSDU: delimiter 4 + length 112. Final PSDU:
            // delimiter 4 + length 113, with its three padding bytes
            // removed by the recovered tail rule.
            bytes: 233,
            subframes: 2,
        }
    );
    assert_eq!(storage.frame_count(), 2);
    assert_eq!(storage.state(), TxSlotState::Reserved);
    storage.as_mut().cancel(cookie).unwrap();
    assert_eq!(storage.state(), TxSlotState::Free);
}

#[test]
fn referenced_commit_uses_the_retained_allocation_without_copying_payload() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut external = [0xa5_u8; 256];
    external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100].fill(0x5a);
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    storage
        .as_mut()
        .commit_referenced_frame(cookie, &mut external, frame_layout(0, 100, 8), 0)
        .unwrap();

    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 116,
            subframes: 1,
        }
    );
    storage.as_mut().cancel(cookie).unwrap();
    assert_eq!(
        storage.buffer_addresses[0],
        external.as_ptr().addr(),
        "descriptor backing must remain the referenced allocation"
    );

    assert_eq!(u32::from_le_bytes(external[..4].try_into().unwrap()), 112);
    assert_eq!(
        &external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100],
        &[0x5a; 100]
    );
    // The ordinary internal backing was not used as a staging buffer.
    assert_eq!(storage.buffers[0].0[TX_AMPDU_METADATA_SIZE], 0);
}

#[test]
fn referenced_he_commit_uses_external_capacity_with_descriptor_only_storage() {
    let storage = HtAmpduTxStorage::<2, 0>::new();
    let mut first = [0xa5_u8; 256];
    let mut second = [0x5a_u8; 256];
    let rate = HeRate::bcc_dcm(
        crate::tx::HeBccDcmMcs::Mcs3,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    for external in [&mut first, &mut second] {
        storage
            .as_mut()
            .commit_referenced_he_frame(
                cookie,
                external,
                HeAmpduFrameRequest::new(
                    frame_layout(0, 16, 8),
                    HeAmpduPolicy::new(
                        rate,
                        HtAmpduDensity::SixteenMicroseconds,
                        HeEdcaTxopLimit::DEFAULT,
                    ),
                ),
            )
            .unwrap();
    }

    assert_eq!(storage.empty_delimiters[..2], [1, 1]);
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 68,
            subframes: 2,
        }
    );
    assert_eq!(first[4], 1);
    assert_eq!(second[4], 1);
    assert_eq!(storage.buffer_addresses[0], first.as_ptr().addr());
    assert_eq!(storage.buffer_addresses[1], second.as_ptr().addr());
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn referenced_ht_commit_stops_at_the_vendor_rate_byte_ceiling() {
    let storage = HtAmpduTxStorage::<8, 0>::new();
    let mut external = [[0xa5_u8; 1_600]; 8];
    let mcs0_sgi = HtRate::new(
        crate::tx::HtMcs::Mcs0,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    let mut storage = core::pin::pin!(storage);
    storage
        .as_mut()
        .configure_max_aggregate_bytes(u16::MAX)
        .unwrap();
    let cookie = storage.as_mut().begin().unwrap();

    for frame in external.iter_mut().take(6) {
        storage
            .as_mut()
            .commit_referenced_ht_frame(cookie, frame, ht_frame_request(0, 1_500, 8, 0, mcs0_sgi))
            .unwrap();
    }
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 9_096,
            subframes: 6,
        }
    );
    assert!(
        !storage
            .can_commit_referenced_ht_frame(cookie, 1_500, 8, 0, mcs0_sgi, external[6].len())
            .unwrap()
    );
    assert_eq!(
        storage.as_mut().commit_referenced_ht_frame(
            cookie,
            &mut external[6],
            ht_frame_request(0, 1_500, 8, 0, mcs0_sgi),
        ),
        Err(HtAmpduTxError::AggregateFull)
    );
    assert_eq!(storage.frame_count(), 6);
    storage.as_mut().cancel(cookie).unwrap();

    // The complete oracle table uses zero for SGI MCS7. That means this
    // particular leaf adds no ceiling: the peer/static limit still
    // permits a seventh MPDU.
    let mcs7_sgi = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    let cookie = storage.as_mut().begin().unwrap();
    for frame in external.iter_mut().take(7) {
        storage
            .as_mut()
            .commit_referenced_ht_frame(cookie, frame, ht_frame_request(0, 1_500, 8, 0, mcs7_sgi))
            .unwrap();
    }
    assert_eq!(storage.frame_count(), 7);
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn owned_dma_pool_preserves_one_subframe_he_ampdu_length() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..100].fill(0xa5);
    storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();

    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            // One delimiter plus MPDU, hardware MIC and FCS. The 112-byte
            // PSDU is already aligned, so the tail rule removes nothing.
            bytes: 116,
            subframes: 1,
        }
    );
}

#[test]
fn owned_he_commit_derives_empty_delimiters_from_rate_and_peer_density() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let rate = HeRate::bcc_dcm(
        crate::tx::HeBccDcmMcs::Mcs3,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );

    storage.as_mut().next_frame_buffer(cookie).unwrap()[..16].fill(0xa5);
    storage
        .as_mut()
        .commit_he_frame(cookie, 16, 8, rate, HtAmpduDensity::SixteenMicroseconds)
        .unwrap();
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..16].fill(0x5a);
    storage
        .as_mut()
        .commit_he_frame(cookie, 16, 8, rate, HtAmpduDensity::SixteenMicroseconds)
        .unwrap();

    // frame + MIC + FCS is 28 bytes. At DCM MCS3/GI800 and 16 us the
    // blob minimum is 35 bytes, so ppCalDeliNum requests one empty
    // delimiter. The trailing delimiter of the final MPDU is omitted
    // from aggregate length exactly as ppEmptyDelimiterLength requires.
    assert_eq!(storage.empty_delimiters[..2], [1, 1]);
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 68,
            subframes: 2,
        }
    );
    let first = &storage.buffers[0].0;
    assert_eq!(first[4], 1);
}

#[test]
fn hardware_he_control_uses_vendor_metadata_bit_without_dma_placeholder() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let rate = HeRate::new(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let frame = storage.as_mut().next_frame_buffer(cookie).unwrap();
    frame[..64].fill(0xa5);
    // Model the QoS/CCMP boundary: CCMP starts immediately at byte 26.
    frame[26..34].copy_from_slice(&[0x0f, 0, 0, 0x20, 0, 0, 0, 0]);
    storage
        .as_mut()
        .commit_hardware_he_control_frame(cookie, 64, 8, rate, HtAmpduDensity::NoRestriction)
        .unwrap();

    // Base MPDU length is frame + MIC + FCS = 76. Hardware HE-Control is
    // encoded only by metadata[7].bit0 and contributes four to APEP.
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 84,
            subframes: 1,
        }
    );
    let dma = &storage.buffers[0].0;
    assert_eq!(&dma[..4], &76_u32.to_le_bytes());
    assert_eq!(&dma[4..8], &0x0100_0000_u32.to_le_bytes());
    assert_eq!(
        &dma[TX_AMPDU_METADATA_SIZE + 26..TX_AMPDU_METADATA_SIZE + 34],
        &[0x0f, 0, 0, 0x20, 0, 0, 0, 0]
    );
}

#[test]
fn he_trigger_preparation_uses_original_msdu_lengths_and_exact_link_range() {
    let storage = HtAmpduTxStorage::<4, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let rate = HeRate::new(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    for (frame_length, msdu_length) in [(100, 80), (101, 81)] {
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..frame_length].fill(0xa5);
        storage
            .as_mut()
            .commit_he_msdu_frame(
                cookie,
                frame_length,
                8,
                msdu_length,
                rate,
                HtAmpduDensity::NoRestriction,
            )
            .unwrap();
    }
    let aggregate = storage.prepared_aggregate(cookie).unwrap();
    let trigger =
        crate::tx::HeTriggerBasedTxConfig::new(MacHeTbTidLimit::Three, MacHeTid::new(0).unwrap())
            .unwrap();
    let config = HeAmpduTxConfig::new(
        rate,
        0,
        aggregate.bytes,
        aggregate.subframes,
        HtAmpduDensity::NoRestriction,
    )
    .unwrap()
    .with_trigger_based(trigger);
    let prepared = storage
        .prepared_he_trigger(LegacyTxQueue::BestEffort, config)
        .unwrap()
        .unwrap();

    assert_eq!(prepared.policy, MacHeTbTidLimit::Three);
    assert_eq!(prepared.tid, MacHeTid::new(0).unwrap());
    assert_eq!(prepared.reservation.queue(), 2);
    assert_eq!(prepared.reservation.first(), 0);
    assert_eq!(prepared.reservation.count(), 2);
    assert_eq!(prepared.queued_msdu_bytes, 161);
    assert_eq!(storage.psdu_lengths[..2], [112, 113]);
}

#[test]
fn he_trigger_preparation_fails_closed_without_original_msdu_length() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let rate = HeRate::new(
        crate::tx::HeMcs::Mcs0,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..64].fill(0x5a);
    storage
        .as_mut()
        .commit_he_frame(cookie, 64, 8, rate, HtAmpduDensity::NoRestriction)
        .unwrap();
    let aggregate = storage.prepared_aggregate(cookie).unwrap();
    let trigger =
        crate::tx::HeTriggerBasedTxConfig::new(MacHeTbTidLimit::Three, MacHeTid::new(0).unwrap())
            .unwrap();
    let config = HeAmpduTxConfig::new(
        rate,
        0,
        aggregate.bytes,
        aggregate.subframes,
        HtAmpduDensity::NoRestriction,
    )
    .unwrap()
    .with_trigger_based(trigger);

    assert_eq!(
        storage.prepared_he_trigger(LegacyTxQueue::BestEffort, config),
        Err(HtAmpduTxError::TriggerMsduLengthUnavailable)
    );
}

#[test]
fn completion_exposes_publication_snapshot_then_clears_tb_enable() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let reservation = MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 2, 2).unwrap();
    let snapshot = MacHeTriggerTxQueueSnapshot {
        logical_queue: 2,
        tid: 0,
        trigger_based_enabled: true,
        mu_edca_timer_select: 2,
        mu_edca_timer_enabled: true,
        first_link: 0,
        first_mpdu_length: 112,
        first_next_link: 1,
        tail_link: 1,
        programmed_msdu_bytes: 161,
        queued_msdu_bytes: 161,
        queue_valid: true,
    };
    {
        let storage = storage.as_mut().project();
        *storage.state = TxSlotState::HardwareOwned;
        *storage.queue = LegacyTxQueue::BestEffort;
        *storage.trigger_reservation = Some(reservation);
        *storage.trigger_publication_snapshot = Some(snapshot);
    }
    let mut hardware = CompletionHardware {
        completion: Some(MacHtAmpduCompletionRegisters {
            tx: MacTxCompletionRegisters {
                aux_a: 0,
                aux_b: 0,
                aux_c: 0,
                primary: 0,
                alternate: 0,
                trigger_flow: false,
            },
            block_ack_control_and_sequence: 0,
            block_ack_bitmap_low: 0,
            block_ack_bitmap_high: 0,
        }),
        cleared: None,
        trigger_snapshot: Some(snapshot),
    };

    assert_eq!(
        storage
            .as_ref()
            .he_trigger_based_snapshot(&hardware, cookie),
        Ok(Some(snapshot))
    );
    assert!(
        storage
            .as_mut()
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    assert_eq!(hardware.cleared, Some(reservation));
    assert_eq!(
        storage
            .as_ref()
            .he_trigger_based_snapshot(&hardware, cookie),
        Err(HtAmpduTxError::Stale)
    );
}

#[test]
fn incremental_pool_length_matches_blob_accumulator_for_full_window() {
    let storage = HtAmpduTxStorage::<32, 256>::new();
    let mut storage = core::pin::pin!(storage);
    storage
        .as_mut()
        .configure_max_aggregate_bytes(u16::MAX)
        .unwrap();
    let cookie = storage.as_mut().begin().unwrap();
    let mut oracle = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();

    for index in 0..32_u8 {
        let frame_length = 100 + usize::from(index % 7);
        let empty_delimiters = index % 3;
        storage.as_mut().next_frame_buffer(cookie).unwrap()[..frame_length].fill(index);
        storage
            .as_mut()
            .commit_frame(cookie, frame_length, 8, empty_delimiters)
            .unwrap();
        oracle
            .push(
                (frame_length + 8 + usize::from(TX_FCS_SIZE)) as u32,
                empty_delimiters,
            )
            .unwrap();
        if index != 0 {
            assert_eq!(
                storage.prepared_aggregate(cookie).unwrap(),
                oracle.finish().unwrap()
            );
        }
    }
}

#[test]
fn completed_pool_retains_mpdu_until_explicit_release() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x5a);
    storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();

    // Model the two hardware ownership edges independently: completion
    // alone must not expose a buffer that the queue still references.
    *storage.as_mut().project().state = TxSlotState::Completed;
    assert_eq!(
        storage.completed_frame(cookie, 0),
        Err(HtAmpduTxError::Stale)
    );
    *storage.as_mut().project().detached = true;
    let (frame, mic_length) = storage.completed_frame(cookie, 0).unwrap();
    assert_eq!(frame, &[0x5a; 32]);
    assert_eq!(mic_length, 8);

    storage.as_mut().release_completed(cookie).unwrap();
    assert_eq!(storage.state(), TxSlotState::Free);
    assert_eq!(
        storage.completed_frame(cookie, 0),
        Err(HtAmpduTxError::Stale)
    );
}

#[test]
fn detached_pool_compacts_only_missing_frames_for_ampdu_retry() {
    let storage = HtAmpduTxStorage::<4, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    for index in 0..4_u8 {
        let frame = storage.as_mut().next_frame_buffer(cookie).unwrap();
        frame[..32].fill(index);
        frame[1] = 0x41;
        storage.as_mut().commit_frame(cookie, 32, 8, 0).unwrap();
    }
    {
        let storage = storage.as_mut().project();
        *storage.state = TxSlotState::Completed;
        *storage.detached = true;
    }

    let aggregate = storage
        .as_mut()
        .retain_for_ampdu_retry(cookie, 0b1010)
        .unwrap();
    assert_eq!(aggregate.subframes, 2);
    assert_eq!(storage.frame_count(), 2);
    assert_eq!(storage.state(), TxSlotState::Reserved);
    {
        let view = storage.as_ref().get_ref();
        assert_eq!(view.buffer_addresses[0], view.buffers[1].0.as_ptr().addr());
        assert_eq!(view.buffer_addresses[1], view.buffers[3].0.as_ptr().addr());
        assert_eq!(view.buffers[1].0[TX_AMPDU_METADATA_SIZE], 1);
        assert_eq!(view.buffers[3].0[TX_AMPDU_METADATA_SIZE], 3);
        assert_eq!(view.buffers[1].0[TX_AMPDU_METADATA_SIZE + 1], 0x49);
        assert_eq!(view.buffers[3].0[TX_AMPDU_METADATA_SIZE + 1], 0x49);
    }
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn detached_he_pool_retains_one_missing_mpdu_at_the_original_rate() {
    let storage = HtAmpduTxStorage::<2, 256>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let initial = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
    );
    storage.as_mut().next_frame_buffer(cookie).unwrap()[..32].fill(0x5a);
    storage
        .as_mut()
        .commit_he_frame(cookie, 32, 8, initial, HtAmpduDensity::EightMicroseconds)
        .unwrap();
    {
        let storage = storage.as_mut().project();
        *storage.state = TxSlotState::Completed;
        *storage.detached = true;
    }

    let retained = storage
        .as_mut()
        .retain_for_ampdu_retry(cookie, 0b1)
        .unwrap();
    assert_eq!(retained.subframes, 1);
    let buffer = &storage.as_ref().get_ref().buffers[0].0;
    assert_eq!(buffer[TX_AMPDU_METADATA_SIZE + 1] & 0x08, 0x08);

    assert_eq!(
        storage.prepared_empty_delimiters(cookie, 0).unwrap(),
        initial
            .ampdu_empty_delimiters(32 + 8 + TX_FCS_SIZE, HtAmpduDensity::EightMicroseconds)
            .unwrap()
    );
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn full_window_and_byte_ceiling_are_independent() {
    let storage = HtAmpduTxStorage::<32, 1700>::new();
    let mut storage = core::pin::pin!(storage);
    storage
        .as_mut()
        .configure_max_aggregate_bytes(0x7fff)
        .unwrap();
    let cookie = storage.as_mut().begin().unwrap();
    for _ in 0..20 {
        assert!(storage.can_commit_frame(cookie, 1600, 8, 0).unwrap());
        storage.as_mut().commit_frame(cookie, 1600, 8, 0).unwrap();
    }
    assert_eq!(storage.frame_count(), 20);
    assert!(!storage.can_commit_frame(cookie, 1600, 8, 0).unwrap());
    assert_eq!(
        storage.as_mut().commit_frame(cookie, 1600, 8, 0),
        Err(HtAmpduTxError::AggregateFull)
    );
    assert_eq!(storage.frame_count(), 20);
    storage.as_mut().cancel(cookie).unwrap();

    storage
        .as_mut()
        .configure_max_aggregate_bytes(0x1fff)
        .unwrap();
    let cookie = storage.as_mut().begin().unwrap();
    for _ in 0..32 {
        assert!(storage.can_commit_frame(cookie, 100, 8, 0).unwrap());
        storage.as_mut().commit_frame(cookie, 100, 8, 0).unwrap();
    }
    assert_eq!(storage.frame_count(), 32);
    assert!(!storage.can_commit_frame(cookie, 100, 8, 0).unwrap());
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn he_rate_duration_gate_prevents_an_oversized_dma_publication() {
    let storage = HtAmpduTxStorage::<32, 1_600>::new();
    let mut storage = core::pin::pin!(storage);
    storage
        .as_mut()
        .configure_max_aggregate_bytes(u16::MAX)
        .unwrap();
    let density = HtAmpduDensity::NoRestriction;
    let gi_1600 = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
    );
    let cookie = storage.as_mut().begin().unwrap();
    for _ in 0..31 {
        assert!(
            storage
                .can_commit_he_frame(cookie, 1_500, 8, gi_1600, density)
                .unwrap()
        );
        storage
            .as_mut()
            .commit_he_frame(cookie, 1_500, 8, gi_1600, density)
            .unwrap();
    }
    assert_eq!(storage.prepared_aggregate(cookie).unwrap().bytes, 46_996);
    assert!(
        !storage
            .can_commit_he_frame(cookie, 1_500, 8, gi_1600, density)
            .unwrap()
    );
    assert_eq!(
        storage
            .as_mut()
            .commit_he_frame(cookie, 1_500, 8, gi_1600, density),
        Err(HtAmpduTxError::AggregateFull)
    );
    storage.as_mut().cancel(cookie).unwrap();

    let gi_800 = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let cookie = storage.as_mut().begin().unwrap();
    for _ in 0..32 {
        storage
            .as_mut()
            .commit_he_frame(cookie, 1_500, 8, gi_800, density)
            .unwrap();
    }
    assert_eq!(storage.prepared_aggregate(cookie).unwrap().bytes, 48_512);
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn parses_block_ack_action_bodies_without_state() {
    assert_eq!(
        parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: 7,
            tid: 1,
            immediate: true,
            amsdu: true,
            window: 30,
            timeout_tu: 0,
            starting_sequence: 0x123,
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 7,
            status: 0,
            tid: 1,
            immediate: true,
            amsdu: false,
            window: 30,
            timeout_tu: 5,
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
        Some(BlockAckAction::Delba {
            tid: 5,
            initiator: true,
            reason: 39,
        })
    );
    assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
}

#[test]
fn ht_ampdu_length_matches_the_s31_six_mpdu_oracle() {
    let mut length = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();
    for sequence in 0x15_u32..=0x1a {
        // The HIL payload metadata was 0x00ss0612 followed by a zero
        // delimiter byte: six 1,554-byte MPDUs with two-byte padding.
        length.push((sequence << 16) | 0x0612, 0).unwrap();
    }
    assert_eq!(
        length.finish(),
        Ok(HtAmpduLength {
            bytes: 9_358,
            subframes: 6,
        })
    );
}

#[test]
fn ht_ampdu_length_is_bounded_and_removes_only_the_tail_trailer() {
    let mut length = HtAmpduLengthAccumulator::new(2, 4_096).unwrap();
    length.push(1_001, 2).unwrap();
    length.push(1_002, 1).unwrap();
    // First: 1001 + 3 padding + 8 empty + 4 mandatory.
    // Last: 1002 + 4 mandatory; its 2 padding + 4 empty bytes are removed.
    assert_eq!(length.finish().unwrap().bytes, 2_022);
    assert_eq!(length.push(1, 0), Err(HtAmpduLengthError::WindowFull));

    let mut too_short = HtAmpduLengthAccumulator::new(1, 1_000).unwrap();
    assert_eq!(
        too_short.push(1_001, 0),
        Err(HtAmpduLengthError::AggregateTooLong(1_005))
    );
    assert!(matches!(
        HtAmpduLengthAccumulator::new(0, 1),
        Err(HtAmpduLengthError::InvalidLimits)
    ));
}

#[test]
fn block_ack_register_decode_matches_the_pinned_leaf_layout() {
    let decoded = decode_ht_block_ack_registers(0x000a_bc50, 0x89ab_cdef, 0x0123_4567);
    assert_eq!(decoded.control, 0x0a);
    assert_eq!(decoded.block_ack.starting_sequence, 0x0bc5);
    assert_eq!(decoded.block_ack.bitmap, 0x0123_4567_89ab_cdef);
}

#[test]
fn basic_ht_assembly_matches_the_s31_hardware_oracle() {
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            aggregate_length: 9_358,
            first_header_length: 34,
            first_payload_word: 0xff12_3456,
            first_descriptor_flags: 0x0004_2009,
            first_descriptor_word1: 0xa5a5_0020,
            first_rate: 33,
            tail_buffer_flags: 0xa186_8612,
            tail_timestamp: 0x1234_5678,
        }),
        Ok(BasicHtAmpduAssemblyOutput {
            first_remaining_length: 9_324,
            first_payload_word: 0xfe12_3456,
            first_descriptor_flags: 0x004c_2009,
            first_descriptor_word1: 0x20,
            tail_buffer_flags: 0xe186_8612,
            first_timestamp: 0x1234_5678,
        })
    );
}

#[test]
fn partial_block_ack_mutations_match_the_s31_retry_oracle() {
    let base = BasicHtAmpduCompletionInput {
        descriptor_flags: 0x0004_2009,
        descriptor_queue_word: 0x00a0_0304,
        frame_control: 0x4188,
        acknowledged: true,
    };
    assert_eq!(
        basic_ht_ampdu_completion(base),
        BasicHtAmpduCompletionOutput {
            descriptor_flags: 0x0044_2009,
            descriptor_queue_word: 0x01a0_0304,
            frame_control: 0x4188,
        }
    );
    assert_eq!(
        basic_ht_ampdu_completion(BasicHtAmpduCompletionInput {
            acknowledged: false,
            ..base
        }),
        BasicHtAmpduCompletionOutput {
            descriptor_flags: 0x0004_2009,
            descriptor_queue_word: 0x00a0_0304,
            frame_control: 0x4988,
        }
    );
}

#[test]
fn basic_ht_assembly_rejects_he_bar_ampdu_and_bad_lengths_before_mutation() {
    let input = BasicHtAmpduAssemblyInput {
        aggregate_length: 1_500,
        first_header_length: 34,
        first_payload_word: 0,
        first_descriptor_flags: 0x0004_2009,
        first_descriptor_word1: 0x20,
        first_rate: 33,
        tail_buffer_flags: 0xa186_8612,
        tail_timestamp: 0,
    };
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            first_descriptor_flags: input.first_descriptor_flags | TX_DESCRIPTOR_HE_BIT,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
            0x8004_2009
        ))
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            first_rate: 15,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::UnsupportedRate(15))
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            aggregate_length: 33,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::AggregateShorterThanHeader)
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            tail_buffer_flags: input.tail_buffer_flags | TX_BUFFER_END_BIT,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::TailAlreadyTerminated(
            0xe186_8612
        ))
    );
}

#[test]
fn protection_spacing_matches_every_recovered_density_branch() {
    let expected = [20, 20, 20, 20, 20, 40, 76, 148];
    for (density, expected) in expected.into_iter().enumerate() {
        assert_eq!(
            basic_ht_ampdu_protection_spacing((density as u8) << 2),
            expected
        );
    }
    // Maximum A-MPDU length exponent and reserved high bits do not alter
    // the minimum-spacing field.
    assert_eq!(basic_ht_ampdu_protection_spacing(0xf7), 40);
}

#[test]
fn request_encoding_is_exact_and_bounded() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x1abc, 50).unwrap();
    assert_eq!(request.starting_sequence, 0x0abc);
    assert_eq!(request.alarm.deadline_us, 100_050);
    assert_eq!(request.body, [3, 0, 1, 0x1f, 0x08, 0, 0, 0xc0, 0xab]);
}

#[test]
fn shared_dialog_tokens_reproduce_the_vendor_three_tid_order_and_modulus() {
    let mut tokens = TxBlockAckDialogTokenSequence::new();
    assert_eq!(tokens.take().value(), 1);
    assert_eq!(tokens.take().value(), 2);
    assert_eq!(tokens.take().value(), 3);

    tokens.next = 62;
    assert_eq!(tokens.take().value(), 62);
    assert_eq!(tokens.take().value(), 0);
    assert_eq!(tokens.take().value(), 1);
}

#[test]
fn station_sessions_own_vendor_tid_order_response_routing_and_alarms() {
    let mut sessions = StaTxBlockAckSessions::new(32, 100_000, true).unwrap();
    let tid0 = sessions.begin(0, 0x100, 0).unwrap();
    let tid7 = sessions.begin(7, 0x200, 0).unwrap();
    let tid5 = sessions.begin(5, 0x300, 0).unwrap();
    assert_eq!(
        [tid0.dialog_token, tid7.dialog_token, tid5.dialog_token],
        [1, 2, 3]
    );
    assert_eq!(sessions.alarm(0), Some(tid0.alarm));
    assert_eq!(sessions.alarm(7), Some(tid7.alarm));
    assert_eq!(sessions.alarm(5), Some(tid5.alarm));

    let parameters = encode_ba_parameters(7, 16, false).to_le_bytes();
    let response = [
        3,
        1,
        tid7.dialog_token,
        0,
        0,
        parameters[0],
        parameters[1],
        0,
        0,
    ];
    assert_eq!(
        sessions.on_response(&response),
        Ok(StaTxBlockAckResponse {
            tid: 7,
            response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                tid: 7,
                window: 16,
                timeout_tu: 0,
                starting_sequence: 0x200,
                amsdu: false,
            }),
        })
    );
    assert_eq!(sessions.alarm(7), None);
    assert_eq!(sessions.expire_next(100_000), Some(0));
    assert_eq!(sessions.expire_next(100_000), Some(5));
    assert_eq!(sessions.expire_next(100_000), None);
    assert!(sessions.operational(7).is_some());
}

#[test]
fn parsed_response_can_cross_the_staged_rx_ownership_boundary() {
    let mut sessions = StaTxBlockAckSessions::new(32, 100_000, true).unwrap();
    let request = sessions.begin(0, 0x123, 0).unwrap();

    assert_eq!(
        sessions.on_response_action(BlockAckAction::AddbaResponse {
            dialog_token: request.dialog_token,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 7,
        }),
        Ok(StaTxBlockAckResponse {
            tid: 0,
            response: TxBlockAckResponse::Operational(OperationalTxBlockAck {
                tid: 0,
                window: 16,
                timeout_tu: 7,
                starting_sequence: 0x123,
                amsdu: true,
            }),
        })
    );
    assert_eq!(sessions.alarm(0), None);
}

#[test]
fn station_sessions_reject_unowned_tid_and_stale_dialog_token() {
    let mut sessions = StaTxBlockAckSessions::new(32, 100_000, false).unwrap();
    assert_eq!(
        sessions.begin(3, 0, 0),
        Err(StaTxBlockAckSessionsError::UnsupportedTid(3))
    );
    assert_eq!(
        sessions.on_response(&[3, 1, 42, 0, 0, 0, 0, 0, 0]),
        Err(StaTxBlockAckSessionsError::UnexpectedDialogToken(42))
    );
    assert!(!sessions.stop(3));
}

#[test]
fn matching_response_commits_only_the_static_window() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x123, 0).unwrap();
    let response = [3, 1, request.dialog_token, 0, 0, 0x1f, 0x08, 0, 0];
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(OperationalTxBlockAck {
            tid: 7,
            window: 32,
            timeout_tu: 0,
            starting_sequence: 0x123,
            amsdu: true,
        }))
    );
    assert_eq!(
        session.operational(),
        Some(OperationalTxBlockAck {
            tid: 7,
            window: 32,
            timeout_tu: 0,
            starting_sequence: 0x123,
            amsdu: true,
        })
    );
    assert!(!session.on_alarm(request.alarm));
}

#[test]
fn matching_he_response_accepts_an_addba_extension_ie() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x123, 0).unwrap();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        0x1f,
        0x08,
        0,
        0,
        159,
        1,
        0,
    ];
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(OperationalTxBlockAck {
            tid: 7,
            window: 32,
            timeout_tu: 0,
            starting_sequence: 0x123,
            amsdu: true,
        }))
    );
}

#[test]
fn stale_alarm_cannot_cancel_a_new_generation() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let stale = session.begin(1, 0).unwrap().alarm;
    let current = session.begin(2, 10).unwrap().alarm;
    assert!(!session.on_alarm(stale));
    assert!(session.on_alarm(current));
    assert_eq!(session.operational(), None);
}

#[test]
fn response_cannot_expand_the_static_capacity() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0, 0).unwrap();
    let parameters = encode_ba_parameters(7, 64, false).to_le_bytes();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        parameters[0],
        parameters[1],
        0,
        0,
    ];
    assert_eq!(
        session.on_response(&response),
        Err(TxBlockAckError::WindowExceedsCapacity(64))
    );
}

#[test]
fn rejected_response_returns_to_idle_without_a_timer_retry() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0, 0).unwrap();
    let response = [3, 1, request.dialog_token, 37, 0, 0, 0, 0, 0];
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Rejected(37))
    );
    assert_eq!(session.operational(), None);
    assert!(!session.on_alarm(request.alarm));
}

#[test]
fn block_ack_bitmap_handles_sequence_wrap() {
    let ack = TxBlockAckBitmap::new(0x0ffe, 0b1101);
    assert!(ack.acknowledges(0x0ffe));
    assert!(!ack.acknowledges(0x0fff));
    assert!(ack.acknowledges(0));
    assert!(ack.acknowledges(1));
    assert!(!ack.acknowledges(2));
}

#[test]
fn batch_returns_one_block_ack_result_per_step() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(0x0ffe, 4).unwrap();
    for slot in 3..7 {
        batch.push(slot).unwrap();
    }
    batch
        .complete_with_block_ack(TxBlockAckBitmap::new(0x0ffe, 0b1101))
        .unwrap();

    for (slot, sequence, disposition) in [
        (3, 0x0ffe, TxAmpduDisposition::Acknowledged),
        (4, 0x0fff, TxAmpduDisposition::Retry),
        (5, 0, TxAmpduDisposition::Acknowledged),
        (6, 1, TxAmpduDisposition::Acknowledged),
    ] {
        assert_eq!(
            batch.next_completion(),
            Some(TxAmpduCompletion {
                mpdu: TxAmpduMpdu {
                    slot: TxAmpduSlot::new(slot).unwrap(),
                    sequence,
                },
                disposition,
            })
        );
    }
    assert!(batch.is_idle());
    assert_eq!(batch.next_completion(), None);
}

#[test]
fn missing_block_ack_retries_every_mpdu_without_a_drain() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(9, 2).unwrap();
    batch.push(0).unwrap();
    batch.push(31).unwrap();
    batch.complete_without_block_ack().unwrap();
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Retry
    );
    assert!(!batch.is_idle());
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Retry
    );
    assert!(batch.is_idle());
}

#[test]
fn batch_rejects_duplicate_static_slot_ownership() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(0, 32).unwrap();
    batch.push(17).unwrap();
    assert_eq!(batch.push(17), Err(TxAmpduBatchError::DuplicateSlot(17)));
}

#[test]
fn batch_preserves_nonconsecutive_hardware_sequences() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(0x120, 4).unwrap();
    assert_eq!(batch.push_sequence(3, 0x120).unwrap().sequence, 0x120);
    assert_eq!(batch.push_sequence(4, 0x123).unwrap().sequence, 0x123);
    assert_eq!(
        batch.push_sequence(5, 0x1123),
        Err(TxAmpduBatchError::DuplicateSequence(0x123))
    );
    batch
        .complete_with_block_ack(TxBlockAckBitmap::new(0x120, 0b1001))
        .unwrap();
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Acknowledged
    );
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Acknowledged
    );
    assert!(batch.is_idle());
}

#[test]
fn batch_never_exceeds_negotiated_or_static_window() {
    let mut batch = TxAmpduBatch::new();
    assert_eq!(batch.begin(0, 0), Err(TxAmpduBatchError::InvalidWindow(0)));
    assert_eq!(
        batch.begin(0, 33),
        Err(TxAmpduBatchError::InvalidWindow(33))
    );
    batch.begin(0, 1).unwrap();
    batch.push(0).unwrap();
    assert_eq!(batch.push(1), Err(TxAmpduBatchError::Full));
}
