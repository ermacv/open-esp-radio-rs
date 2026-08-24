//! ESP32-S31 handoff for the supported plaintext ESP-NOW v1 TX profile.
//!
//! The portable protocol owner supplies an already validated vendor Action
//! MPDU. This module binds it to the ordinary station queue only after the
//! caller proves that the active channel context is still the configured home
//! channel. Encryption and LR are rejected before descriptor publication.

use core::fmt;

use open_esp_radio_esp32s31_wifi_mac::tx::{LegacyRate, TxHardware, TxPhyRate};
use open_esp_radio_ieee80211::{channel::WifiChannel, wmm::WmmAccessCategory};
use open_esp_radio_wifi_softmac::{
    EspNowPhyMode, EspNowPreparedV1Tx, MacTxPlan, interface::BoundVirtualInterface,
};

use crate::{
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOwner, OrdinaryTxPlan, TX_METADATA_SIZE,
    },
    tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxProgress, WifiTxTimer},
};

/// Bounded publication policy for one ESP-NOW Action MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31EspNowTxConfig {
    unicast_publication_limit: u8,
    publication_timeout_micros: u64,
}

impl Esp32s31EspNowTxConfig {
    pub const fn new(
        unicast_publication_limit: u8,
        publication_timeout_micros: u64,
    ) -> Result<Self, Esp32s31EspNowTxConfigError> {
        if unicast_publication_limit == 0 {
            return Err(Esp32s31EspNowTxConfigError::ZeroPublicationLimit);
        }
        if publication_timeout_micros == 0 {
            return Err(Esp32s31EspNowTxConfigError::ZeroPublicationTimeout);
        }
        Ok(Self {
            unicast_publication_limit,
            publication_timeout_micros,
        })
    }

    pub const fn unicast_publication_limit(self) -> u8 {
        self.unicast_publication_limit
    }

    pub const fn publication_timeout_micros(self) -> u64 {
        self.publication_timeout_micros
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31EspNowTxConfigError {
    ZeroPublicationLimit,
    ZeroPublicationTimeout,
}

impl fmt::Display for Esp32s31EspNowTxConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroPublicationLimit => "ESP-NOW unicast publication limit must be nonzero",
            Self::ZeroPublicationTimeout => "ESP-NOW publication timeout must be nonzero",
        })
    }
}

impl core::error::Error for Esp32s31EspNowTxConfigError {}

/// Encode and publish one validated plaintext ESP-NOW v1 frame.
///
/// `active_channel` and `active_station` must be read from the role/channel
/// owner while it excludes a concurrent retune. A mismatch is reported before
/// the ordinary TX buffer or descriptor is touched.
pub fn start_esp_now_v1_plaintext<H, P, E, T, const BUFFER_SIZE: usize>(
    ordinary: &mut OrdinaryTxOwner<'_, P, E, T, BUFFER_SIZE>,
    hardware: &mut H,
    prepared: EspNowPreparedV1Tx<'_>,
    active_channel: WifiChannel,
    active_station: BoundVirtualInterface,
    config: Esp32s31EspNowTxConfig,
) -> Result<WifiTxProgress, Esp32s31EspNowTxError>
where
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    if prepared.home_channel() != active_channel {
        return Err(Esp32s31EspNowTxError::ChannelMismatch {
            prepared: prepared.home_channel(),
            active: active_channel,
        });
    }
    if prepared.station() != active_station {
        return Err(Esp32s31EspNowTxError::StationBindingMismatch {
            prepared: prepared.station(),
            active: active_station,
        });
    }
    let initial_rate = match prepared.phy_mode() {
        EspNowPhyMode::LegacyDsss1M => TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        // The recovered LR schedule and AGC enable leaf do not define the
        // missing LR PLCP/RX contract. Never reinterpret 0x29/0x2a here.
        EspNowPhyMode::LongRange => {
            return Err(Esp32s31EspNowTxError::LongRangePlcpUnsupported);
        }
    };

    let frame_length = {
        let buffer = ordinary.buffer_mut().map_err(Esp32s31EspNowTxError::Tx)?;
        let Some(frame_buffer) = buffer.get_mut(TX_METADATA_SIZE..) else {
            return Err(Esp32s31EspNowTxError::Tx(
                OrdinaryTxError::BufferSizeOverflow,
            ));
        };
        prepared
            .encode(frame_buffer)
            .map_err(Esp32s31EspNowTxError::Wire)?
    };
    let publication_limit = if prepared.destination().is_broadcast() {
        1
    } else {
        config.unicast_publication_limit
    };

    ordinary
        .start(
            hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: WmmAccessCategory::Voice,
                    initial_rate,
                    publication_limit,
                    publication_timeout_micros: config.publication_timeout_micros,
                },
                hardware_mic_length: 0,
                // The portable handoff has no encrypted variant and therefore
                // cannot consume any WPA2-owned hardware key slot.
                hardware_key_selector: 0,
                interface: OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
            },
        )
        .map_err(Esp32s31EspNowTxError::Tx)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31EspNowTxError {
    ChannelMismatch {
        prepared: WifiChannel,
        active: WifiChannel,
    },
    StationBindingMismatch {
        prepared: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    LongRangePlcpUnsupported,
    Wire(open_esp_radio_ieee80211::esp_now::EspNowV1WireError),
    Tx(OrdinaryTxError),
}

impl fmt::Display for Esp32s31EspNowTxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelMismatch { prepared, active } => write!(
                formatter,
                "ESP-NOW prepared channel {prepared:?} differs from active channel {active:?}"
            ),
            Self::StationBindingMismatch { prepared, active } => write!(
                formatter,
                "ESP-NOW prepared station binding {prepared:?} differs from active binding {active:?}"
            ),
            Self::LongRangePlcpUnsupported => formatter.write_str(
                "ESP32-S31 ESP-NOW LR transmit is unavailable until the LR PLCP contract is owned",
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
            Self::Tx(error) => write!(formatter, "ESP-NOW ordinary TX error: {error:?}"),
        }
    }
}

impl core::error::Error for Esp32s31EspNowTxError {}
