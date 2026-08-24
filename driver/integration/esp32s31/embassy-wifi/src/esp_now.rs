//! Application-facing ESP-NOW resources for explicit connected composition.
//!
//! This module does not implicitly enable ESP-NOW in the stock supervisor.
//! An application starts one bounded mailbox epoch, retains the returned
//! handle, and moves the scheduler owner into `attach_esp_now_tx` from the
//! connected driver's `map_services` composition edge.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub use open_esp_radio_esp32s31_wifi::esp_now::{
    Esp32s31EspNowLongRangeMissing, Esp32s31EspNowLongRangeRate,
    Esp32s31EspNowLongRangeReached, Esp32s31EspNowLongRangeUnsupported,
    Esp32s31EspNowTxConfig, Esp32s31EspNowTxConfigError, Esp32s31EspNowTxError,
};
pub use open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::{
    Esp32s31EspNowConnectedControl, Esp32s31EspNowConnectedControlConfigError,
    Esp32s31EspNowConnectedControlError, Esp32s31EspNowConnectedControlShutdown,
    Esp32s31EspNowTxBinding, EspNowOwnedV1Tx, EspNowTxBackpressure, EspNowTxCancelReason,
    EspNowTxCompletion, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError,
    EspNowTxMailboxShutdown, EspNowTxRuntimeFailure, EspNowTxTerminal, EspNowTxTicket,
    EspNowTxTrySendError, attach_esp_now_tx,
};
pub use open_esp_radio_ieee80211::esp_now::{
    ESP_NOW_V1_MAX_PAYLOAD_LEN, EspNowDestination, EspNowRandomValue, EspNowUnicastAddress,
    EspNowV1WireError,
};
pub use open_esp_radio_wifi_softmac::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, EspNowConfig, EspNowConfigError, EspNowPeerConfig, EspNowPeerId,
    EspNowPeerSecurity, EspNowPeerTableError, EspNowPhyMode, EspNowProtocol,
};

/// Small product default; applications may select another fixed capacity.
pub const ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH: usize = 4;

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
    inner: open_esp_radio_esp32s31_wifi_embassy::roles::station::connected::EspNowTxMailboxResources<
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
