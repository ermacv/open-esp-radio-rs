//! The Espressif station's transmit rate control: the per-association
//! schedule selection and its adaptation, recovered from the pinned
//! ESP32-S31 `libpp.a[trc.o]`.
//!
//! The controller walks the schedule arenas of [`crate::rate_schedule`]: it
//! selects the association's schedules (`rcUpdatePhyMode`), lowers the
//! ordinary schedule under retry pressure (`rcTxUpdatePer`), moves the
//! independent A-MPDU schedule from BlockAck windows
//! (`rcUpdateTxDoneAmpdu2`), filters ACK SNR (`rcUpdateAckSnr`) and selects
//! the beamforming report rate (`trc_set_bf_report_rate`). It holds values
//! only: a backend turns a schedule into its PHY rate and programs its own
//! report-rate registers.

use crate::rate_schedule::{RateScheduleKind, RateScheduleRef, schedule_state};
use oer_ieee80211_mac::{
    channel::WifiChannelWidth,
    he::{He20Capabilities, He20PeerState, HeDcmConstellation, HeMcsNssSupport},
    phy::{FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth},
    station::association::PhyMode,
};
use oer_ieee80211_sta::{association::StaAssociatedPeer, rate_control::StaRateControl};
use oer_time::Instant;

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
/// SOURCE(esp32s31): complete `libpp.a[trc.o]::rcUpdateAckSnr` (`0x42`
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
}

/// Recovered PHY-family discriminator stored in a rate-control record, and
/// its rate-code callbacks: the Espressif family's policy data.
pub(crate) use crate::rate_schedule::RateIndexMap;
#[cfg(test)]
use crate::rate_schedule::rate_to_schedule_index;

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
/// SOURCE(esp32s31): complete `libnet80211.a[wl_cnx.o]::ic_set_sta` copies
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
    /// A peer maximum above MCS 9 takes the MCS 9 value, the highest the
    /// recovered table encodes. The finite values below are the
    /// rounded half-megabit encodings accepted by complete
    /// `libpp.a[trc.o]::{rcGetHighestRateIdx,
    /// rc11AXRate2SchedIdx}`: `17,34,51,68,104,137,154,172,206,229`.
    pub const fn he20_one_spatial_stream(maximum_mcs: HeMcs) -> Self {
        Self(match maximum_mcs.index() {
            0 => 17,
            1 => 34,
            2 => 51,
            3 => 68,
            4 => 104,
            5 => 137,
            6 => 154,
            7 => 172,
            8 => 206,
            _ => 229,
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
/// SOURCE(esp32s31): complete `libnet80211.a[wl_cnx.o]::ic_set_sta`
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
    control_schedule: RateScheduleRef,
    beamforming_report: BeamformingReportRate,
    ack_snr: AckSnrFilter,
    runtime: RateControlState,
    ampdu_runtime: Option<AmpduRateControlState>,
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
            control_schedule: control_schedule(input.phy, input.p2p),
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

    /// Schedule record of every non-data frame this association sends:
    /// management, BlockAck control, power-management Null and EAPOL.
    pub const fn control_schedule(&self) -> RateScheduleRef {
        self.control_schedule
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

/// Non-data schedule of an interface whose association has not selected
/// one: `rc11BSchedTbl` record 3, 1 Mbit/s with a 32-publication budget.
///
/// SOURCE(esp32s31): `libpp.a[trc.o]::trc_init` stores this record at trc `+0x68`
/// (and `+0x64`, `+0x6c`) of the interface's default trc, which every frame
/// uses until `ic_set_sta` enables the association's own trc; station
/// authentication and association frames therefore use it.
pub const DEFAULT_CONTROL_SCHEDULE: RateScheduleRef = schedule(RateScheduleKind::Dot11B, 3);

/// Non-data schedule `rcUpdatePhyMode` stores at trc `+0x68`.
///
/// `rcGetSched` selects it for every frame that is not ordinary data
/// (descriptor word 0 bit 3 clear or bit 25 set): management, BlockAck
/// control, power-management Null and EAPOL, whose classification sets bit 25.
///
/// SOURCE(esp32s31): `libpp.a[trc.o]::rcUpdatePhyMode` and `rcGetSched`, and
/// `libnet80211.a::ic_set_vif`, whose flag is set only by
/// `esp_wifi_config_11b_rate`: an 802.11b-only association keeps
/// `rc11BSchedTbl` record 3, 802.11g/n/ax use `BasicOFDMSched`, the flagged
/// 802.11g association uses `rcP2P11GSchedTbl` record 7 while flagged n/ax
/// keep the `trc_init` record, and LoRa uses `rcLoRaSchedTbl`.
const fn control_schedule(phy: StaRateControlPhy, flagged: bool) -> RateScheduleRef {
    match (phy, flagged) {
        (StaRateControlPhy::Dot11B, _) => DEFAULT_CONTROL_SCHEDULE,
        (StaRateControlPhy::Lora, _) => schedule(RateScheduleKind::Lora, 0),
        (StaRateControlPhy::Dot11G, true) => schedule(RateScheduleKind::P2pDot11G, 7),
        (StaRateControlPhy::Ht | StaRateControlPhy::He, true) => DEFAULT_CONTROL_SCHEDULE,
        (StaRateControlPhy::Dot11G | StaRateControlPhy::Ht | StaRateControlPhy::He, false) => {
            schedule(RateScheduleKind::BasicOfdm, 0)
        }
    }
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

/// The Espressif controller behind the station's rate-control seam: the
/// vendor's per-association rate control, its schedules decoded as
/// portable rates within what the association negotiated.
///
/// A schedule's HT rate takes the association's width, and its short guard
/// interval only where the access point supports one at that width; an HE
/// rate takes 0.8 us with one HE-LTF where the access point supports it,
/// two otherwise, and LDPC where the access point receives it. A schedule
/// without a portable rate (the vendor's Long Range modes) sends at the
/// vendor station's fallback: 54 Mb/s legacy, or HT MCS 7 with the long
/// guard interval.
pub struct EspressifRateControl {
    association: StaRateControlAssociation,
    decode: RateDecode,
}

/// What turns a schedule into a rate the association can send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RateDecode {
    high_throughput: bool,
    ht_bandwidth: PpduBandwidth,
    ht_short_gi: bool,
    he_800ns_gi_ltf: HeGiLtf,
    he_ldpc: bool,
}

impl RateDecode {
    fn rate(self, schedule: RateScheduleRef) -> PhyRate {
        let code = schedule_state(schedule).rate;
        match crate::rate_code::phy_rate(
            schedule.kind,
            code,
            self.ht_bandwidth,
            self.he_800ns_gi_ltf,
        ) {
            Some(PhyRate::Ht(rate)) if rate.short_gi() && !self.ht_short_gi => PhyRate::Ht(
                HtRate::new(rate.mcs(), rate.bandwidth(), false)
                    .expect("an HT rate at its own width"),
            ),
            Some(PhyRate::He(rate)) if self.he_ldpc => PhyRate::He(
                HeRate::new(
                    rate.mcs(),
                    rate.spatial_streams(),
                    rate.bandwidth(),
                    rate.gi_ltf(),
                    FecCoding::Ldpc,
                    rate.dcm(),
                )
                .expect("an HE rate with LDPC"),
            ),
            Some(rate) => rate,
            None if self.high_throughput => PhyRate::Ht(
                HtRate::new(HtMcs::new(7).expect("HT MCS 7"), self.ht_bandwidth, false)
                    .expect("HT MCS 7 at the association's width"),
            ),
            None => PhyRate::Legacy(LegacyRate::Ofdm54M),
        }
    }
}

impl StaRateControl for EspressifRateControl {
    type Config = ();

    fn associate((): (), peer: &StaAssociatedPeer, link_metric: Option<i8>) -> Self {
        let he = peer.he_capabilities;
        let phy = match peer.phy {
            PhyMode::Legacy => StaRateControlPhy::Dot11G,
            PhyMode::Ht20 | PhyMode::Ht40 => StaRateControlPhy::Ht,
            PhyMode::He20 => StaRateControlPhy::He,
        };
        // As the vendor station: an HE access point's maximum one-stream MCS,
        // capped at MCS 9.
        let peer_highest_rate = he
            .filter(|_| peer.phy == PhyMode::He20)
            .and_then(|capability| match capability.receive_nss1 {
                HeMcsNssSupport::Mcs0To7 => HeMcs::new(7),
                HeMcsNssSupport::Mcs0To9 | HeMcsNssSupport::Mcs0To11 => HeMcs::new(9),
                HeMcsNssSupport::NotSupported => None,
            })
            .map(StaRateControlPeerHighestRate::he20_one_spatial_stream);
        let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy,
            // An unknown metric is the weakest link: the selection starts at
            // the format's slowest schedule and adaptation raises it.
            link_metric: StaLinkMetric::from_estimator(link_metric.unwrap_or(i8::MIN)),
            p2p: false,
            peer_highest_rate,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures {
                dcm_receive_supported: he.is_some_and(|capability| {
                    capability.dcm_receive_constellation() != HeDcmConstellation::NotSupported
                }),
                extended_range_single_user_permitted: peer
                    .he_peer_state
                    .is_some_and(He20PeerState::extended_range_single_user_permitted),
            },
        });
        let (ht_bandwidth, width) = if peer.phy == PhyMode::Ht40 {
            (PpduBandwidth::Mhz40, WifiChannelWidth::Mhz40Above)
        } else {
            (PpduBandwidth::Mhz20, WifiChannelWidth::Mhz20)
        };
        Self {
            association,
            decode: RateDecode {
                high_throughput: peer.phy != PhyMode::Legacy,
                ht_bandwidth,
                ht_short_gi: peer
                    .ht_capabilities
                    .is_some_and(|capability| capability.supports_short_guard_interval(width)),
                he_800ns_gi_ltf: if he.is_some_and(He20Capabilities::supports_one_ltf_800ns_gi) {
                    HeGiLtf::Ltf1xGi800Ns
                } else {
                    HeGiLtf::Ltf2xGi800Ns
                },
                he_ldpc: he.is_some_and(He20Capabilities::supports_ldpc_coding_in_payload),
            },
        }
    }

    fn mpdu_rate(&self) -> PhyRate {
        self.decode.rate(self.association.current_schedule())
    }

    fn ampdu_rate(&self) -> PhyRate {
        self.decode.rate(
            self.association
                .current_ampdu_schedule()
                .unwrap_or_else(|| self.association.current_schedule()),
        )
    }

    fn observe_mpdu(&mut self, attempts: u8, acknowledged: bool, ack_snr_db: Option<i8>) {
        if acknowledged && let Some(sample) = ack_snr_db {
            self.association.update_ack_snr(sample);
        }
        self.association
            .update_tx_per(u32::from(attempts.saturating_sub(1)));
    }

    fn observe_ampdu(&mut self, now: Instant, attempted: u16, acknowledged: u16) {
        // The vendor's wrapping 32-bit microsecond clock.
        let _ = self.association.observe_ampdu_block_ack(
            now.as_micros() as u32,
            attempted,
            acknowledged,
        );
    }
}

impl EspressifRateControl {
    /// The association's rate control, as the vendor holds it.
    pub const fn association(&self) -> &StaRateControlAssociation {
        &self.association
    }
}

#[cfg(test)]
mod tests;
