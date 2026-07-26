#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxSecurityLayoutInput {
    pub header_len: u16,
    pub remaining_len: u16,
    pub layout: u16,
    pub buffer_flags: u32,
    pub descriptor_flags: u32,
    pub descriptor_security: u32,
    pub frame_control: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxSecurityLayoutOutput {
    pub header_len: u16,
    pub remaining_len: u16,
    pub layout: u16,
    pub buffer_flags: u32,
    pub metadata_len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApBeaconCompletionLayout {
    pub remaining_len: u16,
    pub buffer_flags: u32,
    pub descriptor_security: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistentFrameCompletionLayout {
    pub header_len: u16,
    pub remaining_len: u16,
    pub layout: u16,
    pub buffer_flags: u32,
    pub descriptor_flags: u32,
    pub descriptor_security: u32,
}

const AP_GROUP_MAX_MPDU_LEN: u16 = crate::data_tx::WIFI_DATA_TX_FRAME_CAPACITY as u16 + 18;
const AP_PAIRWISE_MAX_MPDU_LEN: u16 = crate::data_tx::WIFI_DATA_TX_FRAME_CAPACITY as u16 + 20;
// Must match the fixed strict management slot in `esf.rs`. Persistent
// management/beacon completion never accepts a transmitted object beyond it.
const MANAGEMENT_PAYLOAD_CAPACITY: u16 = 1600;

const fn is_protected_ap_group_data(input: TxSecurityLayoutInput) -> bool {
    if input.frame_control & !0x2000 != 0x4208
        || !crate::tx_proto::is_ap_group_ccmp_descriptor(input.descriptor_flags)
        || input.descriptor_security != 0x0004_0342
        || input.header_len != 0x0018
        || input.remaining_len < 8
        || input.layout & 0xe000 != 0
    {
        return false;
    }
    let mpdu_len = match input.header_len.checked_add(input.remaining_len) {
        Some(value) if value <= AP_GROUP_MAX_MPDU_LEN => value,
        _ => return false,
    };
    input.buffer_flags
        == 0xc000_0000 | (mpdu_len as u32) << 14 | (mpdu_len as u32 + 14)
}

const fn is_protected_ap_pairwise_data(input: TxSecurityLayoutInput) -> bool {
    if input.frame_control != 0x4288
        || !crate::tx_proto::is_ap_pairwise_ccmp_descriptor(input.descriptor_flags)
        || !matches!(input.descriptor_security, 0x0004_0348 | 0x0004_0349)
        || input.header_len != 0x001a
        || input.remaining_len < 8
        || input.layout & 0xe000 != 0
    {
        return false;
    }
    let mpdu_len = match input.header_len.checked_add(input.remaining_len) {
        Some(value) if value <= AP_PAIRWISE_MAX_MPDU_LEN => value,
        _ => return false,
    };
    input.buffer_flags
        == 0xc000_0000 | (mpdu_len as u32) << 14 | (mpdu_len as u32 + 12)
}

pub(crate) const fn strict_ap_group_power_save_completion(
    frame_control: u16,
    header_len: u16,
    remaining_len: u16,
    layout: u16,
    buffer_flags: u32,
    descriptor_flags: u32,
    descriptor_security: u32,
) -> bool {
    if frame_control & !(0x0800 | 0x2000) != 0x4208
        || !crate::tx_proto::is_ap_group_ccmp_descriptor(descriptor_flags)
        || !matches!(
            descriptor_security,
            0x0104_0342 | 0x0114_0342 | 0x0404_0342 | 0x0414_0342
        )
        || header_len != 0x0020
        || remaining_len < 20
        || layout & 0xe000 != 0x2000
    {
        return false;
    }
    let output_len = match header_len.checked_add(remaining_len) {
        Some(value) if value <= AP_GROUP_MAX_MPDU_LEN + 20 => value,
        _ => return false,
    };
    buffer_flags
        == 0xc000_0000 | (output_len as u32) << 14 | (output_len as u32 - 6)
}

pub(crate) const fn strict_ap_pairwise_power_save_completion(
    frame_control: u16,
    header_len: u16,
    remaining_len: u16,
    layout: u16,
    buffer_flags: u32,
    descriptor_flags: u32,
    descriptor_security: u32,
) -> bool {
    if frame_control & !0x0800 != 0x4288
        || descriptor_flags & !(0x0200_0000 | 0x0000_1100) != 0x0000_2009
        || !matches!(
            descriptor_security,
            0x0104_0348
                | 0x0104_0349
                | 0x0114_0348
                | 0x01a4_0348
                | 0x0204_0348
                | 0x0214_0348
                | 0x02a4_0348
                | 0x0404_0348
                | 0x0404_0349
                | 0x0414_0348
                | 0x04a4_0348
        )
        || header_len != 0x0022
        || remaining_len < 20
        || layout & 0xe000 != 0x2000
    {
        return false;
    }
    let output_len = match header_len.checked_add(remaining_len) {
        Some(value) if value <= AP_PAIRWISE_MAX_MPDU_LEN + 20 => value,
        _ => return false,
    };
    buffer_flags
        == 0xc000_0000 | (output_len as u32) << 14 | (output_len as u32 - 8)
}

/// Restore one retained plaintext management or beacon buffer after TX done.
///
/// The pinned `ppProcTxDone` branch removes the four-byte FCS reservation and
/// the one-transmission eight-byte PP metadata prefix, then leaves the ESF
/// owned by net80211 instead of recycling it. AP probe/authentication/
/// association replies use this path so their cached fixed-pool object can be
/// submitted again. An initialization beacon can also already be present on
/// the ordinary TX-done list when the Rust owner takes over.
pub const fn strict_persistent_frame_completion_layout(
    input: TxSecurityLayoutInput,
) -> Option<PersistentFrameCompletionLayout> {
    const BUFFER_LENGTH_MASK: u32 = 0x0fff_c000;
    const PERSISTENT_BIT: u32 = 0x0080_0000;

    let subtype = input.frame_control & 0x00f0;
    let management_reply = input.frame_control & 0x000c == 0
        && matches!(subtype, 0x0010 | 0x0030 | 0x0050 | 0x00b0)
        && (input.descriptor_flags == PERSISTENT_BIT
            || input.descriptor_flags == PERSISTENT_BIT | 0x0000_0412)
        && matches!(input.descriptor_security, 0 | 0x0114_0000);
    let beacon = input.frame_control == 0x0080
        && input.descriptor_flags == PERSISTENT_BIT | 0x0000_0412
        && matches!(
            input.descriptor_security,
            // The strict B/G/N AP HIL completes its first 204-byte beacon
            // with hardware-success bit 24 plus the fixed AP selector.  The
            // older 0x0114/0x0414 states remain measured vendor completion
            // variants, but must not be required for this direct LMAC path.
            0x0104_0000 | 0x0114_0000 | 0x0404_0000 | 0x0414_0000
        )
        && input.header_len == 0x20;
    if input.frame_control & 0x000c != 0
        || !(management_reply || beacon)
        || input.header_len < 8
        || input.remaining_len < 4
        || input.layout & 0xe000 != 0x2000
    {
        return None;
    }
    let encoded_len = ((input.buffer_flags & BUFFER_LENGTH_MASK) >> 14) as u16;
    let transmitted_len = match input.header_len.checked_add(input.remaining_len) {
        Some(value) => value,
        None => return None,
    };
    if encoded_len != transmitted_len || transmitted_len > MANAGEMENT_PAYLOAD_CAPACITY {
        return None;
    }
    let restored_len = match encoded_len.checked_sub(12) {
        Some(value) => value,
        None => return None,
    };

    Some(PersistentFrameCompletionLayout {
        header_len: input.header_len - 8,
        remaining_len: input.remaining_len - 4,
        layout: input.layout & !0x2000,
        buffer_flags: (input.buffer_flags & !BUFFER_LENGTH_MASK) | ((restored_len as u32) << 14),
        descriptor_flags: input.descriptor_flags & !PERSISTENT_BIT,
        // Queue, ownership and direct-recycle bits in this word describe the
        // completed submission, not the retained object's next transmission.
        // A beacon keeps only its fixed hardware-key direction selector;
        // ordinary retained management replies return to plaintext base state.
        descriptor_security: if beacon { 0x0004_0000 } else { 0 },
    })
}

/// Remove the per-transmission FCS reservation from a persistent AP beacon
/// while retaining its one-time PP metadata headroom.
pub const fn strict_ap_beacon_completion_layout(
    input: TxSecurityLayoutInput,
) -> Option<ApBeaconCompletionLayout> {
    const BUFFER_LENGTH_MASK: u32 = 0x0fff_c000;

    if input.header_len != 0x20
        || input.remaining_len != 0x78
        || input.layout & 0xc000 != 0
        || input.layout & 0x2000 == 0
        || input.descriptor_flags != 0x0080_0412
        || input.descriptor_security != 0x0114_0000
        || input.frame_control != 0x0080
    {
        return None;
    }
    let encoded_len = ((input.buffer_flags & BUFFER_LENGTH_MASK) >> 14) as u16;
    if encoded_len != 0x98 {
        return None;
    }

    Some(ApBeaconCompletionLayout {
        remaining_len: 0x74,
        buffer_flags: (input.buffer_flags & !BUFFER_LENGTH_MASK) | (0x94 << 14),
        descriptor_security: 0x0004_0000,
    })
}

/// Recover the complete headroom/trailer transformation observed at the
/// `ppProcTxSecFrame` boundary. This is deliberately a closed set: plaintext
/// management/EAPOL/Action frames, the AP beacon descriptor, and the measured
/// WPA2-CCMP QoS descriptor states are admitted; any new descriptor state must
/// be measured first.
pub const fn strict_tx_security_layout(
    input: TxSecurityLayoutInput,
) -> Option<TxSecurityLayoutOutput> {
    const BUFFER_LENGTH_MASK: u32 = 0x0fff_c000;
    const BUFFER_TERMINAL: u32 = 0x4000_0000;

    let headroom_applied = input.layout & 0x2000 != 0;

    let ap_beacon = input.frame_control == 0x0080
        && input.descriptor_flags == 0x0080_0412
        && input.descriptor_security == 0x0004_0000;
    let persistent_management_reply = matches!(input.descriptor_security, 0 | 0x0004_0000)
        && matches!(input.frame_control, 0x0010 | 0x0030 | 0x0050 | 0x00b0)
        && matches!(input.descriptor_flags, 0x0080_0000 | 0x0080_0412);
    let transient_probe_response = matches!(input.descriptor_security, 0 | 0x0004_0000)
        && input.frame_control == 0x0050
        && input.descriptor_flags == 0x0800_0010;
    let transient_authentication_response = input.frame_control == 0x00b0
        && input.descriptor_flags == 0
        && input.descriptor_security == 0x0004_0000;
    let transient_association_response = matches!(input.frame_control, 0x0010 | 0x0030)
        && input.descriptor_flags == 0
        && input.descriptor_security == 0x0004_0000;
    let transient_deauthentication = input.frame_control == 0x00c0
        && matches!(
            (input.descriptor_flags, input.descriptor_security),
            (0x0000_0010, 0 | 0x0004_0000) | (0x0800_0000, 0x0004_0000)
        );
    let transient_ap_addba_response = input.frame_control == 0x00d0
        && input.descriptor_flags == 0
        && input.descriptor_security == 0x0004_0000
        && input.header_len == 0x0018
        && input.remaining_len == 0x0009
        // The low twelve bits mirror the 802.11 sequence number. Only the
        // upper layout flags are structural; pre-security ADDBA responses
        // have no headroom or reserved layout flags set.
        && input.layout & 0xf000 == 0
        && input.buffer_flags == 0xc008_402c;
    let transient_ap_eapol = input.frame_control == 0x0288
        && input.descriptor_flags == 0x0200_200c
        && input.descriptor_security == 0x0004_0000
        && input.header_len == 0x001a
        && ((input.remaining_len == 0x006b
            && input.layout == 0
            && input.buffer_flags == 0xc021_4099)
            || (input.remaining_len == 0x00a3
                && input.layout == 1
                && input.buffer_flags == 0xc02f_40d1));
    let transient_ap_plaintext_data = input.frame_control == 0x0208
        && input.descriptor_flags == 0x0000_200a
        && input.descriptor_security == 0x0004_0000
        && input.header_len == 0x0018
        && input.remaining_len == 0x0054
        && input.layout == 0
        && input.buffer_flags == 0xc01b_0082;
    let protected_ap_group_data = is_protected_ap_group_data(input);
    let protected_ap_pairwise_data = is_protected_ap_pairwise_data(input);
    let trailer_len = if (input.descriptor_security == 0
        && ((matches!(input.frame_control, 0x00b0 | 0x0000 | 0x00d0)
            && input.descriptor_flags == 0)
            || (input.frame_control == 0x0188 && input.descriptor_flags == 0x0200_200c)))
        || persistent_management_reply
        || transient_probe_response
        || transient_authentication_response
        || transient_association_response
        || transient_deauthentication
        || transient_ap_addba_response
        || transient_ap_eapol
        || transient_ap_plaintext_data
        || ap_beacon
    {
        // AP beacons carry the pinned hardware-key direction word even though
        // the 802.11 Protected bit is clear. The first strict AP bring-up
        // trapped this exact descriptor tuple before any state was mutated.
        4_u16
    } else if (input.frame_control == 0x4188
        && matches!(input.descriptor_flags, 0x0000_2009 | 0x0200_2009)
        && input.descriptor_security == 0x0000_0304)
        || protected_ap_group_data
        || protected_ap_pairwise_data
    {
        // The security selector in bits 8..11 is 3. The pinned vendor table
        // maps that selector to the eight-byte CCMP MIC plus four-byte FCS.
        12_u16
    } else {
        return None;
    };

    // The two hostap beacon buffers are persistent. Their constructor normally
    // retains both the PP headroom and its length accounting. Peer removal was
    // also measured resetting the length fields to the base 24-byte header
    // while retaining the installed-headroom layout bit and physical pointer.
    // Admit both finite states; ordinary management, EAPOL and data buffers
    // remain single-owner, one-shot objects.
    let beacon_lengths_reset = headroom_applied
        && ap_beacon
        && input.header_len == 0x18
        && input.remaining_len == 0x74;
    if headroom_applied
        && (!ap_beacon
            || input.layout & 0xc000 != 0
            || (!beacon_lengths_reset
                && (input.header_len != 0x20 || input.remaining_len != 0x74)))
    {
        return None;
    }

    let payload_len = match input.header_len.checked_add(input.remaining_len) {
        Some(value) => value,
        None => return None,
    };
    let encoded_len = ((input.buffer_flags & BUFFER_LENGTH_MASK) >> 14) as u16;
    if encoded_len != payload_len {
        return None;
    }

    let header_len = if headroom_applied && !beacon_lengths_reset {
        input.header_len
    } else {
        match input.header_len.checked_add(8) {
            Some(value) => value,
            None => return None,
        }
    };
    let remaining_len = match input.remaining_len.checked_add(trailer_len) {
        Some(value) => value,
        None => return None,
    };
    let headroom_len = if headroom_applied && !beacon_lengths_reset {
        0
    } else {
        8
    };
    let buffer_len = match encoded_len.checked_add(headroom_len) {
        Some(value) => match value.checked_add(trailer_len) {
            Some(value) if value <= 0x3fff => value,
            _ => return None,
        },
        None => return None,
    };
    let metadata_len = header_len as u32 + remaining_len as u32 - 8;

    Some(TxSecurityLayoutOutput {
        header_len,
        remaining_len,
        layout: input.layout | 0x2000,
        buffer_flags: (input.buffer_flags & !BUFFER_LENGTH_MASK)
            | ((buffer_len as u32) << 14)
            | BUFFER_TERMINAL,
        metadata_len,
    })
}

/// Return the LLC offset for one measured AP EAPOL completion carrying the
/// stock hostap power-save callback bit.
///
/// These are the exact post-security-layout values produced for WPA2 messages
/// one and three. Keeping this leaf closed prevents callback slot 12 from
/// silently admitting ordinary power-save data, whose TIM/queue state is not
/// owned by the strict Rust runtime.
pub(crate) const fn strict_ap_eapol_power_save_completion_llc_offset(
    frame_control: u16,
    header_len: u16,
    remaining_len: u16,
    layout: u16,
) -> Option<usize> {
    // A naturally retransmitted M1/M3 carries the IEEE 802.11 Retry bit.
    // It has the same callback contract and buffer layout as the first
    // attempt, so classify it without admitting any other FC mutation.
    if frame_control & !0x0800 == 0x0288
        && header_len == 0x22
        && remaining_len == 0x6f
        && layout == 0x2000
    {
        Some(0x1a)
    } else if frame_control & !0x0800 == 0x0288
        && header_len == 0x22
        && remaining_len == 0xa7
        && layout == 0x2001
    {
        Some(0x1a)
    } else {
        None
    }
}

/// SRAM-resident, allocation-free replacement for the measured plaintext and
/// WPA2-CCMP branches of the vendor `ppProcTxSecFrame` leaf.
///
/// # Safety
///
/// `frame` must be the live single-buffer TX frame supplied by `ppTxPkt`.
/// Its frame, buffer and descriptor storage must remain exclusively owned by
/// the caller for the duration of this run-to-completion transformation.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_security"]
pub unsafe extern "C" fn strict_pp_proc_tx_sec_frame(frame: *mut u8) -> i32 {
    const FRAME_FIRST_BUFFER_OFFSET: usize = 0x04;
    const FRAME_TAIL_BUFFER_OFFSET: usize = 0x08;
    const FRAME_LENGTHS_OFFSET: usize = 0x14;
    const FRAME_LAYOUT_OFFSET: usize = 0x24;
    const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
    const BUFFER_DATA_OFFSET: usize = 0x04;
    const DESCRIPTOR_SECURITY_OFFSET: usize = 0x10;

    if frame.is_null() {
        trap_invalid_tx_security();
    }
    let first_buffer = frame
        .add(FRAME_FIRST_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    let tail_buffer = frame
        .add(FRAME_TAIL_BUFFER_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    let descriptor = frame
        .add(FRAME_DESCRIPTOR_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    if first_buffer.is_null() || first_buffer != tail_buffer || descriptor.is_null() {
        trap_invalid_tx_security();
    }
    let data = first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .read_unaligned();
    if data.is_null() || data.addr() < 8 {
        trap_invalid_tx_security();
    }

    let lengths = frame
        .add(FRAME_LENGTHS_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    let layout = frame
        .add(FRAME_LAYOUT_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    let header = if layout & 0x2000 != 0 {
        data.add(8)
    } else {
        data
    };
    let input = TxSecurityLayoutInput {
        header_len: lengths as u16,
        remaining_len: (lengths >> 16) as u16,
        layout,
        buffer_flags: first_buffer.cast::<u32>().read_unaligned(),
        descriptor_flags: descriptor.cast::<u32>().read_unaligned(),
        descriptor_security: descriptor
            .add(DESCRIPTOR_SECURITY_OFFSET)
            .cast::<u32>()
            .read_unaligned(),
        frame_control: header.cast::<u16>().read_unaligned(),
    };
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::SecurityInput,
        frame,
        descriptor,
        input.frame_control,
        u8::MAX,
        0,
        lengths,
        u32::from(input.layout),
        input.buffer_flags,
    );
    let output = match strict_tx_security_layout(input) {
        Some(value) => value,
        None => {
            #[cfg(feature = "hil-vendor-tx")]
            {
                #[cfg(feature = "hil-tx-deep-telemetry")]
                crate::tx_trace::record_descriptor_transition(
                    crate::tx_trace::TxTraceEvent::SecurityRejected,
                    frame,
                    descriptor,
                    input.frame_control,
                    u8::MAX,
                    0,
                    lengths,
                    u32::from(input.layout),
                    input.buffer_flags,
                );
                #[cfg(feature = "hil-tx-deep-telemetry")]
                crate::tx_trace::freeze_tx_trace();
                record_hil_rejected_tx_security(input);
                return -1;
            }
            #[cfg(not(feature = "hil-vendor-tx"))]
            trap_invalid_tx_security_layout(input)
        }
    };

    // No packet state is mutated until every pointer and recovered invariant
    // above has been checked. Unknown states therefore trap transactionally.
    // Keep the admitted mutation order identical to the pinned leaf because
    // the frame is shared with the TX interrupt path once preparation starts.
    const BUFFER_LENGTH_MASK: u32 = 0x0fff_c000;
    const BUFFER_TERMINAL: u32 = 0x4000_0000;
    let encoded_len = ((input.buffer_flags & BUFFER_LENGTH_MASK) >> 14) as u16;
    let security_len = output.remaining_len - input.remaining_len + encoded_len;
    let security_flags =
        (input.buffer_flags & !BUFFER_LENGTH_MASK) | (u32::from(security_len) << 14);

    frame
        .add(FRAME_LENGTHS_OFFSET + 2)
        .cast::<u16>()
        .write_unaligned(output.remaining_len);
    tail_buffer.cast::<u32>().write_unaligned(security_flags);
    tail_buffer
        .cast::<u32>()
        .write_unaligned(security_flags | BUFFER_TERMINAL);

    let metadata = if input.layout & 0x2000 != 0 {
        data
    } else {
        data.sub(8)
    };
    first_buffer
        .add(BUFFER_DATA_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(metadata);
    frame
        .add(FRAME_LENGTHS_OFFSET)
        .cast::<u16>()
        .write_unaligned(output.header_len);
    frame
        .add(FRAME_LAYOUT_OFFSET)
        .cast::<u16>()
        .write_unaligned(output.layout);
    first_buffer
        .cast::<u32>()
        .write_unaligned(output.buffer_flags);

    metadata.cast::<u32>().write_unaligned(0);
    metadata.add(4).cast::<u32>().write_unaligned(0);
    metadata.cast::<u32>().write_unaligned(output.metadata_len);
    #[cfg(feature = "hil-tx-deep-telemetry")]
    crate::tx_trace::record_descriptor_transition(
        crate::tx_trace::TxTraceEvent::SecurityPrepared,
        frame,
        descriptor,
        input.frame_control,
        u8::MAX,
        0,
        u32::from(output.header_len) | (u32::from(output.remaining_len) << 16),
        u32::from(output.layout),
        output.buffer_flags,
    );
    0
}

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
fn record_hil_rejected_tx_security(input: TxSecurityLayoutInput) {
    HIL_REJECTED_DESCRIPTOR_FLAGS.store(input.descriptor_flags, Ordering::Release);
    HIL_REJECTED_DESCRIPTOR_SECURITY.store(input.descriptor_security, Ordering::Release);
    HIL_REJECTED_FRAME_CONTROL.store(u32::from(input.frame_control), Ordering::Release);
    HIL_REJECTED_LENGTHS.store(
        u32::from(input.header_len) | (u32::from(input.remaining_len) << 16),
        Ordering::Release,
    );
    HIL_REJECTED_LAYOUT.store(u32::from(input.layout), Ordering::Release);
    HIL_REJECTED_BUFFER_FLAGS.store(input.buffer_flags, Ordering::Release);
    HIL_REJECTED_COUNT.fetch_add(1, Ordering::AcqRel);
    unsafe {
        ets_printf(
            c"HIL TX security reject: df=%08x ds=%08x fc=%04x len=%04x:%04x layout=%04x buffer=%08x\r\n"
                .as_ptr()
                .cast(),
            input.descriptor_flags,
            input.descriptor_security,
            u32::from(input.frame_control),
            u32::from(input.remaining_len),
            u32::from(input.header_len),
            u32::from(input.layout),
            input.buffer_flags,
        );
    }
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn trap_invalid_tx_security() -> ! {
    core::arch::asm!("ebreak", options(noreturn))
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn trap_invalid_tx_security_layout(input: TxSecurityLayoutInput) -> ! {
    let lengths = u32::from(input.header_len) | (u32::from(input.remaining_len) << 16);
    core::arch::asm!(
        "ebreak",
        in("a0") input.descriptor_flags,
        in("a1") input.descriptor_security,
        in("a2") u32::from(input.frame_control),
        in("a3") lengths,
        in("a4") u32::from(input.layout),
        in("a5") input.buffer_flags,
        options(noreturn)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        strict_ap_beacon_completion_layout, strict_ap_group_power_save_completion,
        strict_ap_pairwise_power_save_completion,
        strict_ap_eapol_power_save_completion_llc_offset,
        strict_persistent_frame_completion_layout, strict_tx_security_layout,
        ApBeaconCompletionLayout, PersistentFrameCompletionLayout, TxSecurityLayoutInput,
        TxSecurityLayoutOutput, MANAGEMENT_PAYLOAD_CAPACITY,
    };

    const fn input(
        lengths: u32,
        layout: u16,
        buffer_flags: u32,
        descriptor_flags: u32,
        frame_control: u16,
    ) -> TxSecurityLayoutInput {
        TxSecurityLayoutInput {
            header_len: lengths as u16,
            remaining_len: (lengths >> 16) as u16,
            layout,
            buffer_flags,
            descriptor_flags,
            descriptor_security: 0,
            frame_control,
        }
    }

    #[test]
    fn reproduces_all_hardware_observed_plaintext_layouts() {
        let cases = [
            (
                input(0x0062_0018, 0, 0xc01e_8084, 0, 0x00b0),
                TxSecurityLayoutOutput {
                    header_len: 0x20,
                    remaining_len: 0x66,
                    layout: 0x2000,
                    buffer_flags: 0xc021_8084,
                    metadata_len: 0x7e,
                },
            ),
            (
                input(0x006b_001a, 1, 0xc021_4099, 0x0200_200c, 0x0188),
                TxSecurityLayoutOutput {
                    header_len: 0x22,
                    remaining_len: 0x6f,
                    layout: 0x2001,
                    buffer_flags: 0xc024_4099,
                    metadata_len: 0x89,
                },
            ),
            (
                input(0x0009_0018, 2, 0xc008_402c, 0, 0x00d0),
                TxSecurityLayoutOutput {
                    header_len: 0x20,
                    remaining_len: 0x0d,
                    layout: 0x2002,
                    buffer_flags: 0xc00b_402c,
                    metadata_len: 0x25,
                },
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(strict_tx_security_layout(input), Some(expected));
        }
    }

    #[test]
    fn reproduces_retained_ap_management_reply_layouts() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0069_0018, 0x006c, 0xc020_4084, 0x0080_0000, 0x0050)
        };
        let expected = TxSecurityLayoutOutput {
            header_len: 0x20,
            remaining_len: 0x6d,
            layout: 0x206c,
            buffer_flags: 0xc023_4084,
            metadata_len: 0x85,
        };
        assert_eq!(strict_tx_security_layout(measured), Some(expected));

        for frame_control in [0x0010, 0x0030, 0x0050, 0x00b0] {
            assert_eq!(
                strict_tx_security_layout(TxSecurityLayoutInput {
                    frame_control,
                    ..measured
                }),
                Some(expected),
            );
        }
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_flags: 0x0080_0412,
                ..measured
            }),
            Some(expected),
        );
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_security: 0,
                ..measured
            }),
            Some(expected),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x0040,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0x0080_0001,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 1,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_transient_ap_probe_response_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0,
            ..input(0x006d_0018, 0x016d, 0xc021_4084, 0x0800_0010, 0x0050)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x71,
                layout: 0x216d,
                buffer_flags: 0xc024_4084,
                metadata_len: 0x89,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x00b0,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0x0800_0011,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 1,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_transient_ap_authentication_response_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0006_0018, 0x0730, 0xc007_8028, 0, 0x00b0)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x0a,
                layout: 0x2730,
                buffer_flags: 0xc00a_8028,
                metadata_len: 0x22,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x0050,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 1,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0008_0000,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_transient_ap_association_response_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0033_0018, 0x0732, 0xc012_c0d0, 0, 0x0010)
        };
        let expected = TxSecurityLayoutOutput {
            header_len: 0x20,
            remaining_len: 0x37,
            layout: 0x2732,
            buffer_flags: 0xc015_c0d0,
            metadata_len: 0x4f,
        };
        assert_eq!(strict_tx_security_layout(measured), Some(expected));
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                frame_control: 0x0030,
                ..measured
            }),
            Some(expected),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x0020,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 1,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0008_0000,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_only_the_measured_transient_ap_addba_response_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0009_0018, 0x0732, 0xc008_402c, 0, 0x00d0)
        };
        for layout in [0, 0x0732, 0x0733, 0x0734, 0x0fff] {
            assert_eq!(
                strict_tx_security_layout(TxSecurityLayoutInput {
                    layout,
                    ..measured
                }),
                Some(TxSecurityLayoutOutput {
                    header_len: 0x20,
                    remaining_len: 0x0d,
                    layout: layout | 0x2000,
                    buffer_flags: 0xc00b_402c,
                    metadata_len: 0x25,
                }),
            );
        }
        for rejected in [
            TxSecurityLayoutInput {
                remaining_len: 8,
                ..measured
            },
            TxSecurityLayoutInput {
                layout: 0x1000,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 1,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_transient_ap_deauthentication_layout() {
        let measured = input(0x0002_0018, 0x00e9, 0xc006_8084, 0x0000_0010, 0x00c0);
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x06,
                layout: 0x20e9,
                buffer_flags: 0xc009_8084,
                metadata_len: 0x1e,
            }),
        );
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_flags: 0x0800_0000,
                descriptor_security: 0x0004_0000,
                ..measured
            }),
            strict_tx_security_layout(measured),
        );
        assert_eq!(
            strict_tx_security_layout(
                input(0x0002_0018, 0x0428, 0xc006_8024, 0x0000_0010, 0x00c0,)
            ),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x06,
                layout: 0x2428,
                buffer_flags: 0xc009_8024,
                metadata_len: 0x1e,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x00a0,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0x0000_0010,
                descriptor_security: 0x0008_0000,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_transient_ap_message1_eapol_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x006b_001a, 0, 0xc021_4099, 0x0200_200c, 0x0288)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x22,
                remaining_len: 0x6f,
                layout: 0x2000,
                buffer_flags: 0xc024_4099,
                metadata_len: 0x89,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x0188,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0,
                ..measured
            },
            TxSecurityLayoutInput {
                remaining_len: 0x006a,
                ..measured
            },
            TxSecurityLayoutInput {
                buffer_flags: 0xc021_0099,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn restores_retained_management_and_beacon_layouts() {
        let completed = TxSecurityLayoutInput {
            descriptor_security: 0x0114_0000,
            ..input(0x0066_0020, 0x2000, 0xc021_8084, 0x0080_0412, 0x00b0)
        };
        assert_eq!(
            strict_persistent_frame_completion_layout(completed),
            Some(PersistentFrameCompletionLayout {
                header_len: 0x18,
                remaining_len: 0x62,
                layout: 0,
                buffer_flags: 0xc01e_8084,
                descriptor_flags: 0x0000_0412,
                descriptor_security: 0,
            })
        );
        assert_eq!(
            strict_persistent_frame_completion_layout(TxSecurityLayoutInput {
                descriptor_security: 0,
                ..completed
            }),
            strict_persistent_frame_completion_layout(completed),
        );

        for rejected in [
            TxSecurityLayoutInput {
                layout: 0,
                ..completed
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0x0080_0410,
                ..completed
            },
        ] {
            assert_eq!(strict_persistent_frame_completion_layout(rejected), None);
        }

        let beacon = TxSecurityLayoutInput {
            descriptor_security: 0x0114_0000,
            ..input(0x0078_0020, 0x2000, 0xc026_00f8, 0x0080_0412, 0x0080)
        };
        assert_eq!(
            strict_persistent_frame_completion_layout(beacon),
            Some(PersistentFrameCompletionLayout {
                header_len: 0x18,
                remaining_len: 0x74,
                layout: 0,
                buffer_flags: 0xc023_00f8,
                descriptor_flags: 0x0000_0412,
                descriptor_security: 0x0004_0000,
            })
        );
        assert_eq!(
            strict_persistent_frame_completion_layout(TxSecurityLayoutInput {
                descriptor_security: 0x0414_0000,
                layout: 0x22db,
                ..beacon
            }),
            Some(PersistentFrameCompletionLayout {
                header_len: 0x18,
                remaining_len: 0x74,
                layout: 0x02db,
                buffer_flags: 0xc023_00f8,
                descriptor_flags: 0x0000_0412,
                descriptor_security: 0x0004_0000,
            }),
        );

        // Strict AP direct-LMAC HIL, B/G/N profile: the 204-byte transmitted
        // beacon carries no 0x0010_0000 queue-state bit at completion.
        let direct_lmac_beacon = TxSecurityLayoutInput {
            remaining_len: 0x00ac,
            buffer_flags: 0xc033_02f8,
            descriptor_security: 0x0104_0000,
            ..beacon
        };
        assert_eq!(
            strict_persistent_frame_completion_layout(direct_lmac_beacon),
            Some(PersistentFrameCompletionLayout {
                header_len: 0x18,
                remaining_len: 0x00a8,
                layout: 0,
                buffer_flags: 0xc030_02f8,
                descriptor_flags: 0x0000_0412,
                descriptor_security: 0x0004_0000,
            }),
        );
        // The direct LMAC ACK-timeout branch changes only hardware status
        // bits 24/26. Persistent ownership and reversible frame geometry are
        // identical, so completion must return the same retained base object
        // instead of terminating the radio owner.
        assert_eq!(
            strict_persistent_frame_completion_layout(TxSecurityLayoutInput {
                descriptor_security: 0x0404_0000,
                ..direct_lmac_beacon
            }),
            strict_persistent_frame_completion_layout(direct_lmac_beacon),
        );

        // WPA2 + HT/HE capability IEs make the strict bgnax beacon longer
        // than the original legacy oracle. The reversible descriptor and
        // fixed-pool bounds, rather than one SSID/profile-specific length,
        // define the safe persistent completion.
        let extended_beacon = TxSecurityLayoutInput {
            remaining_len: 0x00da,
            buffer_flags: 0xc03e_82f8,
            ..beacon
        };
        assert_eq!(
            strict_persistent_frame_completion_layout(extended_beacon),
            Some(PersistentFrameCompletionLayout {
                header_len: 0x18,
                remaining_len: 0x00d6,
                layout: 0,
                buffer_flags: 0xc03b_82f8,
                descriptor_flags: 0x0000_0412,
                descriptor_security: 0x0004_0000,
            }),
        );
        assert_eq!(
            strict_persistent_frame_completion_layout(TxSecurityLayoutInput {
                remaining_len: MANAGEMENT_PAYLOAD_CAPACITY as u16,
                buffer_flags: 0xc000_0000
                    | ((MANAGEMENT_PAYLOAD_CAPACITY as u32 + 0x20) << 14),
                ..beacon
            }),
            None,
        );
    }

    #[test]
    fn reproduces_hardware_observed_wpa2_ccmp_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0304,
            ..input(0x0048_001a, 2, 0xc018_806e, 0x0000_2009, 0x4188)
        };
        let expected = TxSecurityLayoutOutput {
            header_len: 0x22,
            remaining_len: 0x54,
            layout: 0x2002,
            buffer_flags: 0xc01d_806e,
            metadata_len: 0x6e,
        };
        assert_eq!(strict_tx_security_layout(measured), Some(expected));
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_flags: 0x0200_2009,
                ..measured
            }),
            Some(expected),
        );
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                layout: 8,
                ..measured
            }),
            Some(TxSecurityLayoutOutput {
                layout: 0x2008,
                ..expected
            }),
        );
    }

    #[test]
    fn reproduces_hardware_observed_wpa2_ap_group_ccmp_layout() {
        let measured = TxSecurityLayoutInput {
            header_len: 0x0018,
            remaining_len: 0x005c,
            layout: 0,
            buffer_flags: 0xc01d_0082,
            descriptor_flags: 0x0000_200b,
            descriptor_security: 0x0004_0342,
            frame_control: 0x4208,
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x0020,
                remaining_len: 0x0068,
                layout: 0x2000,
                buffer_flags: 0xc022_0082,
                metadata_len: 0x0080,
            }),
        );
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                frame_control: 0x6208,
                remaining_len: 0x0058,
                layout: 1,
                buffer_flags: 0xc01c_007e,
                ..measured
            }),
            Some(TxSecurityLayoutOutput {
                header_len: 0x0020,
                remaining_len: 0x0064,
                layout: 0x2001,
                buffer_flags: 0xc021_007e,
                metadata_len: 0x007c,
            }),
        );
        assert!(strict_ap_group_power_save_completion(
            0x6208,
            0x0020,
            0x0064,
            0x2001,
            0xc021_007e,
            0x0000_200b,
            0x0414_0342,
        ));
        for (remaining_len, layout, buffer_flags, expected) in [
            (
                0x015a,
                3,
                0xc05c_8180,
                TxSecurityLayoutOutput {
                    header_len: 0x0020,
                    remaining_len: 0x0166,
                    layout: 0x2003,
                    buffer_flags: 0xc061_8180,
                    metadata_len: 0x017e,
                },
            ),
            (
                0x0161,
                4,
                0xc05e_4187,
                TxSecurityLayoutOutput {
                    header_len: 0x0020,
                    remaining_len: 0x016d,
                    layout: 0x2004,
                    buffer_flags: 0xc063_4187,
                    metadata_len: 0x0185,
                },
            ),
        ] {
            assert_eq!(
                strict_tx_security_layout(TxSecurityLayoutInput {
                    remaining_len,
                    layout,
                    buffer_flags,
                    // Android IPv6 multicast exercised the same group-CCMP
                    // layout with the rate-control state bit set.
                    descriptor_flags: 0x0200_200b,
                    ..measured
                }),
                Some(expected),
            );
        }
        for layout in [0, 1, 2, 3, 4, 0x1fff] {
            for (remaining_len, buffer_flags, output_remaining, output_buffer, metadata_len) in [
                (0x002c, 0xc011_0052, 0x0038, 0xc016_0052, 0x0050),
                (0x005c, 0xc01d_0082, 0x0068, 0xc022_0082, 0x0080),
                (0x0070, 0xc022_0096, 0x007c, 0xc027_0096, 0x0094),
            ] {
                assert_eq!(
                    strict_tx_security_layout(TxSecurityLayoutInput {
                        remaining_len,
                        layout,
                        buffer_flags,
                        ..measured
                    }),
                    Some(TxSecurityLayoutOutput {
                        header_len: 0x0020,
                        remaining_len: output_remaining,
                        layout: 0x2000 | layout,
                        buffer_flags: output_buffer,
                        metadata_len,
                    }),
                );
                for descriptor_flags in [0x0000_200b, 0x0200_200b] {
                    for descriptor_security in
                        [0x0104_0342, 0x0114_0342, 0x0404_0342, 0x0414_0342]
                    {
                        assert!(strict_ap_group_power_save_completion(
                            0x4208,
                            0x0020,
                            output_remaining,
                            0x2000 | layout,
                            output_buffer,
                            descriptor_flags,
                            descriptor_security,
                        ));
                    }
                }
            }
        }
        assert!(!strict_ap_group_power_save_completion(
            0x4208,
            0x0020,
            0x0068,
            0x2000,
            0xc022_0082,
            0x0000_200b,
            0x0105_0342,
        ));
        assert!(!strict_ap_group_power_save_completion(
            0x4208,
            0x0020,
            0x0068,
            0x2000,
            0xc022_0082,
            0x0000_200b,
            0x0405_0342,
        ));
        let max_mpdu_len = super::AP_GROUP_MAX_MPDU_LEN;
        let max_input_flags =
            0xc000_0000 | (u32::from(max_mpdu_len) << 14) | (u32::from(max_mpdu_len) + 14);
        assert!(strict_tx_security_layout(TxSecurityLayoutInput {
            remaining_len: max_mpdu_len - 0x18,
            buffer_flags: max_input_flags,
            ..measured
        })
        .is_some());
        let oversized_len = max_mpdu_len + 1;
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                remaining_len: oversized_len - 0x18,
                buffer_flags: 0xc000_0000
                    | (u32::from(oversized_len) << 14)
                    | (u32::from(oversized_len) + 14),
                ..measured
            }),
            None,
        );
        for rejected in [
            TxSecurityLayoutInput {
                descriptor_flags: 0x0000_200a,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0004_0304,
                ..measured
            },
            TxSecurityLayoutInput {
                remaining_len: 0x005d,
                ..measured
            },
            TxSecurityLayoutInput {
                buffer_flags: 0xc01d_0083,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_hardware_observed_wpa2_ap_pairwise_ccmp_layout() {
        let measured = TxSecurityLayoutInput {
            header_len: 0x001a,
            remaining_len: 0x002c,
            layout: 0,
            buffer_flags: 0xc011_8052,
            descriptor_flags: 0x0000_2009,
            descriptor_security: 0x0004_0348,
            frame_control: 0x4288,
        };
        let expected = TxSecurityLayoutOutput {
            header_len: 0x0022,
            remaining_len: 0x0038,
            layout: 0x2000,
            buffer_flags: 0xc016_8052,
            metadata_len: 0x0052,
        };
        assert_eq!(strict_tx_security_layout(measured), Some(expected));
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_security: 0x0004_0349,
                ..measured
            }),
            Some(expected),
        );
        assert_eq!(
            strict_tx_security_layout(TxSecurityLayoutInput {
                descriptor_security: 0x0004_034a,
                ..measured
            }),
            None,
        );
        for (remaining_len, layout, buffer_flags, expected) in [
            (
                0x005b,
                2,
                0xc01d_4081,
                TxSecurityLayoutOutput {
                    header_len: 0x0022,
                    remaining_len: 0x0067,
                    layout: 0x2002,
                    buffer_flags: 0xc022_4081,
                    metadata_len: 0x0081,
                },
            ),
            (
                0x006d,
                4,
                0xc021_c093,
                TxSecurityLayoutOutput {
                    header_len: 0x0022,
                    remaining_len: 0x0079,
                    layout: 0x2004,
                    buffer_flags: 0xc026_c093,
                    metadata_len: 0x0093,
                },
            ),
        ] {
            assert_eq!(
                strict_tx_security_layout(TxSecurityLayoutInput {
                    remaining_len,
                    layout,
                    buffer_flags,
                    descriptor_flags: 0x0200_2009,
                    ..measured
                }),
                Some(expected),
            );
        }
        for (frame_control, descriptor_flags, descriptor_security) in [
            // First successful AP pairwise downlink after WPA2 authorization.
            (0x4288, 0x0000_3009, 0x0104_0348),
            (0x4288, 0x0000_2009, 0x0114_0348),
            (0x4288, 0x0000_3009, 0x01a4_0348),
            (0x4a88, 0x0000_2109, 0x0214_0348),
            (0x4288, 0x0000_3009, 0x0114_0348),
            // Maximum-MTU retry completion under sustained AP-to-STA TCP.
            (0x4a88, 0x0000_2109, 0x0204_0348),
            (0x4a88, 0x0000_3109, 0x0214_0348),
            (0x4a88, 0x0000_2109, 0x0414_0348),
            // Maximum-MTU success after the rate controller reaches outcome 4.
            (0x4288, 0x0000_3009, 0x0404_0348),
            // Observed after a Q10 hardware-timeout recovery under concurrent
            // ICMP and HTTP load. The high status nibble changes while the
            // exact pairwise CCMP layout and terminal buffer equation remain
            // unchanged.
            (0x4288, 0x0000_3009, 0x04a4_0348),
            // Android's rate-control state survives through TX success/retry.
            (0x4288, 0x0200_2009, 0x0114_0348),
            (0x4a88, 0x0200_2109, 0x0214_0348),
            (0x4a88, 0x0000_2109, 0x02a4_0348),
        ] {
            assert!(strict_ap_pairwise_power_save_completion(
                frame_control,
                expected.header_len,
                expected.remaining_len,
                expected.layout,
                expected.buffer_flags,
                descriptor_flags,
                descriptor_security,
            ));
        }
        assert!(strict_ap_pairwise_power_save_completion(
            0x4288,
            0x0022,
            0x0050,
            0x237e,
            0xc01c_806a,
            0x0000_3009,
            0x04a4_0348,
        ));
        assert!(strict_ap_pairwise_power_save_completion(
            0x4288,
            expected.header_len,
            expected.remaining_len,
            expected.layout,
            expected.buffer_flags,
            0x0000_3009,
            0x0104_0349,
        ));
        assert!(!strict_ap_pairwise_power_save_completion(
            0x4288,
            expected.header_len,
            expected.remaining_len,
            expected.layout,
            expected.buffer_flags,
            0x0000_3009,
            0x0114_0349,
        ));
        assert!(strict_ap_pairwise_power_save_completion(
            0x4288,
            0x0022,
            0x05ea,
            0x2ad0,
            0xc183_0604,
            0x0000_3009,
            0x0404_0349,
        ));
        assert!(!strict_ap_pairwise_power_save_completion(
            0x4288,
            0x0022,
            0x05ea,
            0x2ad0,
            0xc183_0604,
            0x0000_3009,
            0x0405_0349,
        ));
        assert!(!strict_ap_pairwise_power_save_completion(
            0x4288,
            expected.header_len,
            expected.remaining_len,
            expected.layout,
            expected.buffer_flags,
            0x0000_3009,
            0x0105_0348,
        ));
        assert!(strict_ap_pairwise_power_save_completion(
            0x4a88,
            0x0022,
            0x05ea,
            0x2c71,
            0xc183_0604,
            0x0000_2109,
            0x0204_0348,
        ));
        assert!(!strict_ap_pairwise_power_save_completion(
            0x4a88,
            0x0022,
            0x05ea,
            0x2c71,
            0xc183_0604,
            0x0000_2109,
            0x0205_0348,
        ));
        assert!(!strict_ap_pairwise_power_save_completion(
            0x4288,
            0x0022,
            0x05ea,
            0x27ac,
            0xc183_0604,
            0x0000_3009,
            0x0405_0348,
        ));
        for rejected in [
            TxSecurityLayoutInput {
                descriptor_flags: 0x0000_200b,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0004_034a,
                ..measured
            },
            TxSecurityLayoutInput {
                buffer_flags: 0xc011_8053,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn keeps_transient_ap_message3_carrier_plaintext() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x00a3_001a, 1, 0xc02f_40d1, 0x0200_200c, 0x0288)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x22,
                remaining_len: 0xa7,
                layout: 0x2001,
                buffer_flags: 0xc032_40d1,
                metadata_len: 0xc1,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x4288,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0004_0348,
                ..measured
            },
            TxSecurityLayoutInput {
                remaining_len: 0xa2,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_measured_plaintext_ap_data_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0054_0018, 0, 0xc01b_0082, 0x0000_200a, 0x0208)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x58,
                layout: 0x2000,
                buffer_flags: 0xc01e_0082,
                metadata_len: 0x70,
            }),
        );
        for rejected in [
            TxSecurityLayoutInput {
                frame_control: 0x4208,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_flags: 0x0000_2009,
                ..measured
            },
            TxSecurityLayoutInput {
                remaining_len: 0x0055,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn reproduces_hardware_observed_wpa2_ap_beacon_layout() {
        let measured = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0074_0018, 0, 0xc023_00f8, 0x0080_0412, 0x0080)
        };
        assert_eq!(
            strict_tx_security_layout(measured),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x78,
                layout: 0x2000,
                buffer_flags: 0xc026_00f8,
                metadata_len: 0x90,
            })
        );

        // WPA2 plus the bgnax capability IEs grows the persistent beacon.
        // Metadata covers the complete submitted MPDU minus its eight-byte
        // one-transmission prefix; it is therefore not a profile-independent
        // 0x90 constant.
        let extended = TxSecurityLayoutInput {
            remaining_len: 0x00d6,
            buffer_flags: 0xc03b_82f8,
            ..measured
        };
        assert_eq!(
            strict_tx_security_layout(extended),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x00da,
                layout: 0x2000,
                buffer_flags: 0xc03e_82f8,
                metadata_len: 0x00f2,
            })
        );

        for rejected in [
            TxSecurityLayoutInput {
                descriptor_flags: 0x0080_0410,
                ..measured
            },
            TxSecurityLayoutInput {
                descriptor_security: 0,
                ..measured
            },
            TxSecurityLayoutInput {
                frame_control: 0x4080,
                ..measured
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn refreshes_persistent_wpa2_ap_beacon_without_duplicating_headroom() {
        let refreshed = TxSecurityLayoutInput {
            descriptor_security: 0x0004_0000,
            ..input(0x0074_0020, 0x2001, 0xc025_00f8, 0x0080_0412, 0x0080)
        };
        assert_eq!(
            strict_tx_security_layout(refreshed),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x78,
                layout: 0x2001,
                buffer_flags: 0xc026_00f8,
                metadata_len: 0x90,
            })
        );

        let peer_removed_refresh = TxSecurityLayoutInput {
            header_len: 0x18,
            layout: 0x2240,
            buffer_flags: 0xc023_02f8,
            ..refreshed
        };
        assert_eq!(
            strict_tx_security_layout(peer_removed_refresh),
            Some(TxSecurityLayoutOutput {
                header_len: 0x20,
                remaining_len: 0x78,
                layout: 0x2240,
                buffer_flags: 0xc026_02f8,
                metadata_len: 0x90,
            })
        );

        for rejected in [
            TxSecurityLayoutInput {
                header_len: 0x28,
                ..refreshed
            },
            TxSecurityLayoutInput {
                remaining_len: 0x78,
                ..refreshed
            },
            TxSecurityLayoutInput {
                layout: 0x6001,
                ..refreshed
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn completion_removes_only_the_persistent_beacon_trailer() {
        let completed = TxSecurityLayoutInput {
            header_len: 0x20,
            remaining_len: 0x78,
            layout: 0x2001,
            buffer_flags: 0xc026_00f8,
            descriptor_flags: 0x0080_0412,
            descriptor_security: 0x0114_0000,
            frame_control: 0x0080,
        };
        assert_eq!(
            strict_ap_beacon_completion_layout(completed),
            Some(ApBeaconCompletionLayout {
                remaining_len: 0x74,
                buffer_flags: 0xc025_00f8,
                descriptor_security: 0x0004_0000,
            })
        );
        assert_eq!(
            strict_ap_beacon_completion_layout(TxSecurityLayoutInput {
                layout: 0x2008,
                ..completed
            }),
            strict_ap_beacon_completion_layout(completed)
        );
        assert_eq!(
            strict_ap_beacon_completion_layout(TxSecurityLayoutInput {
                remaining_len: 0x74,
                ..completed
            }),
            None
        );
    }

    #[test]
    fn ignores_rate_control_words_and_rejects_unmeasured_security_layout_and_length_states() {
        let base = input(0x0062_0018, 0, 0xc01e_8084, 0, 0x00b0);
        for rejected in [
            TxSecurityLayoutInput {
                descriptor_security: 1,
                ..base
            },
            TxSecurityLayoutInput {
                layout: 0x2000,
                ..base
            },
            TxSecurityLayoutInput {
                buffer_flags: 0xc01e_4084,
                ..base
            },
            TxSecurityLayoutInput {
                frame_control: 0x4188,
                ..base
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0304,
                descriptor_flags: 0x2009,
                frame_control: 0x0188,
                ..base
            },
            TxSecurityLayoutInput {
                descriptor_security: 0x0404,
                descriptor_flags: 0x2009,
                frame_control: 0x4188,
                ..base
            },
        ] {
            assert_eq!(strict_tx_security_layout(rejected), None);
        }
    }

    #[test]
    fn admits_only_measured_ap_eapol_power_save_completion() {
        assert_eq!(
            strict_ap_eapol_power_save_completion_llc_offset(0x0288, 0x22, 0x6f, 0x2000),
            Some(0x1a)
        );
        assert_eq!(
            strict_ap_eapol_power_save_completion_llc_offset(0x0288, 0x22, 0xa7, 0x2001),
            Some(0x1a)
        );
        assert_eq!(
            strict_ap_eapol_power_save_completion_llc_offset(0x0a88, 0x22, 0x6f, 0x2000),
            Some(0x1a)
        );
        assert_eq!(
            strict_ap_eapol_power_save_completion_llc_offset(0x0a88, 0x22, 0xa7, 0x2001),
            Some(0x1a)
        );
        for rejected in [
            (0x0188, 0x22, 0x6f, 0x2000),
            (0x0288, 0x1a, 0x6f, 0x2000),
            (0x0288, 0x22, 0x6e, 0x2000),
            (0x0288, 0x22, 0x6f, 0),
            (0x0288, 0x22, 0xa6, 0x2001),
            (0x0288, 0x22, 0xa7, 0x2000),
        ] {
            assert_eq!(
                strict_ap_eapol_power_save_completion_llc_offset(
                    rejected.0, rejected.1, rejected.2, rejected.3,
                ),
                None
            );
        }
    }
}
#[cfg(feature = "hil-vendor-tx")]
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_DESCRIPTOR_FLAGS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_DESCRIPTOR_SECURITY: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_FRAME_CONTROL: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_LENGTHS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_LAYOUT: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "hil-vendor-tx")]
static HIL_REJECTED_BUFFER_FLAGS: AtomicU32 = AtomicU32::new(0);

#[cfg(feature = "hil-vendor-tx")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HilTxSecurityRejectedSnapshot {
    pub count: usize,
    pub input: TxSecurityLayoutInput,
}

#[cfg(feature = "hil-vendor-tx")]
pub fn hil_tx_security_rejected_snapshot() -> HilTxSecurityRejectedSnapshot {
    HilTxSecurityRejectedSnapshot {
        count: HIL_REJECTED_COUNT.load(Ordering::Acquire),
        input: TxSecurityLayoutInput {
            header_len: HIL_REJECTED_LENGTHS.load(Ordering::Acquire) as u16,
            remaining_len: (HIL_REJECTED_LENGTHS.load(Ordering::Acquire) >> 16) as u16,
            layout: HIL_REJECTED_LAYOUT.load(Ordering::Acquire) as u16,
            buffer_flags: HIL_REJECTED_BUFFER_FLAGS.load(Ordering::Acquire),
            descriptor_flags: HIL_REJECTED_DESCRIPTOR_FLAGS.load(Ordering::Acquire),
            descriptor_security: HIL_REJECTED_DESCRIPTOR_SECURITY.load(Ordering::Acquire),
            frame_control: HIL_REJECTED_FRAME_CONTROL.load(Ordering::Acquire) as u16,
        },
    }
}
