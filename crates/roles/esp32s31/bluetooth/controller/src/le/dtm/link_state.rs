//! Exact reviewed region of the Direct Test Mode link-state reset.
//!
//! Current `r_sym_ble_VikJlxpO0kioDchKDFeI` and its named same-chip predecessor
//! `r_ble_lll_dtm_reset_link_state` perform the same positional transforms
//! below. The complete body copies the private TX head from link-state
//! `+0x6c` into word `+0x00` and the private RX tail from `+0x70` into word
//! `+0x08` before each scheduler insertion. This module deliberately models
//! only the eight observed words. It is not a complete hardware descriptor,
//! exposes no publication operation and cannot claim an on-air DTM path.

#![forbid(unsafe_code)]

pub use oer_esp32s31_bluetooth_memory::{DtmLinkStateReviewedWords, DtmRole};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{DtmRxHeaderTailProjection, DtmTxHeaderHeadProjection};

/// Source-level default transmit power consumed by the S31 DTM profile.
///
/// The signed dBm value is kept distinct from the private five-bit scheduler
/// image. Every `i8` value has a defined result in the reviewed vendor
/// conversion, including saturation below -15 dBm and above 19 dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmDefaultTxPowerDbm(i8);

impl DtmDefaultTxPowerDbm {
    /// Bind one physical default-power request to this chip-owned DTM profile.
    pub const fn new(dbm: i8) -> Self {
        Self(dbm)
    }

    /// Return the signed dBm request without exposing its hardware encoding.
    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Private ESP32-S31 DTM hardware policy.
///
/// The configuration image remains positional: current evidence proves the
/// value used by the reviewed standalone profile, but not meanings for its
/// individual bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
struct DtmHardwareProfile {
    default_tx_power_dbm: DtmDefaultTxPowerDbm,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmHardwareProfile {
    const REVIEWED_CONFIG: u8 = 3;

    const fn reviewed_esp32s31(default_tx_power_dbm: DtmDefaultTxPowerDbm) -> Self {
        Self {
            default_tx_power_dbm,
        }
    }
}

/// Typed dynamic inputs to one DTM link-state reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct DtmLinkStateReset {
    tx_header_head: Option<DtmTxHeaderHeadProjection>,
    rx_header_tail: Option<DtmRxHeaderTailProjection>,
    hardware_profile: DtmHardwareProfile,
    role: DtmRole,
}

#[cfg(any(target_arch = "riscv32", test))]
impl DtmLinkStateReset {
    /// Bind semantic inputs to the reviewed ESP32-S31 hardware profile.
    ///
    /// Callers cannot supply either the private rounded-power image or the
    /// positional configuration image.
    pub(crate) const fn new(default_tx_power_dbm: DtmDefaultTxPowerDbm, role: DtmRole) -> Self {
        Self {
            tx_header_head: None,
            rx_header_tail: None,
            hardware_profile: DtmHardwareProfile::reviewed_esp32s31(default_tx_power_dbm),
            role,
        }
    }

    /// Apply the complete reviewed reset transforms to the positional words.
    ///
    /// `WORD_00` retains the exact overlap from the reference body: software
    /// first replaces its low-twenty-bit TX-head link, then transforms the
    /// halfword at byte offset `+0x02`. `WORD_08` receives the private RX tail.
    /// `WORD_34` is overwritten only for the RX role.
    pub(crate) const fn apply(
        self,
        current: DtmLinkStateReviewedWords,
    ) -> DtmLinkStateReviewedWords {
        current.apply_reset(
            self.tx_header_head,
            self.rx_header_tail,
            self.hardware_profile.default_tx_power_dbm.dbm(),
            DtmHardwareProfile::REVIEWED_CONFIG,
            self.role,
        )
    }

    /// Return the DTM role encoded by this validated reset.
    pub(crate) const fn role(self) -> DtmRole {
        self.role
    }

    /// Replace both list links with one freshly sampled private-chain pair.
    ///
    /// The consuming memory-graph transaction calls this with links sampled
    /// after taking ownership, so a plan cannot retain stale links from an
    /// earlier event or another graph.
    pub(crate) const fn with_private_links(
        self,
        tx_header_head: DtmTxHeaderHeadProjection,
        rx_header_tail: DtmRxHeaderTailProjection,
    ) -> Self {
        Self {
            tx_header_head: Some(tx_header_head),
            rx_header_tail: Some(rx_header_tail),
            ..self
        }
    }
}
