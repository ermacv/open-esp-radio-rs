// Shared roots and interposition boundaries for the two ESP32-S31 strict
// auditors. Keep this policy in one place: the control-flow proof and the
// linked-state inventory must classify the same vendor graph.

pub const ROOTS: &[&str] = &["wDev_ProcessRxSucData"];

// Ownership-completion classification for every current runtime root.
//
// This is deliberately more conservative than the no-wait call graph.
// "Evidenced MMIO" means the pinned complete body only accesses registers and
// scalar arguments; it is still temporary and must eventually move to the
// radio HAL. Everything not proved to that standard remains stateful/unproven.
// Keep these three sets a disjoint, exhaustive partition of ROOTS.
pub const RUST_BOUNDARIES_WITH_VENDOR_FALLBACK: &[&str] = &["wDev_ProcessRxSucData"];

pub const STATEFUL_OR_UNPROVEN_RUNTIME_ROOTS: &[&str] = &[];

pub const TEMPORARY_EVIDENCED_MMIO_ROOTS: &[&str] = &[];

// The strict Rust PHY sequence calls several absolute ROM leaves whose bytes
// are not present in the final ELF. Audit the pinned archive implementation as
// a conservative control-flow oracle, but do not report its `phy_param` body
// as runtime-reachable state: `channel_switch` no longer calls this function.
pub const STRICT_REFERENCE_ROOTS: &[&str] = &["phy_change_channel"];

// Channel-manager getters are intentionally absent. Strict handoff adopts the
// finite `gChmCxt` selector/table state once; runtime channel lookup and
// home/current checks are then Rust-owned and atomic.

// These pinned leaves only bind fixed archive storage into ROM ABI pointer
// cells. They contain no allocation, wait, indirect call or control-flow
// cycle, and are the first reusable part of a future Rust-owned cold init.
pub const STATIC_BINDING_ROOTS: &[&str] = &["net80211_data_ptr_init", "wdev_data_init"];

// The Rust PM cold-init wrapper supplies fixed storage, then calls only this
// finite callback-table publisher.
pub const STATIC_PM_INIT_ROOTS: &[&str] = &["pm_beacon_offset_funcs_init"];

pub const WRAPPED_VENDOR_BOUNDARIES: &[&str] = &[
    "lmacTxDone",
    "hal_mac_get_txq_state",
    "hal_mac_get_txq_complete",
    "ieee80211_hostapd_beacon_txcb",
    "ieee80211_tx_mgt_cb",
    "wDev_record_ftm_data",
    "pm_on_beacon_rx",
    "pm_on_data_rx",
    "pm_on_data_tx",
    "pm_set_beacon_duration",
    "dbg_read_tx_ppdu",
    "dbg_dump_rx_ppdu",
    "dbg_dump_rx_sigb",
    "wifi_gpio_debug",
    "esp_test_tx_enab_statistics",
    "esp_test_set_rx_error_occurs",
    "esp_test_rx_parse_mu",
    "esp_test_rx_process_complete",
    "wDev_SnifferRxData",
    "wdev_csi_rx_process",
    "wDev_ftm_set_t1t4",
    "wDev_isNANPktInValidSlot",
    "wDev_AppendRxBlocks",
    "ppRxProtoProc",
    "ppRecycleRxPkt",
    "esp_wifi_internal_free_rx_buffer",
    "rc_get_trc",
    "rcUpdateRxDone",
    "wDev_IndicateCtrlFrame",
    "wpa_sm_rx_eapol",
    "wpa_ap_rx_eapol",
    "hal_crypto_set_key_entry",
    "wifi_log",
    "wifi_assert",
    "pp_post",
    "ieee80211_post_hmac_tx",
    "ieee80211_timer_process",
    "chm_start_op",
    "chm_return_home_channel",
    "esf_buf_alloc",
    "esf_buf_recycle",
    "ieee80211_mgmt_output",
    "ieee80211_set_tx_pti",
    "ieee80211_classify",
    "ieee80211_align_eb",
    "ieee80211_crypto_encap",
    "ieee80211_search_node",
    "cnx_node_alloc",
    "cnx_node_search",
    "rcGetSched",
    "ppTxProtoProc",
    "ppProcTxSecFrame",
    "ppTxPkt",
    "rcUpdateTxDone",
];

/// Public runtime entry points which must resolve to an exact Rust symbol.
///
/// Some ESP32-S31 ROM linker exports cannot use ordinary GNU `--wrap`: the
/// generated `__wrap_*` name is itself captured by the absolute ROM alias.
/// Keep these pairs shared by the enforcing and reporting audits so a direct
/// alias cannot disappear from the linked-state report.
pub const REQUIRED_RUNTIME_ALIASES: &[(&str, &str)] = &[
    ("pm_on_data_rx", "__wrap_pm_on_data_rx"),
    ("wDev_AppendRxBlocks", "__wrap_wDev_AppendRxBlocks"),
    ("wDev_DiscardFrame", "wifi_strict_wdev_discard_frame"),
    (
        "wDev_ProcessRxSucData",
        "wifi_strict_wdev_process_rx_success_data",
    ),
    ("ppRecycleRxPkt", "wifi_strict_pp_recycle_rx_pkt"),
    (
        "esp_wifi_internal_free_rx_buffer",
        "wifi_strict_esp_wifi_internal_free_rx_buffer",
    ),
    (
        "esp_test_set_rx_error_occurs",
        "wifi_strict_esp_test_set_rx_error_occurs",
    ),
    ("rcUpdateTxDone", "wifi_strict_rc_update_tx_done"),
    ("rcUpdateAckSnr", "wifi_strict_rc_update_ack_snr"),
    ("rcTxUpdatePer", "wifi_strict_rc_update_tx_per"),
    (
        "trc_update_ifx_phy_mode",
        "wifi_strict_trc_update_ifx_phy_mode",
    ),
    ("rcAttach", "wifi_strict_rc_attach"),
    ("rcUpdatePhyMode", "wifi_strict_rc_update_phy_mode"),
    (
        "rc_get_default_sched",
        "wifi_strict_rc_get_default_schedule",
    ),
    ("rc_get_G6M_sched", "wifi_strict_rc_get_g6m_schedule"),
    (
        "lmacRequestTxopQueue",
        "wifi_strict_lmac_request_txop_queue",
    ),
    (
        "lmacReleaseTxopQueue",
        "wifi_strict_lmac_release_txop_queue",
    ),
    ("hal_get_tsf_time", "wifi_strict_hal_get_tsf_time"),
    (
        "hal_mac_rx_get_last_dscr",
        "wifi_strict_hal_mac_rx_get_last_dscr",
    ),
    ("hal_mac_tx_set_cca", "wifi_strict_hal_mac_tx_set_cca"),
    ("hal_mac_is_txq_valid", "wifi_strict_hal_mac_is_txq_valid"),
    (
        "hal_mac_set_txq_invalid",
        "wifi_strict_hal_mac_set_txq_invalid",
    ),
    ("hal_mac_txq_disable", "wifi_strict_hal_mac_txq_disable"),
    ("hal_mac_set_csi_cbw", "wifi_strict_hal_mac_set_csi_cbw"),
    ("ic_set_mac", "wifi_strict_ic_set_mac"),
    ("ic_set_rx_policy", "wifi_strict_ic_set_rx_policy"),
    (
        "ic_set_rx_policy_ubssid_check",
        "wifi_strict_ic_set_rx_policy_ubssid_check",
    ),
    ("ieee80211_getmgtframe", "wifi_strict_ieee80211_getmgtframe"),
    ("ic_set_key", "wifi_strict_ic_set_key"),
    ("ic_del_key", "wifi_strict_ic_del_key"),
    ("wDev_Insert_KeyEntry", "wifi_strict_wdev_insert_key_entry"),
    ("phy_set_rx_comp_new", "wifi_strict_phy_set_rx_comp_new"),
    ("phy_dc_mem_clr", "wifi_strict_phy_dc_mem_clr"),
    ("phy_bbpll_cal", "wifi_strict_phy_bbpll_cal"),
    (
        "phy_set_tx_gain_mem_new",
        "wifi_strict_phy_set_tx_gain_mem_new",
    ),
    (
        "ieee80211_post_hmac_tx",
        "wifi_strict_ieee80211_post_hmac_tx",
    ),
    (
        "ieee80211_crypto_encap",
        "wifi_strict_ieee80211_crypto_encap",
    ),
    ("ieee80211_align_eb", "wifi_strict_ieee80211_align_eb"),
    ("ieee80211_set_tx_desc", "wifi_strict_ieee80211_set_tx_desc"),
    ("ppTxProtoProc", "wifi_strict_pp_tx_proto_proc"),
    ("ppProcTxSecFrame", "wifi_strict_pp_proc_tx_sec_frame"),
];
