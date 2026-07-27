#![no_std]
#![doc = "Experimental heap-free async Wi-Fi runtime for ESP32-S31."]
#![doc = ""]
#![doc = "The primary profile replaces the top-level `ppTask`, OS timers, WPA2"]
#![doc = "continuations, queue ownership, and a growing set of MAC/LMAC/PHY leaves"]
#![doc = "with wake-driven Rust state machines. Remaining finite vendor leaves and"]
#![doc = "cold hardware initialization are isolated behind audited compatibility"]
#![doc = "boundaries; no RTOS Wi-Fi task or runtime allocator is required."]

#[cfg(test)]
extern crate std;

#[cfg(all(feature = "strict-no-wait", feature = "wpa-async-eap"))]
compile_error!(
    "strict-no-wait cannot be combined with wpa-async-eap: the vendor Enterprise EAP leaves still allocate"
);

pub mod adapter;
pub mod allocation;
mod ap_power_save;
mod atomic_once;
mod beacon;
pub mod channel;
mod channel_state;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod channel_switch;
pub mod command;
pub mod context;
pub mod critical;
pub mod crypto;
pub mod data_rx;
pub mod data_tx;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod debug;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod delay;
pub mod diagnostics;
mod direct_api;
#[cfg(all(target_arch = "riscv32", feature = "wpa-async-eap"))]
mod eap;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod esf;
pub mod event;
pub mod event_bridge;
#[cfg(target_arch = "riscv32")]
mod handoff;
pub mod he;
pub mod interrupt;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod lmac;
#[cfg(all(target_arch = "riscv32", feature = "wpa-async-mic"))]
pub mod michael;
mod net80211_align;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod net80211_align_tx;
mod net80211_classify;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod net80211_crypto_tx;
mod net80211_descriptor;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod net80211_descriptor_tx;
mod net80211_encap;
mod net80211_state;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod net80211_timer;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod net80211_tx;
pub mod osi;
// Qualified PHY modules now live only in
// `crates/open-esp-radio-phy-esp32s31`; see `../PORTING_MAP.md`.
pub mod policy;
pub mod queue;
pub mod radio;
mod radio_hal;
mod rate_control;
mod rate_schedule;
pub mod runtime;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod rx;
pub mod rx_ampdu;
mod rx_ownership;
mod rx_proto;
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-rx-ampdu"
))]
mod rx_ampdu_ap;
pub mod rx_ampdu_hw;
mod rx_descriptor;
// Qualified passive-scan parsing and records now live only in
// `crates/open-esp-radio-mac-esp32s31`; see `../PORTING_MAP.md`.
mod sta_link;
#[cfg(target_arch = "riscv32")]
mod static_bindings;
#[cfg(target_arch = "riscv32")]
mod static_cold_init;
#[cfg(target_arch = "riscv32")]
mod static_misc_nvs;
#[cfg(target_arch = "riscv32")]
mod static_pm;
#[cfg(target_arch = "riscv32")]
mod static_pmksa;
#[cfg(target_arch = "riscv32")]
mod static_tbtt;
#[cfg(target_arch = "riscv32")]
mod static_trc;
pub mod strict;
pub mod task;
mod tbtt;
pub mod timer;
pub mod tx_ampdu;
#[cfg(all(target_arch = "riscv32", feature = "hil-ampdu-intercept"))]
mod tx_intercept;
mod tx_mapper;
mod tx_plcp;
mod tx_proto;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod tx_queue;
mod tx_queue_state;
mod tx_rate;
mod tx_security;
mod tx_submit;
#[cfg(feature = "hil-vendor-tx")]
mod tx_trace;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod txdone;
#[cfg(target_arch = "riscv32")]
pub mod vendor;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
mod wdev;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use wdev::{
    indicate_frame_snapshot, rx_metadata_snapshot, rx_recycle_snapshot, WdevIndicateFrameSnapshot,
    WdevRxMetadataSnapshot, WdevRxRecycleSnapshot, WdevRxVendorFallbackSnapshot,
};
pub mod wpa2;
pub mod wpa2_aes;
pub mod wpa2_ap;
pub mod wpa2_ap_async;
pub mod wpa2_crypto;
pub mod wpa2_frames;
pub mod wpa2_io;
pub mod wpa2_retry;
pub mod wpa2_rx;
pub mod wpa2_s31;
pub mod wpa2_sha1;
pub mod wpa2_sta;
pub mod wpa2_sta_async;
pub mod wpa2_state;
pub mod wpa2_txdone;

#[cfg(target_arch = "riscv32")]
pub use adapter::invalid_pp_post_snapshot;
pub use adapter::{
    blocking_probe, internal_event_queue_snapshot, next_timer_deadline_us, radio_queue,
    task_delay_snapshot, timer_alarm_interrupt, timer_snapshot, ShutdownQueueFull,
    TaskDelaySnapshot, DEFAULT_EVENT_BUDGET, INTERNAL_EVENT_QUEUE_CAPACITY, PP_QUEUE_CAPACITY,
    TIMER_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use adapter::{
    configure_wifi_runtime_clock, drain_wifi_initialization_events, patch_pp_runtime_callbacks,
    request_shutdown, static_pp_task_bound, static_wifi_init_locks_bound, take_radio_future,
    take_wifi_runtime, InitializationDrainError, InvalidPpPostSnapshot,
};
pub use allocation::{allocation_probe, AllocationProbe, AllocationSnapshot};
#[cfg(target_arch = "riscv32")]
pub use allocation::{allow_heap_for_wifi_teardown, patch_allocator_probes};
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-cold-api-envelope-storage"
))]
pub use allocation::static_cold_api_envelope_storage_quiescent;
#[cfg(all(target_arch = "riscv32", feature = "rust-static-pp-bar-storage"))]
pub use allocation::static_pp_bar_storage_bound;
#[cfg(all(
    target_arch = "riscv32",
    feature = "rust-static-supplicant-callback-storage"
))]
pub use allocation::static_supplicant_callback_table_bound;
#[cfg(feature = "hil-cold-allocation-trace")]
pub use allocation::{
    cold_allocation_trace_entry, cold_allocation_trace_len, cold_allocation_trace_overflow,
    ColdAllocationTraceEntry, COLD_ALLOCATION_TRACE_CAPACITY,
};
pub use ap_power_save::{ap_power_save_snapshot, ApPowerSaveSnapshot};
pub use channel::{BoundedChannel, Receive, TrySendError};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use channel_state::ChannelStateAdoptionError;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use channel_switch::{
    channel_state_snapshot, channel_switch_snapshot, ChannelStateSnapshot, ChannelSwitchError,
    ChannelSwitchSnapshot,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use phy_channel::PhyChannelStateAdoptionError;
pub use command::{
    command_budget_self_wakes,
    PendingCommandAction, RadioCommandHandler, RadioCommandQueue, RadioCommandReady,
    RadioCommandSnapshot, RadioOwnerFuture, RADIO_COMMAND_CONTEXT_EVENT,
};
pub use context::{current_event, in_radio_context, RadioContextGuard};
#[cfg(target_arch = "riscv32")]
pub use critical::{allow_core_stalls_for_wifi_teardown, patch_critical_section_probes};
pub use critical::{critical_section_probe, CriticalSectionProbe, CriticalSectionSnapshot};
#[cfg(target_arch = "riscv32")]
pub use crypto::{install_precomputed_wpa_pmk, PmkInstallError};
pub use crypto::{
    CryptoFuture, CryptoJob, CryptoJobError, CryptoOperation, InterruptCryptoBackend,
    InterruptCryptoEngine, SoftwarePbkdf2Future, SoftwarePbkdf2Progress, WpaPskJob, AES_BLOCK_SIZE,
    WPA_PBKDF2_ITERATIONS, WPA_PMK_LENGTH, WPA_PSK_PASSPHRASE_CAPACITY, WPA_SSID_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use data_rx::{
    async_wifi_data_rx_installed, install_async_wifi_data_rx, WifiDataRxInstallError,
};
pub use data_rx::{
    poll_receive_wifi_data, receive_wifi_data, rejected_wifi_data_frames, try_receive_wifi_data,
    wifi_data_rx_snapshot, OwnedWifiDataFrame, WifiDataInterface, WifiDataRxSnapshot,
    WIFI_DATA_RX_CAPACITY, WIFI_DATA_RX_FRAME_CAPACITY,
};
pub use data_tx::{
    flush_wifi_data_tx, poll_wifi_data_tx_ready, receive_wifi_data_tx, try_receive_wifi_data_tx,
    try_send_wifi_data, wifi_data_tx_snapshot, OwnedWifiDataTxFrame, WifiDataTxEnqueueError,
    WifiDataTxSnapshot, WIFI_DATA_TX_CAPACITY, WIFI_DATA_TX_FRAME_CAPACITY,
};
pub use direct_api::{
    direct_cold_stop_snapshot, direct_promiscuous_snapshot, direct_reg_mgmt_frame_snapshot,
    direct_reg_rxcb_snapshot,
    direct_set_config_snapshot, direct_set_country_snapshot, direct_set_max_tx_power_snapshot,
    direct_set_inactive_time_snapshot, direct_set_mode_snapshot, direct_set_protocols_snapshot,
    direct_set_ps_snapshot,
    DirectColdStopSnapshot, DirectPromiscuousSnapshot, DirectRegMgmtFrameSnapshot,
    DirectRegRxcbSnapshot, DirectSetConfigSnapshot, DirectSetCountrySnapshot,
    DirectSetInactiveTimeSnapshot, DirectSetMaxTxPowerSnapshot, DirectSetModeSnapshot,
    DirectSetProtocolsSnapshot, DirectSetPsSnapshot,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use delay::{
    direct_delay_snapshot, DirectDelaySiteSnapshot, DirectDelaySnapshot, DIRECT_DELAY_SITE_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use esf::enable_prestart_management_pool;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use esf::{fixed_esf_pool_snapshot, rejected_esf_operations, FixedEsfPoolSnapshot};
pub use event::{PpAction, PpEvent};
#[cfg(target_arch = "riscv32")]
pub use event_bridge::patch_async_event_post;
pub use event_bridge::{EventCopyError, OwnedWifiEvent, WifiEventBridge, WIFI_EVENT_BASE_CAPACITY};
#[cfg(target_arch = "riscv32")]
pub use handoff::{
    arm_pp_task_handoff, begin_pp_task_handoff, install_pp_task_handoff,
    request_armed_pp_task_handoff, PpTaskHandoff, PpTaskHandoffError, PpTaskHandoffInstallError,
    TaskDeleteCompletionRegistrar,
};
pub use he::{
    parse_he20_capabilities, parse_he20_operation, He20Capabilities, He20Operation, HeElementError,
    HeMcsNssSupport, HE_CAPABILITIES_EXTENSION_ID, HE_CAPABILITIES_IE_MIN_LEN,
    HE_OPERATION_EXTENSION_ID, HE_OPERATION_IE_MIN_LEN,
};
pub use interrupt::{InterruptSignal, WaitForInterrupt};
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-vendor-tx"
))]
pub use lmac::{
    lmac_retry_snapshot, lmac_tx_complete_snapshot, LmacRetrySnapshot, LmacTxCompleteSnapshot,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use lmac::{submit_basic_ht_ampdu, LmacAsyncError};
#[cfg(all(target_arch = "riscv32", feature = "wpa-async-mic"))]
pub use michael::{
    async_michael_callback_installed, install_async_michael_callback,
    uninstall_async_michael_callback, MichaelInstallError,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use net80211_timer::{
    rejected_net80211_timer_events, request_initial_ap_beacon, Net80211TimerError,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use net80211_state::{
    net80211_interface_registry_snapshot, Net80211InterfaceRegistrySnapshot,
    Net80211InterfaceRole, Net80211StateAdoptionError,
};
pub use osi::{OsiPpQueue, RawQueueError};
#[cfg(target_arch = "riscv32")]
pub use policy::{
    configure_static_wifi_buffers, disable_dynamic_wifi_buffers, disable_frame_aggregation,
    disable_ftm, disable_vendor_nvs, prepare_strict_runtime, prepare_strict_runtime_before_handoff,
    validate_strict_basic_config, StaticWifiBufferConfig, StrictConfigError, StrictRuntimeError,
    StrictRuntimePreparation, StrictRuntimeProof,
};
pub use queue::{PushError, RadioQueue, RadioQueueSnapshot};
pub use radio::{
    radio_future_snapshot, DispatchControl, PpDispatcher, RadioFuture, RadioFutureSnapshot,
};
pub use runtime::timer_budget_self_wakes;
pub use queue::{waker_cell_snapshot, WakerCellSnapshot};
pub use runtime::WifiRuntimeFuture;
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-rx-ampdu"
))]
pub use rx::expire_rx_ampdu_gap;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use rx::{
    block_ack_rx_snapshot, strict_rx_snapshot, BlockAckRxSnapshot, RxInterruptAdoptionError,
    RxPumpError, StrictRxSnapshot,
};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use rx::RxStateAdoptionError;
pub use rx_ampdu::{
    write_successful_addba_response, RxAddbaResponseError, RxAmpduError, RxAmpduMpdu,
    RxAmpduRelease, RxBlockAckReorder, RX_AMPDU_SLOT_CAPACITY, RX_BLOCK_ACK_MAX_WINDOW,
};
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-rx-ampdu"
))]
pub use rx_ampdu_ap::{
    remove_peer as remove_rx_ampdu_peer, snapshot as rx_ampdu_ap_snapshot,
    wait_for_gap as wait_for_rx_ampdu_gap, RxAmpduApSnapshot, RxAmpduGapFuture,
};
pub use rx_ampdu_hw::{S31RxBlockAckAgreement, S31RxBlockAckAgreementError};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use sta_link::{associate_sta, authenticate_open};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use sta_link::{sta_assoc_snapshot, sta_auth_snapshot};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use sta_link::{sta_link_reset_generation, wait_sta_link_reset_after};
pub use sta_link::{
    StaAssocError, StaAssocSecurityError, StaAssocSnapshot, StaAssociation, StaAuthError,
    StaAuthSnapshot, OPEN_AUTH_DEFAULT_ATTEMPTS, OPEN_AUTH_DEFAULT_TIMEOUT_US,
    STA_ASSOC_DEFAULT_ATTEMPTS, STA_ASSOC_DEFAULT_TIMEOUT_US,
};
#[cfg(target_arch = "riscv32")]
pub use static_bindings::{
    bind_static_vendor_state, bind_static_vendor_state_in_rust, validate_static_vendor_bindings,
    StaticVendorBinding, StaticVendorBindingError, StaticVendorBindings,
};
#[cfg(target_arch = "riscv32")]
pub use static_misc_nvs::static_misc_nvs_bound;
#[cfg(target_arch = "riscv32")]
pub use static_pm::static_pm_functions_bound;
#[cfg(target_arch = "riscv32")]
pub use static_pmksa::static_pmksa_cache_bound;
#[cfg(target_arch = "riscv32")]
pub use static_tbtt::static_tbtt_adaptive_bound;
#[cfg(target_arch = "riscv32")]
pub use static_trc::static_trc_contexts_bound;
pub use strict::{AuditedFuture, StrictAudit, StrictPolicy, StrictViolation};
pub use task::VirtualPpTask;
pub use timer::{RawOsiTimer, RuntimeTimerPool, RuntimeTimerSnapshot, TIMER_CONTEXT_EVENT};
#[cfg(target_arch = "riscv32")]
pub use tx_ampdu::{
    apply_basic_ht_ampdu_completion, assemble_basic_ht_ampdu, prepare_basic_ht_ampdu_chain,
    read_ht_block_ack, restore_basic_ht_ampdu_chain,
};
pub use tx_ampdu::{
    basic_ht_ampdu_assembly, basic_ht_ampdu_completion, decode_ht_block_ack_registers,
    parse_block_ack_action, AddbaRequest, BasicHtAmpduAssemblyError, BasicHtAmpduAssemblyInput,
    BasicHtAmpduAssemblyOutput, BasicHtAmpduChain, BasicHtAmpduChainError,
    BasicHtAmpduCompletionInput, BasicHtAmpduCompletionOutput, BasicHtAmpduFrameCompletionError,
    BasicHtAmpduRestoreError, BlockAckAction, HtAmpduLength, HtAmpduLengthAccumulator,
    HtAmpduLengthError, HtBlockAckReadError, HtBlockAckRegisters, OperationalTxBlockAck,
    TxAmpduBatch, TxAmpduBatchError, TxAmpduCompletion, TxAmpduDisposition, TxAmpduMpdu,
    TxAmpduSlot, TxBlockAckAlarm, TxBlockAckBitmap, TxBlockAckConfig, TxBlockAckError,
    TxBlockAckResponse, TxBlockAckSession, ADDBA_ACTION_BODY_LEN, DELBA_ACTION,
    TX_AMPDU_SLOT_CAPACITY, TX_BLOCK_ACK_MAX_WINDOW,
};
#[cfg(all(target_arch = "riscv32", feature = "hil-ampdu-intercept"))]
pub use tx_intercept::{
    hil_ampdu_hardware_snapshot, hil_ampdu_intercept_pp_map_tx_queue, hil_ampdu_intercept_snapshot,
    hil_pre_enable_mapper_snapshot, HilAmpduHardwareSnapshot, HilAmpduInterceptSnapshot,
    HilPreEnableMapperRecord, HilPreEnableMapperSnapshot, HIL_AMPDU_SIZE_HISTOGRAM_CAPACITY,
    HIL_PRE_ENABLE_MAPPER_RECORD_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use tx_proto::strict_pp_tx_proto_proc;
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use tx_queue::{TxQueueProcessError, TxQueueStateAdoptionError};
#[cfg(all(target_arch = "riscv32", feature = "strict-no-wait"))]
pub use txdone::TxDoneStateAdoptionError;
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-vendor-tx"
))]
pub use tx_queue::{hil_tx_queue_process_snapshot, HilTxQueueProcessSnapshot};
pub use tx_rate::FixedRateScheduleSnapshot;
#[cfg(target_arch = "riscv32")]
pub use tx_rate::{fixed_rate_schedule_snapshot, strict_rate_schedule, try_fixed_rate_schedule};
#[cfg(target_arch = "riscv32")]
pub use tx_security::strict_pp_proc_tx_sec_frame;
#[cfg(feature = "hil-vendor-tx")]
pub use tx_security::{hil_tx_security_rejected_snapshot, HilTxSecurityRejectedSnapshot};
pub use tx_security::{
    strict_ap_beacon_completion_layout, strict_tx_security_layout, ApBeaconCompletionLayout,
    TxSecurityLayoutInput, TxSecurityLayoutOutput,
};
pub use tx_mapper::{tx_mapper_rejection_snapshot, TxMapperRejectionSnapshot};
#[cfg(feature = "hil-vendor-tx")]
pub use tx_trace::{
    freeze_tx_trace, mark_tx_trace_scenario, tx_trace_entry, tx_trace_snapshot, TxTraceEntry,
    TxTraceEvent, TxTraceSnapshot, TX_TRACE_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use txdone::{
    complete_initial_ap_start, strict_management_tx_done_snapshot, InitialApStartError,
    StrictManagementTxDoneSnapshot,
};
#[cfg(all(
    target_arch = "riscv32",
    feature = "strict-no-wait",
    feature = "hil-vendor-tx"
))]
pub use txdone::{
    hil_data_tx_done_snapshot, hil_eapol_tx_done_snapshot, HilDataTxDoneSnapshot,
    HilEapolTxDoneSnapshot,
};
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use vendor::{
    pp_timer_diagnostic_snapshot, vendor_rx_diagnostic_snapshot, PpTimerDiagnosticSnapshot,
    VendorRxDiagnosticSnapshot,
};
#[cfg(target_arch = "riscv32")]
pub use vendor::{VendorDispatchError, VendorPpDispatcher};
pub use wpa2::{
    EapolCopyError, EapolKeyFrame, EapolKeyInfo, EapolKeyMessage, EapolParseError, OwnedEapolFrame,
    Wpa2Ingress, Wpa2IngressError, Wpa2Interface, DEFAULT_EAPOL_FRAME_CAPACITY, EAPOL_HEADER_LEN,
    EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN,
};
pub use wpa2_aes::{
    AsyncWpa2KeyUnwrap, AsyncWpa2KeyWrap, SoftwareAesKeyUnwrapError, SoftwareAesKeyWrapError,
    Wpa2SoftwareAes, Wpa2UnwrappedKeyData, Wpa2WrappedKeyData, WPA2_WRAPPED_KEY_DATA_CAPACITY,
};
#[cfg(target_arch = "riscv32")]
pub use wpa2_ap::{
    ap_association_rejection_snapshot, ap_association_response_snapshot,
    async_wpa2_ap_callbacks_installed, deferred_ap_management_snapshot,
    install_async_wpa2_ap_callbacks, management_tx_rejection_snapshot, Wpa2ApInstallError,
};
pub use wpa2_ap::{
    receive_wpa2_ap_event, rejected_wpa2_ap_events, try_receive_wpa2_ap_event,
    validate_wpa2_ap_rsn, wpa2_ap_join_snapshot, ApAssociationRejectionSnapshot,
    ApAssociationResponseSnapshot, DeferredApManagementSnapshot, ManagementTxRejectionSnapshot,
    Wpa2ApJoinSnapshot, Wpa2ApPeerEvent, Wpa2ApRsnError, AP_ASSOCIATION_RESPONSE_CAPTURE_CAPACITY,
    WPA2_AP_ASSOC_CAPACITY,
};
pub use wpa2_ap_async::{
    complete_wpa2_ap_message2, complete_wpa2_ap_message3, complete_wpa2_ap_message4,
    start_wpa2_ap_handshake, Wpa2ApMessage2, Wpa2ApMessage2Error, Wpa2ApMessage3,
    Wpa2ApMessage3Error, Wpa2ApMessage4, Wpa2ApMessage4Error, Wpa2ApStartError,
};
pub use wpa2_crypto::{
    new_key_data_job, new_key_data_wrap_job, new_mic_job, new_ptk_job, new_tx_mic_job, verify_mic,
    Wpa2KeyDataJob, Wpa2KeyDataWrapJob, Wpa2MicJob, Wpa2Ptk, Wpa2PtkJob, WPA2_KCK_LEN,
    WPA2_KEK_LEN, WPA2_KEY_DATA_CAPACITY, WPA2_MIC_OUTPUT_LEN, WPA2_PTK_CONTEXT_LEN, WPA2_PTK_LEN,
    WPA2_TK_LEN, WPA2_UNWRAPPED_KEY_DATA_CAPACITY,
};
pub use wpa2_frames::{
    build_ap_action_frame, build_sta_action_frame, parse_gtk_key_data, OwnedAssociationSecurityIes,
    OwnedRsnIe, Wpa2EthernetFrame, Wpa2FrameError, Wpa2Gtk, Wpa2PlainKeyData, Wpa2TxFrame,
    WPA2_ASSOC_SECURITY_IES_CAPACITY, WPA2_GTK_LEN, WPA2_PLAIN_KEY_DATA_CAPACITY,
    WPA2_RSN_IE_CAPACITY, WPA2_TX_EAPOL_CAPACITY, WPA2_TX_ETHERNET_CAPACITY,
};
pub use wpa2_io::{
    AlignedCcmpKey, StaticKeyTableError, StaticWpa2Keys, TryWpa2Io, Wpa2IoCommand, Wpa2IoFailure,
    Wpa2IoHandler, Wpa2IoQueue, Wpa2KeyInstall, Wpa2KeyKind,
};
pub use wpa2_retry::{Wpa2Retry, Wpa2RetryAction, Wpa2RetryAlarm, Wpa2RetryConfig, Wpa2RetryError};
pub use wpa2_rx::{
    receive_wpa2_eapol, rejected_wpa2_eapol, try_receive_wpa2_eapol, WPA2_RX_CAPACITY,
};
#[cfg(feature = "hil-vendor-tx")]
pub use wpa2_rx::{wpa2_rx_diagnostic_snapshot, Wpa2RxDiagnosticSnapshot};
#[cfg(target_arch = "riscv32")]
pub use wpa2_s31::S31StaticWpa2Io;
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use wpa2_s31::{
    hil_cancel_deferred_ap_transmit, hil_sta_pairwise_key_snapshot, HilStaPairwiseKeySnapshot,
};
pub use wpa2_s31::{S31StaticKeyStorage, S31Wpa2IoError};
pub use wpa2_sha1::{
    AsyncSha1, SoftwareSha1Error, Wpa2Sha1Crypto, Wpa2SoftwareSha1,
    WPA2_SOFTWARE_SHA1_MAX_MESSAGE_LEN,
};
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use wpa2_sta::publish_vendor_wpa2_sta_m2_diagnostic_state;
#[cfg(target_arch = "riscv32")]
pub use wpa2_sta::{
    async_wpa2_sta_callbacks_installed, copy_wpa2_sta_assoc_rsn_ie,
    copy_wpa2_sta_assoc_security_ies, install_async_wpa2_sta_callbacks, Wpa2StaAssocRsnError,
    Wpa2StaInstallError,
};
pub use wpa2_sta::{
    receive_wpa2_sta_link_event, rejected_wpa2_sta_link_events, set_wpa2_sta_handshake_active,
    try_receive_wpa2_sta_link_event, Wpa2StaLinkEvent, WPA2_STA_LINK_CAPACITY,
};
pub use wpa2_sta_async::{
    complete_wpa2_sta_message3, derive_wpa2_sta_message2, AsyncWpa2StaCrypto, Wpa2StaMessage2,
    Wpa2StaMessage2Error, Wpa2StaMessage4, Wpa2StaMessage4Error,
};
pub use wpa2_state::{
    PtkContext, Wpa2ApAction, Wpa2ApPeerError, Wpa2ApPeers, Wpa2ApPhase, Wpa2ApState,
    Wpa2StaAction, Wpa2StaPhase, Wpa2StaState, Wpa2StateError, Wpa2Ticket, Wpa2Transmit,
    Wpa2TxMessage, WPA2_NONCE_LEN,
};
pub use wpa2_txdone::{
    async_wpa2_sta_tx_done_installed, receive_wpa2_sta_tx_done, rejected_wpa2_sta_tx_done,
    try_receive_wpa2_sta_tx_done, Wpa2StaTxDone, Wpa2TxDoneInstallError, WPA2_STA_TX_DONE_CAPACITY,
};
#[cfg(all(target_arch = "riscv32", feature = "hil-vendor-tx"))]
pub use wpa2_txdone::{hil_completed_eapol_snapshot, HilCompletedEapolSnapshot};
#[cfg(target_arch = "riscv32")]
pub use wpa2_txdone::{install_async_wpa2_sta_tx_done, uninstall_async_wpa2_sta_tx_done};
