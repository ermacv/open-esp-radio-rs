#[cfg(target_arch = "riscv32")]
use core::sync::atomic::{AtomicU8, Ordering};
#[cfg(target_arch = "riscv32")]
use esp_wifi_sys_esp32s31::include::wifi_init_config_t;

#[cfg(target_arch = "riscv32")]
static STRICT_PREPARATION_STAGE: AtomicU8 = AtomicU8::new(0);

#[cfg(target_arch = "riscv32")]
unsafe extern "C" {
    static mut g_tx_done_cb_func: usize;
    static mut wifi_sta_rx_probe_req: usize;
    fn esp_wifi_set_sta_rx_probe_req(callback: *mut core::ffi::c_void) -> i32;
}

#[cfg(target_arch = "riscv32")]
#[unsafe(no_mangle)]
pub extern "C" fn esp_wifi_async_strict_preparation_debug_stage() -> u8 {
    STRICT_PREPARATION_STAGE.load(Ordering::Acquire)
}

/// Caller-selected capacities for pools owned by the vendor Wi-Fi core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticWifiBufferConfig {
    pub rx: u16,
    pub tx: u16,
    pub management_rx: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictConfigError {
    VendorNvsEnabled,
    DynamicRxBuffers,
    DynamicTxBuffers,
    CachedTxBuffers,
    NonStaticTxBufferType,
    NonStaticManagementBufferType,
    MissingStaticRxBuffers,
    MissingStaticTxBuffers,
    MissingStaticManagementBuffers,
    FrameAggregationEnabled,
    CsiEnabled,
    FtmEnabled,
    DisconnectedPowerSaveEnabled,
    InvalidWifiCore,
}

#[cfg(target_arch = "riscv32")]
pub struct StrictRuntimeProof {
    _private: (),
}

/// Evidence that vendor control APIs were completed while the initialization
/// OS adapter was still active.
#[cfg(target_arch = "riscv32")]
pub struct StrictRuntimePreparation {
    configured_hart: u32,
}

#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrictRuntimeError {
    Config(StrictConfigError),
    StaticVendorBindings(crate::static_bindings::StaticVendorBindingError),
    ChannelStateAdoption(crate::channel_state::ChannelStateAdoptionError),
    PhyChannelStateAdoption(crate::phy_channel::PhyChannelStateAdoptionError),
    Net80211StateAdoption(crate::net80211_state::Net80211StateAdoptionError),
    TxQueueStateAdoption(crate::tx_queue::TxQueueStateAdoptionError),
    TxDoneStateAdoption(crate::txdone::TxDoneStateAdoptionError),
    RxStateAdoption(crate::rx::RxStateAdoptionError),
    ActionRxPolicyAdoption(crate::wdev::WdevActionRxAdoptionError),
    RxInterruptStateAdoption(crate::rx::RxInterruptAdoptionError),
    RuntimeCallbacksNotPatched,
    PpTaskHandoffIncomplete,
    PpPostLinkWrapperMissing,
    Net80211TxLinkWrapperMissing,
    Net80211ClassificationLinkWrapperMissing,
    Net80211AlignmentLinkWrapperMissing,
    Net80211CryptoEncapLinkWrapperMissing,
    Net80211DescriptorLinkWrapperMissing,
    Net80211TxMailboxNotEmpty,
    Net80211RustTxMailboxNotEmpty,
    Net80211TimerLinkWrapperMissing,
    ChannelSwitchLinkWrappersMissing,
    EsfBufferLinkWrappersMissing,
    ManagementTxLinkWrapperMissing,
    AllocatorCallbacksNotPatched,
    CriticalCallbacksNotPatched,
    DirectHeapLinkWrappersMissing,
    DirectDelayLinkWrapperMissing,
    TxLinkWrappersMissing,
    RxLinkWrappersMissing,
    DebugLinkWrappersMissing,
    Wpa2RxLinkWrappersMissing,
    Wpa2KeyLinkWrapperMissing,
    ConnectionBlacklistLinkWrappersMissing,
    Wpa2ApCallbacksNotPatched,
    Wpa2StaTxDoneNotPatched,
    Wpa2StaCallbacksNotPatched,
    WifiDataRxCallbacksNotPatched,
    SetPowerSave(i32),
    ReadPowerSave(i32),
    PowerSaveStillEnabled(u32),
    SetLogLevel(i32),
    ReadLogLevel(i32),
    LoggingStillEnabled(u32),
    DisableUserTxDoneCallback(i32),
    UserTxDoneCallbackStillInstalled,
    DisablePromiscuous(i32),
    ReadPromiscuous(i32),
    PromiscuousStillEnabled,
    DisablePromiscuousCallback(i32),
    DisableStaProbeRequestCallback(i32),
    StaProbeRequestCallbackStillInstalled,
    OptionalRxModesStillEnabled {
        promiscuous: u8,
        dump_errors: u8,
        csi_callback: usize,
    },
    WifiCoreAffinityMismatch {
        configured: u32,
        current: u32,
    },
}

/// Disable all vendor-managed NVS access before `esp_wifi_init`.
///
/// With this policy the application owns credential/config persistence and
/// must restore the desired Wi-Fi configuration explicitly. This is the only
/// way to guarantee that `wifi_nvs_set`/`wifi_nvs_commit` do not enter a flash
/// implementation from a run-to-completion radio handler.
#[cfg(target_arch = "riscv32")]
pub fn disable_vendor_nvs(config: &mut wifi_init_config_t) {
    config.nvs_enable = 0;
}

/// Remove vendor buffer modes that are explicitly labelled dynamic/cache.
///
/// The caller must configure adequate static RX/TX pools and select the
/// target's static `tx_buf_type`/`rx_mgmt_buf_type` values before init. This
/// helper deliberately does not guess those target configuration constants.
#[cfg(target_arch = "riscv32")]
pub fn disable_dynamic_wifi_buffers(config: &mut wifi_init_config_t) {
    config.dynamic_rx_buf_num = 0;
    config.dynamic_tx_buf_num = 0;
    config.cache_tx_buf_num = 0;
}

/// Select fixed vendor pools. Capacities are explicit because silently using
/// the archive defaults would leave S31 with dynamic TX and zero static TX
/// buffers.
#[cfg(target_arch = "riscv32")]
pub fn configure_static_wifi_buffers(
    config: &mut wifi_init_config_t,
    pools: StaticWifiBufferConfig,
) {
    config.tx_buf_type = 0;
    config.rx_mgmt_buf_type = 0;
    config.static_rx_buf_num = i32::from(pools.rx);
    config.static_tx_buf_num = i32::from(pools.tx);
    config.rx_mgmt_buf_num = i32::from(pools.management_rx);
    disable_dynamic_wifi_buffers(config);
}

/// Disable frame aggregation for the strict, allocation-free AP/STA profile.
///
/// Besides removing vendor-managed aggregation buffers, this makes the
/// AMPDU-only branches in the recovered LMACK TX-timeout path unreachable.
/// The runtime still checks descriptor flags and fails closed if the archive
/// violates this initialization invariant.
#[cfg(target_arch = "riscv32")]
pub fn disable_frame_aggregation(config: &mut wifi_init_config_t) {
    config.ampdu_rx_enable = 0;
    config.ampdu_tx_enable = 0;
    config.amsdu_tx_enable = 0;
    config.rx_ba_win = 0;
    config.sta_disconnected_pm = false;
}

/// Disable Fine Timing Measurement for the strict basic AP/STA profile.
///
/// FTM RX accounting in the pinned blob begins with a 50 microsecond busy
/// delay, so neither initiator nor responder capability is permitted.
#[cfg(target_arch = "riscv32")]
pub fn disable_ftm(config: &mut wifi_init_config_t) {
    use esp_wifi_sys_esp32s31::include::{
        CONFIG_FEATURE_FTM_INITIATOR_BIT, CONFIG_FEATURE_FTM_RESPONDER_BIT,
    };

    config.feature_caps &=
        !u64::from(CONFIG_FEATURE_FTM_INITIATOR_BIT | CONFIG_FEATURE_FTM_RESPONDER_BIT);
}

/// Validate all initialization invariants relied upon by `strict-no-wait`.
/// This function performs no allocation and does not modify the config.
#[cfg(target_arch = "riscv32")]
pub fn validate_strict_basic_config(config: &wifi_init_config_t) -> Result<(), StrictConfigError> {
    if config.nvs_enable != 0 {
        return Err(StrictConfigError::VendorNvsEnabled);
    }
    if config.dynamic_rx_buf_num != 0 {
        return Err(StrictConfigError::DynamicRxBuffers);
    }
    if config.dynamic_tx_buf_num != 0 {
        return Err(StrictConfigError::DynamicTxBuffers);
    }
    if config.cache_tx_buf_num != 0 {
        return Err(StrictConfigError::CachedTxBuffers);
    }
    if config.tx_buf_type != 0 {
        return Err(StrictConfigError::NonStaticTxBufferType);
    }
    if config.rx_mgmt_buf_type != 0 {
        return Err(StrictConfigError::NonStaticManagementBufferType);
    }
    if config.static_rx_buf_num <= 0 {
        return Err(StrictConfigError::MissingStaticRxBuffers);
    }
    if config.static_tx_buf_num <= 0 {
        return Err(StrictConfigError::MissingStaticTxBuffers);
    }
    if config.rx_mgmt_buf_num <= 0 {
        return Err(StrictConfigError::MissingStaticManagementBuffers);
    }
    if config.ampdu_rx_enable != 0
        || config.ampdu_tx_enable != 0
        || config.amsdu_tx_enable != 0
        || config.rx_ba_win != 0
    {
        return Err(StrictConfigError::FrameAggregationEnabled);
    }
    if config.csi_enable != 0 {
        return Err(StrictConfigError::CsiEnabled);
    }
    let ftm_mask = u64::from(
        esp_wifi_sys_esp32s31::include::CONFIG_FEATURE_FTM_INITIATOR_BIT
            | esp_wifi_sys_esp32s31::include::CONFIG_FEATURE_FTM_RESPONDER_BIT,
    );
    if config.feature_caps & ftm_mask != 0 {
        return Err(StrictConfigError::FtmEnabled);
    }
    if config.sta_disconnected_pm {
        return Err(StrictConfigError::DisconnectedPowerSaveEnabled);
    }
    if config.wifi_task_core_id < 0
        || config.wifi_task_core_id as u32 >= esp_wifi_sys_esp32s31::include::SOC_CPU_CORES_NUM
    {
        return Err(StrictConfigError::InvalidWifiCore);
    }
    Ok(())
}

/// Establish runtime invariants after Wi-Fi init and before the async radio
/// owner starts processing events.
///
/// This intentionally performs the vendor control calls outside the strict
/// executor. The returned proof is required by the S31 WPA2 backend, whose
/// lower TX leaves rely on the pre-init Rust OSI callback replacement plus
/// disabled logging and power-save branches.
///
/// # Safety
/// `config` must be the configuration used by the active Wi-Fi instance. No
/// radio event or application TX may execute concurrently, and callers must
/// not re-enable Wi-Fi logging or power save while the proof is in use.
#[cfg(target_arch = "riscv32")]
pub unsafe fn prepare_strict_runtime_before_handoff(
    config: &wifi_init_config_t,
) -> Result<StrictRuntimePreparation, StrictRuntimeError> {
    use esp_wifi_sys_esp32s31::include::{
        esp_wifi_get_promiscuous, esp_wifi_get_ps, esp_wifi_internal_get_log,
        esp_wifi_internal_set_log_level, esp_wifi_set_promiscuous, esp_wifi_set_promiscuous_rx_cb,
        esp_wifi_set_ps, esp_wifi_set_tx_done_cb, wifi_log_level_t_WIFI_LOG_NONE,
        wifi_ps_type_t_WIFI_PS_NONE,
    };

    validate_strict_basic_config(config).map_err(StrictRuntimeError::Config)?;
    crate::static_bindings::validate_static_vendor_bindings()
        .map_err(StrictRuntimeError::StaticVendorBindings)?;
    #[cfg(feature = "strict-no-wait")]
    crate::channel_switch::adopt_vendor_channel_state()
        .map_err(StrictRuntimeError::ChannelStateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    crate::phy_channel::adopt_vendor_phy_channel_state()
        .map_err(StrictRuntimeError::PhyChannelStateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    crate::net80211_state::adopt_vendor_interface_registry()
        .map_err(StrictRuntimeError::Net80211StateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    crate::tx_queue::adopt_vendor_tx_queue_state()
        .map_err(StrictRuntimeError::TxQueueStateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    crate::txdone::adopt_vendor_tx_done_state().map_err(StrictRuntimeError::TxDoneStateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    crate::rx::adopt_vendor_rx_state().map_err(StrictRuntimeError::RxStateAdoption)?;
    STRICT_PREPARATION_STAGE.store(1, Ordering::Release);
    let result = esp_wifi_set_ps(wifi_ps_type_t_WIFI_PS_NONE);
    STRICT_PREPARATION_STAGE.store(2, Ordering::Release);
    if result != 0 {
        return Err(StrictRuntimeError::SetPowerSave(result));
    }
    let mut power_save = u32::MAX;
    let result = esp_wifi_get_ps(&mut power_save);
    STRICT_PREPARATION_STAGE.store(3, Ordering::Release);
    if result != 0 {
        return Err(StrictRuntimeError::ReadPowerSave(result));
    }
    if power_save != wifi_ps_type_t_WIFI_PS_NONE {
        return Err(StrictRuntimeError::PowerSaveStillEnabled(power_save));
    }

    let result = esp_wifi_internal_set_log_level(wifi_log_level_t_WIFI_LOG_NONE);
    STRICT_PREPARATION_STAGE.store(4, Ordering::Release);
    if result != 0 {
        return Err(StrictRuntimeError::SetLogLevel(result));
    }
    let mut log_level = u32::MAX;
    // The S31 archive copies all six `g_log_mod` words (24 bytes) to the
    // second pointer even though the generated vendor header declares a
    // single `uint32_t *`. Supplying one word corrupts the caller's frame.
    let mut log_modules = [0_u32; 6];
    let result = esp_wifi_internal_get_log(&mut log_level, log_modules.as_mut_ptr());
    STRICT_PREPARATION_STAGE.store(5, Ordering::Release);
    if result != 0 {
        return Err(StrictRuntimeError::ReadLogLevel(result));
    }
    if log_level != wifi_log_level_t_WIFI_LOG_NONE {
        return Err(StrictRuntimeError::LoggingStillEnabled(log_level));
    }

    // esp-radio installs an application-facing data-TX observer. The strict
    // radio owner has its own fixed completion channels, so retaining this
    // arbitrary indirect callback would violate both ownership and the
    // no-unproven-call invariant. The STA EAPOL completion callback is a
    // separate registration and remains installed.
    let result = esp_wifi_set_tx_done_cb(None);
    STRICT_PREPARATION_STAGE.store(6, Ordering::Release);
    if result != 0 {
        return Err(StrictRuntimeError::DisableUserTxDoneCallback(result));
    }
    if core::ptr::addr_of!(g_tx_done_cb_func).read() != 0 {
        return Err(StrictRuntimeError::UserTxDoneCallbackStillInstalled);
    }

    let result = esp_wifi_set_promiscuous(false);
    if result != 0 {
        return Err(StrictRuntimeError::DisablePromiscuous(result));
    }
    let mut promiscuous = true;
    let result = esp_wifi_get_promiscuous(&mut promiscuous);
    if result != 0 {
        return Err(StrictRuntimeError::ReadPromiscuous(result));
    }
    if promiscuous {
        return Err(StrictRuntimeError::PromiscuousStillEnabled);
    }
    let result = esp_wifi_set_promiscuous_rx_cb(None);
    if result != 0 {
        return Err(StrictRuntimeError::DisablePromiscuousCallback(result));
    }
    // `wDev_ProcessRxSucData+0x296` dispatches received probe requests through
    // this global callback. It is an optional observation API, not part of
    // ordinary AP/STA management delivery. The pinned setter is one pointer
    // store; verify the ROM-BSS readback before the strict RX root is armed.
    let result = esp_wifi_set_sta_rx_probe_req(core::ptr::null_mut());
    if result != 0 {
        return Err(StrictRuntimeError::DisableStaProbeRequestCallback(result));
    }
    if core::ptr::addr_of!(wifi_sta_rx_probe_req).read() != 0 {
        return Err(StrictRuntimeError::StaProbeRequestCallbackStillInstalled);
    }
    let (promiscuous, dump_errors, csi_callback) = crate::wdev::strict_optional_rx_mode_state();
    if promiscuous != 0 || dump_errors != 0 || csi_callback != 0 {
        return Err(StrictRuntimeError::OptionalRxModesStillEnabled {
            promiscuous,
            dump_errors,
            csi_callback,
        });
    }
    #[cfg(feature = "strict-no-wait")]
    crate::wdev::adopt_action_rx_policy().map_err(StrictRuntimeError::ActionRxPolicyAdoption)?;
    let configured_hart = config.wifi_task_core_id as u32;
    let current_hart = crate::critical::current_hart().min(u32::MAX as usize) as u32;
    STRICT_PREPARATION_STAGE.store(7, Ordering::Release);
    if current_hart != configured_hart {
        return Err(StrictRuntimeError::WifiCoreAffinityMismatch {
            configured: configured_hart,
            current: current_hart,
        });
    }

    #[cfg(feature = "strict-no-wait")]
    {
        if !crate::esf::link_wrappers_active() {
            return Err(StrictRuntimeError::EsfBufferLinkWrappersMissing);
        }
        crate::esf::enable_prearm_management_pool(current_hart as usize);
    }

    STRICT_PREPARATION_STAGE.store(8, Ordering::Release);
    Ok(StrictRuntimePreparation { configured_hart })
}

/// Enter the strict phase without calling back into a vendor control API.
///
/// All vendor configuration and readback must have completed through
/// [`prepare_strict_runtime_before_handoff`] while the initialization OSI
/// table and `ppTask` were still alive.
#[cfg(target_arch = "riscv32")]
pub unsafe fn prepare_strict_runtime(
    config: &wifi_init_config_t,
    preparation: StrictRuntimePreparation,
) -> Result<StrictRuntimeProof, StrictRuntimeError> {
    validate_strict_basic_config(config).map_err(StrictRuntimeError::Config)?;
    if preparation.configured_hart != config.wifi_task_core_id as u32 {
        return Err(StrictRuntimeError::WifiCoreAffinityMismatch {
            configured: preparation.configured_hart,
            current: config.wifi_task_core_id as u32,
        });
    }
    if !crate::handoff::pp_task_handoff_complete() && !crate::adapter::virtual_pp_task_started() {
        return Err(StrictRuntimeError::PpTaskHandoffIncomplete);
    }
    if !crate::adapter::pp_runtime_callbacks_patched() {
        return Err(StrictRuntimeError::RuntimeCallbacksNotPatched);
    }
    if !crate::adapter::pp_post_link_wrapper_active() {
        return Err(StrictRuntimeError::PpPostLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    crate::rx::adopt_rx_interrupt_queue().map_err(StrictRuntimeError::RxInterruptStateAdoption)?;
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_tx::link_wrapper_active() {
        return Err(StrictRuntimeError::Net80211TxLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_tx::adopt_classification_callback()
        || !crate::net80211_tx::classification_link_wrapper_active()
    {
        return Err(StrictRuntimeError::Net80211ClassificationLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_align_tx::link_wrapper_active() {
        return Err(StrictRuntimeError::Net80211AlignmentLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_crypto_tx::link_wrapper_active()
        || !crate::net80211_crypto_tx::adopt_callback()
        || !crate::net80211_crypto_tx::callback_active()
    {
        return Err(StrictRuntimeError::Net80211CryptoEncapLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_descriptor_tx::link_wrapper_active() {
        return Err(StrictRuntimeError::Net80211DescriptorLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_tx::vendor_mailbox_empty() {
        return Err(StrictRuntimeError::Net80211TxMailboxNotEmpty);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_tx::rust_mailbox_empty() {
        return Err(StrictRuntimeError::Net80211RustTxMailboxNotEmpty);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::net80211_timer::timer_process_link_wrapper_active() {
        return Err(StrictRuntimeError::Net80211TimerLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::channel_switch::link_wrappers_active() {
        return Err(StrictRuntimeError::ChannelSwitchLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::esf::link_wrappers_active() {
        return Err(StrictRuntimeError::EsfBufferLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_ap::management_link_wrappers_active() {
        return Err(StrictRuntimeError::ManagementTxLinkWrapperMissing);
    }
    if !crate::allocation::allocator_callbacks_patched() {
        return Err(StrictRuntimeError::AllocatorCallbacksNotPatched);
    }
    if !crate::critical::critical_callbacks_patched() {
        return Err(StrictRuntimeError::CriticalCallbacksNotPatched);
    }
    if !crate::allocation::direct_heap_link_wrappers_active() {
        return Err(StrictRuntimeError::DirectHeapLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::delay::runtime_delay_link_wrapper_active() {
        return Err(StrictRuntimeError::DirectDelayLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::lmac::runtime_tx_link_wrappers_active() {
        return Err(StrictRuntimeError::TxLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wdev::runtime_wdev_link_wrapper_active() {
        return Err(StrictRuntimeError::RxLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::debug::runtime_debug_link_wrappers_active() {
        return Err(StrictRuntimeError::DebugLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_rx::runtime_wpa2_rx_link_wrappers_active() {
        return Err(StrictRuntimeError::Wpa2RxLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_s31::runtime_key_link_wrapper_active() {
        return Err(StrictRuntimeError::Wpa2KeyLinkWrapperMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::scan::connection_blacklist_link_wrappers_active() {
        return Err(StrictRuntimeError::ConnectionBlacklistLinkWrappersMissing);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_ap::async_wpa2_ap_callbacks_installed() {
        return Err(StrictRuntimeError::Wpa2ApCallbacksNotPatched);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_txdone::async_wpa2_sta_tx_done_installed() {
        return Err(StrictRuntimeError::Wpa2StaTxDoneNotPatched);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::wpa2_sta::async_wpa2_sta_callbacks_installed() {
        return Err(StrictRuntimeError::Wpa2StaCallbacksNotPatched);
    }
    #[cfg(feature = "strict-no-wait")]
    if !crate::data_rx::async_wifi_data_rx_installed() {
        return Err(StrictRuntimeError::WifiDataRxCallbacksNotPatched);
    }

    let configured_hart = config.wifi_task_core_id as u32;
    let current_hart = crate::critical::current_hart().min(u32::MAX as usize) as u32;
    if current_hart != configured_hart {
        return Err(StrictRuntimeError::WifiCoreAffinityMismatch {
            configured: configured_hart,
            current: current_hart,
        });
    }
    if !crate::critical::forbid_runtime_core_stalls(configured_hart as usize) {
        return Err(StrictRuntimeError::CriticalCallbacksNotPatched);
    }
    if !crate::allocation::forbid_runtime_heap() {
        return Err(StrictRuntimeError::AllocatorCallbacksNotPatched);
    }

    Ok(StrictRuntimeProof { _private: () })
}
