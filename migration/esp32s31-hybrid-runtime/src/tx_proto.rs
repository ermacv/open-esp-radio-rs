/// Reproduce the complete `ppTxProtoProc` descriptor transformation recovered
/// from the ESP32-S31 `libpp.a` leaf. The result depends only on the two MAC
/// header bytes and the descriptor's existing words; it has no hidden state.
pub(crate) const fn strict_tx_proto_flags(
    mut flags: u32,
    descriptor_word_12: u32,
    frame_control: u8,
    header_flags: u8,
) -> u32 {
    if header_flags & 1 != 0 {
        flags |= 2;
    }

    let frame_type = frame_control & 0x0c;
    if frame_type == 0x08 {
        flags |= 8;
        if descriptor_word_12 & 0x0002_0000 == 0 && frame_control & 0x70 == 0x40 {
            flags &= !8;
        }
    } else if frame_type == 0 {
        match frame_control & 0xf0 {
            0x50 if flags & 2 == 0 => flags |= 0x0800_0000,
            0x40 if flags & 2 == 0 => flags |= 0x0000_0800,
            _ => {}
        }
    }
    flags
}

/// Reject vendor packet-kind two while preserving the ordinary queues whose
/// high logical-queue bits alias that discriminator.
pub(crate) const fn admitted_basic_packet_kind(
    hardware_queue: u8,
    descriptor_word: u32,
    descriptor_flags: u32,
) -> bool {
    let packet_kind = descriptor_word & 0x00c0_0000;
    if packet_kind != 0x0080_0000 {
        return true;
    }
    let logical_queue = ((descriptor_word >> 20) & 0x0f) as u8;
    // With a second AP peer, the pinned net80211 scheduler maps its plaintext
    // EAPOL handshake descriptor onto HW0/Q8. Bits 22..23 used by the vendor
    // packet-kind discriminator overlap the upper logical-queue bits, so Q8
    // aliases packet kind two even though this exact descriptor is ordinary
    // WPA2 EAPOL, not NAN. Keep the exception bound to the measured descriptor.
    if hardware_queue == 0 && logical_queue == 8 && descriptor_flags == 0x0200_200c {
        return true;
    }
    // AP pairwise data was measured on the initialized WMM mapping HW2/Q10.
    hardware_queue == 2 && logical_queue == 10
}

/// The two bounded descriptor states observed for AP group-CCMP traffic.
pub(crate) const fn is_ap_group_ccmp_descriptor(descriptor_flags: u32) -> bool {
    matches!(descriptor_flags, 0x0000_200b | 0x0200_200b)
}

/// The two bounded pre-encryption states observed for AP pairwise-CCMP.
pub(crate) const fn is_ap_pairwise_ccmp_descriptor(descriptor_flags: u32) -> bool {
    matches!(descriptor_flags, 0x0000_2009 | 0x0200_2009)
}

/// Stateless SRAM-resident replacement for the vendor `ppTxProtoProc` leaf.
///
/// # Safety
///
/// `frame` must be the live vendor TX frame passed to `ppTxPkt`, with valid
/// first-buffer, payload and descriptor pointers. Ownership remains with the
/// caller and no concurrent context may mutate the descriptor.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_proto"]
pub unsafe extern "C" fn strict_pp_tx_proto_proc(frame: *mut u8) {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const DESCRIPTOR_WORD_12_OFFSET: usize = 0x30;

    if frame.is_null() {
        trap_invalid_tx_proto();
    }
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    let descriptor = frame
        .add(FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    if first_buffer.is_null() || descriptor.is_null() {
        trap_invalid_tx_proto();
    }
    let payload = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    if payload.is_null() {
        trap_invalid_tx_proto();
    }
    let layout = frame
        .add(FRAME_LAYOUT_FLAGS_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let header = payload.add(if layout & 0x2000 != 0 { 8 } else { 0 });
    let flags = descriptor.cast::<u32>().read_unaligned();
    let word_12 = descriptor
        .add(DESCRIPTOR_WORD_12_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    descriptor
        .cast::<u32>()
        .write_unaligned(strict_tx_proto_flags(
            flags,
            word_12,
            header.read(),
            header.add(4).read(),
        ));
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn trap_invalid_tx_proto() -> ! {
    core::arch::asm!("ebreak", options(noreturn))
}

#[cfg(test)]
mod tests {
    use super::{
        admitted_basic_packet_kind, is_ap_group_ccmp_descriptor, is_ap_pairwise_ccmp_descriptor,
        strict_tx_proto_flags,
    };

    #[test]
    fn propagates_header_flag_and_data_class() {
        assert_eq!(strict_tx_proto_flags(0x100, 0, 0x08, 1), 0x10a);
        assert_eq!(strict_tx_proto_flags(0x100, 0, 0x88, 0), 0x108);
    }

    #[test]
    fn reproduces_vendor_data_subtype_exception() {
        assert_eq!(strict_tx_proto_flags(0x100, 0, 0x48, 0), 0x100);
        assert_eq!(strict_tx_proto_flags(0x100, 0x0002_0000, 0x48, 0), 0x108);
    }

    #[test]
    fn marks_probe_and_beacon_management_classes() {
        assert_eq!(strict_tx_proto_flags(0, 0, 0x50, 0), 0x0800_0000);
        assert_eq!(strict_tx_proto_flags(0, 0, 0x40, 0), 0x800);
        assert_eq!(strict_tx_proto_flags(2, 0, 0x50, 0), 2);
        assert_eq!(strict_tx_proto_flags(2, 0, 0x40, 0), 2);
    }

    #[test]
    fn leaves_other_management_and_control_classes_unchanged() {
        assert_eq!(strict_tx_proto_flags(0x1234, 0, 0xb0, 0), 0x1234);
        assert_eq!(strict_tx_proto_flags(0x1234, 0, 0xd4, 0), 0x1234);
    }

    #[test]
    fn admits_only_the_measured_second_ap_peer_eapol_alias() {
        let q8_word = 8_u32 << 20;
        assert!(admitted_basic_packet_kind(0, q8_word, 0x0200_200c));
        assert!(!admitted_basic_packet_kind(0, q8_word, 0x0200_200b));
        assert!(!admitted_basic_packet_kind(1, q8_word, 0x0200_200c));
    }

    #[test]
    fn recognizes_only_measured_ap_group_ccmp_descriptors() {
        assert!(is_ap_group_ccmp_descriptor(0x0000_200b));
        assert!(is_ap_group_ccmp_descriptor(0x0200_200b));
        assert!(!is_ap_group_ccmp_descriptor(0x0200_2009));
        assert!(!is_ap_group_ccmp_descriptor(0x0400_200b));
    }

    #[test]
    fn recognizes_only_measured_ap_pairwise_ccmp_descriptors() {
        assert!(is_ap_pairwise_ccmp_descriptor(0x0000_2009));
        assert!(is_ap_pairwise_ccmp_descriptor(0x0200_2009));
        assert!(!is_ap_pairwise_ccmp_descriptor(0x0200_200b));
        assert!(!is_ap_pairwise_ccmp_descriptor(0x0400_2009));
    }
}
