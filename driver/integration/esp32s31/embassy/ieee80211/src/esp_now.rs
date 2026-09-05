//! Application-facing ESP-NOW resources for explicit connected composition.
//!
//! This module does not implicitly enable ESP-NOW in the stock supervisor.
//! An application starts one bounded mailbox epoch, retains the returned
//! handle, and moves the scheduler owner into `attach_esp_now_tx` from the
//! connected driver's `map_services` composition edge.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub use open_esp_radio::{
    StandaloneEspNowPeerError, StandaloneEspNowRequest, WifiStandaloneEspNowPlan,
};

pub use open_esp_radio_esp32s31_wifi::esp_now::{
    Esp32s31EspNowCryptoDiagnostics, Esp32s31EspNowCryptoError, Esp32s31EspNowKeyOwner,
    Esp32s31EspNowKeySlot, Esp32s31EspNowLongRangeMissing, Esp32s31EspNowLongRangeRate,
    Esp32s31EspNowLongRangeReached, Esp32s31EspNowLongRangeUnsupported, Esp32s31EspNowPhySupport,
    Esp32s31EspNowRxMetadata, Esp32s31EspNowRxRateNormalization, Esp32s31EspNowTxConfig,
    Esp32s31EspNowTxConfigError, Esp32s31EspNowTxError, esp32s31_esp_now_phy_support,
    normalize_esp_now_rx_metadata,
};
#[cfg(target_arch = "riscv32")]
pub use open_esp_radio_esp32s31_wifi_embassy::roles::esp_now::Esp32s31StandaloneEspNowPhyChannelControl;
pub use open_esp_radio_esp32s31_wifi_embassy::roles::esp_now::{
    Esp32s31StandaloneEspNowBinding, Esp32s31StandaloneEspNowBindingError,
    Esp32s31StandaloneEspNowChannelControl, Esp32s31StandaloneEspNowOffChannelRunError,
    Esp32s31StandaloneEspNowOffChannelRunFailure, Esp32s31StandaloneEspNowReceive,
    Esp32s31StandaloneEspNowRunError, Esp32s31StandaloneEspNowRunFailure,
    Esp32s31StandaloneEspNowRunReport, Esp32s31StandaloneEspNowRx,
    Esp32s31StandaloneEspNowRxProgress, Esp32s31StandaloneEspNowService,
    Esp32s31StandaloneEspNowStopError, Esp32s31StandaloneEspNowStopped, EspNowRxMailboxEpochError,
    EspNowRxMailboxResources, EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher,
    EspNowRxReceiver, EspNowV2RxEvent, EspNowV2RxMailboxError,
};
pub use open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::{
    Esp32s31EspNowConnectedControl, Esp32s31EspNowConnectedControlConfigError,
    Esp32s31EspNowConnectedControlError, Esp32s31EspNowConnectedControlShutdown,
    Esp32s31EspNowTxBinding, EspNowOffChannelFailureStage, EspNowOwnedV1Tx, EspNowTxBackpressure,
    EspNowTxCancelReason, EspNowTxCompletion, EspNowTxMailboxEpochError,
    EspNowTxMailboxInvariantError, EspNowTxMailboxShutdown, EspNowTxRuntimeFailure,
    EspNowTxTerminal, EspNowTxTicket, EspNowTxTrySendError, EspNowV2TxTrySendError,
    attach_esp_now_tx,
};
pub use open_esp_radio_ieee80211::esp_now::{
    ESP_NOW_CCMP_HEADER_LEN, ESP_NOW_CCMP_MIC_LEN, ESP_NOW_V1_MAX_PAYLOAD_LEN,
    ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN, ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN,
    ESP_NOW_V2_ACTION_PREFIX_LEN, ESP_NOW_V2_MAX_ACTION_LEN, ESP_NOW_V2_MAX_ELEMENT_COUNT,
    ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN, ESP_NOW_V2_MAX_MPDU_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN,
    ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN, ESP_NOW_V2_VERSION, EspNowCcmpPacketNumber,
    EspNowCcmpPacketNumberError, EspNowDestination, EspNowEncryptedV1Unavailable,
    EspNowProtectedV1Envelope, EspNowProtectedV1WireError, EspNowRandomValue, EspNowUnicastAddress,
    EspNowV1WireError, EspNowV2Action, EspNowV2Element, EspNowV2Elements, EspNowV2Frame,
    EspNowV2Payload, EspNowV2Reassembly, EspNowV2WireError, EspNowVersionError, EspNowWireVersion,
    esp_now_wire_version,
};
pub use open_esp_radio_wifi_softmac::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_KEY_LEN,
    ESP_NOW_RX_REPLAY_WINDOW_BITS, EspNowConfig, EspNowConfigError, EspNowEncryptedPeerConfig,
    EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError, EspNowEncryptedPeerId,
    EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTableError, EspNowPhyMode,
    EspNowPmk, EspNowPmkError, EspNowPmkId, EspNowPmkMutationFailure, EspNowPmkOwner,
    EspNowPreparedEncryptedV1Tx, EspNowPreparedV2Tx, EspNowProtocol, EspNowReceivedV2,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, EspNowV2ReceiveError, EspNowV2RxOutcome,
    EspNowV2SendError, encrypted_peer_destination, esp_now_encrypted_v1_codec_status,
};

/// Failed standalone materialization retains the portable owners unchanged.
pub struct Esp32s31StandaloneEspNowPrepareFailure<const PEERS: usize> {
    pub error: Esp32s31StandaloneEspNowBindingError,
    pub plan: WifiStandaloneEspNowPlan,
    pub protocol: EspNowProtocol<PEERS>,
}

/// Join a portable standalone request to the channel currently held by the
/// exclusive radio owner.
///
/// Integration must tune and verify `active_channel` before calling this
/// function. The returned binding is the only chip-runtime input; this hook
/// intentionally performs no scan, association, off-channel fallback or
/// guessed channel programming.
pub fn prepare_esp32s31_standalone_esp_now<const PEERS: usize>(
    request: StandaloneEspNowRequest<PEERS>,
    active_channel: open_esp_radio_ieee80211::channel::WifiChannel,
    tx: Esp32s31EspNowTxConfig,
) -> Result<
    (EspNowProtocol<PEERS>, Esp32s31StandaloneEspNowBinding),
    Esp32s31StandaloneEspNowPrepareFailure<PEERS>,
> {
    let (plan, protocol) = request.into_parts();
    match Esp32s31StandaloneEspNowBinding::new(plan, &protocol, active_channel, tx) {
        Ok(binding) => Ok((protocol, binding)),
        Err(error) => Err(Esp32s31StandaloneEspNowPrepareFailure {
            error,
            plan,
            protocol,
        }),
    }
}

/// Small product default; applications may select another fixed capacity.
pub const ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH: usize = 4;
pub const ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH: usize = 4;

pub type Esp32s31EspNowTxHandle<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
> = open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowTxHandle<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

pub type Esp32s31EspNowTxMailboxOwner<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
> = open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowTxMailboxOwner<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

/// Statically locatable, allocation-free TX request/completion storage.
pub struct Esp32s31EspNowTxResources<
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
> {
    inner:
        open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowTxMailboxResources<
            CriticalSectionRawMutex,
            CAPACITY,
        >,
}

impl<const CAPACITY: usize> Esp32s31EspNowTxResources<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            inner: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowTxMailboxResources::new(),
        }
    }

    /// Create the next reconnect generation. Retain the handle in the
    /// application and move the owner into [`attach_esp_now_tx`].
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            Esp32s31EspNowTxHandle<'_, CAPACITY>,
            Esp32s31EspNowTxMailboxOwner<'_, CAPACITY>,
        ),
        EspNowTxMailboxEpochError,
    > {
        self.inner.begin_epoch()
    }
}

impl<const CAPACITY: usize> Default for Esp32s31EspNowTxResources<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

pub type Esp32s31EspNowRxPublisher<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH,
> = open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowRxPublisher<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

pub type Esp32s31EspNowRxReceiver<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH,
> = open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowRxReceiver<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

/// Explicit, allocation-free RX mailbox storage for v1 events and v2 payload
/// slots. Applications opt in by placing this owner and attaching its
/// publisher to the connected or standalone normal-RX sink.
pub struct Esp32s31EspNowRxResources<
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH,
> {
    inner: EspNowRxMailboxResources<CriticalSectionRawMutex, CAPACITY>,
}

impl<const CAPACITY: usize> Esp32s31EspNowRxResources<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            inner: EspNowRxMailboxResources::new(),
        }
    }

    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            Esp32s31EspNowRxPublisher<'_, CAPACITY>,
            Esp32s31EspNowRxReceiver<'_, CAPACITY>,
        ),
        EspNowRxMailboxEpochError,
    > {
        self.inner.begin_epoch()
    }
}

impl<const CAPACITY: usize> Default for Esp32s31EspNowRxResources<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
