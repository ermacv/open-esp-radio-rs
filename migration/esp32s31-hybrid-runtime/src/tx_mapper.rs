use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxMapperRejectionSnapshot {
    pub detail: u32,
    pub frame: u32,
    pub rate_layout: u32,
    pub frame_control_peer_flag: u32,
    pub descriptor_flags: u32,
    pub descriptor_priority: u32,
    pub descriptor_control: u32,
    pub peer_state: u32,
}

struct TxMapperRejectionRecord {
    detail: AtomicU32,
    frame: AtomicU32,
    rate_layout: AtomicU32,
    frame_control_peer_flag: AtomicU32,
    descriptor_flags: AtomicU32,
    descriptor_priority: AtomicU32,
    descriptor_control: AtomicU32,
    peer_state: AtomicU32,
}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.tx_mapper_rejection"
)]
static TX_MAPPER_REJECTION: TxMapperRejectionRecord = TxMapperRejectionRecord {
    detail: AtomicU32::new(0),
    frame: AtomicU32::new(0),
    rate_layout: AtomicU32::new(0),
    frame_control_peer_flag: AtomicU32::new(0),
    descriptor_flags: AtomicU32::new(0),
    descriptor_priority: AtomicU32::new(0),
    descriptor_control: AtomicU32::new(0),
    peer_state: AtomicU32::new(0),
};

/// Last fail-closed mapper input, retained in fixed SRAM for post-trap
/// inspection without depending on stack-heavy trap-frame formatting.
pub fn tx_mapper_rejection_snapshot() -> TxMapperRejectionSnapshot {
    let detail = TX_MAPPER_REJECTION.detail.load(Ordering::Acquire);
    TxMapperRejectionSnapshot {
        detail,
        frame: TX_MAPPER_REJECTION.frame.load(Ordering::Relaxed),
        rate_layout: TX_MAPPER_REJECTION.rate_layout.load(Ordering::Relaxed),
        frame_control_peer_flag: TX_MAPPER_REJECTION
            .frame_control_peer_flag
            .load(Ordering::Relaxed),
        descriptor_flags: TX_MAPPER_REJECTION
            .descriptor_flags
            .load(Ordering::Relaxed),
        descriptor_priority: TX_MAPPER_REJECTION
            .descriptor_priority
            .load(Ordering::Relaxed),
        descriptor_control: TX_MAPPER_REJECTION
            .descriptor_control
            .load(Ordering::Relaxed),
        peer_state: TX_MAPPER_REJECTION.peer_state.load(Ordering::Relaxed),
    }
}

/// Decide the only descriptor treatment used by the guarded strict STA
/// ordinary strict mapper states. Every admitted state maps to logical queue zero;
/// `Some(7)` means descriptor byte four must contain the recovered treatment.
pub(crate) fn strict_sta_ap_treatment(
    rate: u8,
    layout: u16,
    frame_control: u16,
    state: [u32; 5],
) -> Option<u8> {
    // The pinned mapper tests only layout bit 0x2000 to select the eight-byte
    // ESF prefix. Lower bits are the static slot's changing buffer identity
    // (`0x2000`, `0x2008`, `0x2010`, ...), not mapper state. Keep the adjacent
    // security leaf's upper-bit invariant but do not accidentally bind queue
    // policy to those opaque low bits.
    if layout & 0xe000 != 0x2000 {
        return None;
    }

    let management = rate == 0
        && state[0] == 0
        && state[1] == 7
        && state[2] == 0
        && state[4] == 0
        && ((state[3] == 0x80 && matches!(frame_control, 0x00b0 | 0x0000))
            || (state[3] == 0x81 && frame_control == 0x00d0));
    let eapol = rate == 0 && frame_control == 0x0188 && state == [0x0200_200c, 7, 0, 0x81, 0];
    // The retained WPA2 AP beacon enters this leaf after the descriptor and
    // security policies have applied their already-qualified fixed layout.
    // Pinned ppMapTxQueue preserves descriptor byte four as 0x07 for this
    // class; no aggregation or power-save search state is consulted.
    let ap_beacon =
        rate == 12 && frame_control == 0x0080 && state == [0x0080_0412, 7, 0x0004_0000, 0x83, 0];
    // Once the qualified beacon is on air, an active scanner immediately
    // exercises the AP probe-response class.  It shares the AP peer/rate
    // context but is a transient frame, identified by the exact descriptor
    // ownership word already admitted by the security leaf.
    let ap_probe_response =
        rate == 12 && frame_control == 0x0050 && state == [0x0800_0010, 7, 0x0004_0000, 0x83, 0];
    // Open-system authentication is still plaintext, but an AP-generated
    // response uses the same fixed AP selector and peer context as the
    // beacon/probe path rather than the STA management tuple above.
    let ap_authentication_response =
        rate == 12 && frame_control == 0x00b0 && state == [0, 7, 0x0004_0000, 0x83, 0];
    // A successful AP association response observes the node after the vendor
    // state transition: peer[0x0c] and peer[0x84] are therefore intentionally
    // different from the pre-association authentication tuple.
    // The first two statically provisioned AP connection nodes publish their
    // one-based table identity in peer byte 0x84. Both values have now been
    // observed with independently associated WPA2 stations. Keep the finite
    // two-client HIL domain explicit; zero and unqualified later slots remain
    // fail-closed.
    let bounded_ap_peer =
        state[3] == 0x2100_0000 && matches!(state[4], 1 | 2);
    // The AP pairwise hardware slot is encoded in the low descriptor-control
    // byte after CCMP setup: peer identity one used slot 8 (`0x48`) and peer
    // identity two used slot 9 (`0x49`). Bind the two observations instead of
    // accepting either key selector for either node.
    let bounded_ap_pairwise_selector = bounded_ap_peer
        && matches!(
            (state[4], state[2]),
            (1, 0x0004_0348) | (2, 0x0004_0349)
        );
    let ap_association_response = rate == 11
        && frame_control == 0x0010
        && state[0] == 0
        && state[1] == 7
        && state[2] == 0x0004_0000
        && bounded_ap_peer;
    // WPA2 message one is emitted immediately after successful association.
    // It remains plaintext at this boundary and is bound to the same
    // post-association node identity as the response above.
    let ap_eapol_message_one = rate == 11
        && frame_control == 0x0288
        && state[0] == 0x0200_200c
        && state[1] == 7
        && state[2] == 0x0004_0000
        && bounded_ap_peer;
    // The first network packet after AP authorization is the protected
    // broadcast response needed by the joining station. Security has already
    // expanded this exact group-CCMP descriptor before the mapper runs.
    // A DTIM FIFO element which is not last carries IEEE 802.11 More Data
    // (0x2000). The HIL-observed tuple is otherwise byte-for-byte identical
    // to the already-qualified AP group CCMP class.
    let ap_group_ccmp_data = rate == 12
        && matches!(frame_control, 0x4208 | 0x6208)
        && matches!(state[0], 0x0000_200b | 0x0200_200b)
        && state[1] == 7
        && state[2] == 0x0004_0342
        && state[3] == 0x83
        && state[4] == 0;
    // Once the associated peer requests RX aggregation, the bounded Rust AP
    // response enters the mapper as a post-association Action frame.
    let ap_addba_response = rate == 11
        && frame_control == 0x00d0
        && state[0] == 0
        && state[1] == 7
        && state[2] == 0x0004_0000
        && bounded_ap_peer;
    // The first protected unicast network packet from the AP is a pairwise
    // CCMP QoS downlink. It uses the associated peer rather than the AP's
    // group/broadcast pseudo-peer and reaches the mapper after the security
    // leaf has installed the measured pairwise selector in descriptor word
    // four. Keep this separate from the STA uplink HT-QoS class below.
    let ap_pairwise_ht_qos = rate == 33
        && frame_control == 0x4288
        && state[0] == 0x0000_2009
        && state[1] == 0x20
        && bounded_ap_pairwise_selector;
    // Android exposed the adjacent first-downlink state before the selected
    // rate has converged to the fixed HT tuple above. The descriptor carries
    // the already-qualified fixed-per-packet-rate bit, internal rate code 11,
    // and the untouched
    // treatment byte 7. Keep it as a separate complete class: admitting the
    // bit on the HT tuple would hide an unobserved combination.
    let ap_pairwise_rate_control_qos = rate == 11
        && frame_control == 0x4288
        && state[0] == 0x0200_2009
        && state[1] == 7
        && bounded_ap_pairwise_selector;
    // Bytes five through seven are PP aggregation-search hints. They are zero
    // before ADDBA and become nonzero after the peer accepts ADDBA, but the
    // strict single-MPDU path deliberately does not enter ppSearchTxQueue.
    // Descriptor byte four remains the complete priority/treatment input used
    // by the finite queue mapping.
    let priority_treatment = state[1] as u8;
    let ht_qos = rate == 33
        && frame_control == 0x4188
        && state[0] == 0x0000_2009
        && matches!(priority_treatment, 7 | 0x20)
        && state[2] == 0x304
        && state[3] == 0x81
        && state[4] == 0;
    let legacy_qos =
        rate == 0 && frame_control == 0x4188 && state == [0x0200_2009, 7, 0x304, 0x81, 0];

    if management
        || eapol
        || ap_beacon
        || ap_probe_response
        || ap_authentication_response
        || ap_association_response
        || ap_eapol_message_one
        || ap_group_ccmp_data
        || ap_addba_response
        || ap_pairwise_ht_qos
        || ap_pairwise_rate_control_qos
        || ht_qos
        || legacy_qos
    {
        Some(7)
    } else {
        None
    }
}

/// Apply the complete mapper transformation admitted by the ordinary strict
/// STA/AP profile before Rust aggregation takes ownership.
///
/// This is the recovered finite `ppMapTxQueue` domain used by authentication,
/// association, EAPOL, Action, and single-MPDU data traffic. It reads only the
/// live frame, descriptor, and peer supplied by the caller and changes exactly
/// descriptor byte four (`0x20 -> 0x07` for a fresh QoS frame, or preserves
/// `0x07`). No PP queue, power-save state, callback, allocation, or wait is
/// reachable.
///
/// # Safety
///
/// `frame` and all pointers reached through its pinned ESF layout must remain
/// valid and exclusively owned for this run-to-completion transformation.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_mapper"]
pub(crate) unsafe fn apply_strict_sta_ap(frame: *mut u8) -> bool {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
    const FRAME_PEER_OFFSET: usize = 0x2c;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const DESCRIPTOR_RATE_OFFSET: usize = 0x0c;

    if frame.is_null() {
        return false;
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let peer = frame.add(FRAME_PEER_OFFSET).cast::<*mut u8>().read();
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() || peer.is_null() || first_buffer.is_null() {
        return false;
    }
    let layout = frame
        .add(FRAME_LAYOUT_FLAGS_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let mut header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if header.is_null() {
        return false;
    }
    if layout & 0x2000 != 0 {
        header = header.add(8);
    }
    let state = [
        descriptor.cast::<u32>().read_unaligned(),
        descriptor.add(4).cast::<u32>().read_unaligned(),
        descriptor.add(0x10).cast::<u32>().read_unaligned(),
        peer.add(0x0c).cast::<u32>().read_unaligned(),
        u32::from(peer.add(0x84).read()),
    ];
    let Some(treatment) = strict_sta_ap_treatment(
        descriptor.add(DESCRIPTOR_RATE_OFFSET).read(),
        layout,
        header.cast::<u16>().read_unaligned(),
        state,
    ) else {
        return false;
    };
    descriptor.add(4).write(treatment);
    true
}

/// Fail closed while preserving the complete mapper input in the trap frame.
///
/// The register contract is intentionally stable for hardware qualification:
/// `a0=frame`, `a1=detail`, `a2=rate|(layout<<8)`,
/// `a3=frame_control|(peer[0x84]<<16)`, and `a4..a7` are the four remaining
/// state words consumed by [`strict_sta_ap_treatment`].
#[cfg(target_arch = "riscv32")]
#[inline(never)]
pub(crate) unsafe fn trap_unadmitted_strict_sta_ap(frame: *mut u8) -> ! {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_LAYOUT_FLAGS_OFFSET: usize = 0x24;
    const FRAME_PEER_OFFSET: usize = 0x2c;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const DESCRIPTOR_RATE_OFFSET: usize = 0x0c;

    #[inline(always)]
    unsafe fn trap(
        frame: *mut u8,
        detail: u32,
        rate_layout: u32,
        frame_control_peer_flag: u32,
        descriptor_flags: u32,
        descriptor_priority: u32,
        descriptor_control: u32,
        peer_state: u32,
    ) -> ! {
        // Invalidate publication while populating the fixed record, then
        // release-publish `detail` last. No formatter, callback, allocation,
        // lock, or retry is entered on this terminal diagnostic path.
        TX_MAPPER_REJECTION.detail.store(0, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .frame
            .store(frame.addr() as u32, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .rate_layout
            .store(rate_layout, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .frame_control_peer_flag
            .store(frame_control_peer_flag, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .descriptor_flags
            .store(descriptor_flags, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .descriptor_priority
            .store(descriptor_priority, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .descriptor_control
            .store(descriptor_control, Ordering::Relaxed);
        TX_MAPPER_REJECTION
            .peer_state
            .store(peer_state, Ordering::Relaxed);
        TX_MAPPER_REJECTION.detail.store(detail, Ordering::Release);
        core::arch::asm!(
            "ebreak",
            in("a0") frame,
            in("a1") detail,
            in("a2") rate_layout,
            in("a3") frame_control_peer_flag,
            in("a4") descriptor_flags,
            in("a5") descriptor_priority,
            in("a6") descriptor_control,
            in("a7") peer_state,
            options(noreturn)
        )
    }

    if frame.is_null() {
        trap(frame, 0x3010, 0, 0, 0, 0, 0, 0);
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    let peer = frame.add(FRAME_PEER_OFFSET).cast::<*mut u8>().read();
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read();
    if descriptor.is_null() || peer.is_null() || first_buffer.is_null() {
        trap(frame, 0x3011, 0, 0, 0, 0, 0, 0);
    }
    let layout = frame
        .add(FRAME_LAYOUT_FLAGS_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let mut header = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read();
    if header.is_null() {
        trap(frame, 0x3012, u32::from(layout) << 8, 0, 0, 0, 0, 0);
    }
    if layout & 0x2000 != 0 {
        header = header.add(8);
    }
    let rate_layout =
        u32::from(descriptor.add(DESCRIPTOR_RATE_OFFSET).read()) | (u32::from(layout) << 8);
    let frame_control_peer_flag =
        u32::from(header.cast::<u16>().read_unaligned()) | (u32::from(peer.add(0x84).read()) << 16);
    trap(
        frame,
        0x3001,
        rate_layout,
        frame_control_peer_flag,
        descriptor.cast::<u32>().read_unaligned(),
        descriptor.add(4).cast::<u32>().read_unaligned(),
        descriptor.add(0x10).cast::<u32>().read_unaligned(),
        peer.add(0x0c).cast::<u32>().read_unaligned(),
    )
}

#[cfg(test)]
mod tests {
    use super::strict_sta_ap_treatment;

    #[test]
    fn admits_every_hardware_observed_strict_class() {
        let cases = [
            (0, 0x2000, 0x00b0, [0, 7, 0, 0x80, 0]),
            (0, 0x2000, 0x0000, [0, 7, 0, 0x80, 0]),
            (0, 0x2000, 0x0188, [0x0200_200c, 7, 0, 0x81, 0]),
            (0, 0x2001, 0x0188, [0x0200_200c, 7, 0, 0x81, 0]),
            (12, 0x2000, 0x0080, [0x0080_0412, 7, 0x0004_0000, 0x83, 0]),
            (12, 0x2001, 0x0080, [0x0080_0412, 7, 0x0004_0000, 0x83, 0]),
            (12, 0x2003, 0x0050, [0x0800_0010, 7, 0x0004_0000, 0x83, 0]),
            (12, 0x2730, 0x00b0, [0, 7, 0x0004_0000, 0x83, 0]),
            (
                11,
                0x2731,
                0x0010,
                [0, 7, 0x0004_0000, 0x2100_0000, 1],
            ),
            (
                11,
                0x2f31,
                0x0010,
                [0, 7, 0x0004_0000, 0x2100_0000, 2],
            ),
            (
                11,
                0x2000,
                0x0288,
                [0x0200_200c, 7, 0x0004_0000, 0x2100_0000, 1],
            ),
            (
                11,
                0x2001,
                0x0288,
                [0x0200_200c, 7, 0x0004_0000, 0x2100_0000, 2],
            ),
            (12, 0x2000, 0x4208, [0x0000_200b, 7, 0x0004_0342, 0x83, 0]),
            (12, 0x2029, 0x6208, [0x0000_200b, 7, 0x0004_0342, 0x83, 0]),
            (
                12,
                0x2003,
                0x4208,
                [0x0200_200b, 7, 0x0004_0342, 0x83, 0],
            ),
            (11, 0x2732, 0x00d0, [0, 7, 0x0004_0000, 0x2100_0000, 1]),
            (11, 0x2f32, 0x00d0, [0, 7, 0x0004_0000, 0x2100_0000, 2]),
            (
                33,
                0x2000,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0348, 0x2100_0000, 1],
            ),
            (
                33,
                0x2001,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0349, 0x2100_0000, 2],
            ),
            (
                11,
                0x2002,
                0x4288,
                [0x0200_2009, 7, 0x0004_0348, 0x2100_0000, 1],
            ),
            (
                11,
                0x2003,
                0x4288,
                [0x0200_2009, 7, 0x0004_0349, 0x2100_0000, 2],
            ),
            (0, 0x2001, 0x00d0, [0, 7, 0, 0x81, 0]),
            (33, 0x2002, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 0]),
            (33, 0x2003, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 0]),
            (0, 0x2004, 0x4188, [0x0200_2009, 7, 0x304, 0x81, 0]),
            (0, 0x2005, 0x4188, [0x0200_2009, 7, 0x304, 0x81, 0]),
            (0, 0x2002, 0x00d0, [0, 7, 0, 0x81, 0]),
            (0, 0x2006, 0x4188, [0x0200_2009, 7, 0x304, 0x81, 0]),
            (0, 0x2007, 0x4188, [0x0200_2009, 7, 0x304, 0x81, 0]),
            (33, 0x2000, 0x4188, [0x0000_2009, 0x20, 0x304, 0x81, 0]),
            (
                33,
                0x2008,
                0x4188,
                [0x0000_2009, 0x00ab_cd20, 0x304, 0x81, 0],
            ),
            (33, 0x2010, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 0]),
            (0, 0x2003, 0x00d0, [0, 7, 0, 0x81, 0]),
        ];
        for (rate, layout, frame_control, state) in cases {
            assert_eq!(
                strict_sta_ap_treatment(rate, layout, frame_control, state),
                Some(7)
            );
        }
    }

    #[test]
    fn rejects_unobserved_queue_peer_rate_and_frame_classes() {
        assert_eq!(
            strict_sta_ap_treatment(33, 0x2000, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 1]),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(35, 0x2000, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 0]),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(0, 0x2000, 0x0080, [0, 7, 0, 0x80, 0]),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(12, 0x2000, 0x0080, [0x0080_0412, 7, 0x0004_0000, 0x80, 0],),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(12, 0x2003, 0x0050, [0x0800_0010, 7, 0x0004_0000, 0x80, 0],),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(12, 0x2730, 0x00b0, [0, 7, 0x0004_0000, 0x80, 0],),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2731,
                0x0010,
                [0, 7, 0x0004_0000, 0x2100_0000, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2f31,
                0x0010,
                [0, 7, 0x0004_0000, 0x2100_0000, 3],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2000,
                0x0288,
                [0x0200_200c, 7, 0x0004_0000, 0x2100_0000, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                12,
                0x2003,
                0x4208,
                [0x0100_200b, 7, 0x0004_0342, 0x83, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                12,
                0x2000,
                0x4288,
                [0x0000_200b, 7, 0x0004_0342, 0x83, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2732,
                0x00d0,
                [0, 7, 0x0004_0000, 0x2100_0000, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2000,
                0x4288,
                [0x0000_2009, 7, 0x0004_0348, 0x2100_0000, 1],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2002,
                0x4288,
                [0x0100_2009, 7, 0x0004_0348, 0x2100_0000, 1],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                11,
                0x2002,
                0x4288,
                [0x0200_2009, 0x20, 0x0004_0348, 0x2100_0000, 1],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2001,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0348, 0x2100_0000, 3],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2001,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0348, 0x2100_0000, 2],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2000,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0349, 0x2100_0000, 1],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2000,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0342, 0x2100_0000, 1],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(
                33,
                0x2000,
                0x4288,
                [0x0000_2009, 0x20, 0x0004_0348, 0x2100_0000, 0],
            ),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(0, 0x4000, 0x00b0, [0, 7, 0, 0x80, 0]),
            None
        );
        assert_eq!(
            strict_sta_ap_treatment(33, 0x0000, 0x4188, [0x0000_2009, 7, 0x304, 0x81, 0]),
            None
        );
    }
}
