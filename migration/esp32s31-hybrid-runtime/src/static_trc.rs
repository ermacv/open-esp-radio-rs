//! Fixed ownership for the three default transmit-rate-control contexts.
//!
//! The pinned `trc_init` allocates three zeroed 0x98-byte objects and publishes
//! them at `g_per_conn_trc[19..=21]`. It then writes the same schedule pointers,
//! flags and interface identities into each object. The strict runtime uses
//! these default STA/AP/NAN contexts but never creates per-peer adaptive-rate
//! contexts after handoff.

use core::ptr;

use crate::{
    rate_control::{
        ampdu_limit_for_rate, beamforming_report_rate_for_metric, rate_to_schedule_index,
        select_phy_mode, PhyModeSelectionInput, RateControlRecord, RateIndexMap,
        RATE_CONTROL_RECORD_SIZE,
    },
    rate_schedule::{schedule_pointer, RateScheduleKind, RateScheduleRef},
};

const TRC_CONTEXT_SIZE: usize = RATE_CONTROL_RECORD_SIZE;
const TRC_CONTEXT_COUNT: usize = 3;
const TRC_TABLE_COUNT: usize = 22;
const TRC_DEFAULT_INDEX: usize = 19;
const TRC_ROUTE_COUNT: usize = 3;
const TRC_PEER_ADDRESS_OFFSET: usize = 0x21;
const PRIMARY_RATE_OFFSET: usize = 0x64;
const SECONDARY_RATE_OFFSET: usize = 0x68;
const FALLBACK_RATE_OFFSET: usize = 0x6c;
const P2P_RATE_OFFSET: usize = 0x70;
const LEGACY_RATE_OFFSET: usize = 0x74;
const FLAGS_OFFSET: usize = 0x0c;
const CURRENT_RATE_OFFSET: usize = 0x28;
const IDENTITY_OFFSET: usize = 0x85;
const FINAL_STATE_OFFSET: usize = 0x87;
const PHY_TYPE_OFFSET: usize = 0x86;
const FEATURE_ENABLED_OFFSET: usize = 0x8b;
const HE_TYPE_OFFSET: usize = 0x8e;
const HE_FEATURE_8F_OFFSET: usize = 0x8f;
const HE_FEATURE_90_OFFSET: usize = 0x90;
const LINK_METRIC_OFFSET: usize = 0x1c;
const HIGHEST_INDEX_OFFSET: usize = 0x04;
const MAXIMUM_INDEX_OFFSET: usize = 0x05;
const SCHEDULE_COUNT_OFFSET: usize = 0x06;
const RATE_INDEX_CALLBACK_OFFSET: usize = 0x78;
const AMPDU_BASE_LIMIT_OFFSET: usize = 0x7c;
const AMPDU_HALF_LIMIT_OFFSET: usize = 0x7e;
const AMPDU_FULL_LIMIT_OFFSET: usize = 0x80;
const REEVALUATE_AFTER_OFFSET: usize = 0x60;
const REEVALUATE_AFTER_US: u32 = 500_000;
const ESP_OK: i32 = 0;
const ESP_ERR_WIFI_STATE: i32 = 0x3006;

#[link_section = ".critical.bss.wifi_strict.trc_default_contexts"]
static mut STATIC_TRC_CONTEXTS: [RateControlRecord; TRC_CONTEXT_COUNT] = [
    RateControlRecord::zeroed(),
    RateControlRecord::zeroed(),
    RateControlRecord::zeroed(),
];

unsafe extern "C" {
    static mut g_ic: u8;
    static mut g_per_conn_trc: u8;
    static mut trc_ctl: u8;
    static wDevCtrl: u8;
}

/// Apply the finite receive-signal update from the pinned 0x66-byte
/// `rcUpdateRxDone` leaf.
///
/// `wDevCtrl+0x2e` is the recovered signed-sample calibration byte. Its public
/// field name is unknown, so the compatibility access remains explicit here
/// until the WDEV backing moves into a typed Rust owner.
///
/// # Safety
///
/// `rate_control` must be a live context returned by
/// [`wifi_strict_rc_get_trc`], and `rx_control` must name the current hardware
/// RX-control block.
#[no_mangle]
#[inline(never)]
#[link_section = ".rwtext.wifi_strict.rx_proto"]
pub unsafe extern "C" fn wifi_strict_rc_update_rx_done(rate_control: *mut u8, rx_control: *mut u8) {
    if rate_control.is_null() || rx_control.is_null() {
        return;
    }
    let context_flags = unsafe { rate_control.add(0x0c).cast::<u16>().read() };
    let state_flags = unsafe { rate_control.add(0x1b).read() };
    let calibration = unsafe { ptr::addr_of!(wDevCtrl).add(0x2e).read() };
    let raw_sample = unsafe { rx_control.read() };
    let previous_latest = unsafe { rate_control.add(2).cast::<i8>().read() };
    let previous_smoothed = unsafe { rate_control.add(3).cast::<i8>().read() };
    let Some(update) = crate::rx_proto::update_rx_rate_sample(
        context_flags,
        state_flags,
        calibration,
        raw_sample,
        previous_latest,
        previous_smoothed,
    ) else {
        return;
    };
    unsafe {
        rate_control.add(2).cast::<i8>().write(update.latest);
        rate_control.add(3).cast::<i8>().write(update.smoothed);
    }
}

/// Select the live rate-control context for one received peer.
///
/// This is the complete state lookup from the pinned 0x76-byte
/// `rc_get_trc`. `trc_ctl[(route + 1)]` is a bitmap of the 22 fixed ROM-ABI
/// table slots at `g_per_conn_trc`; each live context stores its six-byte peer
/// address at offset `0x21`. The public C enum names are unavailable, so the
/// caller supplies only the recovered route number 0 through 2.
///
/// Unlike the vendor loop, the Rust boundary never follows a bitmap bit beyond
/// the actual 22-slot ABI range and never dereferences a null publication.
/// The scan has a compile-time bound and does not poll hardware or external
/// progress.
///
/// # Safety
///
/// Cold rate-control initialization must have published the table, and
/// `receiver` must point to six readable address bytes owned by the current RX
/// frame.
#[no_mangle]
#[inline(never)]
#[link_section = ".rwtext.wifi_strict.rx_proto"]
pub unsafe extern "C" fn wifi_strict_rc_get_trc(route: u32, receiver: *mut u8) -> *mut u8 {
    if route as usize >= TRC_ROUTE_COUNT || receiver.is_null() {
        return ptr::null_mut();
    }

    let controls = ptr::addr_of!(trc_ctl).cast::<u32>();
    let mut candidates = unsafe { controls.add(route as usize + 1).read() };
    for _ in 0..TRC_TABLE_COUNT {
        if candidates == 0 {
            return ptr::null_mut();
        }
        let index = candidates.trailing_zeros() as usize;
        if index >= TRC_TABLE_COUNT {
            return ptr::null_mut();
        }
        candidates &= !(1_u32 << index);

        let context = unsafe { table_slot(index).read() };
        if context.is_null() {
            return ptr::null_mut();
        }
        let peer = unsafe { context.add(TRC_PEER_ADDRESS_OFFSET) };
        let mut matches = true;
        for byte in 0..6 {
            if unsafe { peer.add(byte).read() != receiver.add(byte).read() } {
                matches = false;
                break;
            }
        }
        if matches {
            return context;
        }
    }
    ptr::null_mut()
}

unsafe fn table_slot(index: usize) -> *mut *mut u8 {
    ptr::addr_of_mut!(g_per_conn_trc)
        .add(index * size_of::<*mut u8>())
        .cast::<*mut u8>()
}

unsafe fn context(index: usize) -> *mut u8 {
    ptr::addr_of_mut!(STATIC_TRC_CONTEXTS)
        .cast::<u8>()
        .add(index * TRC_CONTEXT_SIZE)
}

/// Return whether a temporary ABI pointer names one of the three
/// Rust-owned default contexts.
///
/// The check only establishes backing-storage provenance. The single radio
/// owner separately guarantees that a published context is not concurrently
/// mutated.
pub(crate) unsafe fn owns_rate_control_record(candidate: *mut u8) -> bool {
    for index in 0..TRC_CONTEXT_COUNT {
        if context(index) == candidate {
            return true;
        }
    }
    false
}

unsafe fn owns_any_rate_control_record(candidate: *mut u8) -> bool {
    owns_rate_control_record(candidate) || crate::allocation::owns_rate_control_record(candidate)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn wifi_strict_rc11b_rate_to_schedule_index(rate: u32) -> u32 {
    u32::from(rate_to_schedule_index(RateIndexMap::Dot11B, rate as u8))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn wifi_strict_rc11g_rate_to_schedule_index(rate: u32) -> u32 {
    u32::from(rate_to_schedule_index(RateIndexMap::Dot11G, rate as u8))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn wifi_strict_rc11n_rate_to_schedule_index(rate: u32) -> u32 {
    u32::from(rate_to_schedule_index(RateIndexMap::Dot11N, rate as u8))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn wifi_strict_rc11ax_rate_to_schedule_index(rate: u32) -> u32 {
    u32::from(rate_to_schedule_index(RateIndexMap::Dot11Ax, rate as u8))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn wifi_strict_lora_rate_to_schedule_index(rate: u32) -> u32 {
    u32::from(rate_to_schedule_index(RateIndexMap::Lora, rate as u8))
}

fn rate_index_callback(map: RateIndexMap) -> usize {
    match map {
        RateIndexMap::Dot11B => wifi_strict_rc11b_rate_to_schedule_index as *const () as usize,
        RateIndexMap::Dot11G => wifi_strict_rc11g_rate_to_schedule_index as *const () as usize,
        RateIndexMap::Dot11N => wifi_strict_rc11n_rate_to_schedule_index as *const () as usize,
        RateIndexMap::Dot11Ax => wifi_strict_rc11ax_rate_to_schedule_index as *const () as usize,
        RateIndexMap::Lora => wifi_strict_lora_rate_to_schedule_index as *const () as usize,
    }
}

unsafe fn initialize_context(context: *mut u8, identity: u8) {
    // These are the exact post-`trc_init` records recovered from the pinned
    // archive: B[3] for current/fallback, P2P-G[7], and B[0] as the legacy
    // table base.  The pointers are now projections of typed Rust references,
    // not addresses of vendor-owned data symbols.
    let legacy = schedule_pointer(RateScheduleRef {
        kind: RateScheduleKind::Dot11B,
        index: 0,
    }) as u32;
    let primary = schedule_pointer(RateScheduleRef {
        kind: RateScheduleKind::Dot11B,
        index: 3,
    }) as u32;
    let p2p = schedule_pointer(RateScheduleRef {
        kind: RateScheduleKind::P2pDot11G,
        index: 7,
    }) as u32;
    context
        .add(PRIMARY_RATE_OFFSET)
        .cast::<u32>()
        .write_unaligned(primary);
    context
        .add(SECONDARY_RATE_OFFSET)
        .cast::<u32>()
        .write_unaligned(primary);
    context
        .add(FALLBACK_RATE_OFFSET)
        .cast::<u32>()
        .write_unaligned(primary);
    context
        .add(P2P_RATE_OFFSET)
        .cast::<u32>()
        .write_unaligned(p2p);
    context
        .add(LEGACY_RATE_OFFSET)
        .cast::<u32>()
        .write_unaligned(legacy);
    context
        .add(FLAGS_OFFSET)
        .cast::<u16>()
        .write_unaligned(0x80);
    context.add(CURRENT_RATE_OFFSET).write(0);
    context.add(FINAL_STATE_OFFSET).write(0);
    context.add(IDENTITY_OFFSET).write(identity);
}

unsafe fn publish_schedule_set(
    context: *mut u8,
    primary: RateScheduleRef,
    secondary: RateScheduleRef,
    fallback: RateScheduleRef,
    legacy: RateScheduleRef,
) {
    context
        .add(PRIMARY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(primary));
    context
        .add(SECONDARY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(secondary));
    context
        .add(FALLBACK_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(fallback));
    context
        .add(LEGACY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(legacy));
}

/// Replace the vendor `rcAttach` initializer.
///
/// The seven `rcBuildIndex` calls only wrote the record number to byte 0x0a.
/// Rust schedule literals already contain those indices, leaving four finite
/// compatibility control words to reset while their remaining readers are
/// migrated.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_rc_attach() {
    let controls = ptr::addr_of_mut!(trc_ctl);
    controls.add(4).cast::<u32>().write_unaligned(0);
    controls.add(8).cast::<u32>().write_unaligned(0);
    controls.add(12).cast::<u32>().write_unaligned(0);
    controls.add(20).cast::<u32>().write_unaligned(0);
}

/// Return the vendor ABI's default B[3] schedule from Rust-owned storage.
#[no_mangle]
pub extern "C" fn wifi_strict_rc_get_default_schedule() -> *mut u8 {
    schedule_pointer(RateScheduleRef {
        kind: RateScheduleKind::Dot11B,
        index: 3,
    })
}

/// Return the vendor ABI's 6-Mbit OFDM schedule (G[7]) from Rust storage.
#[no_mangle]
pub extern "C" fn wifi_strict_rc_get_g6m_schedule() -> *mut u8 {
    schedule_pointer(RateScheduleRef {
        kind: RateScheduleKind::Dot11G,
        index: 7,
    })
}

unsafe fn clear_current_schedule_state(context: *mut u8) {
    // The vendor leaf additionally cleared schedule[11]. That byte belonged
    // to its shared mutable arena; strict runtime eliminated its last reader
    // and therefore keeps all per-context mutation inside this record.
    context
        .add(REEVALUATE_AFTER_OFFSET)
        .cast::<u32>()
        .write_unaligned(REEVALUATE_AFTER_US);
    for offset in [0x30, 0x2c, 0x40] {
        context.add(offset).cast::<u32>().write_unaligned(0);
    }
    for offset in [0x1d, 0x1e, 0x07] {
        context.add(offset).write(0);
    }
}

unsafe fn initialize_ampdu_state(context: *mut u8, limit_rate: u8) {
    let requested = ampdu_limit_for_rate(limit_rate);
    let base = context
        .add(AMPDU_BASE_LIMIT_OFFSET)
        .cast::<u16>()
        .read_unaligned();
    context
        .add(AMPDU_FULL_LIMIT_OFFSET)
        .cast::<u16>()
        .write_unaligned(requested.min(base));
    context
        .add(AMPDU_HALF_LIMIT_OFFSET)
        .cast::<u16>()
        .write_unaligned((requested >> 1).min(base));

    let active_tids = context.add(0x12).read();
    for tid in 0..8 {
        if active_tids & (1 << tid) != 0 {
            context.add(0x13 + tid).write(4);
        }
    }
    context.add(0x10).write(4);
    for offset in [0x48, 0x44, 0x4c, 0x5c] {
        context.add(offset).cast::<u32>().write_unaligned(0);
    }
    for offset in [0x1f, 0x20, 0x11] {
        context.add(offset).write(0);
    }
    context
        .add(REEVALUATE_AFTER_OFFSET)
        .cast::<u32>()
        .write_unaligned(REEVALUATE_AFTER_US);
}

/// Complete Rust-owned replacement for `libpp.a[trc.o]::rcUpdatePhyMode`.
///
/// The ABI projection is accepted only for a fixed default context or a
/// currently claimed Rust peer record. Schedule choice, highest-rate lookup,
/// rate-code mapping and AMPDU limits are safe value operations. The only
/// compatibility global write left here is `trc_ctl[0x18] = 0x0808`, whose
/// downstream vendor readers are tracked for the later record-layout removal.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_rc_update_phy_mode(
    context: *mut u8,
    metric: i32,
    p2p: u32,
    supplied_highest_rate: u32,
    use_supplied_highest_rate: u32,
) {
    if context.is_null() || !owns_any_rate_control_record(context) {
        return;
    }

    let phy_type = context.add(PHY_TYPE_OFFSET).read();
    let he_type = context.add(HE_TYPE_OFFSET).read();
    let feature_enabled = context.add(FEATURE_ENABLED_OFFSET).read() != 0;
    let selection = select_phy_mode(PhyModeSelectionInput {
        phy_type,
        he_type,
        metric,
        p2p: p2p != 0,
        supplied_highest_rate,
        use_supplied_highest_rate: use_supplied_highest_rate != 0,
        feature_enabled,
    });

    context.add(LINK_METRIC_OFFSET).write(metric as u8);
    context
        .add(PRIMARY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(selection.current));
    context
        .add(SECONDARY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(selection.secondary));
    context
        .add(FALLBACK_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(selection.fallback));
    context
        .add(LEGACY_RATE_OFFSET)
        .cast::<*mut u8>()
        .write_unaligned(schedule_pointer(selection.legacy));
    context
        .add(HIGHEST_INDEX_OFFSET)
        .write(selection.highest_index);
    context
        .add(MAXIMUM_INDEX_OFFSET)
        .write(selection.maximum_index);
    context
        .add(SCHEDULE_COUNT_OFFSET)
        .write(selection.schedule_count);
    context
        .add(RATE_INDEX_CALLBACK_OFFSET)
        .cast::<usize>()
        .write_unaligned(rate_index_callback(selection.index_map));

    if let Some(limit_rate) = selection.ampdu_limit_rate {
        // Preserve this short-lived compatibility publication until all
        // consumers of the old trc_ctl layout have typed Rust owners.
        ptr::addr_of_mut!(trc_ctl)
            .add(0x18)
            .cast::<u16>()
            .write_unaligned(0x0808);
        context.add(0x0f).write(limit_rate);
        initialize_ampdu_state(context, limit_rate);

        let report = beamforming_report_rate_for_metric(
            metric,
            context.add(HE_FEATURE_8F_OFFSET).read() != 0,
            context.add(HE_FEATURE_90_OFFSET).read() != 0,
        );
        crate::txdone::set_bf_report_rate(report.mode, report.rate, report.dcm, report.ersu);
        crate::txdone::set_ersu_ack_rate(report.ersu_ack);
    }

    clear_current_schedule_state(context);
    context.cast::<u32>().write_unaligned(0x7f7f_7f7f);
}

/// Replace the complete default-context schedule selector recovered from
/// `libpp.a[trc.o]::trc_update_ifx_phy_mode`.
///
/// The vendor body selects only three finite layouts: LoRa records 1/0/1,
/// dot11b record 3, or P2P-dot11g record 7.  This adapter preserves the
/// interface-specific protocol-bit choice but publishes pointers exclusively
/// into the Rust-owned schedule bank.
#[no_mangle]
pub unsafe extern "C" fn wifi_strict_trc_update_ifx_phy_mode(interface: u32, phy_mode: u32) -> i32 {
    let (table_index, p2p_enabled) = match interface {
        0 => (
            TRC_DEFAULT_INDEX,
            ptr::addr_of!(g_ic).add(0x1e0).read() & (1 << 4) != 0,
        ),
        1 => (
            TRC_DEFAULT_INDEX + 1,
            ptr::addr_of!(g_ic).add(0x1e0).read() & (1 << 5) != 0,
        ),
        2 => (TRC_DEFAULT_INDEX + 2, false),
        _ => return 0x102,
    };
    let context = table_slot(table_index).read();
    if context.is_null() || !owns_rate_control_record(context) {
        return ESP_ERR_WIFI_STATE;
    }

    if phy_mode == 6 {
        publish_schedule_set(
            context,
            RateScheduleRef {
                kind: RateScheduleKind::Lora,
                index: 1,
            },
            RateScheduleRef {
                kind: RateScheduleKind::Lora,
                index: 0,
            },
            RateScheduleRef {
                kind: RateScheduleKind::Lora,
                index: 1,
            },
            RateScheduleRef {
                kind: RateScheduleKind::Lora,
                index: 0,
            },
        );
    } else {
        let selected = if p2p_enabled {
            RateScheduleRef {
                kind: RateScheduleKind::P2pDot11G,
                index: 7,
            }
        } else {
            RateScheduleRef {
                kind: RateScheduleKind::Dot11B,
                index: 3,
            }
        };
        publish_schedule_set(context, selected, selected, selected, selected);
    }
    ESP_OK
}

/// Return whether all three default table cells own the fixed Rust contexts.
///
/// # Safety
///
/// The caller must serialize this check with Wi-Fi initialization teardown.
pub unsafe fn static_trc_contexts_bound() -> bool {
    table_slot(TRC_DEFAULT_INDEX).read_volatile() == context(0)
        && table_slot(TRC_DEFAULT_INDEX + 1).read_volatile() == context(1)
        && table_slot(TRC_DEFAULT_INDEX + 2).read_volatile() == context(2)
}

/// Replace the three-allocation vendor default-context initializer.
#[cfg(feature = "rust-static-trc-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_trc_init() -> i32 {
    if !table_slot(TRC_DEFAULT_INDEX).read_volatile().is_null()
        || !table_slot(TRC_DEFAULT_INDEX + 1).read_volatile().is_null()
        || !table_slot(TRC_DEFAULT_INDEX + 2).read_volatile().is_null()
    {
        return ESP_ERR_WIFI_STATE;
    }
    ptr::addr_of_mut!(STATIC_TRC_CONTEXTS)
        .cast::<u8>()
        .write_bytes(0, TRC_CONTEXT_SIZE * TRC_CONTEXT_COUNT);
    let context0 = context(0);
    let context1 = context(1);
    let context2 = context(2);
    initialize_context(context0, 0);
    initialize_context(context1, 1);
    initialize_context(context2, 2);
    table_slot(TRC_DEFAULT_INDEX).write_volatile(context0);
    table_slot(TRC_DEFAULT_INDEX + 1).write_volatile(context1);
    table_slot(TRC_DEFAULT_INDEX + 2).write_volatile(context2);
    ESP_OK
}

/// Withdraw only the three exact fixed publications.
///
/// Any changed pointer fails closed: the strict owner never delegates an
/// unknown object to `free`.
#[cfg(feature = "rust-static-trc-init-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_trc_deinit() -> i32 {
    if !static_trc_contexts_bound() {
        return ESP_ERR_WIFI_STATE;
    }
    table_slot(TRC_DEFAULT_INDEX).write_volatile(ptr::null_mut());
    table_slot(TRC_DEFAULT_INDEX + 1).write_volatile(ptr::null_mut());
    table_slot(TRC_DEFAULT_INDEX + 2).write_volatile(ptr::null_mut());
    ESP_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_arena_has_exact_vendor_shape() {
        assert_eq!(size_of::<RateControlRecord>(), TRC_CONTEXT_SIZE);
        assert_eq!(align_of::<RateControlRecord>(), 4);
        assert_eq!(
            size_of::<[RateControlRecord; TRC_CONTEXT_COUNT]>(),
            TRC_CONTEXT_SIZE * TRC_CONTEXT_COUNT
        );
    }

    #[test]
    fn recovered_fields_fit_the_fixed_context() {
        for offset in [
            PRIMARY_RATE_OFFSET,
            SECONDARY_RATE_OFFSET,
            FALLBACK_RATE_OFFSET,
            P2P_RATE_OFFSET,
            LEGACY_RATE_OFFSET,
        ] {
            assert!(offset + size_of::<u32>() <= TRC_CONTEXT_SIZE);
        }
        assert!(IDENTITY_OFFSET < TRC_CONTEXT_SIZE);
        assert!(FINAL_STATE_OFFSET < TRC_CONTEXT_SIZE);
    }
}
