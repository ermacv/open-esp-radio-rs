const LENGTH_MASK: u32 = 0x0000_3fff;
const PRESERVE_MASK: u32 = 0xf000_3fff;
const OWNER_BIT: u32 = 1 << 31;
const END_BITS: u32 = (1 << 30) | (1 << 29);
const ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET: usize = 0x04;
const ESF_BUFFER_DESCRIPTOR_DATA_OFFSET: usize = 0x04;
const ESF_RX_CONTROL_POINTER_OFFSET: usize = 0x10;
pub(crate) const RX_METADATA_PREFIX_BYTES: usize = 0x2c;
const RX_METADATA_BASE_PAYLOAD_OFFSET: usize = 0x38;

/// Byte-layout result produced by the pinned lower-MAC RX metadata prefix.
///
/// This is intentionally independent of pointers, MMIO and global WDEV state.
/// The target boundary copies the fixed prefix and performs the one register
/// read; all variable-offset arithmetic remains safe, host-tested Rust.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RxMetadataLayout {
    pub(crate) payload_offset: usize,
    pub(crate) sublength: u16,
    pub(crate) has_sublength: bool,
    pub(crate) has_extra_field: bool,
}

/// Bounded copy plan for the singleton, zero-CSI indication path.
///
/// The pinned ROM leaf copies the fixed 0x38-byte RX-control prefix, skips
/// the optional rounded sublength, then copies the remaining MPDU bytes back
/// against the prefix. Keeping this arithmetic in safe Rust leaves the ESF
/// boundary with only two finite non-overlapping copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleRxCopyPlan {
    pub(crate) source_payload_offset: usize,
    pub(crate) payload_length: usize,
    pub(crate) indicated_length: usize,
}

/// Bounded copy plan for one MPDU split across hardware RX descriptors.
///
/// The pinned S31 indication leaf keeps the first descriptor's 0x38-byte
/// control prefix, appends the remainder of that full segment, then appends
/// every complete middle segment and the tail's received byte count. Hardware
/// and ESF publish lengths in fourteen bits, so malformed or oversized chains
/// are rejected before the raw-pointer boundary sees them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MultiRxCopyPlan {
    pub(crate) descriptor_count: usize,
    pub(crate) segment_capacity: usize,
    pub(crate) first_payload_length: usize,
    pub(crate) middle_descriptor_count: usize,
    pub(crate) tail_payload_length: usize,
    pub(crate) indicated_length: usize,
}

/// Mutually exclusive reason why a decoded RX unit cannot enter the currently
/// Rust-owned ordinary-STA indication route.
///
/// The first four variants are recorded by the raw-pointer boundary before a
/// complete set of safe facts exists. Every later variant is selected by
/// [`rx_vendor_fallback_reason`]. Keeping the decision independent of
/// pointers and MMIO makes both precedence and accounting host-testable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum RxVendorFallbackReason {
    MissingHead,
    InvalidDescriptor,
    InvalidMetadataLayout,
    InvalidStatusOffset,
    NonSuccessStatus,
    InvalidChain,
    ExtendedMetadata,
    CsiMetadata,
    CopyPlanRejected,
    ApRoute,
    NanRoute,
    OtherRoute,
    OptionalControl30,
    OptionalControl46,
    NonOrdinaryProfile,
    MissingStationInterface,
    UnclassifiedFrame,
}

impl RxVendorFallbackReason {
    pub(crate) const COUNT: usize = Self::UnclassifiedFrame as usize + 1;

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

/// Pointer-free facts used to classify the final `wDev_ProcessRxSucData`
/// compatibility boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RxVendorFallbackFacts {
    pub(crate) status: u8,
    pub(crate) chain_valid: bool,
    pub(crate) has_extra_field: bool,
    pub(crate) csi_length: Option<u16>,
    pub(crate) copy_plan_valid: bool,
    pub(crate) route: u8,
    pub(crate) optional_control_30: bool,
    pub(crate) optional_control_46: bool,
    pub(crate) ordinary_profile: bool,
    pub(crate) station_interface_present: bool,
    pub(crate) frame_classified: bool,
}

/// Select exactly one remaining vendor fallback reason in pinned predicate
/// order, or return `None` when the facts describe the Rust-owned STA route.
pub(crate) const fn rx_vendor_fallback_reason(
    facts: RxVendorFallbackFacts,
) -> Option<RxVendorFallbackReason> {
    if facts.status != 0 {
        return Some(RxVendorFallbackReason::NonSuccessStatus);
    }
    if !facts.chain_valid {
        return Some(RxVendorFallbackReason::InvalidChain);
    }
    if facts.has_extra_field {
        return Some(RxVendorFallbackReason::ExtendedMetadata);
    }
    if !matches!(facts.csi_length, Some(0)) {
        return Some(RxVendorFallbackReason::CsiMetadata);
    }
    if !facts.copy_plan_valid {
        return Some(RxVendorFallbackReason::CopyPlanRejected);
    }
    match facts.route {
        0x10 => {}
        0x20 => return Some(RxVendorFallbackReason::ApRoute),
        0x40 => return Some(RxVendorFallbackReason::NanRoute),
        _ => return Some(RxVendorFallbackReason::OtherRoute),
    }
    if facts.optional_control_30 {
        return Some(RxVendorFallbackReason::OptionalControl30);
    }
    if facts.optional_control_46 {
        return Some(RxVendorFallbackReason::OptionalControl46);
    }
    if !facts.ordinary_profile {
        return Some(RxVendorFallbackReason::NonOrdinaryProfile);
    }
    if !facts.station_interface_present {
        return Some(RxVendorFallbackReason::MissingStationInterface);
    }
    if !facts.frame_classified {
        return Some(RxVendorFallbackReason::UnclassifiedFrame);
    }
    None
}

fn round_up_four(value: usize) -> Option<usize> {
    value.checked_add(3).map(|rounded| rounded & !3)
}

/// Reproduce `libpp.a[wdev.o]::get_sublen_offset` without its disabled
/// logging/dump side branches.
///
/// `extended_metadata_enabled` is bit 23 of MAC register `0x2010_4098`.
/// Bytes 0x26..0x27 form a ten-bit optional field; bit 2 of byte 0x27 also
/// makes that field present when its encoded length is zero.
pub(crate) fn decode_rx_metadata_layout(
    metadata: &[u8],
    extended_metadata_enabled: bool,
) -> Option<RxMetadataLayout> {
    if metadata.len() < RX_METADATA_PREFIX_BYTES {
        return None;
    }

    let has_sublength = metadata[0x2b] & 0x80 != 0;
    let sublength = if has_sublength {
        usize::from(metadata[0x2b] & 0x7f) + usize::from(metadata[0x2a] >> 5 != 0)
    } else {
        0
    };
    let rounded_sublength = round_up_four(sublength)?;
    let mut payload_offset = RX_METADATA_BASE_PAYLOAD_OFFSET.checked_add(rounded_sublength)?;

    let extra_length = usize::from(metadata[0x26]) | (usize::from(metadata[0x27] & 0x03) << 8);
    let has_extra_field =
        extended_metadata_enabled && (metadata[0x27] & 0x04 != 0 || extra_length != 0);
    if has_extra_field {
        payload_offset = payload_offset.checked_add(round_up_four(extra_length)?)?;
    }

    Some(RxMetadataLayout {
        payload_offset,
        sublength: u16::try_from(rounded_sublength).ok()?,
        has_sublength,
        has_extra_field,
    })
}

/// Produce the exact singleton copy/length transform for layouts whose only
/// optional field is the rounded sublength.
///
/// CSI/extended metadata has a distinct alignment-versus-published-length
/// contract and remains fail-closed until that format is qualified. A base
/// layout is the zero-sublength member of this same transform.
pub(crate) fn single_rx_copy_plan(
    descriptor_length: usize,
    layout: RxMetadataLayout,
    csi_length: u16,
) -> Option<SingleRxCopyPlan> {
    if layout.has_extra_field || csi_length != 0 {
        return None;
    }
    let rounded_sublength = usize::from(layout.sublength);
    let expected_payload_offset = RX_METADATA_BASE_PAYLOAD_OFFSET.checked_add(rounded_sublength)?;
    if layout.payload_offset != expected_payload_offset || layout.payload_offset > descriptor_length
    {
        return None;
    }
    let payload_length = descriptor_length.checked_sub(layout.payload_offset)?;
    let indicated_length = descriptor_length.checked_sub(rounded_sublength)?;
    if RX_METADATA_BASE_PAYLOAD_OFFSET.checked_add(payload_length)? != indicated_length {
        return None;
    }
    Some(SingleRxCopyPlan {
        source_payload_offset: layout.payload_offset,
        payload_length,
        indicated_length,
    })
}

/// Produce the exact base-layout multi-descriptor copy/length transform.
///
/// Optional sublength, extended metadata and CSI are intentionally excluded
/// by the caller. This function owns only finite integer arithmetic and
/// enforces both the recovered 64-descriptor event bound and the 14-bit ESF
/// length ABI.
pub(crate) fn multi_rx_copy_plan(
    descriptor_count: usize,
    segment_capacity: usize,
    tail_payload_length: usize,
) -> Option<MultiRxCopyPlan> {
    if !(2..=64).contains(&descriptor_count)
        || segment_capacity < RX_METADATA_BASE_PAYLOAD_OFFSET
        || segment_capacity > LENGTH_MASK as usize
        || tail_payload_length == 0
        || tail_payload_length > segment_capacity
    {
        return None;
    }
    let preceding_length = segment_capacity.checked_mul(descriptor_count.checked_sub(1)?)?;
    let indicated_length = preceding_length.checked_add(tail_payload_length)?;
    if indicated_length > LENGTH_MASK as usize {
        return None;
    }
    Some(MultiRxCopyPlan {
        descriptor_count,
        segment_capacity,
        first_payload_length: segment_capacity - RX_METADATA_BASE_PAYLOAD_OFFSET,
        middle_descriptor_count: descriptor_count - 2,
        tail_payload_length,
        indicated_length,
    })
}

/// Recover the second argument passed by the pinned
/// `wDev_ProcessRxSucData` body to `wDev_IndicateFrame`.
///
/// The value is normally zero. A signed-negative metadata mode forces one;
/// mode `0b01` in the high two bits selects bit 27 of the following word.
/// Keeping this bit decode separate from the pointer-owning WDEV boundary
/// makes all three branches host-testable.
pub(crate) fn rx_indicate_aggregate_flag(metadata: &[u8]) -> Option<u32> {
    if metadata.len() < 8 {
        return None;
    }
    let mode = metadata[1];
    if (mode as i8) < 0 {
        return Some(1);
    }
    if mode & 0xc0 != 0x40 {
        return Some(0);
    }
    Some((u32::from_le_bytes(metadata[4..8].try_into().ok()?) >> 27) & 1)
}

/// Recover the first `wDev_IndicateFrame` argument for a data MPDU.
///
/// Only frame-control classification is performed here. The caller separately
/// owns descriptor bounds, interface routing and optional-mode admission.
pub(crate) const fn rx_sta_data_copy_mode(frame_control: u16) -> Option<u32> {
    if frame_control & 0x0f != 0x08 {
        return None;
    }
    if frame_control & 0x0400 != 0 {
        return Some(0);
    }
    Some((frame_control & 0x70 == 0x40) as u32)
}

/// Admit management subtypes whose pinned STA path has no optional side
/// branch.
///
/// Association responses, beacons and authentication frames all join the
/// vendor classifier with copy mode one. The common fragmentation join clears
/// that flag. Probe requests and action frames deliberately return `None`;
/// action admission additionally requires an adopted NAN/FTM policy proof.
pub(crate) const fn rx_sta_management_copy_mode(frame_control: u16) -> Option<u32> {
    if frame_control & 0x0f != 0 {
        return None;
    }
    match (frame_control >> 4) & 0x0f {
        1 | 8 | 11 => Some((frame_control & 0x0400 == 0) as u32),
        _ => None,
    }
}

/// Recover the common copy mode for a management Action frame.
///
/// `wDev_ProcessRxSucData+0x2d8..=0x312` first dispatches optional NAN and FTM
/// observers, then joins the same copy-mode-one management path. This pure
/// classifier owns only the frame-control decision. Its caller must hold the
/// one-shot proof that NAN interface bit two and FTM menu bit `0x04` were both
/// disabled before the strict runtime handoff.
pub(crate) const fn rx_sta_action_copy_mode(frame_control: u16) -> Option<u32> {
    if frame_control & 0x0f != 0 || (frame_control >> 4) & 0x0f != 13 {
        return None;
    }
    Some((frame_control & 0x0400 == 0) as u32)
}

/// Identify the Probe Request that the pinned STA-only path reroutes to AP.
///
/// `wDev_ProcessRxSucData+0x25a..=0x268` replaces the descriptor route bits
/// with AP, optionally calls the separately disabled observation callback,
/// and later discards the unit when no AP interface is enabled. The caller
/// owns those profile invariants and the unique discard authority.
pub(crate) const fn rx_sta_probe_request_is_discarded(frame_control: u16) -> bool {
    frame_control & 0x0f == 0 && (frame_control >> 4) & 0x0f == 4
}

/// Reproduce the pinned S31 RX recycle descriptor transformation.
///
/// This leaf is deliberately independent of global state, registers and raw
/// pointers. The hardware boundary in `wdev` owns the unsafe descriptor load
/// and store; the bit-level operation remains host-testable safe Rust.
pub(crate) const fn recycled_descriptor_word(word: u32) -> u32 {
    let length = word & LENGTH_MASK;
    ((word | OWNER_BIT) & !END_BITS & PRESERVE_MASK) | (length << 14)
}

pub(crate) const fn descriptor_buffer_length(word: u32) -> usize {
    (word & LENGTH_MASK) as usize
}

/// Return the hardware-published received byte count at descriptor bits
/// 14..27.
///
/// The lower fourteen bits remain the backing segment capacity. Keeping the
/// two fields separate prevents a short management frame from being mistaken
/// for the configured 1700-byte RX buffer.
pub(crate) const fn descriptor_received_length(word: u32) -> usize {
    ((word >> 14) & LENGTH_MASK) as usize
}

/// Recover the CSI byte count returned by the pinned `wdev_csi_len_align`
/// leaf.
///
/// Despite its name, the S31 implementation is only two bounded byte loads
/// and a ten-bit join. Strict ordinary AP/STA disables CSI, so the Rust-owned
/// `wDev_IndicateFrame` route admits only a zero result.
pub(crate) const fn rx_csi_length(metadata: &[u8]) -> Option<u16> {
    if metadata.len() <= 0x27 {
        return None;
    }
    Some(u16::from_le_bytes([metadata[0x26], metadata[0x27] & 0x03]))
}

/// Publish the received MPDU length into the allocated ESF buffer descriptor.
///
/// The pinned leaf preserves bits 0..13 and 28..31, replacing only the
/// fourteen-bit length at 14..27.
pub(crate) const fn indicated_rx_descriptor_word(word: u32, length: usize) -> Option<u32> {
    if length > LENGTH_MASK as usize {
        return None;
    }
    Some((word & PRESERVE_MASK) | ((length as u32) << 14))
}

/// Add the exact single/multi-descriptor and aggregate flags to the ESF RX
/// descriptor without disturbing its existing low twelve or high twenty bits.
pub(crate) const fn indicated_rx_flags_word(word: u32, count: u32, aggregate: bool) -> u32 {
    let count_flag = if count == 1 { 0x100 } else { 0x80 };
    word | count_flag | if aggregate { 0x10 } else { 0 }
}

/// Restore the sole mutable buffer view changed by the RX protocol path.
///
/// Pinned `libpp.a[pp.o]::ppRecycleRxPkt` is fourteen bytes: it loads the ESF
/// buffer descriptor at `frame+0x04`, loads the original RX control pointer at
/// `frame+0x10`, stores that pointer at `descriptor+0x04`, and tail-calls
/// `esf_buf_recycle`. Keeping this field transform independent makes the
/// recovered ABI host-testable without emulating the fixed ESF pools.
///
/// # Safety
///
/// `frame` must point to a live pinned-layout ESF object. Its embedded buffer
/// descriptor must remain writable for the duration of the call.
pub(crate) unsafe fn restore_received_packet_buffer_view(frame: *mut u8) -> bool {
    if frame.is_null() {
        return false;
    }
    let buffer_descriptor = frame
        .add(ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    let rx_control = frame
        .add(ESF_RX_CONTROL_POINTER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if buffer_descriptor.is_null() || rx_control.is_null() {
        return false;
    }
    buffer_descriptor
        .add(ESF_BUFFER_DESCRIPTOR_DATA_OFFSET)
        .cast::<*mut u8>()
        .write(rx_control);
    true
}

#[cfg(test)]
mod tests {
    use core::ptr;

    use super::{
        decode_rx_metadata_layout, descriptor_buffer_length, descriptor_received_length,
        indicated_rx_descriptor_word, indicated_rx_flags_word, multi_rx_copy_plan,
        recycled_descriptor_word, restore_received_packet_buffer_view, rx_csi_length,
        rx_indicate_aggregate_flag, rx_sta_action_copy_mode, rx_sta_data_copy_mode,
        rx_sta_management_copy_mode, rx_sta_probe_request_is_discarded, rx_vendor_fallback_reason,
        single_rx_copy_plan, RxVendorFallbackFacts, RxVendorFallbackReason,
        ESF_BUFFER_DESCRIPTOR_DATA_OFFSET, ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET,
        ESF_RX_CONTROL_POINTER_OFFSET, RX_METADATA_PREFIX_BYTES,
    };

    #[test]
    fn recycle_word_matches_the_pinned_vendor_bit_sequence() {
        for word in [0, 1, 0x0000_3fff, 0x6000_1234, 0x9abc_def0, u32::MAX] {
            let mut expected = word | (1 << 31);
            expected &= !(1 << 30);
            expected &= !(1 << 29);
            let length = expected & 0x3fff;
            expected = (expected & 0xf000_3fff) | (length << 14);
            assert_eq!(recycled_descriptor_word(word), expected);
            assert_eq!(descriptor_buffer_length(word), (word & 0x3fff) as usize);
            assert_eq!(
                descriptor_received_length(word),
                ((word >> 14) & 0x3fff) as usize
            );
        }
    }

    #[test]
    fn indicated_rx_words_match_the_pinned_single_frame_stores() {
        assert_eq!(
            indicated_rx_descriptor_word(0xdead_beef, 0x1234),
            Some((0xdead_beef & 0xf000_3fff) | (0x1234 << 14))
        );
        assert_eq!(indicated_rx_descriptor_word(0, 0x4000), None);

        assert_eq!(indicated_rx_flags_word(0xabcd_e002, 1, false), 0xabcd_e102);
        assert_eq!(indicated_rx_flags_word(0xabcd_e002, 2, false), 0xabcd_e082);
        assert_eq!(indicated_rx_flags_word(0xabcd_e002, 1, true), 0xabcd_e112);
    }

    #[test]
    fn csi_length_is_the_exact_ten_bit_metadata_join() {
        assert_eq!(rx_csi_length(&[0; 0x27]), None);
        let mut metadata = [0_u8; RX_METADATA_PREFIX_BYTES];
        assert_eq!(rx_csi_length(&metadata), Some(0));
        metadata[0x26] = 0xa5;
        metadata[0x27] = 0xfe;
        assert_eq!(rx_csi_length(&metadata), Some(0x2a5));
    }

    #[test]
    fn rx_packet_recycle_restores_the_original_buffer_view() {
        let mut frame = [0_u8; 0x90];
        let mut descriptor = [0_u8; 8];
        let mut rx_control = [0_u8; 80];
        let stale_view = ptr::without_provenance_mut::<u8>(0x1234);

        unsafe {
            frame
                .as_mut_ptr()
                .add(ESF_BUFFER_DESCRIPTOR_POINTER_OFFSET)
                .cast::<*mut u8>()
                .write(descriptor.as_mut_ptr());
            frame
                .as_mut_ptr()
                .add(ESF_RX_CONTROL_POINTER_OFFSET)
                .cast::<*mut u8>()
                .write(rx_control.as_mut_ptr());
            descriptor
                .as_mut_ptr()
                .add(ESF_BUFFER_DESCRIPTOR_DATA_OFFSET)
                .cast::<*mut u8>()
                .write(stale_view);

            assert!(restore_received_packet_buffer_view(frame.as_mut_ptr()));
            assert_eq!(
                descriptor
                    .as_ptr()
                    .add(ESF_BUFFER_DESCRIPTOR_DATA_OFFSET)
                    .cast::<*mut u8>()
                    .read(),
                rx_control.as_mut_ptr()
            );
        }
    }

    #[test]
    fn rx_packet_recycle_rejects_missing_owners_without_writing() {
        let mut frame = [0_u8; 0x90];
        unsafe {
            assert!(!restore_received_packet_buffer_view(ptr::null_mut()));
            assert!(!restore_received_packet_buffer_view(frame.as_mut_ptr()));
        }
    }

    #[test]
    fn rx_metadata_layout_reproduces_base_and_rounded_sublength() {
        let mut metadata = [0_u8; RX_METADATA_PREFIX_BYTES];
        assert_eq!(
            decode_rx_metadata_layout(&metadata, false)
                .unwrap()
                .payload_offset,
            0x38
        );

        metadata[0x2b] = 0x81;
        let one_byte = decode_rx_metadata_layout(&metadata, false).unwrap();
        assert_eq!(one_byte.payload_offset, 0x3c);
        assert_eq!(one_byte.sublength, 4);
        assert!(one_byte.has_sublength);

        metadata[0x2a] = 0x20;
        metadata[0x2b] = 0xff;
        let maximum = decode_rx_metadata_layout(&metadata, false).unwrap();
        assert_eq!(maximum.payload_offset, 0xb8);
        assert_eq!(maximum.sublength, 128);
    }

    #[test]
    fn rx_metadata_layout_gates_and_rounds_the_ten_bit_extra_field() {
        let mut metadata = [0_u8; RX_METADATA_PREFIX_BYTES];
        metadata[0x26] = 1;
        let disabled = decode_rx_metadata_layout(&metadata, false).unwrap();
        assert_eq!(disabled.payload_offset, 0x38);
        assert!(!disabled.has_extra_field);

        let enabled = decode_rx_metadata_layout(&metadata, true).unwrap();
        assert_eq!(enabled.payload_offset, 0x3c);
        assert!(enabled.has_extra_field);

        metadata[0x26] = 0xff;
        metadata[0x27] = 0x03;
        let maximum = decode_rx_metadata_layout(&metadata, true).unwrap();
        assert_eq!(maximum.payload_offset, 0x438);

        metadata[0x26] = 0;
        metadata[0x27] = 0x04;
        let present_zero_length = decode_rx_metadata_layout(&metadata, true).unwrap();
        assert_eq!(present_zero_length.payload_offset, 0x38);
        assert!(present_zero_length.has_extra_field);
    }

    #[test]
    fn rx_metadata_layout_rejects_a_truncated_prefix() {
        assert_eq!(decode_rx_metadata_layout(&[0_u8; 0x2b], true), None);
    }

    #[test]
    fn singleton_copy_plan_skips_only_the_rounded_sublength() {
        let mut metadata = [0_u8; RX_METADATA_PREFIX_BYTES];
        let base = decode_rx_metadata_layout(&metadata, false).unwrap();
        let base_plan = single_rx_copy_plan(100, base, 0).unwrap();
        assert_eq!(base_plan.source_payload_offset, 0x38);
        assert_eq!(base_plan.payload_length, 44);
        assert_eq!(base_plan.indicated_length, 100);

        metadata[0x2b] = 0x81;
        let sublength = decode_rx_metadata_layout(&metadata, false).unwrap();
        let sublength_plan = single_rx_copy_plan(100, sublength, 0).unwrap();
        assert_eq!(sublength_plan.source_payload_offset, 0x3c);
        assert_eq!(sublength_plan.payload_length, 40);
        assert_eq!(sublength_plan.indicated_length, 96);

        assert_eq!(single_rx_copy_plan(0x3b, sublength, 0), None);
        assert_eq!(single_rx_copy_plan(100, sublength, 1), None);

        metadata[0x26] = 1;
        let extra = decode_rx_metadata_layout(&metadata, true).unwrap();
        assert_eq!(single_rx_copy_plan(100, extra, 0), None);
    }

    #[test]
    fn multi_copy_plan_matches_the_pinned_segment_join() {
        let plan = multi_rx_copy_plan(3, 1700, 123).unwrap();
        assert_eq!(plan.descriptor_count, 3);
        assert_eq!(plan.segment_capacity, 1700);
        assert_eq!(plan.first_payload_length, 1700 - 0x38);
        assert_eq!(plan.middle_descriptor_count, 1);
        assert_eq!(plan.tail_payload_length, 123);
        assert_eq!(plan.indicated_length, 3523);

        assert_eq!(multi_rx_copy_plan(1, 1700, 100), None);
        assert_eq!(multi_rx_copy_plan(65, 1700, 100), None);
        assert_eq!(multi_rx_copy_plan(2, 0x37, 1), None);
        assert_eq!(multi_rx_copy_plan(2, 1700, 0), None);
        assert_eq!(multi_rx_copy_plan(2, 1700, 1701), None);
        assert_eq!(multi_rx_copy_plan(11, 1700, 100), None);

        let maximum = multi_rx_copy_plan(2, 0x2000, 0x1fff).unwrap();
        assert_eq!(maximum.indicated_length, 0x3fff);
        assert_eq!(multi_rx_copy_plan(2, 0x2000, 0x2000), None);
    }

    #[test]
    fn indicate_aggregate_flag_matches_all_pinned_branches() {
        assert_eq!(rx_indicate_aggregate_flag(&[0; 7]), None);

        let mut metadata = [0_u8; 8];
        assert_eq!(rx_indicate_aggregate_flag(&metadata), Some(0));

        metadata[1] = 0x80;
        assert_eq!(rx_indicate_aggregate_flag(&metadata), Some(1));

        metadata[1] = 0x40;
        metadata[7] = 0x08;
        assert_eq!(rx_indicate_aggregate_flag(&metadata), Some(1));
        metadata[7] = 0;
        assert_eq!(rx_indicate_aggregate_flag(&metadata), Some(0));
    }

    #[test]
    fn sta_data_copy_mode_matches_the_pinned_classifier_join() {
        assert_eq!(rx_sta_data_copy_mode(0x0080), None);
        assert_eq!(rx_sta_data_copy_mode(0x0008), Some(0));
        assert_eq!(rx_sta_data_copy_mode(0x0048), Some(1));
        assert_eq!(rx_sta_data_copy_mode(0x0448), Some(0));
    }

    #[test]
    fn sta_management_classifier_admits_only_side_effect_free_subtypes() {
        assert_eq!(rx_sta_management_copy_mode(0x0010), Some(1));
        assert_eq!(rx_sta_management_copy_mode(0x0080), Some(1));
        assert_eq!(rx_sta_management_copy_mode(0x00b0), Some(1));
        assert_eq!(rx_sta_management_copy_mode(0x0480), Some(0));
        assert_eq!(rx_sta_management_copy_mode(0x0040), None);
        assert_eq!(rx_sta_management_copy_mode(0x00d0), None);
        assert_eq!(rx_sta_management_copy_mode(0x0008), None);
    }

    #[test]
    fn sta_action_classifier_is_exact_and_clears_copy_on_fragment() {
        assert_eq!(rx_sta_action_copy_mode(0x00d0), Some(1));
        assert_eq!(rx_sta_action_copy_mode(0x04d0), Some(0));
        assert_eq!(rx_sta_action_copy_mode(0x40d0), Some(1));
        assert_eq!(rx_sta_action_copy_mode(0x00d1), None);
        assert_eq!(rx_sta_action_copy_mode(0x00c0), None);
        assert_eq!(rx_sta_action_copy_mode(0x00d8), None);
    }

    #[test]
    fn sta_probe_request_classifier_is_exact_and_flag_independent() {
        assert!(rx_sta_probe_request_is_discarded(0x0040));
        assert!(rx_sta_probe_request_is_discarded(0x0c40));
        assert!(!rx_sta_probe_request_is_discarded(0x0041));
        assert!(!rx_sta_probe_request_is_discarded(0x0050));
        assert!(!rx_sta_probe_request_is_discarded(0x0008));
    }

    #[test]
    fn vendor_fallback_classifier_is_mutually_exclusive_in_pinned_order() {
        let admitted = RxVendorFallbackFacts {
            status: 0,
            chain_valid: true,
            has_extra_field: false,
            csi_length: Some(0),
            copy_plan_valid: true,
            route: 0x10,
            optional_control_30: false,
            optional_control_46: false,
            ordinary_profile: true,
            station_interface_present: true,
            frame_classified: true,
        };
        assert_eq!(rx_vendor_fallback_reason(admitted), None);

        let cases = [
            (
                RxVendorFallbackFacts {
                    status: 0xf5,
                    chain_valid: false,
                    ..admitted
                },
                RxVendorFallbackReason::NonSuccessStatus,
            ),
            (
                RxVendorFallbackFacts {
                    chain_valid: false,
                    has_extra_field: true,
                    ..admitted
                },
                RxVendorFallbackReason::InvalidChain,
            ),
            (
                RxVendorFallbackFacts {
                    has_extra_field: true,
                    csi_length: Some(1),
                    ..admitted
                },
                RxVendorFallbackReason::ExtendedMetadata,
            ),
            (
                RxVendorFallbackFacts {
                    csi_length: Some(1),
                    copy_plan_valid: false,
                    ..admitted
                },
                RxVendorFallbackReason::CsiMetadata,
            ),
            (
                RxVendorFallbackFacts {
                    csi_length: None,
                    ..admitted
                },
                RxVendorFallbackReason::CsiMetadata,
            ),
            (
                RxVendorFallbackFacts {
                    copy_plan_valid: false,
                    route: 0x20,
                    ..admitted
                },
                RxVendorFallbackReason::CopyPlanRejected,
            ),
            (
                RxVendorFallbackFacts {
                    route: 0x20,
                    ..admitted
                },
                RxVendorFallbackReason::ApRoute,
            ),
            (
                RxVendorFallbackFacts {
                    route: 0x40,
                    ..admitted
                },
                RxVendorFallbackReason::NanRoute,
            ),
            (
                RxVendorFallbackFacts {
                    route: 0,
                    ..admitted
                },
                RxVendorFallbackReason::OtherRoute,
            ),
            (
                RxVendorFallbackFacts {
                    optional_control_30: true,
                    optional_control_46: true,
                    ..admitted
                },
                RxVendorFallbackReason::OptionalControl30,
            ),
            (
                RxVendorFallbackFacts {
                    optional_control_46: true,
                    ..admitted
                },
                RxVendorFallbackReason::OptionalControl46,
            ),
            (
                RxVendorFallbackFacts {
                    ordinary_profile: false,
                    station_interface_present: false,
                    ..admitted
                },
                RxVendorFallbackReason::NonOrdinaryProfile,
            ),
            (
                RxVendorFallbackFacts {
                    station_interface_present: false,
                    frame_classified: false,
                    ..admitted
                },
                RxVendorFallbackReason::MissingStationInterface,
            ),
            (
                RxVendorFallbackFacts {
                    frame_classified: false,
                    ..admitted
                },
                RxVendorFallbackReason::UnclassifiedFrame,
            ),
        ];
        for (facts, expected) in cases {
            assert_eq!(rx_vendor_fallback_reason(facts), Some(expected));
        }
        assert_eq!(RxVendorFallbackReason::COUNT, 17);
    }
}
