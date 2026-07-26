//! Fixed-storage vendor-state bindings used by ESP32-S31 ROM leaves.
//!
//! The pinned `net80211_data_ptr_init` and `wdev_data_init` bodies contain
//! exactly 43 direct stores of archive-static addresses into ROM ABI cells.
//! The strict archive audit treats them as separate cold-init roots and proves
//! that they contain no allocation, wait, indirect call, or unbounded control
//! flow. This module also provides an equivalent Rust-owned implementation of
//! those stores.

use core::ptr;

/// One fixed backing-object binding recovered from the two pinned vendor
/// cold-init leaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaticVendorBinding {
    WifiNvs,
    Scan,
    ChannelManager,
    Net80211Interface,
    HmacCounters,
    TxCacheQueue,
    MacSleepEnabled,
    MeshQuickFunctions,
    MeshInitialPowerSaveType,
    MeshStarted,
    MeshRoot,
    MeshTopology,
    TxRxContext,
    LmacConfig,
    WdevControl,
    WdevMacSleep,
    LmacCounters,
    PpSignalCounters,
    WifiMenuConfig,
    EsfBufferLists,
    Fragment,
    InterfaceControl,
    ApNoLongRange,
    LoraRateSchedule,
    Dot11nRateSchedule,
    Dot11bRateSchedule,
    BasicOfdmRateSchedule,
    TrcControl,
    PowerManagementConfig,
    PowerManagement,
    TxopQueueStatus,
    PowerManagementCounters,
    PpTimerInfo,
    RtsThresholds,
    PowerManagementTwt,
    HeMaxApepLengths,
    WdevRxDebug,
    PowerManagementBeaconOffset,
    PowerManagementBeaconOffsetConfig,
    TbttStart,
    OffchannelTxProgress,
    OffchannelPacketLifetime,
    SendWakeNullTimer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticVendorBindingError {
    binding: StaticVendorBinding,
}

impl StaticVendorBindingError {
    pub const fn binding(self) -> StaticVendorBinding {
        self.binding
    }
}

/// Evidence that all 43 ROM ABI cells refer to their qualified fixed backing
/// objects. TXOP availability and the four rate-schedule cells are Rust-owned.
pub struct StaticVendorBindings {
    _private: (),
}

unsafe extern "C" {
    fn net80211_data_ptr_init();
    fn wdev_data_init() -> i32;
}

macro_rules! fixed_bindings {
    (
        net80211 { $($net_binding:ident: $net_cell:ident => $net_backing:ident),+ $(,)? }
        wdev { $($wdev_binding:ident: $wdev_cell:ident => $wdev_backing:ident),+ $(,)? }
    ) => {
        unsafe extern "C" {
            $(
                static mut $net_cell: *mut u8;
                static mut $net_backing: u8;
            )+
            $(
                static mut $wdev_cell: *mut u8;
                static mut $wdev_backing: u8;
            )+
        }

        unsafe fn write_net80211_fixed_bindings() {
            $(
                ptr::addr_of_mut!($net_cell).write_volatile(
                    ptr::addr_of_mut!($net_backing).cast::<u8>(),
                );
            )+
        }

        unsafe fn write_wdev_fixed_bindings() {
            $(
                ptr::addr_of_mut!($wdev_cell).write_volatile(
                    ptr::addr_of_mut!($wdev_backing).cast::<u8>(),
                );
            )+
            write_rust_rate_schedule_bindings();
            write_rust_txop_queue_status_binding();
        }

        unsafe fn validate_fixed_bindings(
        ) -> Result<StaticVendorBindings, StaticVendorBindingError> {
            $(
                if ptr::addr_of!($net_cell).read_volatile()
                    != ptr::addr_of_mut!($net_backing).cast::<u8>()
                {
                    return Err(StaticVendorBindingError {
                        binding: StaticVendorBinding::$net_binding,
                    });
                }
            )+
            $(
                if ptr::addr_of!($wdev_cell).read_volatile()
                    != ptr::addr_of_mut!($wdev_backing).cast::<u8>()
                {
                    return Err(StaticVendorBindingError {
                        binding: StaticVendorBinding::$wdev_binding,
                    });
                }
            )+
            validate_rust_rate_schedule_bindings()?;
            validate_rust_txop_queue_status_binding()?;
            Ok(StaticVendorBindings { _private: () })
        }
    };
}

unsafe extern "C" {
    static mut rcLoRaSchedTbl_ptr: *mut u8;
    static mut rc11NSchedTbl_ptr: *mut u8;
    static mut rc11BSchedTbl_ptr: *mut u8;
    static mut BasicOFDMSched_ptr: *mut u8;
    static mut g_txop_queue_status_ptr: *mut u8;
}

unsafe fn write_rust_rate_schedule_bindings() {
    ptr::addr_of_mut!(rcLoRaSchedTbl_ptr).write_volatile(crate::rate_schedule::lora_abi_ptr());
    ptr::addr_of_mut!(rc11NSchedTbl_ptr).write_volatile(crate::rate_schedule::dot11n_abi_ptr());
    ptr::addr_of_mut!(rc11BSchedTbl_ptr).write_volatile(crate::rate_schedule::dot11b_abi_ptr());
    ptr::addr_of_mut!(BasicOFDMSched_ptr)
        .write_volatile(crate::rate_schedule::basic_ofdm_abi_ptr());
}

unsafe fn validate_rust_rate_schedule_binding(
    cell: *const *mut u8,
    expected: *mut u8,
    binding: StaticVendorBinding,
) -> Result<(), StaticVendorBindingError> {
    if cell.read_volatile() != expected {
        return Err(StaticVendorBindingError { binding });
    }
    Ok(())
}

unsafe fn validate_rust_rate_schedule_bindings() -> Result<(), StaticVendorBindingError> {
    validate_rust_rate_schedule_binding(
        ptr::addr_of!(rcLoRaSchedTbl_ptr),
        crate::rate_schedule::lora_abi_ptr(),
        StaticVendorBinding::LoraRateSchedule,
    )?;
    validate_rust_rate_schedule_binding(
        ptr::addr_of!(rc11NSchedTbl_ptr),
        crate::rate_schedule::dot11n_abi_ptr(),
        StaticVendorBinding::Dot11nRateSchedule,
    )?;
    validate_rust_rate_schedule_binding(
        ptr::addr_of!(rc11BSchedTbl_ptr),
        crate::rate_schedule::dot11b_abi_ptr(),
        StaticVendorBinding::Dot11bRateSchedule,
    )?;
    validate_rust_rate_schedule_binding(
        ptr::addr_of!(BasicOFDMSched_ptr),
        crate::rate_schedule::basic_ofdm_abi_ptr(),
        StaticVendorBinding::BasicOfdmRateSchedule,
    )
}

unsafe fn write_rust_txop_queue_status_binding() {
    ptr::addr_of_mut!(g_txop_queue_status_ptr)
        .write_volatile(crate::tx_queue::txop_queue_status_abi_ptr());
}

unsafe fn validate_rust_txop_queue_status_binding() -> Result<(), StaticVendorBindingError> {
    if ptr::addr_of!(g_txop_queue_status_ptr).read_volatile()
        != crate::tx_queue::txop_queue_status_abi_ptr()
    {
        return Err(StaticVendorBindingError {
            binding: StaticVendorBinding::TxopQueueStatus,
        });
    }
    Ok(())
}

// The order mirrors the pinned disassembly: first
// net80211_data_ptr_init (12 stores), then wdev_data_init (31 stores).
fixed_bindings! {
    net80211 {
        WifiNvs: g_wifi_nvs => s_wifi_nvs,
        Scan: g_scan => gScanStruct,
        ChannelManager: g_chm => gChmCxt,
        Net80211Interface: g_ic_ptr => g_ic,
        HmacCounters: g_hmac_cnt_ptr => g_hmac_cnt,
        TxCacheQueue: g_tx_cacheq_ptr => s_tx_cacheq,
        MacSleepEnabled: g_mac_sleep_en_ptr => g_mac_sleep_en,
        MeshQuickFunctions: g_esp_mesh_quick_funcs_ptr => esp_mesh_quick_funcs,
        MeshInitialPowerSaveType: g_mesh_init_ps_type_ptr => g_mesh_init_ps_type,
        MeshStarted: g_mesh_is_started_ptr => g_mesh_is_started,
        MeshRoot: g_mesh_is_root_ptr => g_mesh_is_root,
        MeshTopology: g_mesh_topology_ptr => g_mesh_topology,
    }
    wdev {
        TxRxContext: pTxRx => TxRxCxt,
        LmacConfig: lmacConfMib_ptr => lmacConfMib,
        WdevControl: wDevCtrl_ptr => wDevCtrl,
        WdevMacSleep: wDevMacSleep_ptr => wDevMacSleep,
        LmacCounters: g_lmac_cnt_ptr => g_lmac_cnt,
        PpSignalCounters: pp_sig_cnt_ptr => pp_sig_cnt,
        WifiMenuConfig: g_wifi_menuconfig_ptr => g_wifi_menuconfig,
        EsfBufferLists: g_eb_list_desc_ptr => g_eb_list_desc,
        Fragment: s_fragment_ptr => s_fragment,
        InterfaceControl: if_ctrl_ptr => if_ctrl,
        ApNoLongRange: ap_no_lr_ptr => ap_no_lr,
        TrcControl: trc_ctl_ptr => trc_ctl,
        PowerManagementConfig: g_pm_cfg_ptr => g_pm_cfg,
        PowerManagement: g_pm_ptr => g_pm,
        PowerManagementCounters: g_pm_cnt_ptr => g_pm_cnt,
        PpTimerInfo: g_pp_timer_info_ptr => g_pp_timer_info,
        RtsThresholds: g_rts_threshold_bytes_ptr => g_rts_threshold_bytes,
        PowerManagementTwt: g_pm_twt_ptr => g_pm_twt,
        HeMaxApepLengths: g_he_max_apep_length_tab_ptr => g_he_max_apep_length_tab,
        WdevRxDebug: g_wdev_dbg_rx_ptr => g_wdev_dbg_rx,
        PowerManagementBeaconOffset: s_pm_beacon_offset_ptr => s_pm_beacon_offset,
        PowerManagementBeaconOffsetConfig:
            s_pm_beacon_offset_config_ptr => s_pm_beacon_offset_config,
        TbttStart: s_tbttstart_ptr => s_tbttstart,
        OffchannelTxProgress: s_offchan_tx_progress_in_ptr => offchan_tx_progress_in,
        OffchannelPacketLifetime:
            g_offchan_packet_lifetime_ptr => g_offchan_packet_lifetime,
        SendWakeNullTimer: g_send_wake_null_timer_ptr => send_wake_null_timer,
    }
}

/// Run the two audited vendor fixed-storage binding leaves.
///
/// This is intentionally narrower than vendor Wi-Fi initialization: it does
/// not initialize PHY/MAC hardware, allocate buffers, create a task, or start
/// radio processing. It only publishes addresses of already-linked static
/// objects through the S31 ROM ABI cells.
///
/// # Safety
///
/// The caller must serialize this with Wi-Fi initialization and ensure that no
/// ROM or vendor code reads the affected cells concurrently.
pub unsafe fn bind_static_vendor_state() -> Result<StaticVendorBindings, StaticVendorBindingError> {
    net80211_data_ptr_init();
    let _ = wdev_data_init();
    // Replace the five bindings whose backing storage has moved to Rust.
    write_rust_rate_schedule_bindings();
    write_rust_txop_queue_status_binding();
    validate_static_vendor_bindings()
}

/// Publish all 43 fixed backing-object addresses directly from Rust.
///
/// Unlike the vendor net80211 leaf, this has no hidden one-shot guard: the
/// stores are idempotent and serialized ownership is an explicit caller
/// precondition. The rate-schedule and TXOP cells deliberately select
/// Rust-owned state rather than private archive objects. It performs no calls,
/// allocation, waiting, or hardware access.
///
/// # Safety
///
/// The caller must serialize this with Wi-Fi initialization and ensure that no
/// ROM or vendor code reads or writes the affected cells concurrently.
pub unsafe fn bind_static_vendor_state_in_rust(
) -> Result<StaticVendorBindings, StaticVendorBindingError> {
    write_net80211_fixed_bindings();
    write_wdev_fixed_bindings();
    validate_static_vendor_bindings()
}

/// Validate every recovered fixed-state binding without changing any state.
///
/// # Safety
///
/// The bindings must not be concurrently modified by initialization or
/// teardown.
pub unsafe fn validate_static_vendor_bindings(
) -> Result<StaticVendorBindings, StaticVendorBindingError> {
    validate_fixed_bindings()
}

/// Link-time replacement for the guarded net80211 pointer publisher.
#[cfg(feature = "rust-static-bindings-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_net80211_data_ptr_init() {
    write_net80211_fixed_bindings();
}

/// Link-time replacement for the PP/WDEV pointer publisher.
#[cfg(feature = "rust-static-bindings-interpose")]
#[no_mangle]
pub unsafe extern "C" fn __wrap_wdev_data_init() -> i32 {
    write_wdev_fixed_bindings();
    0
}
