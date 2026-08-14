//! Rust-owned transmit rate-control state.
//!
//! The open driver does not retain the vendor's 0x98-byte C-layout record.
//! Association inputs, selected schedules and runtime retry state are separate
//! Rust values, so no caller interprets a byte array by vendor offsets.

use crate::rate_schedule::{RateScheduleKind, RateScheduleRef, schedule_state};
use crate::rx::HeGuardIntervalAndLtf;
use crate::tx::{
    HeDcmRate, HeMcs, HeRate, HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate,
    TxCompletion, TxPhyRate,
};
use open_esp_radio_esp32s31_hal::types::{
    MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeErSuAckRateProfile,
};
use open_esp_radio_esp32s31_hal::{RadioRuntimeOwner, wifi_mac::WifiMacHal};
use open_esp_radio_ieee80211::he::HeDcmConstellation;
use open_esp_radio_ieee80211::station::StaAssociationPhy;

/// Instruction-evidenced fields of one 12-byte rate schedule record.
///
/// The remaining bytes select the actual PHY rate and retry sequence. They
/// are outside this reviewed projection; only the mutable schedule state used
/// by TX completion is owned here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateScheduleState {
    pub reference: RateScheduleRef,
    pub retry_limit: u8,
    pub adaptive: u8,
}

/// Rust-owned two-sample ACK-SNR filter used by transmit rate control.
///
/// Both bytes begin at the vendor sentinel `0x7f`. A sentinel input is not a
/// measurement and leaves the state unchanged. Keeping the bytes typed as
/// signed values makes the two different rounding rules explicit:
///
/// - the midpoint uses an arithmetic right shift, and therefore rounds a
///   negative odd sum toward negative infinity;
/// - the weighted average uses signed division, and therefore truncates
///   toward zero.
///
/// SOURCE: complete `libpp.a[trc.o]::rcUpdateAckSnr` (`0x42`
/// bytes). The body has no MMIO, callback or global-state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckSnrFilter {
    latest: i8,
    filtered: i8,
}

impl AckSnrFilter {
    pub const UNINITIALIZED: i8 = 0x7f;

    pub const fn new() -> Self {
        Self {
            latest: Self::UNINITIALIZED,
            filtered: Self::UNINITIALIZED,
        }
    }

    /// Consume one already decoded signed ACK-SNR sample.
    pub fn update(&mut self, sample: i8) {
        if sample == Self::UNINITIALIZED {
            return;
        }

        let midpoint = if self.latest == Self::UNINITIALIZED {
            0
        } else {
            ((i16::from(self.latest) + i16::from(sample)) >> 1) as i8
        };
        self.latest = sample;
        self.filtered = if self.filtered == Self::UNINITIALIZED {
            midpoint
        } else {
            ((3 * i16::from(self.filtered) + i16::from(midpoint)) / 4) as i8
        };
    }

    pub const fn latest(self) -> Option<i8> {
        if self.latest == Self::UNINITIALIZED {
            None
        } else {
            Some(self.latest)
        }
    }

    pub const fn filtered(self) -> Option<i8> {
        if self.filtered == Self::UNINITIALIZED {
            None
        } else {
            Some(self.filtered)
        }
    }
}

impl Default for AckSnrFilter {
    fn default() -> Self {
        Self::new()
    }
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

/// Result of one Rust-owned A-MPDU BlockAck rate-control observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRateDecision {
    /// The vendor-sized observation window is not complete yet.
    Accumulating,
    /// One complete window was evaluated without changing its rate.
    Retain {
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
    /// Select the preceding (faster) record in the same schedule arena.
    Promote {
        from: RateScheduleRef,
        to: RateScheduleRef,
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
    /// Select the following (slower) record in the same schedule arena.
    Lower {
        from: RateScheduleRef,
        to: RateScheduleRef,
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRateObservationError {
    Unavailable,
    NoAttemptedMpdu,
    AcknowledgedExceedsAttempted,
}

/// Independent rate state used by the vendor A-MPDU schedule getter.
///
/// This is intentionally not folded into [`RateControlState`]:
/// `rcGetAmpduSched` reads a separate rate byte, while `rcGetSched` reads the
/// ordinary current schedule pointer. Complete
/// `libpp.a[trc.o]::rcUpdateTxDoneAmpdu2` updates the former without
/// moving the latter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRateControlState {
    current_schedule: RateScheduleRef,
    highest_schedule_index: u8,
    attempted_mpdu: u32,
    acknowledged_mpdu: u32,
    last_evaluation_us: u32,
    last_lower_us: u32,
    last_promote_us: u32,
    promote_cooldown_us: u32,
    raw_success_ratio: u8,
    filtered_success_ratio: u8,
    evaluation_count: u32,
    promotion_probe: bool,
}

impl AmpduRateControlState {
    const COUNTER_RESCALE_LIMIT: u32 = 0x0200_0000;
    const EVALUATION_MPDU: u32 = 500;
    const EVALUATION_US: u32 = 100_000;
    const LOWER_HOLDOFF_US: u32 = 100_000;
    const INITIAL_PROMOTE_COOLDOWN_US: u32 = 500_000;
    const MAX_PROMOTE_COOLDOWN_US: u32 = 4_000_000;
    const VENDOR_AMPDU_FLOOR_INDEX: u8 = 8;

    fn new(
        schedule_kind: RateScheduleKind,
        ordinary_schedule_index: u8,
        highest_schedule_index: u8,
    ) -> Option<Self> {
        let ampdu_index = ordinary_schedule_index.min(Self::VENDOR_AMPDU_FLOOR_INDEX);
        Some(Self {
            current_schedule: RateScheduleRef::new(schedule_kind, ampdu_index)?,
            highest_schedule_index,
            attempted_mpdu: 0,
            acknowledged_mpdu: 0,
            last_evaluation_us: 0,
            last_lower_us: 0,
            last_promote_us: 0,
            promote_cooldown_us: Self::INITIAL_PROMOTE_COOLDOWN_US,
            raw_success_ratio: 0,
            filtered_success_ratio: 0,
            evaluation_count: 0,
            promotion_probe: false,
        })
    }

    pub const fn current_schedule(&self) -> RateScheduleRef {
        self.current_schedule
    }

    pub const fn raw_success_ratio(&self) -> Option<u8> {
        if self.evaluation_count == 0 {
            None
        } else {
            Some(self.raw_success_ratio)
        }
    }

    pub const fn filtered_success_ratio(&self) -> Option<u8> {
        if self.evaluation_count == 0 {
            None
        } else {
            Some(self.filtered_success_ratio)
        }
    }

    /// Consume one or more completed hardware A-MPDU attempts.
    ///
    /// Ratios use the blob's Q7 scale: 128 is a completely acknowledged
    /// window. `now_us` is the same wrapping 32-bit microsecond domain read by
    /// the vendor body from `0x2010d800`; the application may obtain it from
    /// its already-owned monotonic timer.
    ///
    /// This ports the complete scalar window/filter/adjacent-rate suffix at
    /// `rcUpdateTxDoneAmpdu2+0xb0..+0x2e4`. The earlier vendor-result layout
    /// decoder and the floor-index branch that also changes per-TID aggregate
    /// state remain outside this value API.
    pub fn observe_block_ack(
        &mut self,
        now_us: u32,
        attempted_mpdu: u16,
        acknowledged_mpdu: u16,
        filtered_ack_snr: Option<i8>,
    ) -> Result<AmpduRateDecision, AmpduRateObservationError> {
        if attempted_mpdu == 0 {
            return Err(AmpduRateObservationError::NoAttemptedMpdu);
        }
        if acknowledged_mpdu > attempted_mpdu {
            return Err(AmpduRateObservationError::AcknowledgedExceedsAttempted);
        }

        let attempted_mpdu = u32::from(attempted_mpdu);
        let acknowledged_mpdu = u32::from(acknowledged_mpdu);
        if self.acknowledged_mpdu.wrapping_add(acknowledged_mpdu) >= Self::COUNTER_RESCALE_LIMIT {
            self.attempted_mpdu >>= 1;
            self.acknowledged_mpdu >>= 1;
        }
        self.attempted_mpdu = self.attempted_mpdu.wrapping_add(attempted_mpdu);
        self.acknowledged_mpdu = self.acknowledged_mpdu.wrapping_add(acknowledged_mpdu);

        if self.attempted_mpdu < Self::EVALUATION_MPDU
            && vendor_duration(now_us, self.last_evaluation_us) < Self::EVALUATION_US
        {
            return Ok(AmpduRateDecision::Accumulating);
        }

        let raw_success_ratio = ((self.acknowledged_mpdu << 7) / self.attempted_mpdu) as u8;
        let rate = schedule_state(self.current_schedule).rate;
        let ack_snr = filtered_ack_snr.unwrap_or(AckSnrFilter::UNINITIALIZED) as u8;
        let down_threshold = ampdu_down_threshold(rate, ack_snr);
        let previous_filtered = self.filtered_success_ratio;
        let filtered_success_ratio = if previous_filtered == 0 {
            let initial = ((3 * u16::from(down_threshold) + 128) >> 2) as u8;
            if initial >= raw_success_ratio {
                initial
            } else {
                ((u16::from(initial) + u16::from(raw_success_ratio)) >> 1) as u8
            }
        } else {
            ((3 * u16::from(previous_filtered) + u16::from(raw_success_ratio)) >> 2) as u8
        };

        self.last_evaluation_us = now_us;
        self.raw_success_ratio = raw_success_ratio;
        self.filtered_success_ratio = filtered_success_ratio;
        self.evaluation_count = self.evaluation_count.wrapping_add(1);
        self.attempted_mpdu = 0;
        self.acknowledged_mpdu = 0;

        if filtered_success_ratio < down_threshold && previous_filtered < down_threshold {
            if self.promotion_probe {
                self.promote_cooldown_us = self
                    .promote_cooldown_us
                    .saturating_mul(2)
                    .min(Self::MAX_PROMOTE_COOLDOWN_US);
            }
            if vendor_duration(now_us, self.last_lower_us) > Self::LOWER_HOLDOFF_US
                && self.current_schedule.index < Self::VENDOR_AMPDU_FLOOR_INDEX
            {
                let from = self.current_schedule;
                if let Some(to) = from.advance() {
                    self.current_schedule = to;
                    self.last_lower_us = now_us;
                    self.clear_after_schedule_change();
                    return Ok(AmpduRateDecision::Lower {
                        from,
                        to,
                        raw_success_ratio,
                        filtered_success_ratio,
                    });
                }
            }
            return Ok(AmpduRateDecision::Retain {
                raw_success_ratio,
                filtered_success_ratio,
            });
        }

        if self.promotion_probe {
            self.promotion_probe = false;
            self.promote_cooldown_us = Self::INITIAL_PROMOTE_COOLDOWN_US;
            return Ok(AmpduRateDecision::Retain {
                raw_success_ratio,
                filtered_success_ratio,
            });
        }

        let up_threshold = ampdu_up_threshold(rate, ack_snr);
        if self.highest_schedule_index < self.current_schedule.index
            && up_threshold < filtered_success_ratio
            && up_threshold < previous_filtered
            && vendor_duration(now_us, self.last_promote_us) > self.promote_cooldown_us
        {
            let from = self.current_schedule;
            if let Some(to) = RateScheduleRef::new(from.kind, from.index.wrapping_sub(1)) {
                self.current_schedule = to;
                self.last_promote_us = now_us;
                self.promotion_probe = true;
                self.clear_after_schedule_change();
                return Ok(AmpduRateDecision::Promote {
                    from,
                    to,
                    raw_success_ratio,
                    filtered_success_ratio,
                });
            }
        }

        Ok(AmpduRateDecision::Retain {
            raw_success_ratio,
            filtered_success_ratio,
        })
    }

    fn clear_after_schedule_change(&mut self) {
        self.promote_cooldown_us = Self::INITIAL_PROMOTE_COOLDOWN_US;
        self.attempted_mpdu = 0;
        self.acknowledged_mpdu = 0;
        self.raw_success_ratio = 0;
        self.filtered_success_ratio = 0;
        self.evaluation_count = 0;
    }
}

const fn vendor_duration(now: u32, previous: u32) -> u32 {
    let duration = now.wrapping_sub(previous);
    if now < previous {
        duration.wrapping_sub(1)
    } else {
        duration
    }
}

const fn ampdu_rssi_margin(rate: u8, filtered_ack_snr: u8) -> u8 {
    let mcs = match rate {
        0x10..=0x19 => rate - 0x10,
        0x1a..=0x23 => rate - 0x1a,
        _ => return 0,
    };
    let reference = match mcs {
        0 => 8,
        1 => 11,
        2 => 13,
        3 => 16,
        4 => 21,
        5 => 26,
        6 => 29,
        7 => 33,
        8 => 36,
        _ => 41,
    };
    if filtered_ack_snr == AckSnrFilter::UNINITIALIZED as u8 || filtered_ack_snr <= reference {
        return 0;
    }
    let scaled = (((filtered_ack_snr - reference) >> 1) as u16 * 3) as u8;
    if scaled > 32 { 32 } else { scaled }
}

const fn ampdu_up_threshold(rate: u8, filtered_ack_snr: u8) -> u8 {
    let margin = ampdu_rssi_margin(rate, filtered_ack_snr);
    match rate {
        0x10..=0x12 => 111 - margin,
        0x13 => 116 - margin,
        0x14..=0x19 | 0x21..=0x23 => 121 - margin,
        _ => 121,
    }
}

const fn ampdu_down_threshold(rate: u8, filtered_ack_snr: u8) -> u8 {
    let margin = ampdu_rssi_margin(rate, filtered_ack_snr);
    match rate {
        0x10 => 105 - margin,
        0x11 => 106 - margin,
        0x12 => 107 - margin,
        0x13 => 108 - margin,
        0x14 => 109 - margin,
        0x15 | 0x17 => 115 - margin,
        0x16 | 0x18..=0x19 | 0x21..=0x23 => 114 - margin,
        _ => 114,
    }
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

/// Hardware report-rate selection produced by the recovered link policy.
///
/// Fields are private so callers cannot construct a combination that the
/// complete blob policy never emits. Use [`beamforming_report_rate`] or
/// [`beamforming_report_rate_for_metric`] to obtain a value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeamformingReportRate {
    mode: u8,
    rate: u16,
    dcm: bool,
    ersu: bool,
    ersu_ack: bool,
}

/// Ordered PAC leaves used by the association rate-control transition.
pub trait BeamformingReportHardware {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile);
    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile);
}

impl BeamformingReportHardware for WifiMacHal<'_> {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        WifiMacHal::set_he_beamforming_report_profile(self, profile);
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        WifiMacHal::set_he_ersu_ack_rate_profile(self, profile);
    }
}

impl BeamformingReportHardware for RadioRuntimeOwner {
    fn set_he_beamforming_report_profile(&mut self, profile: MacHeBeamformingReportProfile) {
        BeamformingReportHardware::set_he_beamforming_report_profile(
            &mut self.wifi_mac_hal(),
            profile,
        );
    }

    fn set_he_ersu_ack_rate_profile(&mut self, profile: MacHeErSuAckRateProfile) {
        BeamformingReportHardware::set_he_ersu_ack_rate_profile(&mut self.wifi_mac_hal(), profile);
    }
}

impl BeamformingReportRate {
    pub const fn signal_mode(self) -> u8 {
        self.mode
    }

    pub const fn rate_code(self) -> u16 {
        self.rate
    }

    pub const fn dcm(self) -> bool {
        self.dcm
    }

    pub const fn extended_range_single_user(self) -> bool {
        self.ersu
    }

    pub const fn extended_range_ack(self) -> bool {
        self.ersu_ack
    }

    /// Publish both hardware leaves of complete `trc_set_bf_report_rate`.
    ///
    /// The mutable borrow is the Rust ownership boundary for the radio
    /// registers: while this call runs, no second safe owner can interleave
    /// writes between the report profile and its matching ACK profile.
    ///
    /// SOURCE: complete pinned `libpp.a[trc.o]`
    /// `trc_set_bf_report_rate`, size `0x52`, and its complete
    /// `libpp.a[hal_mac_ctl.o]` children
    /// `hal_he_set_bf_report_rate` and `hal_he_set_ersu_ack_rate`.
    pub fn program<H: BeamformingReportHardware>(
        self,
        hardware: &mut H,
    ) -> Result<(), MacHeBeamformingReportProfileError> {
        let profile = MacHeBeamformingReportProfile::from_hal_arguments(
            self.mode, self.rate, self.dcm, self.ersu,
        )?;
        let ack_profile = if self.ersu_ack {
            MacHeErSuAckRateProfile::ExtendedRange
        } else {
            MacHeErSuAckRateProfile::Ordinary
        };

        // Preserve the complete blob's transaction order: three report-rate
        // RMWs first, then the four ER-SU ACK-rate RMWs.
        hardware.set_he_beamforming_report_profile(profile);
        hardware.set_he_ersu_ack_rate_profile(ack_profile);
        Ok(())
    }
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

/// Protocol-level PHY family used to create one associated STA rate context.
///
/// The blob has four internal HT/HE variants (numeric values 2 through 5),
/// but complete `rcUpdatePhyMode` applies the same schedule policy to all of
/// them. The open owner therefore does not expose those unowned numeric ABI
/// tags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRateControlPhy {
    Dot11B,
    Dot11G,
    Ht,
    He,
    Lora,
}

/// Exact HE peer capabilities consumed by the low-link-metric report branch.
///
/// SOURCE: complete `libnet80211.a[wl_cnx.o]::ic_set_sta` copies
/// `!node[0x35c].bit(10)` and `node[0x348].bits(4:3)` into the scalar TRC
/// input. Complete `ieee80211_parse_heopr` names the former source
/// `ER-SU-Disable`; complete `ieee80211_parse_hecap` names the latter
/// `dcm rx constellation`. Complete
/// `libpp.a[if_hwctrl.o]::ic_set_trc` stores those values at vendor
/// record offsets `0x8f` and `0x90`, where `trc_set_bf_report_rate` tests
/// them for nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeLowMetricReportFeatures {
    pub dcm_receive_supported: bool,
    pub extended_range_single_user_permitted: bool,
}

/// Highest peer rate supplied to the association-time vendor rate policy.
///
/// The scalar is deliberately private: the values consumed by
/// `rcGetHighestRateIdx` are data rates in half-megabit units, not PHY rate
/// bytes or MCS indices. Constructors keep that temporary blob convention out
/// of the application and make the negotiated capability family explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRateControlPeerHighestRate(u32);

impl StaRateControlPeerHighestRate {
    /// Construct the one-spatial-stream HE20 maximum selected from the peer's
    /// negotiated HE RX MCS/NSS map.
    ///
    /// ESP32-S31 supports HE MCS0 through MCS9, so an MCS0..11 peer is capped
    /// by the caller at [`HeMcs::Mcs9`]. The finite values below are the
    /// rounded half-megabit encodings accepted by complete
    /// `libpp.a[trc.o]::{rcGetHighestRateIdx,
    /// rc11AXRate2SchedIdx}`: `17,34,51,68,104,137,154,172,206,229`.
    pub const fn he20_one_spatial_stream(maximum_mcs: HeMcs) -> Self {
        Self(match maximum_mcs {
            HeMcs::Mcs0 => 17,
            HeMcs::Mcs1 => 34,
            HeMcs::Mcs2 => 51,
            HeMcs::Mcs3 => 68,
            HeMcs::Mcs4 => 104,
            HeMcs::Mcs5 => 137,
            HeMcs::Mcs6 => 154,
            HeMcs::Mcs7 => 172,
            HeMcs::Mcs8 => 206,
            HeMcs::Mcs9 => 229,
        })
    }

    const fn vendor_half_mbps(self) -> u32 {
        self.0
    }
}

/// Value-only association input to the recovered per-peer rate policy.
///
/// [`StaLinkMetric`] keeps RSSI and noise floor from being confused with the
/// signed difference consumed by the schedule thresholds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRateControlAssociationInput {
    pub phy: StaRateControlPhy,
    pub link_metric: StaLinkMetric,
    pub p2p: bool,
    pub peer_highest_rate: Option<StaRateControlPeerHighestRate>,
    /// Whether the peer's vendor LR rate list contains at least one local rate.
    ///
    /// Complete `libnet80211.a[ieee80211_phy.o]::
    /// ieee80211_setup_lr_rates` owns the count at node offset `0x84`;
    /// `ic_set_trc` copies it to record offset `0x8b`. It is not an HE
    /// capability or a generic request for more schedules.
    pub long_range_rates_present: bool,
    pub he_low_metric_report: HeLowMetricReportFeatures,
}

/// Signed `RSSI - noise floor` value consumed by the blob rate policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaLinkMetric(i8);

impl StaLinkMetric {
    /// Reproduce the narrowing subtraction performed by complete
    /// `libpp.a[if_hwctrl.o]::ic_set_trc` at instructions
    /// 0xca..0xce. The complete `wl_cnx.o::ic_set_sta` log string names the
    /// two source bytes `rssi` and `nf`.
    pub const fn from_rssi_and_noise_floor(rssi_dbm: i8, noise_floor_dbm: i8) -> Self {
        Self(rssi_dbm.wrapping_sub(noise_floor_dbm))
    }

    /// Admit a metric already produced by an owned running link estimator.
    pub const fn from_estimator(value: i8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i8 {
        self.0
    }
}

/// Rust-owned result of the vendor post-association rate-control transition.
///
/// SOURCE: complete `libnet80211.a[wl_cnx.o]::ic_set_sta`
/// constructs the per-peer input and calls
/// `libpp.a[if_hwctrl.o]::ic_set_trc`. Complete `ic_set_trc`
/// invokes `rcUpdatePhyMode` after copying only scalar peer state, and
/// complete `libpp.a[trc.o]::rcUpdatePhyMode` selects the schedule
/// references before calling `trc_set_bf_report_rate` at instruction 0x26c.
/// This owner preserves that value and hardware-programming order without the
/// vendor 0x98-byte record or its C offsets.
#[derive(Debug, Eq, PartialEq)]
pub struct StaRateControlAssociation {
    selection: PhyModeSelection,
    beamforming_report: BeamformingReportRate,
    ack_snr: AckSnrFilter,
    runtime: RateControlState,
    ampdu_runtime: Option<AmpduRateControlState>,
}

/// Runtime-independent inputs that turn one recovered rate schedule into the
/// exact PHY format published by the S31 TX owner.
///
/// The rate-control arena owns the current rate byte, while association state
/// owns channel width, the peer-qualified HE LTF choice and LDPC capability.
/// Keeping the join here prevents applications from interpreting the
/// overlapping Dot11N/Dot11Ax byte domains themselves.
///
/// The HT and HE MCS/guard-interval overrides are explicit certification/HIL
/// controls. Ordinary operation leaves them `None` and retains the values
/// selected by the recovered schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTxRatePolicy {
    pub association_phy: StaAssociationPhy,
    pub high_throughput_enabled: bool,
    pub fallback_legacy_rate: LegacyRate,
    pub fallback_ht_mcs: HtMcs,
    pub fallback_ht_guard_interval: HtGuardInterval,
    /// Fixed typed MCS for certification; `None` keeps schedule ownership.
    pub ht_mcs_override: Option<HtMcs>,
    pub ht_guard_interval_override: Option<HtGuardInterval>,
    /// Fixed standard HE-SU MCS for certification; `None` keeps the Dot11Ax
    /// schedule selection.
    pub he_mcs_override: Option<HeMcs>,
    /// Fixed HE-SU GI/LTF selector for certification. DCM and trigger-based
    /// profiles use their own typed rate owners instead of this SU override.
    pub he_guard_interval_and_ltf_override: Option<HeGuardIntervalAndLtf>,
    /// Exact typed DCM certification rate. It takes precedence over the
    /// ordinary HE MCS/GI overrides only when the peer capability admits it.
    pub he_dcm_override: Option<HeDcmRate>,
    pub he_800ns_gi_ltf: HeGuardIntervalAndLtf,
    /// Capability for the exact HT width selected by `association_phy`.
    pub peer_supports_ht_short_guard_interval: bool,
    pub peer_supports_ldpc: bool,
    pub peer_dcm_receive: HeDcmConstellation,
}

impl StaTxRatePolicy {
    const fn ht_width(self) -> Option<HtChannelWidth> {
        match self.association_phy {
            StaAssociationPhy::Ht40 => Some(HtChannelWidth::Mhz40),
            StaAssociationPhy::Ht20 | StaAssociationPhy::He20 => Some(HtChannelWidth::Mhz20),
            StaAssociationPhy::Legacy => None,
        }
    }

    const fn qualify_ht_guard_interval(self, requested: HtGuardInterval) -> HtGuardInterval {
        match requested {
            HtGuardInterval::Short400Ns if !self.peer_supports_ht_short_guard_interval => {
                HtGuardInterval::Long800Ns
            }
            guard_interval => guard_interval,
        }
    }

    /// Conservative data rate used when no owned schedule representation can
    /// be published, including the still-proprietary Long Range arena.
    pub const fn fallback_rate(self) -> TxPhyRate {
        if !self.high_throughput_enabled {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        }
        let Some(channel_width) = self.ht_width() else {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        };
        let mcs = match self.ht_mcs_override {
            Some(mcs) => mcs,
            None => self.fallback_ht_mcs,
        };
        let guard_interval =
            self.qualify_ht_guard_interval(match self.ht_guard_interval_override {
                Some(guard_interval) => guard_interval,
                None => self.fallback_ht_guard_interval,
            });
        TxPhyRate::Ht(HtRate::new(mcs, guard_interval, channel_width))
    }

    pub const fn he_dcm_override_is_supported(self) -> bool {
        match self.he_dcm_override {
            Some(rate) => rate.is_supported_by(self.peer_dcm_receive, self.peer_supports_ldpc),
            None => true,
        }
    }

    /// Decode one complete Rust-owned schedule and apply negotiated format
    /// capabilities without exposing vendor rate bytes to the application.
    pub fn rate_for_schedule(self, schedule: RateScheduleRef) -> TxPhyRate {
        if !self.high_throughput_enabled {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        }
        let Some(ht_width) = self.ht_width() else {
            return TxPhyRate::Legacy(self.fallback_legacy_rate);
        };
        let Some(rate) =
            TxPhyRate::from_rate_control_schedule(schedule, ht_width, self.he_800ns_gi_ltf)
        else {
            return self.fallback_rate();
        };
        let rate = match rate {
            TxPhyRate::Ht(rate) => TxPhyRate::Ht(HtRate::new(
                self.ht_mcs_override.unwrap_or(rate.mcs),
                self.qualify_ht_guard_interval(
                    self.ht_guard_interval_override
                        .unwrap_or(rate.guard_interval),
                ),
                rate.channel_width,
            )),
            TxPhyRate::He(rate) => match self.he_dcm_override {
                Some(dcm) if self.he_dcm_override_is_supported() => TxPhyRate::He(dcm.rate()),
                _ => TxPhyRate::He(HeRate::new(
                    self.he_mcs_override.unwrap_or(rate.mcs()),
                    self.he_guard_interval_and_ltf_override
                        .unwrap_or(rate.guard_interval_and_ltf()),
                )),
            },
            rate => rate,
        };
        match rate {
            TxPhyRate::He(rate) if self.peer_supports_ldpc && !rate.is_dcm() => {
                TxPhyRate::He(HeRate::ldpc(rate.mcs(), rate.guard_interval_and_ltf()))
            }
            _ => rate,
        }
    }
}

impl StaRateControlAssociation {
    pub fn new(input: StaRateControlAssociationInput) -> Self {
        let (phy_type, he_type) = match input.phy {
            StaRateControlPhy::Dot11B => (0, 0),
            StaRateControlPhy::Dot11G => (1, 0),
            StaRateControlPhy::Ht => (2, 0),
            StaRateControlPhy::He => (2, 7),
            StaRateControlPhy::Lora => (6, 0),
        };
        let peer_highest_rate = input
            .peer_highest_rate
            .map_or(0, StaRateControlPeerHighestRate::vendor_half_mbps);
        let selection = select_phy_mode(PhyModeSelectionInput {
            phy_type,
            he_type,
            metric: i32::from(input.link_metric.value()),
            p2p: input.p2p,
            supplied_highest_rate: peer_highest_rate,
            use_supplied_highest_rate: input.peer_highest_rate.is_some(),
            long_range_rates_present: input.long_range_rates_present,
        });
        let beamforming_report = beamforming_report_rate_for_metric(
            i32::from(input.link_metric.value()),
            input.he_low_metric_report.dcm_receive_supported,
            input
                .he_low_metric_report
                .extended_range_single_user_permitted,
        );
        let current = schedule_state(selection.current);
        let ampdu_runtime = selection.ampdu_limit_rate.and_then(|_| {
            AmpduRateControlState::new(
                selection.current.kind,
                selection.current.index,
                selection.highest_index,
            )
        });
        Self {
            selection,
            beamforming_report,
            ack_snr: AckSnrFilter::new(),
            runtime: RateControlState {
                retry_pressure: 0,
                weighted_retries: 0,
                transmissions: 0,
                completed: 0,
                reevaluate_after_us: 0,
                retry_state_1d: 0,
                retry_state_1e: 0,
                maximum_schedule_index: selection.maximum_index,
                current_schedule: RateScheduleState {
                    reference: selection.current,
                    retry_limit: current.retry_limit,
                    adaptive: current.adaptive,
                },
                legacy_schedule: selection.legacy,
            },
            ampdu_runtime,
        }
    }

    pub const fn current_schedule(&self) -> RateScheduleRef {
        self.runtime.current_schedule.reference
    }

    pub const fn fallback_schedule(&self) -> RateScheduleRef {
        self.selection.fallback
    }

    pub const fn maximum_schedule_index(&self) -> u8 {
        self.selection.maximum_index
    }

    pub const fn schedule_count(&self) -> u8 {
        self.selection.schedule_count
    }

    pub const fn ampdu_limit_rate(&self) -> Option<u8> {
        self.selection.ampdu_limit_rate
    }

    pub const fn current_ampdu_schedule(&self) -> Option<RateScheduleRef> {
        match &self.ampdu_runtime {
            Some(runtime) => Some(runtime.current_schedule()),
            None => None,
        }
    }

    /// PHY rate selected by the ordinary per-peer schedule.
    pub fn tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate {
        policy.rate_for_schedule(self.current_schedule())
    }

    /// PHY rate selected by the independent A-MPDU schedule when available.
    pub fn ampdu_tx_rate(&self, policy: StaTxRatePolicy) -> TxPhyRate {
        policy.rate_for_schedule(
            self.current_ampdu_schedule()
                .unwrap_or_else(|| self.current_schedule()),
        )
    }

    pub const fn beamforming_report(&self) -> BeamformingReportRate {
        self.beamforming_report
    }

    /// Update the running ACK-SNR estimate with one decoded successful result.
    ///
    /// The caller owns completion classification: failures have no valid ACK
    /// sample and must not call this method.
    pub fn update_ack_snr(&mut self, sample: i8) {
        self.ack_snr.update(sample);
    }

    /// Consume the ACK-SNR sample, if any, from one typed TX completion.
    ///
    /// Failed completions are ignored by [`TxCompletion::ack_snr_sample`], so
    /// callers cannot accidentally feed timeout metadata into the filter.
    pub fn observe_tx_completion(&mut self, completion: TxCompletion) {
        if let Some(sample) = completion.ack_snr_sample() {
            self.update_ack_snr(sample);
        }
    }

    pub const fn latest_ack_snr(&self) -> Option<i8> {
        self.ack_snr.latest()
    }

    pub const fn filtered_ack_snr(&self) -> Option<i8> {
        self.ack_snr.filtered()
    }

    /// Apply one completed exchange's retry count to the owned PER state.
    ///
    /// A schedule transition is installed as a complete typed record; no
    /// pointer or vendor-record offset escapes this owner.
    pub fn update_tx_per(&mut self, retries: u32) -> TxPerUpdate {
        let update = self.runtime.update_tx_per(retries);
        if let ScheduleSelection::Selected(reference) = update.schedule {
            let selected = schedule_state(reference);
            self.runtime.current_schedule = RateScheduleState {
                reference,
                retry_limit: selected.retry_limit,
                adaptive: selected.adaptive,
            };
        }
        update
    }

    pub fn observe_ampdu_block_ack(
        &mut self,
        now_us: u32,
        attempted_mpdu: u16,
        acknowledged_mpdu: u16,
    ) -> Result<AmpduRateDecision, AmpduRateObservationError> {
        let filtered_ack_snr = self.ack_snr.filtered();
        let Some(runtime) = &mut self.ampdu_runtime else {
            return Err(AmpduRateObservationError::Unavailable);
        };
        let decision = runtime.observe_block_ack(
            now_us,
            attempted_mpdu,
            acknowledged_mpdu,
            filtered_ack_snr,
        )?;
        if matches!(
            decision,
            AmpduRateDecision::Promote { .. } | AmpduRateDecision::Lower { .. }
        ) {
            // Exact scalar side effect of `rcClearCurAMPDUSched`.
            self.runtime.current_schedule.adaptive = 0;
        }
        Ok(decision)
    }

    pub const fn ampdu_runtime(&self) -> Option<&AmpduRateControlState> {
        self.ampdu_runtime.as_ref()
    }

    pub const fn runtime(&self) -> &RateControlState {
        &self.runtime
    }

    /// Reproduce the sole hardware side effect of the association transition.
    pub fn program_hardware<H: BeamformingReportHardware>(
        &self,
        hardware: &mut H,
    ) -> Result<(), MacHeBeamformingReportProfileError> {
        self.beamforming_report.program(hardware)
    }
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
    pub long_range_rates_present: bool,
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
/// The result contains values only and is retained directly by the
/// Rust-owned association; no vendor record projection remains.
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
                    if input.long_range_rates_present { 5 } else { 3 },
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
                    if input.p2p { 7 } else { 10 }
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
                    } else if input.long_range_rates_present {
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
                    if input.long_range_rates_present {
                        if he { 15 } else { 13 }
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
                if input.long_range_rates_present { 5 } else { 3 },
                6,
                RateIndexMap::Dot11B,
                None,
            ),
        };

    PhyModeSelection {
        current,
        secondary,
        fallback: if input.long_range_rates_present {
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
            if rate <= 42 { MAP[rate as usize] } else { 0xff }
        }
        RateIndexMap::Dot11G => {
            const MAP: [u8; 43] = [
                10, 9, 8, 0xff, 0xff, 9, 8, 0xff, 1, 3, 5, 7, 0, 2, 4, 6, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            ];
            if rate <= 42 { MAP[rate as usize] } else { 0xff }
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
/// SOURCE: `libpp.a[trc.o]` callback table used by
/// `rcUpdatePhyMode`, recovered above as the `RateIndexMap::Dot11G` branch;
/// the pointed-to record bytes come from `libpp.a` rate-schedule
/// arenas in [`crate::rate_schedule`].
pub(crate) const fn dot11g_schedule_for_legacy_rate(rate: u8) -> Option<RateScheduleRef> {
    let index = rate_to_schedule_index(RateIndexMap::Dot11G, rate);
    if index == 0xff {
        None
    } else {
        RateScheduleRef::new(RateScheduleKind::Dot11G, index)
    }
}

/// Safe policy recovered from complete `trc_set_bf_report_rate`.
///
/// The byte subtraction and signed narrowing intentionally match the RISC-V
/// instructions used by the blob.
pub const fn beamforming_report_rate(
    filtered_ack_snr: u8,
    quarter_noise_floor: i32,
    he_feature_8f: bool,
    he_feature_90: bool,
) -> BeamformingReportRate {
    let metric = filtered_ack_snr.wrapping_sub(quarter_noise_floor as u8) as i8;
    beamforming_report_rate_for_metric(metric as i32, he_feature_8f, he_feature_90)
}

/// Direct form used by `rcLowerSched`, `rcUpSched` and the recovered
/// `rcUpdatePhyMode` branch, whose callers already supply the signed link
/// metric rather than ACK SNR and noise-floor components.
pub const fn beamforming_report_rate_for_metric(
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

    const HE20_POLICY: StaTxRatePolicy = StaTxRatePolicy {
        association_phy: StaAssociationPhy::He20,
        high_throughput_enabled: true,
        fallback_legacy_rate: LegacyRate::Ofdm54M,
        fallback_ht_mcs: HtMcs::Mcs7,
        fallback_ht_guard_interval: HtGuardInterval::Long800Ns,
        ht_mcs_override: None,
        ht_guard_interval_override: None,
        he_mcs_override: None,
        he_guard_interval_and_ltf_override: None,
        he_dcm_override: None,
        he_800ns_gi_ltf: HeGuardIntervalAndLtf::TwoLtf800Ns,
        peer_supports_ht_short_guard_interval: false,
        peer_supports_ldpc: true,
        peer_dcm_receive: HeDcmConstellation::Bpsk,
    };

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
    fn ack_snr_filter_matches_signed_blob_rounding() {
        let mut filter = AckSnrFilter::new();
        assert_eq!(filter.latest(), None);
        assert_eq!(filter.filtered(), None);

        filter.update(AckSnrFilter::UNINITIALIZED);
        assert_eq!(filter, AckSnrFilter::new());

        // The first valid sample is retained, while the first midpoint is
        // exactly zero because the preceding sample was the sentinel.
        filter.update(-21);
        assert_eq!(filter.latest(), Some(-21));
        assert_eq!(filter.filtered(), Some(0));

        // (-21 + -22) >> 1 is -22 (arithmetic shift), then
        // (3 * 0 + -22) / 4 is -5 (signed division toward zero).
        filter.update(-22);
        assert_eq!(filter.latest(), Some(-22));
        assert_eq!(filter.filtered(), Some(-5));

        // (-22 + 19) >> 1 is -2; (-15 + -2) / 4 is -4.
        filter.update(19);
        assert_eq!(filter.latest(), Some(19));
        assert_eq!(filter.filtered(), Some(-4));
    }

    #[test]
    fn association_installs_selected_schedule_as_owned_runtime_state() {
        let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric: StaLinkMetric::from_estimator(70),
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        });
        assert_eq!(
            association.current_schedule(),
            RateScheduleRef::new(RateScheduleKind::Dot11Ax, 1).unwrap()
        );
        assert_eq!(
            association.runtime.current_schedule.retry_limit,
            schedule_state(association.current_schedule()).retry_limit
        );

        association.runtime.retry_pressure = 6;
        let update = association.update_tx_per(5);
        assert_eq!(
            update.schedule,
            ScheduleSelection::Selected(
                RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap()
            )
        );
        assert_eq!(
            association.current_schedule(),
            RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap()
        );
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
            long_range_rates_present: false,
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
    fn typed_he_peer_maximum_covers_the_complete_mcs0_to_mcs9_table() {
        let expected = [17, 34, 51, 68, 104, 137, 154, 172, 206, 229];
        for (mcs, expected) in expected.into_iter().enumerate() {
            let mcs = HeMcs::from_index(mcs as u8).unwrap();
            assert_eq!(
                StaRateControlPeerHighestRate::he20_one_spatial_stream(mcs).vendor_half_mbps(),
                expected
            );
        }
    }

    #[test]
    fn sta_tx_policy_joins_he_schedule_peer_ltf_and_ldpc() {
        let rate = HE20_POLICY
            .rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap());
        assert_eq!(
            rate,
            TxPhyRate::He(HeRate::ldpc(
                HeMcs::Mcs9,
                HeGuardIntervalAndLtf::TwoLtf800Ns,
            ))
        );
    }

    #[test]
    fn sta_tx_policy_keeps_hil_override_and_unknown_arena_explicit() {
        let ht40 = StaTxRatePolicy {
            association_phy: StaAssociationPhy::Ht40,
            ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
            peer_supports_ht_short_guard_interval: true,
            peer_supports_ldpc: false,
            ..HE20_POLICY
        };
        assert_eq!(
            ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
            TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            ))
        );
        let fixed_mcs = StaTxRatePolicy {
            ht_mcs_override: Some(HtMcs::Mcs3),
            ..ht40
        };
        assert_eq!(
            fixed_mcs.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
            TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs3,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            ))
        );
        assert_eq!(
            ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Lora, 0).unwrap()),
            TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            ))
        );
        assert_eq!(
            StaTxRatePolicy {
                high_throughput_enabled: false,
                ..HE20_POLICY
            }
            .rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap()),
            TxPhyRate::Legacy(LegacyRate::Ofdm54M)
        );
    }

    #[test]
    fn sta_tx_policy_fixed_ht_matrix_preserves_negotiated_width() {
        let schedule = RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap();
        for association_phy in [StaAssociationPhy::Ht20, StaAssociationPhy::Ht40] {
            let width = match association_phy {
                StaAssociationPhy::Ht20 => HtChannelWidth::Mhz20,
                StaAssociationPhy::Ht40 => HtChannelWidth::Mhz40,
                _ => unreachable!(),
            };
            for mcs_index in 0..=7 {
                let mcs = HtMcs::from_index(mcs_index).unwrap();
                for guard_interval in [HtGuardInterval::Long800Ns, HtGuardInterval::Short400Ns] {
                    let policy = StaTxRatePolicy {
                        association_phy,
                        ht_mcs_override: Some(mcs),
                        ht_guard_interval_override: Some(guard_interval),
                        peer_supports_ht_short_guard_interval: true,
                        peer_supports_ldpc: false,
                        ..HE20_POLICY
                    };
                    assert_eq!(
                        policy.rate_for_schedule(schedule),
                        TxPhyRate::Ht(HtRate::new(mcs, guard_interval, width))
                    );
                }
            }
        }
    }

    #[test]
    fn sta_tx_policy_never_publishes_unadvertised_ht_short_gi() {
        let policy = StaTxRatePolicy {
            association_phy: StaAssociationPhy::Ht40,
            fallback_ht_guard_interval: HtGuardInterval::Short400Ns,
            ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
            peer_supports_ht_short_guard_interval: false,
            ..HE20_POLICY
        };
        assert_eq!(
            policy.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
            TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ))
        );
        assert_eq!(
            policy.fallback_rate(),
            TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz40,
            ))
        );
    }

    #[test]
    fn sta_tx_policy_fixed_he_su_matrix_preserves_peer_coding() {
        let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
        let gi_ltf_values = [
            HeGuardIntervalAndLtf::OneLtf800Ns,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
            HeGuardIntervalAndLtf::FourLtf3200Ns,
        ];
        for mcs_index in 0..=9 {
            let mcs = HeMcs::from_index(mcs_index).unwrap();
            for guard_interval_and_ltf in gi_ltf_values {
                let policy = StaTxRatePolicy {
                    he_mcs_override: Some(mcs),
                    he_guard_interval_and_ltf_override: Some(guard_interval_and_ltf),
                    ..HE20_POLICY
                };
                assert_eq!(
                    policy.rate_for_schedule(schedule),
                    TxPhyRate::He(HeRate::ldpc(mcs, guard_interval_and_ltf))
                );
            }
        }
    }

    #[test]
    fn sta_tx_policy_dcm_override_is_capability_gated_and_preserves_coding() {
        let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
        let gi = HeGuardIntervalAndLtf::TwoLtf800Ns;
        let bpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs0, gi);
        let bpsk_policy = StaTxRatePolicy {
            he_dcm_override: Some(bpsk),
            peer_dcm_receive: HeDcmConstellation::Bpsk,
            ..HE20_POLICY
        };
        assert!(bpsk_policy.he_dcm_override_is_supported());
        assert_eq!(
            bpsk_policy.rate_for_schedule(schedule),
            TxPhyRate::He(bpsk.rate())
        );
        assert!(!bpsk.rate().is_ldpc());

        let qpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs1, gi);
        let unsupported_qpsk = StaTxRatePolicy {
            he_dcm_override: Some(qpsk),
            peer_dcm_receive: HeDcmConstellation::Bpsk,
            ..HE20_POLICY
        };
        assert!(!unsupported_qpsk.he_dcm_override_is_supported());
        let TxPhyRate::He(fallback) = unsupported_qpsk.rate_for_schedule(schedule) else {
            panic!("HE association retains the ordinary HE schedule");
        };
        assert!(!fallback.is_dcm());

        let supported_qpsk = StaTxRatePolicy {
            peer_dcm_receive: HeDcmConstellation::Qpsk,
            ..unsupported_qpsk
        };
        assert_eq!(
            supported_qpsk.rate_for_schedule(schedule),
            TxPhyRate::He(qpsk.rate())
        );

        let ldpc_16qam = HeDcmRate::ldpc(crate::tx::HeLdpcDcmMcs::Mcs4, gi);
        let no_ldpc = StaTxRatePolicy {
            he_dcm_override: Some(ldpc_16qam),
            peer_dcm_receive: HeDcmConstellation::Qam16,
            peer_supports_ldpc: false,
            ..HE20_POLICY
        };
        assert!(!no_ldpc.he_dcm_override_is_supported());
        let with_ldpc = StaTxRatePolicy {
            peer_supports_ldpc: true,
            ..no_ldpc
        };
        assert!(with_ldpc.he_dcm_override_is_supported());
        assert_eq!(
            with_ldpc.rate_for_schedule(schedule),
            TxPhyRate::He(ldpc_16qam.rate())
        );
    }

    #[test]
    fn association_owns_ordinary_ampdu_and_completion_rate_transitions() {
        let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric: StaLinkMetric::from_estimator(8),
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        });
        assert_eq!(
            association.tx_rate(HE20_POLICY),
            HE20_POLICY.rate_for_schedule(association.current_schedule())
        );
        assert_eq!(
            association.ampdu_tx_rate(HE20_POLICY),
            HE20_POLICY.rate_for_schedule(association.current_ampdu_schedule().unwrap())
        );
        association.observe_tx_completion(TxCompletion {
            cookie: crate::tx::TxCookie(1),
            status: 0,
            trigger_flow: false,
            used_alternate: false,
            auxiliary_a_word: 0,
            auxiliary_b_word: 0,
            auxiliary_c_word: 0,
            primary_word: 0xeb << 16,
            alternate_word: 0,
        });
        assert_eq!(association.latest_ack_snr(), Some(75));
    }

    #[test]
    fn ampdu_thresholds_match_the_he_mcs9_oracle_endpoints() {
        assert_eq!(ampdu_rssi_margin(0x19, 75), 32);
        assert_eq!(ampdu_up_threshold(0x19, 75), 89);
        assert_eq!(ampdu_down_threshold(0x19, 75), 82);
        assert_eq!(
            ampdu_rssi_margin(0x23, AckSnrFilter::UNINITIALIZED as u8),
            0
        );
        assert_eq!(
            ampdu_up_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
            121
        );
        assert_eq!(
            ampdu_down_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
            114
        );
    }

    #[test]
    fn ampdu_owner_promotes_after_two_clean_vendor_windows() {
        let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 1, 0).unwrap();
        assert_eq!(
            state.current_schedule(),
            schedule(RateScheduleKind::Dot11Ax, 1)
        );
        assert_eq!(
            state.observe_block_ack(600_000, 500, 500, Some(75)),
            Ok(AmpduRateDecision::Retain {
                raw_success_ratio: 128,
                filtered_success_ratio: 110,
            })
        );
        assert_eq!(
            state.observe_block_ack(700_001, 500, 500, Some(75)),
            Ok(AmpduRateDecision::Promote {
                from: schedule(RateScheduleKind::Dot11Ax, 1),
                to: schedule(RateScheduleKind::Dot11Ax, 0),
                raw_success_ratio: 128,
                filtered_success_ratio: 114,
            })
        );
        assert_eq!(
            state.current_schedule(),
            schedule(RateScheduleKind::Dot11Ax, 0)
        );
        assert_eq!(state.filtered_success_ratio(), None);
    }

    #[test]
    fn ampdu_owner_lowers_only_after_two_filtered_bad_windows() {
        let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
        assert!(matches!(
            state.observe_block_ack(100_001, 500, 0, Some(75)),
            Ok(AmpduRateDecision::Retain {
                filtered_success_ratio: 93,
                ..
            })
        ));
        assert!(matches!(
            state.observe_block_ack(200_002, 500, 0, Some(75)),
            Ok(AmpduRateDecision::Retain {
                filtered_success_ratio: 69,
                ..
            })
        ));
        assert_eq!(
            state.observe_block_ack(300_003, 500, 0, Some(75)),
            Ok(AmpduRateDecision::Lower {
                from: schedule(RateScheduleKind::Dot11Ax, 0),
                to: schedule(RateScheduleKind::Dot11Ax, 1),
                raw_success_ratio: 0,
                filtered_success_ratio: 51,
            })
        );
    }

    #[test]
    fn ampdu_owner_accumulates_and_rejects_impossible_block_ack_counts() {
        let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
        assert_eq!(
            state.observe_block_ack(10, 16, 16, None),
            Ok(AmpduRateDecision::Accumulating)
        );
        assert_eq!(
            state.observe_block_ack(11, 0, 0, None),
            Err(AmpduRateObservationError::NoAttemptedMpdu)
        );
        assert_eq!(
            state.observe_block_ack(12, 15, 16, None),
            Err(AmpduRateObservationError::AcknowledgedExceedsAttempted)
        );
        assert_eq!(vendor_duration(4, u32::MAX - 5), 9);
    }

    #[test]
    fn ampdu_initial_rate_uses_the_vendor_index_eight_floor() {
        let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric: StaLinkMetric::from_estimator(8),
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        });
        assert_eq!(
            association.current_schedule(),
            schedule(RateScheduleKind::Dot11Ax, 13)
        );
        assert_eq!(
            association.current_ampdu_schedule(),
            Some(schedule(RateScheduleKind::Dot11Ax, 8))
        );
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
        he_input.long_range_rates_present = true;
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
    fn associated_he_owner_joins_schedule_and_report_rate_without_c_layout() {
        let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric: StaLinkMetric::from_estimator(20),
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: true,
            he_low_metric_report: HeLowMetricReportFeatures {
                dcm_receive_supported: true,
                extended_range_single_user_permitted: true,
            },
        });

        assert_eq!(
            association.current_schedule(),
            schedule(RateScheduleKind::Dot11Ax, 7)
        );
        assert_eq!(
            association.fallback_schedule(),
            schedule(RateScheduleKind::Lora, 1)
        );
        assert_eq!(association.maximum_schedule_index(), 15);
        assert_eq!(association.schedule_count(), 16);
        assert_eq!(association.ampdu_limit_rate(), Some(0x13));
        assert_eq!(
            association.beamforming_report(),
            beamforming_report_rate_for_metric(20, true, true)
        );
    }

    #[test]
    fn associated_he_owner_keeps_low_metric_feature_gates_explicit() {
        let mut input = StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric: StaLinkMetric::from_estimator(8),
            p2p: false,
            peer_highest_rate: Some(StaRateControlPeerHighestRate::he20_one_spatial_stream(
                HeMcs::Mcs9,
            )),
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        };
        let ordinary = StaRateControlAssociation::new(input);
        assert_eq!(
            ordinary.current_schedule(),
            schedule(RateScheduleKind::Dot11Ax, 0)
        );
        assert_eq!(
            ordinary.beamforming_report(),
            beamforming_report_rate_for_metric(8, false, false)
        );

        input.he_low_metric_report = HeLowMetricReportFeatures {
            dcm_receive_supported: true,
            extended_range_single_user_permitted: true,
        };
        assert_eq!(
            StaRateControlAssociation::new(input).beamforming_report(),
            beamforming_report_rate_for_metric(8, true, true)
        );
    }

    #[test]
    fn sta_link_metric_preserves_the_blob_signed_byte_subtraction() {
        assert_eq!(
            StaLinkMetric::from_rssi_and_noise_floor(-30, -96).value(),
            66
        );
        assert_eq!(
            StaLinkMetric::from_rssi_and_noise_floor(100, -100).value(),
            -56
        );
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
    }
}
