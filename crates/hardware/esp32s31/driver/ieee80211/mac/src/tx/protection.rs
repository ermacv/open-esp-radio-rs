//! Per-PPDU selection of the IEEE 802.11 protection exchange.
//!
//! Four independent sources can require a control frame before a PPDU:
//!
//! | Source | Owner | Mechanism |
//! | --- | --- | --- |
//! | ERP Use_Protection | BSS ERP element | CTS-to-self at a DSSS/HR rate, any receiver |
//! | HT Protection | BSS HT Operation element | RTS/CTS to an individual receiver, CTS-to-self to a group |
//! | HE TXOP Duration RTS Threshold | BSS HE Operation element | RTS/CTS to an individual receiver |
//! | dot11RTSThreshold | Local configuration | RTS/CTS to an individual receiver |
//!
//! An RTS/CTS exchange also sets the NAV of every station that hears the CTS,
//! so it supersedes CTS-to-self when both are required. Group-addressed PPDUs
//! never carry an RTS: they have no receiver that could answer it.
//!
//! [`WifiTxProtectionPolicy::select`] is total. Every PPDU the ordinary and
//! aggregate owners publish receives exactly one [`TxProtection`]; there is no
//! admission frontier for a protection requirement. The ESP32-S31 MAC generates
//! the RTS or CTS frame, its Duration and the SIFS sequence from the queue
//! request flag and the programmed PPDU.
//!
//! The pinned vendor PP does not implement ERP or HT protection: it stores the
//! HT Protection field without reading it and never requests CTS-to-self.
//! Its only protection sources are the length threshold in `lmacTxFrame` and
//! the HE byte threshold in `ppCheckTxRTS`. The ERP and HT rows above follow
//! IEEE 802.11 and the Linux mac80211, mt76x02 and rt2800 implementations.

use core::num::NonZeroU16;

use oer_ieee80211_mac::protection::{ErpProtection, HtProtectionMode};

use crate::tx::{HeMcs, HeRate, HtChannelWidth, LegacyRate, TxPhyRate};

/// Finite HE TXOP Duration RTS Threshold in the element's native 32-us units.
///
/// Encodings zero and 1023 disable the rule, as in the complete vendor
/// `ieee80211_parse_heopr`, and do not construct this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeTxopDurationRtsThreshold(NonZeroU16);

impl HeTxopDurationRtsThreshold {
    pub const fn new(units_32_us: u16) -> Option<Self> {
        if units_32_us >= 0x03ff {
            return None;
        }
        match NonZeroU16::new(units_32_us) {
            Some(units) => Some(Self(units)),
            None => None,
        }
    }

    pub const fn units_32_us(self) -> u16 {
        self.0.get()
    }
}

/// Peer nominal packet padding used by the vendor HE RTS byte table.
///
/// SOURCE: complete `libnet80211.a[ieee80211_he.o]::ieee80211_parse_hecap`
/// stores this code at node offset `0x354`; complete
/// `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` subtracts 8 us
/// for code one, 16 us for code two and nothing for any other code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HePacketPadding {
    #[default]
    None,
    Us8,
    Us16,
}

impl HePacketPadding {
    pub const fn from_vendor_code(code: u8) -> Self {
        match code {
            1 => Self::Us8,
            2 => Self::Us16,
            _ => Self::None,
        }
    }

    const fn micros(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Us8 => 8.0,
            Self::Us16 => 16.0,
        }
    }
}

/// HE TXOP duration rule applied by one associated non-AP HE station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeTxopRtsRule {
    threshold: HeTxopDurationRtsThreshold,
    padding: HePacketPadding,
}

impl HeTxopRtsRule {
    pub const fn new(threshold: HeTxopDurationRtsThreshold, padding: HePacketPadding) -> Self {
        Self { threshold, padding }
    }

    pub const fn threshold(self) -> HeTxopDurationRtsThreshold {
        self.threshold
    }

    pub const fn padding(self) -> HePacketPadding {
        self.padding
    }

    /// Largest HE SU APEP length whose TXOP stays below the threshold.
    ///
    /// SOURCE: complete `libpp.a[if_hwctrl.o]::
    /// ic_set_he_rts_threshold_bytes_tab` and its `.data` tables in
    /// `libpp.a[hal_mac_ctl.o]` (`he_preamble_su`, `he_time_per_sym`,
    /// `he_data_bits_per_sym`), plus complete `libpp.a[pp_he.o]::
    /// get_estimated_batime`. The vendor evaluates, in single precision,
    /// `threshold * 32 - 20 - preamble - 16 - padding`, subtracts the
    /// BlockAck estimate, divides by the symbol duration, floors, multiplies
    /// by the RU242 data bits per symbol, subtracts the 22 SERVICE/tail bits
    /// and shifts right by three. DCM halves the data bits for MCS 0, 1, 3
    /// and 4. The halfword table stores the arithmetic result modulo 2^16;
    /// a budget no larger than the BlockAck estimate stores zero.
    /// Complete `ic_get_he_rts_threshold_bytes` selects row one for both
    /// 0.8-us guard-interval configurations.
    pub fn maximum_unprotected_apep_bytes(self, rate: HeRate) -> u16 {
        const PREAMBLE_US: [f32; 3] = [23.2, 24.0, 32.0];
        const SYMBOL_US: [f32; 3] = [13.6, 14.4, 16.0];
        const DATA_BITS_PER_SYMBOL_RU242: [i32; 10] =
            [117, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
        let row = match rate.guard_interval_and_ltf() {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => 0,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => 1,
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => 2,
        };
        let mcs = rate.mcs();
        let block_ack_us = match mcs {
            HeMcs::Mcs0 => 68,
            HeMcs::Mcs1 | HeMcs::Mcs2 => 44,
            _ => 32,
        } as f32;
        let budget = (i32::from(self.threshold.units_32_us()) * 32 - 20) as f32
            - PREAMBLE_US[row]
            - 16.0
            - self.padding.micros();
        if block_ack_us >= budget {
            return 0;
        }
        // The quotient is positive here, so truncation equals `floor`.
        let symbols = ((budget - block_ack_us) / SYMBOL_US[row]) as i32;
        let mut bits = DATA_BITS_PER_SYMBOL_RU242[mcs.index() as usize];
        if rate.is_dcm() {
            bits /= 2;
        }
        ((bits * symbols - 22) >> 3) as u16
    }
}

/// Non-HT rates in one BSSBasicRateSet.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BasicRates(u16);

impl BasicRates {
    const ORDER: [(u8, LegacyRate); 12] = [
        (2, LegacyRate::Dsss1MLong),
        (4, LegacyRate::Dsss2MLong),
        (11, LegacyRate::Cck5M5Long),
        (22, LegacyRate::Cck11MLong),
        (12, LegacyRate::Ofdm6M),
        (18, LegacyRate::Ofdm9M),
        (24, LegacyRate::Ofdm12M),
        (36, LegacyRate::Ofdm18M),
        (48, LegacyRate::Ofdm24M),
        (72, LegacyRate::Ofdm36M),
        (96, LegacyRate::Ofdm48M),
        (108, LegacyRate::Ofdm54M),
    ];

    /// Rates that every ERP station supports when a BSS names no usable rate.
    pub const ERP_MANDATORY: Self = Self(0b0000_0001_0101_1111);

    /// Collect rates marked basic (bit seven) in Supported Rates and Extended
    /// Supported Rates element bodies, in 500-kbit/s units.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Self {
        let mut mask = 0;
        for encoded in supported.iter().chain(extended) {
            if encoded & 0x80 == 0 {
                continue;
            }
            for (index, (units, _)) in Self::ORDER.iter().enumerate() {
                if encoded & 0x7f == *units {
                    mask |= 1 << index;
                }
            }
        }
        Self(mask)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn rates(self) -> impl Iterator<Item = LegacyRate> {
        Self::ORDER
            .into_iter()
            .enumerate()
            .filter(move |(index, _)| self.0 & (1 << index) != 0)
            .map(|(_, (_, rate))| rate)
    }
}

/// Protection facts advertised by the BSS that one PPDU belongs to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BssProtection {
    pub erp: ErpProtection,
    pub ht: HtProtectionMode,
    /// HE Operation TXOP Duration RTS Threshold of the BSS.
    pub he_txop_rts_threshold: Option<HeTxopDurationRtsThreshold>,
    /// Peer nominal packet padding, fixed at association.
    pub he_packet_padding: HePacketPadding,
    pub basic_rates: BasicRates,
    /// Capability Information Short Preamble.
    pub short_preamble: bool,
}

impl BssProtection {
    /// A BSS without protection requirements whose basic rates are unknown.
    pub const UNPROTECTED: Self = Self {
        erp: ErpProtection::NONE,
        ht: HtProtectionMode::None,
        he_txop_rts_threshold: None,
        he_packet_padding: HePacketPadding::None,
        basic_rates: BasicRates(0),
        short_preamble: false,
    };
}

/// Local dot11RTSThreshold in PSDU bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RtsLengthThreshold(u16);

impl RtsLengthThreshold {
    /// Complete `lmacInit` stores 0x092a at `lmacConfMib+0x16`; complete
    /// `lmacIsLongFrame` requests RTS for a longer individual PSDU.
    pub const VENDOR_DEFAULT: Self = Self(0x092a);

    pub const fn new(bytes: u16) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> u16 {
        self.0
    }
}

/// Receiver class taken from the MPDU Address 1 field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReceiver {
    Individual,
    Group,
}

impl TxReceiver {
    /// Classify the receiver address (RA). The Ethernet destination of a
    /// To-DS frame is Address 3 and does not select the exchange.
    pub const fn from_address1(address1: &[u8; 6]) -> Self {
        if address1[0] & 1 != 0 {
            Self::Group
        } else {
            Self::Individual
        }
    }
}

/// One PPDU presented for protection selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedPpdu {
    pub rate: TxPhyRate,
    pub receiver: TxReceiver,
    /// PSDU bytes on air: MPDU with MIC and FCS, the complete A-MPDU, or the
    /// HE APEP length.
    pub psdu_length: u32,
}

/// Why one PPDU is protected; several sources can apply at once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxProtectionReasons(u8);

impl TxProtectionReasons {
    pub const ERP: Self = Self(1 << 0);
    pub const HT: Self = Self(1 << 1);
    pub const HE_TXOP_DURATION: Self = Self(1 << 2);
    pub const LENGTH: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    const fn with(self, other: Self, present: bool) -> Self {
        if present {
            Self(self.0 | other.0)
        } else {
            self
        }
    }
}

/// Control exchange that precedes one PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtection {
    None,
    CtsToSelf { rate: LegacyRate },
    RtsCts { rate: LegacyRate },
}

impl TxProtection {
    /// Transmit rate of the protection frame, if any.
    pub const fn control_rate(self) -> Option<LegacyRate> {
        match self {
            Self::None => None,
            Self::CtsToSelf { rate } | Self::RtsCts { rate } => Some(rate),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxProtectionDecision {
    pub protection: TxProtection,
    pub reasons: TxProtectionReasons,
}

impl TxProtectionDecision {
    pub const UNPROTECTED: Self = Self {
        protection: TxProtection::None,
        reasons: TxProtectionReasons(0),
    };
}

/// BSS requirements plus the local length threshold for one transmitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxProtectionPolicy {
    bss: BssProtection,
    rts_length_threshold: Option<RtsLengthThreshold>,
}

impl Default for WifiTxProtectionPolicy {
    fn default() -> Self {
        Self::new(Some(RtsLengthThreshold::VENDOR_DEFAULT))
    }
}

impl WifiTxProtectionPolicy {
    /// A transmitter outside any BSS, with its local length threshold.
    pub const fn new(rts_length_threshold: Option<RtsLengthThreshold>) -> Self {
        Self {
            bss: BssProtection::UNPROTECTED,
            rts_length_threshold,
        }
    }

    pub const fn bss(self) -> BssProtection {
        self.bss
    }

    pub const fn rts_length_threshold(self) -> Option<RtsLengthThreshold> {
        self.rts_length_threshold
    }

    /// Replace the BSS facts, keeping the local length threshold.
    pub fn install_bss(&mut self, bss: BssProtection) {
        self.bss = bss;
    }

    /// Replace the local dot11RTSThreshold; `None` disables the length rule.
    pub fn set_rts_length_threshold(&mut self, threshold: Option<RtsLengthThreshold>) {
        self.rts_length_threshold = threshold;
    }

    /// Leave the BSS: no BSS requirement remains.
    pub fn clear_bss(&mut self) {
        self.bss = BssProtection::UNPROTECTED;
    }

    /// Select the exchange and its control-frame rate for one PPDU.
    pub fn select(&self, ppdu: ProtectedPpdu) -> TxProtectionDecision {
        let individual = matches!(ppdu.receiver, TxReceiver::Individual);
        let (non_dsss, ht_protected) = match ppdu.rate {
            TxPhyRate::Legacy(rate) => (!is_dsss(rate), false),
            TxPhyRate::Ht(rate) => (
                true,
                ht_protects(
                    self.bss.ht,
                    matches!(rate.channel_width, HtChannelWidth::Mhz40),
                ),
            ),
            TxPhyRate::He(_) => (true, ht_protects(self.bss.ht, false)),
        };
        let he_txop = match (self.bss.he_txop_rts_threshold, ppdu.rate) {
            (Some(threshold), TxPhyRate::He(rate)) => {
                let rule = HeTxopRtsRule::new(threshold, self.bss.he_packet_padding);
                ppdu.psdu_length > u32::from(rule.maximum_unprotected_apep_bytes(rate))
            }
            _ => false,
        };
        let length = self
            .rts_length_threshold
            .is_some_and(|threshold| ppdu.psdu_length > u32::from(threshold.0));
        let erp = self.bss.erp.use_protection() && non_dsss;
        let reasons = TxProtectionReasons::default()
            .with(TxProtectionReasons::ERP, erp)
            .with(TxProtectionReasons::HT, ht_protected)
            .with(TxProtectionReasons::HE_TXOP_DURATION, individual && he_txop)
            .with(TxProtectionReasons::LENGTH, individual && length);

        let rts = individual && (ht_protected || he_txop || length);
        let protection = if rts {
            TxProtection::RtsCts {
                rate: self.control_rate(ppdu.rate),
            }
        } else if erp || ht_protected {
            TxProtection::CtsToSelf {
                rate: self.control_rate(ppdu.rate),
            }
        } else {
            TxProtection::None
        };
        TxProtectionDecision {
            protection,
            reasons,
        }
    }

    /// Control-frame rate for one data rate.
    ///
    /// The fastest basic rate not faster than the data rate, or the slowest
    /// basic rate when all are faster, as in mac80211 `rate_control_*`. Under
    /// ERP protection only DSSS/HR rates are eligible so that non-ERP
    /// stations decode the NAV. A BSS whose basic set names no eligible rate
    /// falls back to the ERP mandatory set. DSSS/HR control frames use the
    /// short preamble only when the BSS allows it and does not require
    /// Barker long preambles; 1 Mbit/s always uses the long preamble.
    fn control_rate(&self, data: TxPhyRate) -> LegacyRate {
        let dsss_only = self.bss.erp.use_protection();
        let eligible = |rate: &LegacyRate| !dsss_only || is_dsss(*rate);
        let basic = if self.bss.basic_rates.rates().any(|rate| eligible(&rate)) {
            self.bss.basic_rates
        } else {
            BasicRates::ERP_MANDATORY
        };
        let data_kbps = data.nominal_kbps();
        let mut fastest_not_faster = None;
        let mut slowest = None;
        for rate in basic.rates().filter(eligible) {
            let kbps = rate.nominal_kbps();
            if kbps <= data_kbps
                && fastest_not_faster.is_none_or(|best: LegacyRate| kbps > best.nominal_kbps())
            {
                fastest_not_faster = Some(rate);
            }
            if slowest.is_none_or(|low: LegacyRate| kbps < low.nominal_kbps()) {
                slowest = Some(rate);
            }
        }
        let rate = fastest_not_faster
            .or(slowest)
            .expect("the ERP mandatory set contains an eligible rate");
        let short = self.bss.short_preamble && !self.bss.erp.long_preamble_required();
        match (rate, short) {
            (LegacyRate::Dsss2MLong, true) => LegacyRate::Dsss2MShort,
            (LegacyRate::Cck5M5Long, true) => LegacyRate::Cck5M5Short,
            (LegacyRate::Cck11MLong, true) => LegacyRate::Cck11MShort,
            (rate, _) => rate,
        }
    }
}

/// Whether an HT Protection mode protects one HT or HE PPDU of this width.
///
/// Nonmember and non-HT mixed modes protect every HT PPDU; 20-MHz mode
/// protects only 40-MHz PPDUs. HE PPDUs follow the HT rules, as in mt76x02
/// and rt2800, because non-HE stations cannot decode them either.
const fn ht_protects(mode: HtProtectionMode, forty_mhz: bool) -> bool {
    match mode {
        HtProtectionMode::None => false,
        HtProtectionMode::TwentyMhz => forty_mhz,
        HtProtectionMode::Nonmember | HtProtectionMode::NonHtMixed => true,
    }
}

const fn is_dsss(rate: LegacyRate) -> bool {
    matches!(
        rate,
        LegacyRate::Dsss1MLong
            | LegacyRate::Dsss2MLong
            | LegacyRate::Dsss2MShort
            | LegacyRate::Cck5M5Long
            | LegacyRate::Cck5M5Short
            | LegacyRate::Cck11MLong
            | LegacyRate::Cck11MShort
    )
}

#[cfg(test)]
mod tests;
