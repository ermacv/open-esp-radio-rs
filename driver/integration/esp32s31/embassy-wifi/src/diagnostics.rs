//! Stable, value-only diagnostic observations for ESP32-S31 composition.
//!
//! These types deliberately hide MAC descriptors, raw RX prefixes and
//! protocol-owner enums from HIL. Decoding remains inside the driver graph;
//! observers receive borrowed payloads and copied facts only.

#![cfg(feature = "diagnostics")]

use open_esp_radio_esp32s31_wifi_embassy::diagnostics::network::RxObservedEthernetFrame;
use open_esp_radio_esp32s31_wifi_mac::rx::{
    HeSuSignal, HtDuplicateRxClassification, HtSignal, RxPhyInfo,
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::ConnectedRxEvent;
use open_esp_radio_wifi_softmac::{MacRxEvidence, MacRxMetadata};

/// Evidence provenance retained by a decoded diagnostic fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31RxEvidence<T> {
    Hardware(T),
    Protocol(T),
    Unavailable,
}

impl<T> From<MacRxEvidence<T>> for Esp32s31RxEvidence<T> {
    fn from(evidence: MacRxEvidence<T>) -> Self {
        match evidence {
            MacRxEvidence::HardwareObserved(value) => Self::Hardware(value),
            MacRxEvidence::ProtocolValidated(value) => Self::Protocol(value),
            MacRxEvidence::Unavailable => Self::Unavailable,
        }
    }
}

/// Decoded HE-SU fields required by diagnostic rate evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31HeSuRxObservation {
    pub mcs: u8,
    pub guard_interval_and_ltf: u8,
    pub bandwidth_mhz: u16,
    pub dcm: bool,
    pub ldpc: bool,
}

impl From<HeSuSignal> for Esp32s31HeSuRxObservation {
    fn from(signal: HeSuSignal) -> Self {
        Self {
            mcs: signal.mcs,
            guard_interval_and_ltf: signal.guard_interval_and_ltf.encoding(),
            bandwidth_mhz: signal.bandwidth.mhz(),
            dcm: signal.dcm,
            ldpc: signal.ldpc,
        }
    }
}

/// Decoded HT-SIG fields required by diagnostic rate evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31HtRxObservation {
    pub mcs: u8,
    pub bandwidth_mhz: u16,
    pub short_guard_interval: bool,
    pub aggregation: bool,
    /// True only for the standard MCS32 selector with HT40 geometry.
    pub duplicate_mcs32: bool,
    /// MCS32 was observed with non-HT40 geometry and was not normalized as
    /// duplicate mode.
    pub duplicate_mcs32_width_mismatch: bool,
}

impl From<HtSignal> for Esp32s31HtRxObservation {
    fn from(signal: HtSignal) -> Self {
        let duplicate = signal.ht_duplicate_mcs32_classification();
        Self {
            mcs: signal.mcs,
            bandwidth_mhz: u16::from(signal.channel_width_mhz),
            short_guard_interval: signal.short_guard_interval,
            aggregation: signal.aggregation,
            duplicate_mcs32: matches!(duplicate, HtDuplicateRxClassification::Ht40(_)),
            duplicate_mcs32_width_mismatch: matches!(
                duplicate,
                HtDuplicateRxClassification::Mismatch { .. }
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ht_observation_keeps_valid_mcs32_separate_from_width_mismatch() {
        let valid = Esp32s31HtRxObservation::from(HtSignal {
            mcs: 32,
            channel_width_mhz: 40,
            aggregation: true,
            short_guard_interval: false,
        });
        assert!(valid.duplicate_mcs32);
        assert!(!valid.duplicate_mcs32_width_mismatch);

        let mismatch = Esp32s31HtRxObservation::from(HtSignal {
            mcs: 32,
            channel_width_mhz: 20,
            aggregation: false,
            short_guard_interval: false,
        });
        assert!(!mismatch.duplicate_mcs32);
        assert!(mismatch.duplicate_mcs32_width_mismatch);

        let ordinary = Esp32s31HtRxObservation::from(HtSignal {
            mcs: 7,
            channel_width_mhz: 40,
            aggregation: false,
            short_guard_interval: true,
        });
        assert!(!ordinary.duplicate_mcs32);
        assert!(!ordinary.duplicate_mcs32_width_mismatch);
    }
}

/// Decoded public RX-control facts; no raw header storage escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31DecodedRxPhyObservation {
    pub baseband_format: u8,
    /// Five-bit public RX-control summary. For HT, use [`Self::ht`]'s
    /// seven-bit format-specific MCS field; this byte cannot represent MCS32.
    pub rate: u8,
    pub ht: Option<Esp32s31HtRxObservation>,
    pub he_su: Option<Esp32s31HeSuRxObservation>,
}

impl From<RxPhyInfo> for Esp32s31DecodedRxPhyObservation {
    fn from(phy: RxPhyInfo) -> Self {
        Self {
            baseband_format: phy.baseband_format().raw(),
            rate: phy.rate,
            ht: phy.ht_signal().map(Into::into),
            he_su: phy.he_su_signal().map(Into::into),
        }
    }
}

/// Stable decoded connected-RX event for diagnostics and HIL evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31ConnectedRxObservation<'frame> {
    Beacon {
        s_mpdu: Esp32s31RxEvidence<bool>,
    },
    Ethernet {
        frame: RxObservedEthernetFrame<'frame>,
        s_mpdu: Esp32s31RxEvidence<bool>,
        ampdu: Esp32s31RxEvidence<bool>,
        phy: Esp32s31RxEvidence<Esp32s31DecodedRxPhyObservation>,
    },
    Other,
}

impl<'frame> Esp32s31ConnectedRxObservation<'frame> {
    pub(crate) fn decode(event: ConnectedRxEvent<'frame>, include_phy: bool) -> Self {
        match event {
            ConnectedRxEvent::Beacon { metadata, .. } => Self::Beacon {
                s_mpdu: metadata.s_mpdu.into(),
            },
            ConnectedRxEvent::Ethernet {
                frame, metadata, ..
            } => Self::Ethernet {
                frame: frame.into(),
                s_mpdu: metadata.s_mpdu.into(),
                ampdu: metadata.ampdu.into(),
                phy: if include_phy {
                    decode_phy(metadata)
                } else {
                    Esp32s31RxEvidence::Unavailable
                },
            },
            _ => Self::Other,
        }
    }
}

fn decode_phy(
    metadata: MacRxMetadata<RxPhyInfo>,
) -> Esp32s31RxEvidence<Esp32s31DecodedRxPhyObservation> {
    match metadata.rate {
        MacRxEvidence::HardwareObserved(phy) => {
            Esp32s31RxEvidence::Hardware(phy.into())
        }
        MacRxEvidence::ProtocolValidated(phy) => {
            Esp32s31RxEvidence::Protocol(phy.into())
        }
        MacRxEvidence::Unavailable => Esp32s31RxEvidence::Unavailable,
    }
}

/// Observation-only hook. The decoded event is borrowed synchronously and
/// production delivery proceeds independently after the callback returns.
pub trait Esp32s31ConnectedRxObserver: Sync {
    /// Request decoded PHY evidence for this Ethernet frame.
    ///
    /// PHY decoding is intentionally demand-driven: high-rate diagnostic
    /// observers can sample it without charging every frame in the RX hot
    /// path. The frame is already a stable, decoded driver observation; raw
    /// MAC prefixes never cross this boundary.
    fn requests_phy(&self, frame: RxObservedEthernetFrame<'_>) -> bool;

    fn observe(&self, event: Esp32s31ConnectedRxObservation<'_>);
}
