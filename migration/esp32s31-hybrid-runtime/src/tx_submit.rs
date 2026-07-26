//! Strict outer TX-preparation and PP-list publication boundary.
//!
//! The pinned `libpp.a[pp.o]::ppTxPkt` body sequences five independent jobs:
//! interface admission, descriptor queue validation, protocol/security/rate
//! preparation, queue mapping, and intrusive-list publication. The first four
//! are now Rust-owned finite transformations. This module reproduces the last
//! publication through the Rust-owned logical TX queue registry.

#[cfg(target_arch = "riscv32")]
use core::{ffi::c_void, ptr};

const TX_DESCRIPTOR_PRIORITY_OFFSET: usize = 0x04;

/// Recover the hardware event queue encoded in descriptor byte four from its
/// 802.11 user priority.
///
/// This is the complete pinned `ppTxPkt` decision tree. Priorities 0..=7 map
/// to the four EDCA queues; values above seven select the unsupported special
/// queue and are rejected by the strict runtime.
pub(crate) const fn hardware_queue_for_priority(priority: u8) -> u8 {
    match priority & 0x0f {
        0 | 3 => 2,
        1 | 2 => 3,
        4 | 5 => 1,
        6 | 7 => 0,
        _ => 4,
    }
}

pub(crate) const fn descriptor_queue_is_consistent(priority_byte: u8) -> bool {
    let priority = priority_byte & 0x0f;
    priority_byte >> 4 == hardware_queue_for_priority(priority)
}

#[cfg(target_arch = "riscv32")]
const FRAME_KIND_OFFSET: usize = 0x1a;
#[cfg(target_arch = "riscv32")]
const FRAME_PEER_OFFSET: usize = 0x2c;
#[cfg(target_arch = "riscv32")]
const FRAME_NEXT_OFFSET: usize = 0x30;
#[cfg(target_arch = "riscv32")]
const FRAME_DESCRIPTOR_OFFSET: usize = 0x34;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_CONTROL_OFFSET: usize = 0x10;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_TIMESTAMP_OFFSET: usize = 0x18;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_INTERFACE_SHIFT: u32 = 18;
#[cfg(target_arch = "riscv32")]
const DESCRIPTOR_LOGICAL_QUEUE_SHIFT: u32 = 20;
#[cfg(target_arch = "riscv32")]
const MAC_TIME_LOW_REGISTER: *const u32 = 0x2010_d800 as *const u32;

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    fn esf_buf_recycle(frame: *mut c_void);
    fn pp_post(kind: u32, argument: *mut c_void) -> i32;
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
unsafe fn trap_invalid_tx_submit(frame: *mut u8, detail: u32) -> ! {
    core::arch::asm!(
        "ebreak",
        in("a0") frame,
        in("a1") detail,
        options(noreturn)
    )
}

#[cfg(target_arch = "riscv32")]
unsafe fn interface_is_adopted(selector: u32) -> bool {
    match selector {
        0 => crate::net80211_state::station_interface().is_some(),
        1 => crate::net80211_state::access_point_interface().is_some(),
        _ => false,
    }
}

#[cfg(target_arch = "riscv32")]
unsafe fn map_strict_frame(frame: *mut u8) -> i32 {
    #[cfg(feature = "hil-ampdu-intercept")]
    {
        crate::tx_intercept::hil_ampdu_intercept_pp_map_tx_queue(frame)
    }
    #[cfg(not(feature = "hil-ampdu-intercept"))]
    {
        if !crate::tx_mapper::apply_strict_sta_ap(frame) {
            crate::tx_mapper::trap_unadmitted_strict_sta_ap(frame);
        }
        0
    }
}

/// Append one fully prepared frame to its Rust-owned logical queue.
#[cfg(target_arch = "riscv32")]
#[link_section = ".rwtext.wifi_strict.tx_submit"]
unsafe fn append_logical_queue(frame: *mut u8, descriptor: *mut u8) -> u8 {
    let control = descriptor
        .add(DESCRIPTOR_CONTROL_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    let logical_queue = ((control >> DESCRIPTOR_LOGICAL_QUEUE_SHIFT) & 0x0f) as u8;
    frame
        .add(FRAME_NEXT_OFFSET)
        .cast::<*mut u8>()
        .write(ptr::null_mut());
    descriptor
        .add(DESCRIPTOR_TIMESTAMP_OFFSET)
        .cast::<u32>()
        .write_unaligned(MAC_TIME_LOW_REGISTER.read_volatile());
    if crate::tx_queue::append_logical_queue(logical_queue, frame).is_err() {
        trap_invalid_tx_submit(frame, 0x3003 | (u32::from(logical_queue) << 16));
    }
    descriptor.add(TX_DESCRIPTOR_PRIORITY_OFFSET).read() >> 4
}

/// Final-link replacement for the pinned `ppTxPkt` outer shell.
///
/// TX is forbidden before strict ownership handoff; an early call traps rather
/// than retaining a hidden route into the stock function. Once armed, the
/// function contains no allocation, wait, OSI callback, vendor queue mapper,
/// or vendor rate-control call. A successful mapper value of three means the
/// optional Rust A-MPDU owner retained the frame; zero publishes one ordinary
/// logical queue entry.
///
/// # Safety
///
/// `frame` must be one live, exclusively owned ESF TX object. Every pointer in
/// its pinned descriptor/peer/buffer graph must remain valid until completion.
#[cfg(target_arch = "riscv32")]
#[no_mangle]
#[link_section = ".rwtext.wifi_strict.tx_submit"]
pub unsafe extern "C" fn __wrap_ppTxPkt(frame: *mut u8, ownership: u32) -> i32 {
    if !crate::critical::strict_wifi_hart_armed() {
        trap_invalid_tx_submit(frame, 0x300b);
    }
    if !crate::critical::on_strict_wifi_hart() || frame.is_null() {
        trap_invalid_tx_submit(frame, 0x3004);
    }
    let descriptor = frame.add(FRAME_DESCRIPTOR_OFFSET).cast::<*mut u8>().read();
    if descriptor.is_null() {
        trap_invalid_tx_submit(frame, 0x3005);
    }
    let control = descriptor
        .add(DESCRIPTOR_CONTROL_OFFSET)
        .cast::<u32>()
        .read_unaligned();
    let interface = (control >> DESCRIPTOR_INTERFACE_SHIFT) & 0x03;
    if !interface_is_adopted(interface) {
        trap_invalid_tx_submit(frame, 0x3006 | (interface << 16));
    }
    let priority_byte = descriptor.add(TX_DESCRIPTOR_PRIORITY_OFFSET).read();
    if !descriptor_queue_is_consistent(priority_byte) {
        trap_invalid_tx_submit(frame, 0x3007 | (u32::from(priority_byte) << 16));
    }

    crate::tx_proto::strict_pp_tx_proto_proc(frame);
    if crate::tx_security::strict_pp_proc_tx_sec_frame(frame) == 1 {
        // The vendor kind-one follow-up drains its cached net80211 queue. That
        // queue is proved empty at handoff and ordinary TX now has a Rust
        // mailbox, so recycling is the complete strict disposition.
        debug_assert_eq!(frame.add(FRAME_KIND_OFFSET).read(), 1);
        esf_buf_recycle(frame.cast());
        return 1;
    }
    crate::tx_rate::strict_rate_schedule(
        frame.add(FRAME_PEER_OFFSET).cast::<*mut u8>().read(),
        descriptor,
    );

    match map_strict_frame(frame) {
        0 => {
            let hardware_queue = append_logical_queue(frame, descriptor);
            if hardware_queue > 3 {
                trap_invalid_tx_submit(frame, 0x300a | (u32::from(hardware_queue) << 16));
            }
            if ownership != 0
                && crate::tx_queue::hardware_queue_idle(hardware_queue)
                && pp_post(u32::from(hardware_queue), ptr::null_mut()) != 0
            {
                trap_invalid_tx_submit(frame, 0x3008 | (u32::from(hardware_queue) << 16));
            }
            0
        }
        3 => 0,
        mapped => trap_invalid_tx_submit(frame, 0x3009 | ((mapped as u32) << 16)),
    }
}

#[cfg(test)]
mod tests {
    use super::{descriptor_queue_is_consistent, hardware_queue_for_priority};

    #[test]
    fn reproduces_all_pinned_priority_classes() {
        assert_eq!(
            (0_u8..16)
                .map(hardware_queue_for_priority)
                .collect::<std::vec::Vec<_>>(),
            [2, 3, 3, 2, 1, 1, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4]
        );
    }

    #[test]
    fn validates_encoded_queue_without_masking_unknown_bits() {
        for priority in 0_u8..16 {
            let encoded = priority | (hardware_queue_for_priority(priority) << 4);
            assert!(descriptor_queue_is_consistent(encoded));
            assert!(!descriptor_queue_is_consistent(encoded ^ 0x10));
        }
    }
}
