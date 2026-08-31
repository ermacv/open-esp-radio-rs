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

pub use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmLinkStateReviewedWords, BluetoothDtmRole,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmRxHeaderTailProjection, BluetoothDtmTxHeaderHeadProjection,
};

/// Source-level default transmit power consumed by the S31 DTM profile.
///
/// The signed dBm value is kept distinct from the private five-bit scheduler
/// image. Every `i8` value has a defined result in the reviewed vendor
/// conversion, including saturation below -15 dBm and above 19 dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmDefaultTxPowerDbm(i8);

impl BluetoothDtmDefaultTxPowerDbm {
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
struct BluetoothDtmHardwareProfile {
    default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmHardwareProfile {
    const REVIEWED_CONFIG: u8 = 3;

    const fn reviewed_esp32s31(default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm) -> Self {
        Self {
            default_tx_power_dbm,
        }
    }

    /// Reproduce the complete signed-byte conversion used by the current S31
    /// controller. The result is private to the controller-memory codec.
    const fn rounded_power(self) -> u8 {
        match self.default_tx_power_dbm.dbm() {
            i8::MIN..=-16 => 0,
            -15..=-13 => 3,
            -12..=-10 => 4,
            -9..=-7 => 5,
            -6..=-4 => 6,
            -3..=-1 => 7,
            0..=2 => 8,
            3..=5 => 9,
            6..=8 => 10,
            9..=11 => 11,
            12..=14 => 12,
            15..=17 => 13,
            18..=19 => 14,
            20..=i8::MAX => 15,
        }
    }
}

/// Typed dynamic inputs to one DTM link-state reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) struct BluetoothDtmLinkStateReset {
    tx_header_head: Option<BluetoothDtmTxHeaderHeadProjection>,
    rx_header_tail: Option<BluetoothDtmRxHeaderTailProjection>,
    hardware_profile: BluetoothDtmHardwareProfile,
    role: BluetoothDtmRole,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothDtmLinkStateReset {
    /// Bind semantic inputs to the reviewed ESP32-S31 hardware profile.
    ///
    /// Callers cannot supply either the private rounded-power image or the
    /// positional configuration image.
    pub(crate) const fn new(
        default_tx_power_dbm: BluetoothDtmDefaultTxPowerDbm,
        role: BluetoothDtmRole,
    ) -> Self {
        Self {
            tx_header_head: None,
            rx_header_tail: None,
            hardware_profile: BluetoothDtmHardwareProfile::reviewed_esp32s31(default_tx_power_dbm),
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
        current: BluetoothDtmLinkStateReviewedWords,
    ) -> BluetoothDtmLinkStateReviewedWords {
        current.apply_reset(
            self.tx_header_head,
            self.rx_header_tail,
            self.hardware_profile.rounded_power(),
            BluetoothDtmHardwareProfile::REVIEWED_CONFIG,
            self.role,
        )
    }

    /// Return the DTM role encoded by this validated reset.
    pub(crate) const fn role(self) -> BluetoothDtmRole {
        self.role
    }

    /// Replace both list links with one freshly sampled private-chain pair.
    ///
    /// The consuming memory-graph transaction calls this with links sampled
    /// after taking ownership, so a plan cannot retain stale links from an
    /// earlier event or another graph.
    pub(crate) const fn with_private_links(
        self,
        tx_header_head: BluetoothDtmTxHeaderHeadProjection,
        rx_header_tail: BluetoothDtmRxHeaderTailProjection,
    ) -> Self {
        Self {
            tx_header_head: Some(tx_header_head),
            rx_header_tail: Some(rx_header_tail),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BluetoothDtmDefaultTxPowerDbm, BluetoothDtmHardwareProfile};

    #[test]
    fn reviewed_profile_preserves_the_complete_signed_dbm_bucketing() {
        let cases = [
            (i8::MIN, 0),
            (-16, 0),
            (-15, 3),
            (-13, 3),
            (-12, 4),
            (-10, 4),
            (-9, 5),
            (-7, 5),
            (-6, 6),
            (-4, 6),
            (-3, 7),
            (-1, 7),
            (0, 8),
            (2, 8),
            (3, 9),
            (5, 9),
            (6, 10),
            (8, 10),
            (9, 11),
            (11, 11),
            (12, 12),
            (14, 12),
            (15, 13),
            (17, 13),
            (18, 14),
            (19, 14),
            (20, 15),
            (i8::MAX, 15),
        ];

        for (dbm, expected_bucket) in cases {
            assert_eq!(
                BluetoothDtmHardwareProfile::reviewed_esp32s31(BluetoothDtmDefaultTxPowerDbm::new(
                    dbm
                ),)
                .rounded_power(),
                expected_bucket,
                "unexpected reviewed S31 power bucket for {dbm} dBm",
            );
        }
    }
}
