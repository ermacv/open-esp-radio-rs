//! Negotiated TX-protection policy and the ESP32-S31 publication frontier.
//!
//! ERP, HT and HE Operation elements can require protection independently of
//! the selected data rate.  This module keeps those protocol decisions typed
//! and host-testable.  It deliberately does not turn the queue's basic-rate,
//! power, `SW_RTS` or `SW_CTS` fields into an RTS/CTS claim: the reviewed S31
//! sources do not yet establish the complete generated-frame, duration/NAV,
//! retry and queue-clear lifecycle.

use crate::tx::{HeEdcaTxopLimit, HtChannelWidth, LegacyRate, TxPhyRate};

/// ERP Use Protection policy advertised by one infrastructure BSS.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ErpProtectionMode {
    #[default]
    None,
    /// Protect ERP-OFDM traffic, preferring CTS-to-Self for the bounded open
    /// policy until a complete RTS exchange is available.
    CtsToSelf,
}

impl ErpProtectionMode {
    /// Decode the one-byte ERP Information element payload.
    pub const fn from_information(information: Option<u8>) -> Self {
        match information {
            Some(value) if value & 0x02 != 0 => Self::CtsToSelf,
            _ => Self::None,
        }
    }
}

/// Two-bit HT Protection field from the HT Operation element.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HtProtectionMode {
    #[default]
    None,
    Nonmember,
    TwentyMhz,
    NonHtMixed,
}

impl HtProtectionMode {
    /// Decode a complete 24-byte HT Operation IE retained by scan.
    pub const fn from_operation_ie(operation: Option<&[u8; 24]>) -> Self {
        let Some(operation) = operation else {
            return Self::None;
        };
        if operation[0] != 61 || operation[1] != 22 {
            return Self::None;
        }
        match operation[4] & 0x03 {
            1 => Self::Nonmember,
            2 => Self::TwentyMhz,
            3 => Self::NonHtMixed,
            _ => Self::None,
        }
    }
}

/// Finite HE TXOP Duration RTS Threshold in the element's native 32-us units.
///
/// Encodings zero and 1023 select the peer's disabled/default behavior in the
/// recovered parser and therefore do not construct this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeTxopDurationRtsThreshold(u16);

impl HeTxopDurationRtsThreshold {
    pub const fn new(units_32_us: u16) -> Option<Self> {
        if units_32_us == 0 || units_32_us >= 0x03ff {
            None
        } else {
            Some(Self(units_32_us))
        }
    }

    pub const fn units_32_us(self) -> u16 {
        self.0
    }

    /// Whether one explicit nonzero TXOP ceiling proves that the HE PPDU
    /// cannot cross this threshold.
    pub const fn admits_unprotected_txop(self, txop: HeEdcaTxopLimit) -> bool {
        !txop.is_default() && (txop.units_32_us() as u16) <= self.0
    }
}

/// Association/BSS facts which can require an on-air protection exchange.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiTxProtectionPolicy {
    erp: ErpProtectionMode,
    ht: HtProtectionMode,
    he_txop_duration_rts_threshold: Option<HeTxopDurationRtsThreshold>,
}

impl WifiTxProtectionPolicy {
    pub const fn new(
        erp: ErpProtectionMode,
        ht: HtProtectionMode,
        he_txop_duration_rts_threshold: Option<HeTxopDurationRtsThreshold>,
    ) -> Self {
        Self {
            erp,
            ht,
            he_txop_duration_rts_threshold,
        }
    }

    pub const fn erp(self) -> ErpProtectionMode {
        self.erp
    }

    pub const fn ht(self) -> HtProtectionMode {
        self.ht
    }

    pub const fn he_txop_duration_rts_threshold(self) -> Option<HeTxopDurationRtsThreshold> {
        self.he_txop_duration_rts_threshold
    }

    pub const fn with_he_txop_duration_rts_threshold(
        mut self,
        threshold: Option<HeTxopDurationRtsThreshold>,
    ) -> Self {
        self.he_txop_duration_rts_threshold = threshold;
        self
    }

    /// Admit only an exchange which needs no unreviewed physical protection.
    ///
    /// `he_txop` must be the already-bounded effective WMM/integration
    /// ceiling.  `None` is correct for an ordinary HE S-MPDU because that path
    /// currently owns no exact PPDU-duration calculation.
    pub const fn require_unprotected(
        self,
        rate: TxPhyRate,
        receiver: TxProtectionReceiver,
        he_txop: Option<HeEdcaTxopLimit>,
    ) -> Result<(), TxProtectionAdmissionError> {
        let request = match rate {
            TxPhyRate::Legacy(rate) => {
                if matches!(self.erp, ErpProtectionMode::CtsToSelf) && legacy_is_ofdm(rate) {
                    Some(TxProtectionRequest {
                        mechanism: TxProtectionMechanism::CtsToSelf,
                        reason: TxProtectionReason::ErpUseProtection,
                    })
                } else {
                    None
                }
            }
            TxPhyRate::Ht(rate) => {
                let required = match self.ht {
                    HtProtectionMode::None => false,
                    HtProtectionMode::TwentyMhz => {
                        matches!(rate.channel_width, HtChannelWidth::Mhz40)
                    }
                    HtProtectionMode::Nonmember | HtProtectionMode::NonHtMixed => true,
                };
                if required {
                    Some(TxProtectionRequest {
                        mechanism: receiver.protection_mechanism(),
                        reason: TxProtectionReason::Ht(self.ht),
                    })
                } else {
                    None
                }
            }
            TxPhyRate::He(_) => {
                let Some(threshold) = self.he_txop_duration_rts_threshold else {
                    return Ok(());
                };
                let Some(txop) = he_txop else {
                    return Err(TxProtectionAdmissionError::HePpduDurationUnowned { threshold });
                };
                if threshold.admits_unprotected_txop(txop) {
                    None
                } else {
                    Some(TxProtectionRequest {
                        mechanism: receiver.protection_mechanism(),
                        reason: TxProtectionReason::HeTxopDurationThreshold { threshold, txop },
                    })
                }
            }
        };
        match request {
            Some(request) => {
                Err(TxProtectionAdmissionError::PhysicalPublicationUnverified { request })
            }
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtectionReceiver {
    Individual,
    Group,
}

impl TxProtectionReceiver {
    const fn protection_mechanism(self) -> TxProtectionMechanism {
        match self {
            Self::Individual => TxProtectionMechanism::RtsCts,
            Self::Group => TxProtectionMechanism::CtsToSelf,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtectionMechanism {
    RtsCts,
    CtsToSelf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtectionReason {
    ErpUseProtection,
    Ht(HtProtectionMode),
    HeTxopDurationThreshold {
        threshold: HeTxopDurationRtsThreshold,
        txop: HeEdcaTxopLimit,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxProtectionRequest {
    pub mechanism: TxProtectionMechanism,
    pub reason: TxProtectionReason,
}

/// Exact frontier which prevented a queue publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtectionAdmissionError {
    /// An HE ordinary path has no exact PPDU-duration owner, so it cannot
    /// prove that a finite advertised threshold is not crossed.
    HePpduDurationUnowned {
        threshold: HeTxopDurationRtsThreshold,
    },
    /// Protocol policy requires protection, but the full physical generated
    /// control-frame and queue lifecycle has not been reviewed.
    PhysicalPublicationUnverified { request: TxProtectionRequest },
}

const fn legacy_is_ofdm(rate: LegacyRate) -> bool {
    matches!(
        rate,
        LegacyRate::Ofdm6M
            | LegacyRate::Ofdm9M
            | LegacyRate::Ofdm12M
            | LegacyRate::Ofdm18M
            | LegacyRate::Ofdm24M
            | LegacyRate::Ofdm36M
            | LegacyRate::Ofdm48M
            | LegacyRate::Ofdm54M
    )
}

#[cfg(test)]
mod tests;
