//! Receive metadata of one MPDU.

use oer_ieee80211_mac::{channel::Channel, phy::PhyRate};
use oer_time::RadioInstant;

/// Provenance of one receive value.
///
/// Missing data is explicit because deriving a value from an active Block
/// Ack agreement, a protected frame-control bit, or another adjacent
/// condition is not equivalent to observing it in hardware or validating it
/// in the protocol parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxEvidence<T> {
    /// The receive backend decoded this value from a documented hardware
    /// status field.
    HardwareObserved(T),
    /// Portable protocol processing established this value while validating
    /// the frame.
    ProtocolValidated(T),
    /// Neither layer has evidence for this value at the current boundary.
    Unavailable,
}

impl<T> RxEvidence<T> {
    pub const fn as_ref(&self) -> RxEvidence<&T> {
        match self {
            Self::HardwareObserved(value) => RxEvidence::HardwareObserved(value),
            Self::ProtocolValidated(value) => RxEvidence::ProtocolValidated(value),
            Self::Unavailable => RxEvidence::Unavailable,
        }
    }

    pub const fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    /// The value with its provenance kept; `Unavailable` when `convert`
    /// finds no portable value in it.
    pub fn and_then<U>(self, convert: impl FnOnce(T) -> Option<U>) -> RxEvidence<U> {
        match self {
            Self::HardwareObserved(value) => {
                convert(value).map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved)
            }
            Self::ProtocolValidated(value) => {
                convert(value).map_or(RxEvidence::Unavailable, RxEvidence::ProtocolValidated)
            }
            Self::Unavailable => RxEvidence::Unavailable,
        }
    }
}

/// Crypto result visible for an accepted receive MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxCryptoStatus {
    Unprotected,
    /// The payload is plaintext and its integrity check has succeeded. Key
    /// identity and cipher negotiation remain upper-MAC security state.
    DecryptedAndIntegrityVerified,
}

/// Portable metadata of one received MPDU.
///
/// The backend publishes the physical fields it observed and leaves the
/// rest unavailable; protocol validation above the port may establish more.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxMeta {
    /// The channel the backend was configured to when it received the
    /// frame. This is configuration, not a per-frame observation: the
    /// ESP32-S31 receive prefix carries no channel field the MAC can trust
    /// (`hardware/esp32s31/driver/ieee80211/mac/src/rx.rs`,
    /// `decode_normalized_rx_metadata`).
    pub channel: Channel,
    pub rate: RxEvidence<PhyRate>,
    pub rssi_dbm: RxEvidence<i8>,
    pub noise_floor_dbm: RxEvidence<i8>,
    /// Reception time in the port's radio clock.
    pub timestamp: RxEvidence<RadioInstant>,
    pub crypto: RxEvidence<RxCryptoStatus>,
    /// Whether this is an IEEE VHT/HE S-MPDU: the sole MPDU of an A-MPDU,
    /// its delimiter's EOF bit set. Not a synonym for a non-aggregated MPDU.
    pub s_mpdu: RxEvidence<bool>,
    /// Whether the MPDU was carried in an A-MPDU. An S-MPDU is carried in
    /// one, so it has both `s_mpdu` and `ampdu` true.
    pub ampdu: RxEvidence<bool>,
    /// Whether the MPDU carries an A-MSDU payload.
    pub amsdu: RxEvidence<bool>,
}

impl RxMeta {
    /// Metadata of a frame received on `channel` without any observation.
    pub const fn unavailable(channel: Channel) -> Self {
        Self {
            channel,
            rate: RxEvidence::Unavailable,
            rssi_dbm: RxEvidence::Unavailable,
            noise_floor_dbm: RxEvidence::Unavailable,
            timestamp: RxEvidence::Unavailable,
            crypto: RxEvidence::Unavailable,
            s_mpdu: RxEvidence::Unavailable,
            ampdu: RxEvidence::Unavailable,
            amsdu: RxEvidence::Unavailable,
        }
    }
}
