//! Rust-owned transmit rate-control state.
//!
//! The vendor ABI still passes a stable 0x98-byte record pointer while the
//! surrounding rate-selection code is being migrated.  The storage type below
//! makes that ownership explicit.  Runtime policy is represented separately
//! by [`RateControlState`], so retry accounting and schedule transitions are
//! safe value operations rather than mutations performed by ROM through an
//! untyped pointer.

use crate::rate_schedule::{schedule_state, RateScheduleKind, RateScheduleRef};

pub const RATE_CONTROL_RECORD_SIZE: usize = 0x98;

/// Stable backing for one temporary vendor-compatible rate-control record.
///
/// Unknown fields remain opaque until their readers and writers are migrated.
/// Code outside the target adapter must not interpret `bytes` by offset.
#[repr(C, align(4))]
pub struct RateControlRecord {
    bytes: [u8; RATE_CONTROL_RECORD_SIZE],
}

impl RateControlRecord {
    pub const fn zeroed() -> Self {
        Self {
            bytes: [0; RATE_CONTROL_RECORD_SIZE],
        }
    }
}

/// Instruction-evidenced fields of one 12-byte rate schedule record.
///
/// The remaining bytes select the actual PHY rate and retry sequence.  They
/// stay in the compatibility projection for now; only the mutable schedule
/// state used by TX completion is owned here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateScheduleState {
    pub reference: RateScheduleRef,
    pub retry_limit: u8,
    pub adaptive: u8,
}

/// Safe state mutated by the recovered `rcTxUpdatePer` transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateControlState {
    pub retry_pressure: u8,
    pub weighted_retries: u32,
    pub transmissions: u32,
    pub completed: u32,
    pub reevaluate_after_us: u32,
    pub retry_state_1d: u8,
    pub retry_state_1e: u8,
    pub maximum_schedule_index: u8,
    pub current_schedule: RateScheduleState,
    pub legacy_schedule: RateScheduleRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleSelection {
    Unchanged,
    Selected(RateScheduleRef),
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxPerUpdate {
    pub schedule: ScheduleSelection,
}

impl RateControlState {
    const COUNTER_RESCALE_LIMIT: u32 = 0x0200_0000;
    const REEVALUATE_AFTER_US: u32 = 500_000;

    /// Apply the complete scalar state transition from the pinned
    /// `libpp.a[trc.o]::rcTxUpdatePer` body.
    ///
    /// Arithmetic intentionally follows the RISC-V body: the per-result
    /// penalty is truncated to a byte before it is accumulated, retry pressure
    /// is a wrapping byte, and large counters are halved together.
    pub fn update_tx_per(&mut self, retries: u32) -> TxPerUpdate {
        let penalty = if u32::from(self.current_schedule.retry_limit) < retries {
            self.current_schedule.retry_limit.wrapping_add(2)
        } else {
            retries.wrapping_add(1) as u8
        };

        if self.transmissions.wrapping_add(1) >= Self::COUNTER_RESCALE_LIMIT {
            self.transmissions >>= 1;
            self.weighted_retries >>= 1;
        }
        self.transmissions = self.transmissions.wrapping_add(1);
        self.weighted_retries = self.weighted_retries.wrapping_add(u32::from(penalty));

        match retries {
            0..=2 => self.retry_pressure = 0,
            3..=4 => {}
            5..=7 => self.retry_pressure = self.retry_pressure.wrapping_add(1),
            _ => self.retry_pressure = self.retry_pressure.wrapping_add(2),
        }

        if self.retry_pressure <= 6 {
            return TxPerUpdate {
                schedule: ScheduleSelection::Unchanged,
            };
        }

        // Exact scalar part of `rcClearCurSched`.
        self.current_schedule.adaptive = 0;
        self.reevaluate_after_us = Self::REEVALUATE_AFTER_US;
        self.transmissions = 0;
        self.weighted_retries = 0;
        self.completed = 0;
        self.retry_state_1d = 0;
        self.retry_state_1e = 0;
        self.retry_pressure = 0;

        let next_index = u16::from(self.current_schedule.reference.index) + 1;
        let selected = if u16::from(self.maximum_schedule_index) < next_index {
            self.legacy_schedule.offset(self.maximum_schedule_index)
        } else {
            self.current_schedule.reference.advance()
        };
        let schedule = match selected {
            Some(schedule) => ScheduleSelection::Selected(schedule),
            None => ScheduleSelection::Invalid,
        };
        TxPerUpdate { schedule }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BeamformingReportRate {
    pub mode: u8,
    pub rate: u16,
    pub dcm: bool,
    pub ersu: bool,
    pub ersu_ack: bool,
}

/// Recovered PHY-family discriminator stored in a rate-control record.
///
/// The numeric values are still part of the temporary vendor ABI.  Keeping
/// the family separate from the callback address prevents safe policy from
/// manufacturing or following a C function pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RateIndexMap {
    Dot11B,
    Dot11G,
    Dot11N,
    Dot11Ax,
    Lora,
}

/// Value-only input to the recovered `rcUpdatePhyMode` policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyModeSelectionInput {
    pub phy_type: u8,
    pub he_type: u8,
    pub metric: i32,
    pub p2p: bool,
    pub supplied_highest_rate: u32,
    pub use_supplied_highest_rate: bool,
    pub feature_enabled: bool,
}

/// Complete scalar and schedule result of `rcUpdatePhyMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyModeSelection {
    pub current: RateScheduleRef,
    pub secondary: RateScheduleRef,
    pub fallback: RateScheduleRef,
    pub legacy: RateScheduleRef,
    pub highest_index: u8,
    pub maximum_index: u8,
    pub schedule_count: u8,
    pub index_map: RateIndexMap,
    /// The rate byte used to derive the initial AMPDU limit for HT/HE.
    pub ampdu_limit_rate: Option<u8>,
}

const fn schedule(kind: RateScheduleKind, index: u8) -> RateScheduleRef {
    // Every index used by this recovered finite policy is statically within
    // its corresponding arena. Avoiding Option in the branch code also makes
    // accidental unchecked pointer arithmetic impossible.
    match RateScheduleRef::new(kind, index) {
        Some(reference) => reference,
        None => panic!("invalid recovered rate schedule"),
    }
}

const fn highest_dot11b(rate: u32) -> u8 {
    match rate {
        2 => 3,
        4 => 2,
        11 => 1,
        22 => 0,
        _ => 0,
    }
}

const fn highest_dot11g(rate: u32) -> u8 {
    match rate {
        12 => 7,
        18 => 6,
        24 => 5,
        36 => 4,
        48 => 3,
        72 => 2,
        96 => 1,
        108 => 0,
        _ => 0,
    }
}

const fn highest_dot11n(rate: u32) -> u8 {
    match rate {
        13 => 8,
        26 => 7,
        39 => 6,
        52 => 5,
        78 => 4,
        104 => 3,
        117 => 2,
        130 => 1,
        144 => 0,
        _ => 0,
    }
}

const fn highest_dot11ax(rate: u32) -> u8 {
    match rate {
        17 => 9,
        34 => 8,
        51 => 7,
        68 => 6,
        104 => 5,
        137 => 4,
        154 => 3,
        172 => 2,
        206 => 1,
        229 => 0,
        _ => 0,
    }
}

/// Exact log-free port of `rcGetHighestRateIdx` and its five finite helpers.
///
/// Invalid rate encodings deliberately retain the vendor result of zero.
pub(crate) const fn highest_rate_index(
    phy_type: u8,
    he_type: u8,
    supplied_rate: u32,
    use_supplied_rate: bool,
) -> u8 {
    if !use_supplied_rate {
        return if phy_type == 2 || phy_type == 4 { 1 } else { 0 };
    }
    match phy_type {
        0 => highest_dot11b(supplied_rate),
        1 => highest_dot11g(supplied_rate),
        2..=5 if he_type == 7 => highest_dot11ax(supplied_rate),
        2..=5 => highest_dot11n(supplied_rate),
        _ => {
            if phy_type == 2 || phy_type == 4 {
                1
            } else {
                0
            }
        }
    }
}

const fn ht_metric_index(metric: i32) -> u8 {
    if metric <= 8 {
        11
    } else if metric <= 11 {
        8
    } else if metric <= 13 {
        7
    } else if metric <= 16 {
        6
    } else if metric <= 21 {
        5
    } else if metric <= 26 {
        4
    } else if metric <= 29 {
        3
    } else if metric <= 33 {
        2
    } else if metric <= 36 {
        1
    } else if metric <= 41 {
        0
    } else {
        u8::MAX
    }
}

/// Safe schedule selector recovered from `libpp.a[trc.o]::rcUpdatePhyMode`.
///
/// The result contains values only. The target adapter separately validates
/// record provenance and projects these fields into the temporary 0x98-byte
/// ABI record.
#[inline(never)]
pub(crate) fn select_phy_mode(input: PhyModeSelectionInput) -> PhyModeSelection {
    let highest = highest_rate_index(
        input.phy_type,
        input.he_type,
        input.supplied_highest_rate,
        input.use_supplied_highest_rate,
    );
    let lora_fallback = schedule(RateScheduleKind::Lora, 1);

    let (current, secondary, legacy, maximum_index, schedule_count, index_map, ampdu_limit_rate) =
        match input.phy_type {
            0 => {
                let current_index = if input.use_supplied_highest_rate {
                    highest
                } else {
                    3
                };
                (
                    schedule(RateScheduleKind::Dot11B, current_index),
                    schedule(RateScheduleKind::Dot11B, 3),
                    schedule(RateScheduleKind::Dot11B, 0),
                    if input.feature_enabled { 5 } else { 3 },
                    6,
                    RateIndexMap::Dot11B,
                    None,
                )
            }
            1 => {
                let kind = if input.p2p {
                    RateScheduleKind::P2pDot11G
                } else {
                    RateScheduleKind::Dot11G
                };
                let mut current_index = if input.metric <= 11 {
                    if input.p2p {
                        7
                    } else {
                        10
                    }
                } else if input.metric <= 16 {
                    5
                } else if input.metric <= 21 {
                    3
                } else if input.metric < 30 {
                    2
                } else {
                    0
                };
                if input.use_supplied_highest_rate {
                    current_index = highest;
                }
                (
                    schedule(kind, current_index),
                    if input.p2p {
                        schedule(RateScheduleKind::P2pDot11G, 7)
                    } else {
                        schedule(RateScheduleKind::BasicOfdm, 0)
                    },
                    schedule(kind, 0),
                    if input.p2p {
                        7
                    } else if input.feature_enabled {
                        12
                    } else {
                        10
                    },
                    if input.p2p { 8 } else { 13 },
                    RateIndexMap::Dot11G,
                    None,
                )
            }
            2..=5 => {
                let he = input.he_type == 7;
                let kind = if he {
                    RateScheduleKind::Dot11Ax
                } else {
                    RateScheduleKind::Dot11N
                };
                let threshold = ht_metric_index(input.metric);
                let adjustment = if he { 2 } else { 0 };
                let mut current_index = if threshold == u8::MAX || threshold <= highest {
                    highest
                } else {
                    threshold + adjustment
                };
                if input.p2p && current_index > 7 {
                    current_index = 7;
                }
                if input.use_supplied_highest_rate {
                    current_index = highest;
                }
                let limit_index = if current_index > 8 { 8 } else { current_index };
                let limit_schedule = schedule(kind, limit_index);
                (
                    schedule(kind, current_index),
                    if input.p2p {
                        schedule(RateScheduleKind::P2pDot11G, 7)
                    } else {
                        schedule(RateScheduleKind::BasicOfdm, 0)
                    },
                    schedule(kind, 0),
                    if input.feature_enabled {
                        if he {
                            15
                        } else {
                            13
                        }
                    } else if he {
                        13
                    } else {
                        11
                    },
                    if he { 16 } else { 14 },
                    if he {
                        RateIndexMap::Dot11Ax
                    } else {
                        RateIndexMap::Dot11N
                    },
                    Some(schedule_state(limit_schedule).rate),
                )
            }
            6 => (
                schedule(RateScheduleKind::Lora, 0),
                schedule(RateScheduleKind::Lora, 0),
                schedule(RateScheduleKind::Lora, 0),
                1,
                2,
                RateIndexMap::Lora,
                None,
            ),
            _ => (
                schedule(RateScheduleKind::Dot11B, 3),
                schedule(RateScheduleKind::Dot11B, 3),
                schedule(RateScheduleKind::Dot11B, 0),
                if input.feature_enabled { 5 } else { 3 },
                6,
                RateIndexMap::Dot11B,
                None,
            ),
        };

    PhyModeSelection {
        current,
        secondary,
        fallback: if input.feature_enabled {
            lora_fallback
        } else {
            secondary
        },
        legacy,
        highest_index: highest,
        maximum_index,
        schedule_count,
        index_map,
        ampdu_limit_rate,
    }
}

/// Pure rate-code callback policy stored temporarily at record offset 0x78.
pub(crate) const fn rate_to_schedule_index(map: RateIndexMap, rate: u8) -> u8 {
    match map {
        RateIndexMap::Dot11B => {
            const MAP: [u8; 43] = [
                3, 2, 1, 0, 0xff, 2, 1, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 5, 4,
            ];
            if rate <= 42 {
                MAP[rate as usize]
            } else {
                0xff
            }
        }
        RateIndexMap::Dot11G => {
            const MAP: [u8; 43] = [
                10, 9, 8, 0xff, 0xff, 9, 8, 0xff, 1, 3, 5, 7, 0, 2, 4, 6, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            ];
            if rate <= 42 {
                MAP[rate as usize]
            } else {
                0xff
            }
        }
        RateIndexMap::Dot11N => match rate {
            0 => 11,
            1 => 10,
            2 => 9,
            3 | 4 => 24_u8.wrapping_sub(rate),
            5 => 10,
            6 => 9,
            0x21 => 0,
            0x29 => 13,
            0x2a => 12,
            _ => 24_u8.wrapping_sub(rate),
        },
        RateIndexMap::Dot11Ax => match rate {
            0 => 13,
            1 => 12,
            2 => 11,
            3 | 4 => 26_u8.wrapping_sub(rate),
            5 => 12,
            6 => 11,
            0x17 => 3,
            0x19 => 1,
            0x22 => 2,
            0x23 => 0,
            0x29 => 15,
            0x2a => 14,
            _ => 26_u8.wrapping_sub(rate),
        },
        RateIndexMap::Lora => match rate {
            0x29 => 1,
            0x2a => 0,
            _ => 0xff,
        },
    }
}

/// Locate the complete vendor 802.11g retry record for a legacy rate code.
///
/// SOURCE: `_oracles/libpp.a[trc.o]` callback table used by
/// `rcUpdatePhyMode`, recovered above as the `RateIndexMap::Dot11G` branch;
/// the pointed-to record bytes come from `_oracles/libpp.a` rate-schedule
/// arenas in [`crate::rate_schedule`].
pub(crate) const fn dot11g_schedule_for_legacy_rate(rate: u8) -> Option<RateScheduleRef> {
    let index = rate_to_schedule_index(RateIndexMap::Dot11G, rate);
    if index == 0xff {
        None
    } else {
        RateScheduleRef::new(RateScheduleKind::Dot11G, index)
    }
}

/// Exact fixed table behind the vendor `rx11NRate2AMPDULimit` leaf.
pub(crate) const fn ampdu_limit_for_rate(rate: u8) -> u16 {
    const LIMITS: [u16; 18] = [
        6490, 9600, 19200, 25600, 43200, 50000, 57600, 65535, 0, 6490, 9600, 19200, 25600, 43200,
        50000, 57600, 65535, 0,
    ];
    let index = rate.wrapping_sub(0x10) as usize;
    if index < LIMITS.len() {
        LIMITS[index]
    } else {
        0
    }
}

/// Safe policy recovered from `trc_set_bf_report_rate`.
pub(crate) const fn beamforming_report_rate(
    filtered_ack_snr: u8,
    quarter_noise_floor: i32,
    he_feature_8f: bool,
    he_feature_90: bool,
) -> BeamformingReportRate {
    let metric = filtered_ack_snr.wrapping_sub(quarter_noise_floor as u8) as i8;
    beamforming_report_rate_for_metric(metric as i32, he_feature_8f, he_feature_90)
}

/// Direct form used by `rcUpdatePhyMode`, whose caller already supplies the
/// signed link metric rather than ACK SNR and noise-floor components.
pub(crate) const fn beamforming_report_rate_for_metric(
    metric: i32,
    he_feature_8f: bool,
    he_feature_90: bool,
) -> BeamformingReportRate {
    if metric > 13 {
        BeamformingReportRate {
            mode: 1,
            rate: 16,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    } else if he_feature_8f && he_feature_90 {
        BeamformingReportRate {
            mode: 2,
            rate: 16,
            dcm: true,
            ersu: true,
            ersu_ack: true,
        }
    } else {
        BeamformingReportRate {
            mode: 0,
            rate: 11,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_schedule::{RateScheduleKind, RateScheduleRef};

    fn state() -> RateControlState {
        RateControlState {
            retry_pressure: 4,
            weighted_retries: 10,
            transmissions: 20,
            completed: 30,
            reevaluate_after_us: 1,
            retry_state_1d: 2,
            retry_state_1e: 3,
            maximum_schedule_index: 5,
            current_schedule: RateScheduleState {
                reference: RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap(),
                retry_limit: 7,
                adaptive: 1,
            },
            legacy_schedule: RateScheduleRef::new(RateScheduleKind::Dot11B, 0).unwrap(),
        }
    }

    #[test]
    fn retry_bands_match_the_pinned_transition() {
        let mut low = state();
        assert_eq!(low.update_tx_per(2).schedule, ScheduleSelection::Unchanged);
        assert_eq!(low.retry_pressure, 0);
        assert_eq!(low.transmissions, 21);
        assert_eq!(low.weighted_retries, 13);

        let mut middle = state();
        middle.update_tx_per(4);
        assert_eq!(middle.retry_pressure, 4);

        let mut high = state();
        high.update_tx_per(6);
        assert_eq!(high.retry_pressure, 5);

        let mut very_high = state();
        very_high.update_tx_per(8);
        assert_eq!(very_high.retry_pressure, 6);
        // retry_limit < retries selects retry_limit + 2, not retries + 1.
        assert_eq!(very_high.weighted_retries, 19);
    }

    #[test]
    fn large_counters_are_rescaled_before_accumulation() {
        let mut value = state();
        value.transmissions = 0x01ff_ffff;
        value.weighted_retries = 100;
        value.update_tx_per(0);
        assert_eq!(value.transmissions, 0x0100_0000);
        assert_eq!(value.weighted_retries, 51);
    }

    #[test]
    fn pressure_threshold_clears_state_and_advances_schedule() {
        let mut value = state();
        value.retry_pressure = 6;
        let update = value.update_tx_per(5);
        assert_eq!(
            update.schedule,
            ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11N, 3).unwrap())
        );
        assert_eq!(value.retry_pressure, 0);
        assert_eq!(value.weighted_retries, 0);
        assert_eq!(value.transmissions, 0);
        assert_eq!(value.completed, 0);
        assert_eq!(value.reevaluate_after_us, 500_000);
        assert_eq!(value.retry_state_1d, 0);
        assert_eq!(value.retry_state_1e, 0);
        assert_eq!(value.current_schedule.adaptive, 0);
    }

    #[test]
    fn last_schedule_falls_back_to_the_legacy_table() {
        let mut value = state();
        value.retry_pressure = 6;
        value.maximum_schedule_index = 2;
        value.current_schedule.reference =
            RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap();
        assert_eq!(
            value.update_tx_per(5).schedule,
            ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11B, 2).unwrap())
        );
    }

    #[test]
    fn invalid_schedule_transition_is_explicit() {
        let mut value = state();
        value.retry_pressure = 6;
        value.maximum_schedule_index = 6;
        value.current_schedule.reference =
            RateScheduleRef::new(RateScheduleKind::Dot11N, 13).unwrap();
        assert_eq!(value.update_tx_per(5).schedule, ScheduleSelection::Invalid);
    }

    #[test]
    fn byte_pressure_wrap_is_preserved() {
        let mut value = state();
        value.retry_pressure = 0xff;
        value.update_tx_per(8);
        assert_eq!(value.retry_pressure, 1);
    }

    #[test]
    fn beamforming_policy_has_three_exact_modes() {
        assert_eq!(
            beamforming_report_rate(40, 20, true, true),
            BeamformingReportRate {
                mode: 1,
                rate: 16,
                dcm: false,
                ersu: false,
                ersu_ack: false,
            }
        );
        assert_eq!(
            beamforming_report_rate(20, 20, true, true),
            BeamformingReportRate {
                mode: 2,
                rate: 16,
                dcm: true,
                ersu: true,
                ersu_ack: true,
            }
        );
        assert_eq!(
            beamforming_report_rate(20, 20, true, false),
            BeamformingReportRate {
                mode: 0,
                rate: 11,
                dcm: false,
                ersu: false,
                ersu_ack: false,
            }
        );
    }

    fn selection_input(phy_type: u8) -> PhyModeSelectionInput {
        PhyModeSelectionInput {
            phy_type,
            he_type: 0,
            metric: 20,
            p2p: false,
            supplied_highest_rate: 0,
            use_supplied_highest_rate: false,
            feature_enabled: false,
        }
    }

    #[test]
    fn highest_rate_tables_match_recovered_boundaries() {
        assert_eq!(highest_rate_index(0, 0, 2, true), 3);
        assert_eq!(highest_rate_index(0, 0, 22, true), 0);
        assert_eq!(highest_rate_index(1, 0, 12, true), 7);
        assert_eq!(highest_rate_index(1, 0, 108, true), 0);
        assert_eq!(highest_rate_index(2, 0, 13, true), 8);
        assert_eq!(highest_rate_index(2, 0, 144, true), 0);
        assert_eq!(highest_rate_index(2, 7, 17, true), 9);
        assert_eq!(highest_rate_index(2, 7, 229, true), 0);
        assert_eq!(highest_rate_index(2, 0, 0, false), 1);
        assert_eq!(highest_rate_index(3, 0, 0, false), 0);
        assert_eq!(highest_rate_index(4, 0, 0, false), 1);
    }

    #[test]
    fn phy_mode_selector_covers_legacy_p2p_ht_he_and_lora() {
        let dot11b = select_phy_mode(selection_input(0));
        assert_eq!(dot11b.current, schedule(RateScheduleKind::Dot11B, 3));
        assert_eq!(dot11b.maximum_index, 3);
        assert_eq!(dot11b.schedule_count, 6);

        let mut dot11g_input = selection_input(1);
        dot11g_input.metric = 12;
        dot11g_input.p2p = true;
        let dot11g = select_phy_mode(dot11g_input);
        assert_eq!(dot11g.current, schedule(RateScheduleKind::P2pDot11G, 5));
        assert_eq!(dot11g.secondary, schedule(RateScheduleKind::P2pDot11G, 7));
        assert_eq!(dot11g.maximum_index, 7);

        let mut ht_input = selection_input(2);
        ht_input.metric = 8;
        let ht = select_phy_mode(ht_input);
        assert_eq!(ht.current, schedule(RateScheduleKind::Dot11N, 11));
        assert_eq!(ht.ampdu_limit_rate, Some(0x10));

        let mut he_input = selection_input(4);
        he_input.he_type = 7;
        he_input.metric = 8;
        he_input.feature_enabled = true;
        let he = select_phy_mode(he_input);
        assert_eq!(he.current, schedule(RateScheduleKind::Dot11Ax, 13));
        assert_eq!(he.maximum_index, 15);
        assert_eq!(he.ampdu_limit_rate, Some(0x12));
        assert_eq!(he.fallback, schedule(RateScheduleKind::Lora, 1));

        let lora = select_phy_mode(selection_input(6));
        assert_eq!(lora.current, schedule(RateScheduleKind::Lora, 0));
        assert_eq!(lora.schedule_count, 2);
    }

    #[test]
    fn recovered_rate_callbacks_and_ampdu_table_are_finite() {
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 0), 3);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 42), 4);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 15), 6);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x21), 0);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x29), 13);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x23), 0);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x2a), 14);
        assert_eq!(rate_to_schedule_index(RateIndexMap::Lora, 0x28), 0xff);
        assert_eq!(ampdu_limit_for_rate(0x10), 6490);
        assert_eq!(ampdu_limit_for_rate(0x17), 65535);
        assert_eq!(ampdu_limit_for_rate(0x21), 0);
        assert_eq!(ampdu_limit_for_rate(0x22), 0);
    }
}
