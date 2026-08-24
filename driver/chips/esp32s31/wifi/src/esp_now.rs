//! ESP32-S31 handoff for the supported plaintext ESP-NOW v1 TX profile.
//!
//! The portable protocol owner supplies an already validated vendor Action
//! MPDU. This module binds it to the ordinary station queue only after the
//! caller proves that the active channel context is still the configured home
//! channel. Encryption and LR are rejected before descriptor publication.

use core::fmt;

use open_esp_radio_esp32s31_wifi_mac::{
    rate_schedule::{RateScheduleKind, RateScheduleRef},
    tx::{HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate, TxHardware, TxPhyRate},
    tx_runtime::{OrdinaryRetryRatePolicy, P2pRetryRateSchedule},
};
use open_esp_radio_ieee80211::{channel::WifiChannel, wmm::WmmAccessCategory};
use open_esp_radio_wifi_softmac::{
    EspNowHtGuardInterval, EspNowHtMcs, EspNowOfdmRate, EspNowPhyMode, EspNowPreparedV1Tx,
    MacTxPlan, interface::BoundVirtualInterface,
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
    let (initial_rate, retry_rate_policy) = match prepared.phy_mode() {
        EspNowPhyMode::LegacyDsss1M => (
            TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
            OrdinaryRetryRatePolicy::Normal,
        ),
        EspNowPhyMode::StandardP2pOfdm(rate) => {
            let (rate, schedule_index) = p2p_ofdm_rate(rate);
            (
                TxPhyRate::Legacy(rate),
                p2p_retry_policy(RateScheduleKind::P2pDot11G, schedule_index),
            )
        }
        EspNowPhyMode::StandardP2pHt20(rate) => {
            let mcs = esp_now_ht_mcs(rate.mcs());
            let (guard_interval, schedule_index) = match rate.guard_interval() {
                EspNowHtGuardInterval::Long800Ns => {
                    (HtGuardInterval::Long800Ns, 8 - rate.mcs().index())
                }
                EspNowHtGuardInterval::Short400Ns if rate.mcs() == EspNowHtMcs::Mcs7 => {
                    (HtGuardInterval::Short400Ns, 0)
                }
                EspNowHtGuardInterval::Short400Ns => {
                    return Err(Esp32s31EspNowTxError::P2pHtRetryScheduleUnavailable {
                        mcs: rate.mcs(),
                        guard_interval: rate.guard_interval(),
                    });
                }
            };
            (
                TxPhyRate::Ht(HtRate::new(mcs, guard_interval, HtChannelWidth::Mhz20)),
                p2p_retry_policy(RateScheduleKind::P2pDot11N, schedule_index),
            )
        }
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
        .start_with_retry_rate_policy(
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
            retry_rate_policy,
        )
        .map_err(Esp32s31EspNowTxError::Tx)
}

fn p2p_retry_policy(kind: RateScheduleKind, index: u8) -> OrdinaryRetryRatePolicy {
    let schedule = RateScheduleRef::new(kind, index)
        .expect("ESP-NOW uses an in-range finite P2P schedule index");
    let schedule = P2pRetryRateSchedule::new(schedule)
        .expect("ESP-NOW selects only a standard P2P retry arena");
    OrdinaryRetryRatePolicy::P2p(schedule)
}

const fn p2p_ofdm_rate(rate: EspNowOfdmRate) -> (LegacyRate, u8) {
    match rate {
        EspNowOfdmRate::Mbps54 => (LegacyRate::Ofdm54M, 0),
        EspNowOfdmRate::Mbps48 => (LegacyRate::Ofdm48M, 1),
        EspNowOfdmRate::Mbps36 => (LegacyRate::Ofdm36M, 2),
        EspNowOfdmRate::Mbps24 => (LegacyRate::Ofdm24M, 3),
        EspNowOfdmRate::Mbps18 => (LegacyRate::Ofdm18M, 4),
        EspNowOfdmRate::Mbps12 => (LegacyRate::Ofdm12M, 5),
        EspNowOfdmRate::Mbps9 => (LegacyRate::Ofdm9M, 6),
        EspNowOfdmRate::Mbps6 => (LegacyRate::Ofdm6M, 7),
    }
}

const fn esp_now_ht_mcs(mcs: EspNowHtMcs) -> HtMcs {
    match mcs {
        EspNowHtMcs::Mcs0 => HtMcs::Mcs0,
        EspNowHtMcs::Mcs1 => HtMcs::Mcs1,
        EspNowHtMcs::Mcs2 => HtMcs::Mcs2,
        EspNowHtMcs::Mcs3 => HtMcs::Mcs3,
        EspNowHtMcs::Mcs4 => HtMcs::Mcs4,
        EspNowHtMcs::Mcs5 => HtMcs::Mcs5,
        EspNowHtMcs::Mcs6 => HtMcs::Mcs6,
        EspNowHtMcs::Mcs7 => HtMcs::Mcs7,
    }
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
    P2pHtRetryScheduleUnavailable {
        mcs: EspNowHtMcs,
        guard_interval: EspNowHtGuardInterval,
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
            Self::P2pHtRetryScheduleUnavailable {
                mcs,
                guard_interval,
            } => write!(
                formatter,
                "ESP32-S31 has no recovered ESP-NOW P2P retry record for {mcs:?} {guard_interval:?}"
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
