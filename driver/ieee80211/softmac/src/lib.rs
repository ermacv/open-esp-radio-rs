#![no_std]
#![forbid(unsafe_code)]

//! Portable SoftMAC service contracts and extension protocol owners.
//!
//! [`contract`] describes the boundary between IEEE 802.11 protocol code and a
//! chip MAC backend. [`extensions`] retains peer, protocol and security owners
//! above the lower MAC codecs.
//!
//! A single `hardware_ampdu` or `hardware_retry` flag is too coarse for a
//! split-MAC device.  Hardware may capture a BlockAck bitmap while software
//! owns MPDU retention, or execute one timed transmit attempt while software
//! owns the retry ladder.  The service contract therefore describes each operation
//! independently.
//!
//! The contract describes observable ownership, not a Rust module layout.
//! [`MacOperationOwner::Software`] may be implemented by portable SoftMAC
//! policy, a chip-specific MAC state machine, or their explicit composition. It
//! means only that a caller must not assume the operation is an offload.

pub mod configuration;
pub mod contract;
pub mod extensions;

pub use extensions::espressif::esp_now::{protocol as esp_now, security as esp_now_security};
pub mod interface;
pub mod monitor;

pub use open_esp_radio_ieee80211::channel::WifiChannel;

pub use configuration::{
    WifiAccessPointConfig, WifiConfig, WifiConfigError, WifiMacAddress, WifiMacAddressError,
    WifiMonitorConfig, WifiPlan, WifiStandaloneEspNowPlan, WifiStandaloneMonitorPlan,
    WifiStationConfig,
};
pub use esp_now::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY, EspNowConfig,
    EspNowConfigError, EspNowHt20Rate, EspNowHtGuardInterval, EspNowHtMcs, EspNowOfdmRate,
    EspNowOwnedReceivedV1, EspNowOwnedReceivedV2, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTable, EspNowPeerTableError,
    EspNowPeers, EspNowPhyMode, EspNowPreparedV1Tx, EspNowPreparedV2Tx, EspNowProtocol,
    EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch, EspNowRxOutcome,
    EspNowSendError, EspNowV2ReceiveError, EspNowV2RxOutcome, EspNowV2SendError,
};
pub use esp_now_security::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_KEY_LEN, ESP_NOW_RX_REPLAY_WINDOW_BITS,
    EspNowEncryptedPeerConfig, EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError,
    EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPmk, EspNowPmkError, EspNowPmkId,
    EspNowPmkMutationFailure, EspNowPmkOwner, EspNowPreparedEncryptedV1Tx,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, encrypted_peer_destination,
    esp_now_encrypted_v1_codec_status,
};
pub use monitor::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorChannelPolicy, MonitorChannelSequence,
    MonitorChannelSequenceError, MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameType,
    MonitorFrameTypeMask, MonitorInjectionBindingError, MonitorInjectionChannelBinding,
    MonitorInjectionFrameError, MonitorInjectionFrameType, MonitorInjectionMpdu,
    MonitorInjectionRate, MonitorInjectionRequest, MonitorPublishOutcome, MonitorSink,
};

pub use contract::{
    MacAmpduTxResult, MacAmpduTxStatus, MacInterfaceCapabilities, MacOperationOwner,
    MacOperationOwnership, MacResourceLimits, MacRxCryptoStatus, MacRxEvidence, MacRxMetadata,
    MacServiceCapabilities, MacTxPlan, MacTxQueueState, MacTxResult, MacTxStatus,
};
