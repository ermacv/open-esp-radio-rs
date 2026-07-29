//! Bounded q0 legacy TX descriptor, submission and completion ownership.

use core::pin::Pin;

use open_esp_radio_pac_esp32s31::{
    MacHeTbTidLimit, MacHeTid, MacHeTxProgram, MacHeTxVectorSnapshot, MacHtTxProgram,
    MacLegacyTxProgram, MacTxCompletionRegisters, RadioRegisters,
};

use crate::{
    descriptor::{descriptor_address_valid, dma_range_valid, tx_owned_word, Descriptor},
    rate_control::dot11g_schedule_for_legacy_rate,
    rate_schedule::{schedule_rate_after_failures, RateScheduleKind, RateScheduleRef},
    tx_plcp::{
        apply_basic_txop_control_word, basic_data_length_word, basic_htsig_word,
        basic_length_control_word, basic_non_he_plcp1_word, basic_plcp0_word, he_ampdu_plcp0_word,
        he_plcp1_word, ht_htsig_word,
    },
};

const EXT_ALT_SELECT: u32 = 0x0010_0000;
const LEGACY_FCS_LENGTH: u16 = 4;
// SOURCE: HIL_VENDOR_HT20_MCS0_SINGLE_PPDU_2026_07_29. Interposing the
// pp_wdev_funcs PPDU callback immediately after the complete vendor formatter
// returned captured descriptor flags 0x0000_3009 for a synchronous, non-A-MPDU
// HT20 MCS0 data frame. Its queue image used PLCP format one and entry-class
// zero. The earlier active MCS7 image was an A-MPDU and must not supply the
// direct single-MPDU flags. HIL_OPEN_HT_SINGLE_EXTREMES_2026_07_29 then
// qualified this formatter at both MCS0/HT20/LGI and MCS7/HT40/SGI.
const HT_SINGLE_DESCRIPTOR_FLAGS: u32 = 0x0000_3009;
const HT_SINGLE_ENTRY_FLAGS: u8 = 0;
// SOURCE: HIL_VENDOR_HT_AMPDU_PPDU. The complete vendor two-MPDU formatter
// captured first-descriptor flags 0x004c_2009 after ppAssembleAMPDU, PLCP
// format two, HT-SIG aggregate bit one, and entry class one in both length
// words. These constants deliberately remain separate from the single-MPDU
// formatter above.
const HT_AMPDU_DESCRIPTOR_FLAGS: u32 = 0x004c_2009;
const HT_AMPDU_ENTRY_FLAGS: u8 = 1;
// SOURCE: HIL_VENDOR_HE20_MCS9_SU_2026_07_29. Two synchronous vendor HE SU
// A-MPDU formatter captures used descriptor flags 0xc0403009, entry class one
// and the bounded BCC/non-STBC A2 low control image 0x105.
const HE_AMPDU_ENTRY_FLAGS: u8 = 1;
// SOURCE: complete `_oracles/libpp.a[pp_he.o]::ppCalTxHESMPDULength`
// selects one descriptor/MPDU and leaves HE-SIG-A2 bit 28 clear. The
// synchronous 24-byte vendor raw DCM oracle nevertheless published
// LENGTH_CONTROL 0x0040_02c4, proving that queue length entry class one is
// independent of the on-air A-MPDU selector.
const HE_SMPDU_ENTRY_FLAGS: u8 = 1;
const HE_SMPDU_DESCRIPTOR_FLAGS: u32 = 0xc040_7008;
const HE_SU_A2_CONTROL_BCC: u16 = 0x0105;

/// The four ordinary EDCA hardware queues recovered from `ppTxPkt`.
///
/// The names follow the standard user-priority mapping: priorities 6/7 use
/// voice, 4/5 video, 0/3 best effort, and 1/2 background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LegacyTxQueue {
    #[default]
    Voice = 0,
    Video = 1,
    BestEffort = 2,
    Background = 3,
}

impl LegacyTxQueue {
    pub(crate) const fn index(self) -> u8 {
        self as u8
    }

    /// Packet PTI assigned by complete vendor data encapsulation.
    ///
    /// Complete `_oracles/libnet80211.a[ieee80211_output.o]::
    /// ieee80211_encap_esfbuf` maps voice/video/best-effort/background to
    /// coexistence events 10/11/12/13. The complete pinned
    /// `libcoexist.a[coexist_core.o]::coex_pti_tab` maps those events to
    /// 3/2/1/1.
    pub const fn vendor_data_packet_priority(self) -> u8 {
        match self {
            Self::Voice => 3,
            Self::Video => 2,
            Self::BestEffort | Self::Background => 1,
        }
    }

    /// Queue scheduler PTI selected by complete vendor data encapsulation.
    ///
    /// Complete `_oracles/libpp.a[hal_mac.o]::mac_tx_set_pti` takes the
    /// unsigned minimum of the packet PTI and coexistence event-one PTI 5.
    /// Every ordinary data priority is below five, so it is retained.
    pub const fn vendor_data_scheduler_priority(self) -> u8 {
        self.vendor_data_packet_priority()
    }
}

/// Finite ordinary-queue hardware authority used by one owned TX slot.
pub trait TxHardware {
    fn prepare_legacy_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool;
    fn start_legacy_tx(&mut self, queue: u8, plcp0: u32);
    fn prepare_ht_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool;
    fn start_ht_tx(&mut self, queue: u8, plcp0: u32);
    fn prepare_he_tx(&mut self, queue: u8, program: MacHeTxProgram) -> bool;
    fn start_he_tx(&mut self, queue: u8, plcp0: u32);
    /// Copy a submitted HE vector when the backend exposes typed readback.
    ///
    /// Pure software test backends may leave this unsupported.
    fn he_tx_vector_snapshot(&self, _queue: u8) -> Option<MacHeTxVectorSnapshot> {
        None
    }
    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters>;
    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool;
    fn finish_tx_timeout_abort(&mut self, queue: u8) -> Option<bool>;
    fn detach_completed_tx(&mut self, queue: u8) -> bool;
}

impl TxHardware for RadioRegisters {
    fn prepare_legacy_tx(&mut self, queue: u8, program: MacLegacyTxProgram) -> bool {
        self.prepare_legacy_mac_tx(queue, program)
    }

    fn start_legacy_tx(&mut self, queue: u8, plcp0: u32) {
        self.start_legacy_mac_tx(queue, plcp0);
    }

    fn prepare_ht_tx(&mut self, queue: u8, program: MacHtTxProgram) -> bool {
        self.prepare_ht_mac_tx(queue, program)
    }

    fn start_ht_tx(&mut self, queue: u8, plcp0: u32) {
        self.start_ht_mac_tx(queue, plcp0);
    }

    fn prepare_he_tx(&mut self, queue: u8, program: MacHeTxProgram) -> bool {
        self.prepare_he_mac_tx(queue, program)
    }

    fn start_he_tx(&mut self, queue: u8, plcp0: u32) {
        self.start_he_mac_tx(queue, plcp0);
    }

    fn he_tx_vector_snapshot(&self, queue: u8) -> Option<MacHeTxVectorSnapshot> {
        Some(self.he_mac_tx_vector_snapshot(queue))
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionRegisters> {
        self.take_mac_tx_completion(queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        self.begin_mac_tx_timeout_abort(queue)
    }

    fn finish_tx_timeout_abort(&mut self, queue: u8) -> Option<bool> {
        self.finish_mac_tx_timeout_abort(queue)
    }

    fn detach_completed_tx(&mut self, queue: u8) -> bool {
        self.detach_completed_mac_tx(queue)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxSlotState {
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCookie(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCompletion {
    pub cookie: TxCookie,
    pub status: u8,
    pub trigger_flow: bool,
    pub used_alternate: bool,
    /// Raw completion-extension word A.
    ///
    /// SOURCE: `_oracles/libpp.a[hal_mac_tx.o]` completion reader and the
    /// promoted `migration/esp32s31-hybrid-runtime/src/lmac.rs` decoder.
    /// Bits 19:16 contribute to the reconstructed extension word that selects
    /// the primary or alternate status record. Retaining the raw word lets HIL
    /// distinguish a real ACK-timeout result from a selector-decoding error.
    pub auxiliary_a_word: u32,
    /// Raw completion-extension word B; see [`Self::auxiliary_a_word`].
    pub auxiliary_b_word: u32,
    /// Raw completion-extension word C; see [`Self::auxiliary_a_word`].
    pub auxiliary_c_word: u32,
    pub primary_word: u32,
    pub alternate_word: u32,
}

pub(crate) fn decode_tx_completion(
    cookie: TxCookie,
    registers: MacTxCompletionRegisters,
) -> TxCompletion {
    let ext_word0 = ((registers.aux_a & 0x000f_0000) << 12)
        | (registers.aux_b & 0x001f_e000)
        | (((registers.aux_b >> 25) & 0x7f) << 21);
    let _ext_word1 = ((registers.aux_a >> 20) & 0x03) | ((registers.aux_c >> 5) & 0x1fc);
    let primary = registers.primary;
    let alternate = registers.alternate;
    let used_alternate = ext_word0 & EXT_ALT_SELECT != 0;
    let selected = if used_alternate { alternate } else { primary };
    TxCompletion {
        cookie,
        status: ((selected >> 12) & 0x0f) as u8,
        trigger_flow: registers.trigger_flow,
        used_alternate,
        auxiliary_a_word: registers.aux_a,
        auxiliary_b_word: registers.aux_b,
        auxiliary_c_word: registers.aux_c,
        primary_word: primary,
        alternate_word: alternate,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    Busy,
    Invalid,
    QueueActive,
    Stale,
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
}

/// Hardware encoding of an ESP32-S31 non-HT transmit rate.
///
/// These values are not ordered by bitrate.  They are the exact indices used
/// by the MAC PLCP formatter and by the PHY target-power table.
///
/// SOURCE: sibling oracle
/// `../esp-wifi-sys/esp-wifi-sys-esp32s31/src/include.rs`, generated from
/// Espressif's `esp_wifi_types_generic.h` (`wifi_phy_rate_t`), cross-checked
/// against `_oracles/libpp.a` rate schedules and the recovered
/// `phy_rate_to_index` ROM routine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LegacyRate {
    #[default]
    Dsss1MLong = 0x00,
    Dsss2MLong = 0x01,
    Cck5M5Long = 0x02,
    Cck11MLong = 0x03,
    Dsss2MShort = 0x05,
    Cck5M5Short = 0x06,
    Cck11MShort = 0x07,
    Ofdm48M = 0x08,
    Ofdm24M = 0x09,
    Ofdm12M = 0x0a,
    Ofdm6M = 0x0b,
    Ofdm54M = 0x0c,
    Ofdm36M = 0x0d,
    Ofdm18M = 0x0e,
    Ofdm9M = 0x0f,
}

impl LegacyRate {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0x00 => Some(Self::Dsss1MLong),
            0x01 => Some(Self::Dsss2MLong),
            0x02 => Some(Self::Cck5M5Long),
            0x03 => Some(Self::Cck11MLong),
            0x05 => Some(Self::Dsss2MShort),
            0x06 => Some(Self::Cck5M5Short),
            0x07 => Some(Self::Cck11MShort),
            0x08 => Some(Self::Ofdm48M),
            0x09 => Some(Self::Ofdm24M),
            0x0a => Some(Self::Ofdm12M),
            0x0b => Some(Self::Ofdm6M),
            0x0c => Some(Self::Ofdm54M),
            0x0d => Some(Self::Ofdm36M),
            0x0e => Some(Self::Ofdm18M),
            0x0f => Some(Self::Ofdm9M),
            _ => None,
        }
    }

    /// Select this rate's vendor 802.11g retry-ladder entry.
    ///
    /// `failed_attempts` is the number of ACK/CTS failures already observed
    /// for the same MPDU. The first transmission therefore passes zero. The
    /// complete 54M ladder is `54M x2, 48M x2, 6M x3, 5.5M x25`; the other
    /// legacy rates use their corresponding records. `None` reports that the
    /// record's complete retry budget has been consumed.
    ///
    /// SOURCE: `_oracles/libpp.a[trc.o]::{rcGetRate, rcUpdatePhyMode}` and the
    /// exact Rust-owned schedule arenas in [`crate::rate_schedule`],
    /// cross-checked against the promoted migration
    /// `lmac.rs::select_basic_retry_rate`.
    pub fn vendor_retry_rate(self, failed_attempts: u8) -> Option<Self> {
        let schedule = dot11g_schedule_for_legacy_rate(self.code())?;
        Self::from_code(schedule_rate_after_failures(schedule, failed_attempts)?)
    }

    /// Return the basic protection rate selected by the vendor MAC.
    ///
    /// Despite the vendor symbol name, `mac_tx_get_rts_rate`, the result is
    /// always published by `mac_tx_set_len` in `TX_Q_LENGTH_CONTROL`; it is
    /// therefore part of every legacy PPDU image, not only frames which
    /// request explicit RTS/CTS protection.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::
    /// mac_tx_get_rts_rate` (size `0x96`) and the identical exhaustive
    /// `migration/esp32s31-hybrid-runtime/src/tx_rate.rs::
    /// basic_non_he_rts_rate` reconstruction.
    pub const fn vendor_rts_rate(self) -> Self {
        match self {
            Self::Dsss1MLong => Self::Dsss1MLong,
            Self::Dsss2MLong | Self::Cck5M5Long | Self::Cck11MLong => Self::Dsss2MLong,
            Self::Dsss2MShort | Self::Cck5M5Short | Self::Cck11MShort => Self::Dsss2MShort,
            Self::Ofdm48M | Self::Ofdm24M | Self::Ofdm54M | Self::Ofdm36M => Self::Ofdm24M,
            Self::Ofdm12M | Self::Ofdm18M => Self::Ofdm12M,
            Self::Ofdm6M | Self::Ofdm9M => Self::Ofdm6M,
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        match self {
            Self::Dsss1MLong => 1_000,
            Self::Dsss2MLong | Self::Dsss2MShort => 2_000,
            Self::Cck5M5Long | Self::Cck5M5Short => 5_500,
            Self::Cck11MLong | Self::Cck11MShort => 11_000,
            Self::Ofdm6M => 6_000,
            Self::Ofdm9M => 9_000,
            Self::Ofdm12M => 12_000,
            Self::Ofdm18M => 18_000,
            Self::Ofdm24M => 24_000,
            Self::Ofdm36M => 36_000,
            Self::Ofdm48M => 48_000,
            Self::Ofdm54M => 54_000,
        }
    }
}

/// One-spatial-stream 802.11n modulation and coding scheme.
///
/// The ESP32-S31 is 1T1R, so the open HT path intentionally exposes only the
/// standard single-stream MCS0..MCS7 set. MCS8..MCS31 describe additional
/// spatial streams and cannot be made valid by passing an unchecked integer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HtMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs2 = 2,
    Mcs3 = 3,
    Mcs4 = 4,
    Mcs5 = 5,
    Mcs6 = 6,
    Mcs7 = 7,
}

impl HtMcs {
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Mcs0),
            1 => Some(Self::Mcs1),
            2 => Some(Self::Mcs2),
            3 => Some(Self::Mcs3),
            4 => Some(Self::Mcs4),
            5 => Some(Self::Mcs5),
            6 => Some(Self::Mcs6),
            7 => Some(Self::Mcs7),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtGuardInterval {
    #[default]
    Long800Ns,
    Short400Ns,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtChannelWidth {
    #[default]
    Mhz20,
    Mhz40,
}

/// Finite S31 encoding derived from the peer's HT minimum MPDU start spacing.
///
/// These values are not microseconds. They are the hardware-domain values
/// stored at peer-state offset `0x82` by the complete vendor
/// `rcUpdateAMPDUParam`, then copied three times into the queue protection
/// word by the complete `mac_tx_set_htsig`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u16)]
pub enum HtProtectionSpacing {
    /// IEEE HT A-MPDU Parameters spacing codes zero through four.
    #[default]
    Density0To4 = 20,
    /// IEEE HT A-MPDU Parameters spacing code five.
    Density5 = 40,
    /// IEEE HT A-MPDU Parameters spacing code six.
    Density6 = 76,
    /// IEEE HT A-MPDU Parameters spacing code seven.
    Density7 = 148,
}

impl HtProtectionSpacing {
    /// Derive the exact finite hardware value from a complete HT A-MPDU
    /// Parameters byte (HT Capabilities IE payload byte two).
    ///
    /// SOURCE: complete `_oracles/libpp.a[trc.o]::rcUpdateAMPDUParam`,
    /// size `0xde`, cross-checked against a coherent hardware-owned vendor HT
    /// queue containing three copies of value 40 (`0x0280_a028`).
    pub const fn from_ampdu_parameters(parameters: u8) -> Self {
        Self::from_density(HtAmpduDensity::from_ampdu_parameters(parameters))
    }

    pub const fn from_density(density: HtAmpduDensity) -> Self {
        match density.encoding() {
            0..=4 => Self::Density0To4,
            5 => Self::Density5,
            6 => Self::Density6,
            _ => Self::Density7,
        }
    }

    pub const fn hardware_value(self) -> u16 {
        self as u16
    }
}

/// Peer-advertised HT minimum MPDU start spacing.
///
/// The enum retains all eight IEEE encodings even though the S31 queue's
/// [`HtProtectionSpacing`] field collapses encodings zero through four.
/// HE delimiter construction needs the original encoding: the complete
/// vendor `rcUpdateAMPDUParam` converts it through the integer-microsecond
/// table `[0, 1, 1, 1, 2, 4, 8, 16]` before regenerating the HE minimum
/// subframe tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HtAmpduDensity {
    #[default]
    NoRestriction = 0,
    QuarterMicrosecond = 1,
    HalfMicrosecond = 2,
    OneMicrosecond = 3,
    TwoMicroseconds = 4,
    FourMicroseconds = 5,
    EightMicroseconds = 6,
    SixteenMicroseconds = 7,
}

impl HtAmpduDensity {
    /// Decode bits 4:2 of the complete HT A-MPDU Parameters byte.
    pub const fn from_ampdu_parameters(parameters: u8) -> Self {
        match (parameters >> 2) & 0x07 {
            0 => Self::NoRestriction,
            1 => Self::QuarterMicrosecond,
            2 => Self::HalfMicrosecond,
            3 => Self::OneMicrosecond,
            4 => Self::TwoMicroseconds,
            5 => Self::FourMicroseconds,
            6 => Self::EightMicroseconds,
            _ => Self::SixteenMicroseconds,
        }
    }

    pub const fn encoding(self) -> u8 {
        self as u8
    }

    /// Integer-microsecond value consumed by the pinned PP blob.
    ///
    /// SOURCE: complete `_oracles/libpp.a[trc.o]::rcUpdateAMPDUParam`.
    /// Its two stack constants form the exact byte table
    /// `[0, 1, 1, 1, 2, 4, 8, 16]`; the selected byte is stored in ROM global
    /// `s_ht_ampdu_density_us` before `he_get_min_subframe_len[_dcm]` uses it.
    pub const fn vendor_integer_microseconds(self) -> u8 {
        match self {
            Self::NoRestriction => 0,
            Self::QuarterMicrosecond | Self::HalfMicrosecond | Self::OneMicrosecond => 1,
            Self::TwoMicroseconds => 2,
            Self::FourMicroseconds => 4,
            Self::EightMicroseconds => 8,
            Self::SixteenMicroseconds => 16,
        }
    }
}

/// Complete typed HT PHY rate selected for one transmit attempt.
///
/// Channel width is both a PHY channel-engine precondition and part of the
/// queue vector. The same numeric rate code is used for HT20 and HT40, while
/// HT-SIG1 bit 7 and PLCP1 bit 29 publish CBW for the selected PPDU.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HtRate {
    pub mcs: HtMcs,
    pub guard_interval: HtGuardInterval,
    pub channel_width: HtChannelWidth,
}

impl HtRate {
    pub const fn new(
        mcs: HtMcs,
        guard_interval: HtGuardInterval,
        channel_width: HtChannelWidth,
    ) -> Self {
        Self {
            mcs,
            guard_interval,
            channel_width,
        }
    }

    /// Exact S31 non-HE MAC rate code.
    ///
    /// SOURCE: sibling S31 `wifi_phy_rate_t`, complete
    /// `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_htsig`, and the recovered
    /// Dot11N rate schedules. Long-GI MCS0 starts at 16; short-GI MCS0 starts
    /// at 26.
    pub const fn code(self) -> u8 {
        let base = match self.guard_interval {
            HtGuardInterval::Long800Ns => 16,
            HtGuardInterval::Short400Ns => 26,
        };
        base + self.mcs.index()
    }

    /// Rate code used for target-power lookup.
    ///
    /// `hal_mac_tx_set_ppdu` subtracts ten from an SGI rate before loading the
    /// power pair, so LGI and SGI of the same MCS share one calibrated entry.
    pub const fn power_lookup_code(self) -> u8 {
        16 + self.mcs.index()
    }

    /// Legacy basic rate used by the protection/length vector.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::
    /// mac_tx_get_rts_rate` (size 0x96). Both GI code ranges select 6M for
    /// MCS0, 12M for MCS1/2 and 24M for MCS3..7.
    pub const fn vendor_rts_rate(self) -> LegacyRate {
        match self.mcs {
            HtMcs::Mcs0 => LegacyRate::Ofdm6M,
            HtMcs::Mcs1 | HtMcs::Mcs2 => LegacyRate::Ofdm12M,
            HtMcs::Mcs3 | HtMcs::Mcs4 | HtMcs::Mcs5 | HtMcs::Mcs6 | HtMcs::Mcs7 => {
                LegacyRate::Ofdm24M
            }
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        const HT20_LGI: [u32; 8] = [
            6_500, 13_000, 19_500, 26_000, 39_000, 52_000, 58_500, 65_000,
        ];
        const HT20_SGI: [u32; 8] = [
            7_200, 14_400, 21_700, 28_900, 43_300, 57_800, 65_000, 72_200,
        ];
        const HT40_LGI: [u32; 8] = [
            13_500, 27_000, 40_500, 54_000, 81_000, 108_000, 121_500, 135_000,
        ];
        const HT40_SGI: [u32; 8] = [
            15_000, 30_000, 45_000, 60_000, 90_000, 120_000, 135_000, 150_000,
        ];
        let table = match (self.channel_width, self.guard_interval) {
            (HtChannelWidth::Mhz20, HtGuardInterval::Long800Ns) => &HT20_LGI,
            (HtChannelWidth::Mhz20, HtGuardInterval::Short400Ns) => &HT20_SGI,
            (HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns) => &HT40_LGI,
            (HtChannelWidth::Mhz40, HtGuardInterval::Short400Ns) => &HT40_SGI,
        };
        table[self.mcs.index() as usize]
    }

    /// Select one exact vendor Dot11N retry-ladder attempt.
    ///
    /// The recovered table has explicit records for every LGI MCS0..7 and for
    /// SGI MCS7, the vendor maximum-throughput starting point. Other fixed SGI
    /// MCS values are valid queue rates but have no independent record in the
    /// complete `rcUpdatePhyMode` mapping and therefore return `None` here
    /// instead of inventing a fallback policy.
    pub fn vendor_retry_rate(self, failed_attempts: u8) -> Option<TxPhyRate> {
        let schedule_index = match self.guard_interval {
            HtGuardInterval::Long800Ns => 8 - self.mcs.index(),
            HtGuardInterval::Short400Ns if self.mcs == HtMcs::Mcs7 => 0,
            HtGuardInterval::Short400Ns => return None,
        };
        let schedule = RateScheduleRef::new(RateScheduleKind::Dot11N, schedule_index)?;
        let code = schedule_rate_after_failures(schedule, failed_attempts)?;
        TxPhyRate::from_code(code, self.channel_width)
    }
}

/// One-spatial-stream 802.11ax modulation and coding scheme.
///
/// The ESP32-S31 is 1T1R. HE SU therefore admits MCS0..MCS9 for this first
/// bounded formatter; unchecked integers cannot request a second spatial
/// stream or the still-unqualified MCS10/11 modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HeMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs2 = 2,
    Mcs3 = 3,
    Mcs4 = 4,
    Mcs5 = 5,
    Mcs6 = 6,
    Mcs7 = 7,
    Mcs8 = 8,
    Mcs9 = 9,
}

impl HeMcs {
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Mcs0),
            1 => Some(Self::Mcs1),
            2 => Some(Self::Mcs2),
            3 => Some(Self::Mcs3),
            4 => Some(Self::Mcs4),
            5 => Some(Self::Mcs5),
            6 => Some(Self::Mcs6),
            7 => Some(Self::Mcs7),
            8 => Some(Self::Mcs8),
            9 => Some(Self::Mcs9),
            _ => None,
        }
    }
}

/// HE DCM MCS values valid in the currently owned BCC SU profile.
///
/// This intentionally does not expose an unchecked integer. The pinned
/// `_oracles/libpp.a[trc.o]::rcGetDCMMaxRate` selects exactly the internal
/// rate-control fallback codes `0x10`, `0x11`, and `0x13` for BPSK, QPSK,
/// and 16-QAM DCM. Its RU242 DCM table additionally contains MCS4, but that
/// requires the still-unowned LDPC profile and therefore must not be combined
/// with the current `HE_SU_A2_CONTROL_BCC` image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HeBccDcmMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs3 = 3,
}

impl HeBccDcmMcs {
    pub const fn mcs(self) -> HeMcs {
        match self {
            Self::Mcs0 => HeMcs::Mcs0,
            Self::Mcs1 => HeMcs::Mcs1,
            Self::Mcs3 => HeMcs::Mcs3,
        }
    }
}

/// One EDCA TXOP limit in the 32-us units used by the 802.11 WMM parameter.
///
/// A raw value of zero selects the vendor's default HE PPDU-duration policy;
/// a nonzero value bounds the exchange duration and therefore changes the
/// maximum APEP independently for every MCS and GI/LTF combination. Keeping
/// the raw unit in this type avoids accidentally passing microseconds to the
/// recovered table producer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeEdcaTxopLimit {
    units_32_us: u8,
}

impl HeEdcaTxopLimit {
    /// Vendor default used when the advertised EDCA TXOP field is zero.
    pub const DEFAULT: Self = Self { units_32_us: 0 };
    /// Largest value retained by the complete vendor WMM parser.
    pub const MAXIMUM_SUPPORTED: Self = Self {
        units_32_us: u8::MAX,
    };

    /// Admit a standard 16-bit WMM TXOP when the vendor data path can retain it.
    ///
    /// Complete `ieee80211_parse_wmeparams` stores only the low byte in its
    /// seven-byte per-AC record. Returning `None` for a larger standard value
    /// makes that implementation boundary explicit instead of truncating it.
    pub const fn from_units_32_us(units_32_us: u16) -> Option<Self> {
        if units_32_us <= u8::MAX as u16 {
            Some(Self {
                units_32_us: units_32_us as u8,
            })
        } else {
            None
        }
    }

    pub const fn units_32_us(self) -> u8 {
        self.units_32_us
    }

    pub const fn is_default(self) -> bool {
        self.units_32_us == 0
    }
}

/// Typed HE20 SU transmit rate for the single S31 spatial stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeRate {
    mcs: HeMcs,
    guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    dcm: bool,
}

impl HeRate {
    pub const fn new(mcs: HeMcs, guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf) -> Self {
        Self {
            mcs,
            guard_interval_and_ltf,
            dcm: false,
        }
    }

    /// Construct one standard-valid HE SU BCC+DCM rate.
    ///
    /// SOURCE: complete `_oracles/libpp.a[trc.o]::{rcGetDCMMaxRate,
    /// he_get_min_subframe_len_dcm}`, the RU242 DCM rate table, and complete
    /// `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_hesig`.
    pub const fn bcc_dcm(
        mcs: HeBccDcmMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self {
            mcs: mcs.mcs(),
            guard_interval_and_ltf,
            dcm: true,
        }
    }

    pub const fn mcs(self) -> HeMcs {
        self.mcs
    }

    pub const fn guard_interval_and_ltf(self) -> crate::rx::HeGuardIntervalAndLtf {
        self.guard_interval_and_ltf
    }

    pub const fn is_dcm(self) -> bool {
        self.dcm
    }

    /// Canonical S31 HE descriptor rate code.
    ///
    /// Explicit DCM and non-DCM HE submissions both use `0x1a + MCS`; DCM is
    /// carried independently in descriptor-state bit 15 and HE-SIG-A1 bit 7.
    /// `HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29` qualified descriptor rate
    /// `0x1a`, PLCP1 `0x0401a000`, and HE-SIG-A1 `0xfc204087`.
    ///
    /// Do not confuse this with [`Self::rate_control_dcm_fallback_code`].
    /// Complete `_oracles/libpp.a[trc.o]::rcGetDCMMaxRate` can rewrite the
    /// descriptor to a separate `0x10 + MCS` domain when its internal
    /// rate-control state requests a DCM fallback.
    pub const fn code(self) -> u8 {
        0x1a + self.mcs.index()
    }

    /// Internal vendor rate-control fallback code for an explicit DCM rate.
    ///
    /// SOURCE: complete `_oracles/libpp.a[trc.o]::rcGetDCMMaxRate`. That
    /// function first requires descriptor-state word bit 21, selects the
    /// peer-bounded DCM constellation from bits 1:0, then stores exactly
    /// `0x10`, `0x11`, or `0x13` and sets descriptor byte `+0x31` bit 7.
    /// This is exposed for a future owned rate-control port; the direct queue
    /// formatter must continue to use [`Self::code`].
    pub const fn rate_control_dcm_fallback_code(self) -> Option<u8> {
        if self.dcm {
            Some(0x10 + self.mcs.index())
        } else {
            None
        }
    }

    /// HE and HT use the same MCS-indexed calibrated power pair.
    ///
    /// Complete `hal_mac_tx_set_ppdu` subtracts ten from HE codes 26..35
    /// before indexing the 43-pair MAC power table.
    pub const fn power_lookup_code(self) -> u8 {
        16 + self.mcs.index()
    }

    pub const fn vendor_rts_rate(self) -> LegacyRate {
        match self.mcs {
            HeMcs::Mcs0 => LegacyRate::Ofdm6M,
            HeMcs::Mcs1 | HeMcs::Mcs2 => LegacyRate::Ofdm12M,
            HeMcs::Mcs3
            | HeMcs::Mcs4
            | HeMcs::Mcs5
            | HeMcs::Mcs6
            | HeMcs::Mcs7
            | HeMcs::Mcs8
            | HeMcs::Mcs9 => LegacyRate::Ofdm24M,
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        const GI_800: [u32; 10] = [
            8_600, 17_200, 25_800, 34_400, 51_600, 68_800, 77_400, 86_000, 103_200, 114_700,
        ];
        const GI_1600: [u32; 10] = [
            8_100, 16_300, 24_400, 32_500, 48_800, 65_000, 73_100, 81_300, 97_500, 108_300,
        ];
        const GI_3200: [u32; 10] = [
            7_300, 14_600, 21_900, 29_300, 43_900, 58_500, 65_800, 73_100, 87_800, 97_500,
        ];
        const DCM_GI_800: [u32; 10] = [4_300, 8_600, 0, 17_200, 0, 0, 0, 0, 0, 0];
        const DCM_GI_1600: [u32; 10] = [4_000, 8_100, 0, 16_300, 0, 0, 0, 0, 0, 0];
        const DCM_GI_3200: [u32; 10] = [3_600, 7_300, 0, 14_600, 0, 0, 0, 0, 0, 0];
        let table = match (self.dcm, self.guard_interval_and_ltf) {
            (true, crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns)
            | (true, crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns) => &DCM_GI_800,
            (true, crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns) => &DCM_GI_1600,
            (true, crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns) => &DCM_GI_3200,
            (false, crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns)
            | (false, crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns) => &GI_800,
            (false, crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns) => &GI_1600,
            (false, crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns) => &GI_3200,
        };
        table[self.mcs.index() as usize]
    }

    /// Maximum APEP bytes for the vendor's zero-TXOP HE SU policy.
    ///
    /// SOURCE: ROM rev0 `he_max_apep_length` at `0x2f84fd40`, complete
    /// `_oracles/libpp.a[trc.o]::rx11AXRate2AMPDULimit`, and complete
    /// `_oracles/libpp.a[pp_he.o]::ppCheckTxHEAMPDUlength`.
    ///
    /// The ROM object contains three GI rows by ten MCS columns. The selector
    /// maps both 0.8-us GI encodings to row zero. `ppCheckTxHEAMPDUlength`
    /// consumes the selected value as a `u16` and halves it when descriptor
    /// state bit 15 selects DCM. This limit is independent of the peer's
    /// advertised maximum A-MPDU exponent; an owner must satisfy both.
    ///
    /// A nonzero EDCA TXOP limit uses the separately generated
    /// `rx11AXRate2AMPDULimit_update` table and is deliberately not presented
    /// as this zero-TXOP result.
    pub const fn maximum_default_apep_bytes(self) -> u16 {
        const GI_800: [u16; 10] = [
            3_700, 7_400, 11_100, 14_800, 22_500, 30_000, 33_600, 37_600, 45_000, 50_000,
        ];
        const GI_1600: [u16; 10] = [
            3_500, 7_000, 10_500, 14_000, 21_000, 28_200, 31_500, 35_200, 42_300, 47_000,
        ];
        const GI_3200: [u16; 10] = [
            3_200, 6_400, 9_600, 12_800, 19_000, 25_200, 28_700, 32_000, 37_800, 42_000,
        ];
        let row = match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => &GI_800,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => &GI_1600,
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => &GI_3200,
        };
        let limit = row[self.mcs.index() as usize];
        if self.dcm {
            limit / 2
        } else {
            limit
        }
    }

    /// Maximum APEP bytes for this rate and one EDCA TXOP limit.
    ///
    /// SOURCE: complete `_oracles/libpp.a[trc.o]::
    /// rx11AXRate2AMPDULimit_update` (size `0x136`), complete
    /// `_oracles/libpp.a[pp_he.o]::get_estimated_batime` (size `0x1a`), and
    /// `_oracles/libpp.a[hal_mac_ctl.o]::{he_preamble_ersu,
    /// he_time_per_sym,he_data_bits_per_sym}`.
    ///
    /// The blob generates one three-GI-by-ten-MCS `u32` table for each of the
    /// four EDCA access categories. For a nonzero TXOP it subtracts 36 us,
    /// the rate-dependent estimated BlockAck time, and the GI-dependent
    /// preamble, converts the remaining time to symbols, performs one fused
    /// `NDBPS * symbols - 22` operation, truncates toward zero, and divides by
    /// eight. The `-22` term is the service/tail-bit allowance. The exact
    /// recovered constants are:
    ///
    /// - BlockAck: 68 us for MCS0, 44 us for MCS1/2, 32 us for MCS3..9;
    /// - preamble: 31.2, 32, or 40 us;
    /// - symbol: 13.6, 14.4, or 16 us;
    /// - RU242 NDBPS: 117, 234, 351, 468, 702, 936, 1053, 1170, 1404, 1560.
    ///
    /// A zero limit retains the ROM table exposed by
    /// [`Self::maximum_default_apep_bytes`]. Complete
    /// `ppCheckTxHEAMPDUlength` applies the DCM divide-by-two after either
    /// table lookup, which this method also reproduces.
    pub const fn maximum_apep_bytes(self, txop: HeEdcaTxopLimit) -> u32 {
        if txop.is_default() {
            return self.maximum_default_apep_bytes() as u32;
        }

        const DATA_BITS_PER_SYMBOL_RU242: [i32; 10] =
            [117, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
        let mcs = self.mcs.index() as usize;
        let estimated_block_ack_us = match self.mcs {
            HeMcs::Mcs0 => 68,
            HeMcs::Mcs1 | HeMcs::Mcs2 => 44,
            HeMcs::Mcs3
            | HeMcs::Mcs4
            | HeMcs::Mcs5
            | HeMcs::Mcs6
            | HeMcs::Mcs7
            | HeMcs::Mcs8
            | HeMcs::Mcs9 => 32,
        };
        let exchange_budget_us = txop.units_32_us() as i64 * 32 - 36;
        let data_bits_per_symbol = DATA_BITS_PER_SYMBOL_RU242[mcs] as i64;
        let bytes = match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => {
                // ((budget - BA - 31.2) / 13.6 * NDBPS - 22) / 8
                ((5 * (exchange_budget_us - estimated_block_ack_us) - 156) * data_bits_per_symbol
                    - 22 * 68)
                    / (68 * 8)
            }
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => {
                // ((budget - BA - 32) / 14.4 * NDBPS - 22) / 8
                (5 * (exchange_budget_us - estimated_block_ack_us - 32) * data_bits_per_symbol
                    - 22 * 72)
                    / (72 * 8)
            }
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => {
                // ((budget - BA - 40) / 16 * NDBPS - 22) / 8
                ((exchange_budget_us - estimated_block_ack_us - 40) * data_bits_per_symbol
                    - 22 * 16)
                    / (16 * 8)
            }
        };
        // The exact rational form was exhaustively compared with the blob's
        // f32 instruction sequence for all 256 values admitted by its WMM
        // parser, all three rows, and all ten MCS values.
        let limit = bytes as u32;
        if self.dcm {
            limit / 2
        } else {
            limit
        }
    }

    /// Minimum HE A-MPDU subframe length for a negotiated density.
    ///
    /// This reproduces complete `_oracles/libpp.a[trc.o]::
    /// {he_get_min_subframe_len,he_get_min_subframe_len_dcm}`: select the
    /// ordinary or DCM RU242 rate in 100-kbit/s units, multiply by the blob's
    /// integer-microsecond density, truncate by eight, then divide by ten and
    /// round up when a remainder remains.
    ///
    /// The value is the required complete A-MPDU subframe length in bytes.
    /// The caller still owns the distinction between MPDU bytes, the
    /// four-byte delimiter, four-byte alignment, and any extra empty
    /// delimiters.
    pub const fn minimum_ampdu_subframe_bytes(self, density: HtAmpduDensity) -> u16 {
        let density_us = density.vendor_integer_microseconds();
        let rate_100_kbps = self.nominal_kbps() / 100;
        let truncated_byte_rate = rate_100_kbps * density_us as u32 / 8;
        let bytes = truncated_byte_rate / 10 + (truncated_byte_rate % 10 != 0) as u32;
        bytes as u16
    }

    /// Empty four-byte delimiters required after one HE A-MPDU PSDU.
    ///
    /// `psdu_length` includes the hardware MIC and FCS. Complete
    /// `_oracles/libpp.a[pp_he.o]::ppCheckTxHEAMPDUlength` passes
    /// `round_up(psdu_length + 4, 4)` to `ppCalDeliNum`; with the owned
    /// metadata profile this is `round_up(psdu_length, 4) + 4`.
    /// `ppCalDeliNum` writes `ceil((minimum-current)/4)` to metadata byte
    /// four when the subframe is short and zero otherwise.
    pub const fn ampdu_empty_delimiters(
        self,
        psdu_length: u16,
        density: HtAmpduDensity,
    ) -> Option<u8> {
        if psdu_length == 0 || psdu_length > 0x3fff {
            return None;
        }
        let current = ((psdu_length as u32 + 3) & !3) + 4;
        let minimum = self.minimum_ampdu_subframe_bytes(density) as u32;
        if current >= minimum {
            return Some(0);
        }
        let delimiters = (minimum - current + 3) / 4;
        if delimiters > u8::MAX as u32 {
            None
        } else {
            Some(delimiters as u8)
        }
    }
}

/// One finite legacy, HT, or HE SU rate used by the open transmit path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxPhyRate {
    Legacy(LegacyRate),
    Ht(HtRate),
    He(HeRate),
}

impl TxPhyRate {
    pub const fn from_code(code: u8, ht_width: HtChannelWidth) -> Option<Self> {
        if let Some(rate) = LegacyRate::from_code(code) {
            return Some(Self::Legacy(rate));
        }
        let (mcs, guard_interval) = if code >= 16 && code <= 23 {
            (code - 16, HtGuardInterval::Long800Ns)
        } else if code >= 26 && code <= 33 {
            (code - 26, HtGuardInterval::Short400Ns)
        } else {
            return None;
        };
        let Some(mcs) = HtMcs::from_index(mcs) else {
            return None;
        };
        Some(Self::Ht(HtRate::new(mcs, guard_interval, ht_width)))
    }

    pub const fn code(self) -> u8 {
        match self {
            Self::Legacy(rate) => rate.code(),
            Self::Ht(rate) => rate.code(),
            Self::He(rate) => rate.code(),
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        match self {
            Self::Legacy(rate) => rate.nominal_kbps(),
            Self::Ht(rate) => rate.nominal_kbps(),
            Self::He(rate) => rate.nominal_kbps(),
        }
    }
}

/// Inputs for one finite non-HE q0 attempt.
///
/// For the direct raw q0 management path, `signal` is the transmitted
/// `MPDU + FCS` byte length written to `TX_Q_PLCP1`; it is not a vendor
/// descriptor snapshot. Power values are indices in the PHY gain table, not
/// dBm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyTxConfig {
    pub rate: LegacyRate,
    pub rts_rate: LegacyRate,
    pub signal: u16,
    pub data_power: u8,
    pub rts_power_low: u8,
    pub rts_power_high: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: u8,
    /// Priority written to the ordinary queue scheduler field.
    ///
    /// This can differ from [`Self::pti`]: the pinned vendor
    /// `mac_tx_set_pti` raises it to at least coexistence event one's PTI
    /// while retaining the original packet PTI in the four vector lanes.
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    /// Whether address one is a group address.
    ///
    /// SOURCE: `_oracles/libpp.a[pp.o]::ppTxProtoProc` copies the low bit of
    /// address one into descriptor flag `0x0000_0002`.
    /// `_oracles/libpp.a[hal_mac_tx.o]::mac_tx_set_plcp0` then preserves PLCP0
    /// format zero when that flag is set. For the bounded plain legacy profile
    /// represented here, an ordinary individual address follows the recovered
    /// zero-flags branch and selects format one. This distinction is qualified
    /// by the vendor authentication capture and by repeated open STA
    /// authentication/association/WPA2/DHCP runs.
    pub group_receiver: bool,
    /// Low byte of the recovered descriptor control word. Zero is plaintext;
    /// protected STA pairwise traffic uses its owned hardware key slot.
    pub hardware_key_selector: u8,
}

/// Inputs for one finite, non-aggregate HT MPDU attempt.
///
/// `length` is the complete on-air PSDU byte count produced by hardware:
/// encoded MPDU plus any hardware-generated security MIC and the four-byte
/// FCS. Power bytes are calibrated PHY gain-table indices, not dBm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtTxConfig {
    pub rate: HtRate,
    pub protection_spacing: HtProtectionSpacing,
    pub length: u16,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: u8,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HtTxConfig {
    /// Construct a single-MPDU HT profile from the encoded MPDU length.
    pub const fn single_mpdu(
        rate: HtRate,
        mpdu_length: u16,
        hardware_mic_length: u8,
    ) -> Option<Self> {
        let Some(length) = mpdu_length.checked_add(hardware_mic_length as u16) else {
            return None;
        };
        let Some(length) = length.checked_add(LEGACY_FCS_LENGTH) else {
            return None;
        };
        if length == 0 {
            return None;
        }
        Some(Self {
            rate,
            protection_spacing: HtProtectionSpacing::Density0To4,
            length,
            data_power_primary: 0,
            data_power_alternate: 0,
            rts_power_primary: 0,
            rts_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: 0x03ff,
            interface: 0,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    const fn valid(self) -> bool {
        self.length != 0
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.interface <= 3
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }
}

/// Inputs for one HE20 SU S-MPDU PPDU.
///
/// An HE single MPDU is not the direct non-aggregate HT layout. IEEE 802.11ax
/// carries it in an S-MPDU container, and the pinned PP implementation adds a
/// delimiter around an MPDU length which already includes the FCS before
/// publishing APEP length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeSmpduTxConfig {
    pub rate: HeRate,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    /// Encoded 802.11 MPDU bytes before the hardware-generated FCS.
    pub mpdu_length: u16,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: u8,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
    pub protection_spacing: u16,
}

impl HeSmpduTxConfig {
    /// Construct the exact bounded vendor S-MPDU profile.
    ///
    /// SOURCE: complete `_oracles/libpp.a[pp_he.o]::
    /// ppCalTxHESMPDULength` (`0x58` bytes) first rounds the descriptor
    /// payload length to four, calls `ppCalDeliNum` and
    /// `ppCalSubFrameLength`, then sets descriptor byte `+0x32` bit two,
    /// frame byte `+0x26` to one and descriptor byte `+0x2a` to one.
    /// Complete `ppCalSubFrameLength` (`0x3e` bytes) adds the mandatory
    /// four-byte delimiter. The live vendor raw oracle independently captured
    /// metadata word `0x0100_001c`: payload length 28 is the 24-byte encoded
    /// MPDU plus hardware FCS, metadata bit 24 is set, and metadata byte seven
    /// (the optional extra four-byte term) is zero. Therefore APEP is
    /// `round_up(mpdu + FCS, 4) + delimiter`, equivalent to
    /// `round_up(mpdu, 4) + 8`.
    pub const fn new(rate: HeRate, bss_color: u8, mpdu_length: u16) -> Option<Self> {
        if bss_color > 0x3f || mpdu_length == 0 || mpdu_length > 0x3ff7 {
            return None;
        }
        Some(Self {
            rate,
            bss_color,
            spatial_reuse: 0,
            mpdu_length,
            data_power_primary: 0,
            data_power_alternate: 0,
            rts_power_primary: 0,
            rts_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: 0x03ff,
            interface: 0,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
            protection_spacing: 0x31,
        })
    }

    /// Complete HE S-MPDU APEP length written to HE-SIG-A2.
    pub const fn apep_length(self) -> u16 {
        ((self.mpdu_length + 3) & !3) + 8
    }

    const fn valid(self) -> bool {
        self.bss_color <= 0x3f
            && self.spatial_reuse <= 0x0f
            && self.mpdu_length != 0
            && self.mpdu_length <= 0x3ff7
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.interface <= 3
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
            && self.protection_spacing <= 0x03ff
    }
}

/// Inputs for one basic-HT A-MPDU PPDU.
///
/// Unlike [`HtTxConfig`], `aggregate_length` is the already assembled A-MPDU
/// byte count, including each delimiter and all non-final alignment/empty
/// delimiters. It must come from the bounded aggregate ownership path; this
/// type never adds a second FCS or guesses delimiter padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtAmpduTxConfig {
    pub rate: HtRate,
    pub protection_spacing: HtProtectionSpacing,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: u8,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HtAmpduTxConfig {
    /// Construct an aggregate PPDU profile from an already validated length.
    ///
    /// `subframes` is retained as a finite 1..=32 value because hardware and
    /// the vendor formatter can represent a one-subframe A-MPDU, even though
    /// the ordinary batching policy normally waits for at least two frames.
    pub const fn new(rate: HtRate, aggregate_length: u16, subframes: u8) -> Option<Self> {
        if aggregate_length == 0 || subframes == 0 || subframes > 32 {
            return None;
        }
        Some(Self {
            rate,
            protection_spacing: HtProtectionSpacing::Density0To4,
            aggregate_length,
            subframes,
            data_power_primary: 0,
            data_power_alternate: 0,
            rts_power_primary: 0,
            rts_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: 0x03ff,
            interface: 0,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    const fn valid(self) -> bool {
        self.aggregate_length != 0
            && self.subframes != 0
            && self.subframes <= 32
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.interface <= 3
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }
}

/// Optional Trigger-based eligibility for an owned HE A-MPDU queue.
///
/// This prepares the exact queue/MPLEN/BSR state consumed by an AP Trigger;
/// it does not change the immediately submitted HE-SU PPDU into a TB PPDU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeTriggerBasedTxConfig {
    tid_limit: MacHeTbTidLimit,
    tid: MacHeTid,
}

impl HeTriggerBasedTxConfig {
    /// Select one QoS TID admitted by the exact vendor eligibility table.
    ///
    /// SOURCE: complete `_oracles/libpp.a[if_hecfg.o]::
    /// wifi_he_get_hetb_tid_bitmap`. Keeping the validation here prevents a
    /// later MMIO transaction from silently doing nothing for an ineligible
    /// TID, as the complete `mac_tx_set_tb` body does.
    pub const fn new(tid_limit: MacHeTbTidLimit, tid: MacHeTid) -> Option<Self> {
        if tid_limit.contains(tid) {
            Some(Self { tid_limit, tid })
        } else {
            None
        }
    }

    pub const fn tid_limit(self) -> MacHeTbTidLimit {
        self.tid_limit
    }

    pub const fn tid(self) -> MacHeTid {
        self.tid
    }
}

/// Inputs for one bounded HE20 SU A-MPDU PPDU.
///
/// Trigger eligibility is optional and remains separate from the ordinary
/// HE-SU vector. Enabling it does not turn an SU transmission into a TB PPDU;
/// it prepares the owned queue and BSR/MPLEN state needed if the AP later
/// sends a matching Trigger frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeAmpduTxConfig {
    rate: HeRate,
    ampdu_density: HtAmpduDensity,
    trigger_based: Option<HeTriggerBasedTxConfig>,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    /// Three repeated ten-bit descriptor-derived timing values.
    ///
    /// The complete formatter proves the mapping. Its higher-level vendor
    /// state producer is not yet promoted, so callers must retain the
    /// negotiated/qualified finite value explicitly.
    protection_spacing: u16,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub rts_power_primary: u8,
    pub rts_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: u8,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HeAmpduTxConfig {
    pub const fn new(
        rate: HeRate,
        bss_color: u8,
        aggregate_length: u16,
        subframes: u8,
        ampdu_density: HtAmpduDensity,
    ) -> Option<Self> {
        if bss_color > 0x3f || aggregate_length == 0 || subframes == 0 || subframes > 32 {
            return None;
        }
        Some(Self {
            rate,
            ampdu_density,
            trigger_based: None,
            bss_color,
            spatial_reuse: 0,
            // SOURCE: complete `_oracles/libpp.a[pp_he.o]::ppCalDeliNum`
            // writes he_get_min_subframe_len_static's result to descriptor
            // offset +0x28. Complete `_oracles/libpp.a[hal_mac_tx.o]::
            // mac_tx_set_hesig` later copies that halfword into all three
            // queue PROTECTION spacing lanes. Keeping rate, density and this
            // derived value in one constructor prevents a metadata/MMIO
            // mismatch like the fixed-0x31 failure qualified by
            // HIL_OPEN_HE20_MCS9_EMPTY_DELIMITERS_2026_07_29.
            protection_spacing: rate.minimum_ampdu_subframe_bytes(ampdu_density),
            aggregate_length,
            subframes,
            data_power_primary: 0,
            data_power_alternate: 0,
            rts_power_primary: 0,
            rts_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: 0x03ff,
            interface: 0,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    pub const fn rate(self) -> HeRate {
        self.rate
    }

    pub const fn ampdu_density(self) -> HtAmpduDensity {
        self.ampdu_density
    }

    pub const fn protection_spacing(self) -> u16 {
        self.protection_spacing
    }

    pub const fn with_trigger_based(mut self, trigger_based: HeTriggerBasedTxConfig) -> Self {
        self.trigger_based = Some(trigger_based);
        self
    }

    pub const fn trigger_based(self) -> Option<HeTriggerBasedTxConfig> {
        self.trigger_based
    }

    const fn valid(self) -> bool {
        self.bss_color <= 0x3f
            && self.spatial_reuse <= 0x0f
            && self.protection_spacing <= 0x03ff
            && self.aggregate_length != 0
            && self.subframes != 0
            && self.subframes <= 32
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.interface <= 3
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }
}

impl LegacyTxConfig {
    /// Conservative 1-Mbit/s management-frame profile used by the first HIL.
    ///
    /// The timeout is the exact q0 image observed while the pinned vendor
    /// driver submitted an open-authentication MPDU on ESP32-S31 rev0. The
    /// no-backoff base image is `TX_Q0_CONFIG = 0x1200_03ff`
    /// (`scheduler=1`, `timeout=0x3ff`, `AIFSN=2`, `CW=0`) and
    /// `TX_Q0_PTI = 0x0011_1110` (`count=1`, four packet-PTI lanes at 1).
    /// SOURCE: live `wifi-sta-auth-probe` HIL plus complete
    /// `_oracles/libpp.a[hal_mac_tx.o,hal_mac.o,hal_coex.o]` and
    /// `_oracles/libcoexist.a[coexist_core.o]`.
    pub const fn management_1m(signal: u16) -> Self {
        Self {
            rate: LegacyRate::Dsss1MLong,
            rts_rate: LegacyRate::Dsss1MLong,
            signal,
            data_power: 8,
            rts_power_low: 8,
            rts_power_high: 8,
            aifsn: 2,
            contention_window: 0,
            timeout: 0x03ff,
            interface: 0,
            // Complete libpp.a[hal_mac.o,hal_coex.o] selects the unsigned
            // minimum of packet PTI 1 and coexistence event-one PTI 5.
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            group_receiver: false,
            hardware_key_selector: 0,
        }
    }

    /// Builds the direct q0 profile from the owned MPDU length.
    ///
    /// Hardware appends the four-byte FCS. For example, a 30-byte open
    /// authentication MPDU requires PLCP1 `0x22`. The S31 HIL proved that
    /// replaying the vendor-context value `0x00b6` instead produces TX status
    /// four and no authentication response.
    pub const fn management_1m_from_mpdu_length(mpdu_length: u16) -> Option<Self> {
        let Some(signal) = mpdu_length.checked_add(LEGACY_FCS_LENGTH) else {
            return None;
        };
        if signal > 0x0fff {
            return None;
        }
        Some(Self::management_1m(signal))
    }

    const fn valid(self) -> bool {
        self.signal <= 0x0fff
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.interface <= 3
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }
}

/// Pure register image for one q0 legacy attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyQ0Image {
    pub plcp0: u32,
    pub plcp1: u32,
    pub power: u32,
    pub length_control: u32,
}

/// Pure register image for one HT PPDU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtQ0Image {
    pub plcp0: u32,
    pub plcp1: u32,
    pub ht_signal: u32,
    pub data_length: u32,
    pub power: u32,
    pub length_control: u32,
    pub descriptor_count_a: u8,
    pub descriptor_count_b: u8,
    pub protection_spacing: u16,
}

/// Pure register image for one HE20 SU A-MPDU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeQ0Image {
    pub plcp0: u32,
    pub plcp1: u32,
    pub he_signal_a1: u32,
    pub he_signal_a2_length: u32,
    pub power: u32,
    pub length_control: u32,
    pub descriptor_count_a: u8,
    pub descriptor_count_b: u8,
    pub protection_spacing: u16,
}

const fn he_su_signal_a1(rate: HeRate, bss_color: u8, spatial_reuse: u8) -> u32 {
    0xfc00_4007
        | ((rate.mcs.index() as u32) << 3)
        | ((rate.is_dcm() as u32) << 7)
        | ((bss_color as u32) << 8)
        | ((spatial_reuse as u32) << 15)
        | ((rate.guard_interval_and_ltf.encoding() as u32) << 21)
}

/// Build the instruction-exact bounded HE20 SU S-MPDU queue image.
///
/// SOURCE: complete `_oracles/libpp.a[pp_he.o]::ppCalTxHESMPDULength`,
/// complete `_oracles/libpp.a[hal_mac_tx.o]::{mac_tx_set_plcp0,
/// mac_tx_set_plcp1,mac_tx_set_hesig,mac_tx_set_len,
/// hal_mac_tx_set_ppdu}`, and
/// `HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29`.
pub const fn he_smpdu_q0_image(
    dma_head_address: u32,
    config: HeSmpduTxConfig,
) -> Option<HeQ0Image> {
    if !descriptor_address_valid(dma_head_address) || !config.valid() {
        return None;
    }
    let rate = config.rate.code();
    let rts_rate = config.rate.vendor_rts_rate();
    Some(HeQ0Image {
        // The live single-MPDU descriptor flags 0xc040_7008 select basic
        // PLCP0 format one. HE A-MPDU's separate formatter selects format
        // five and must not be reused here.
        plcp0: basic_plcp0_word(dma_head_address as usize, HE_SMPDU_DESCRIPTOR_FLAGS),
        plcp1: he_plcp1_word(rate, config.hardware_key_selector),
        he_signal_a1: he_su_signal_a1(config.rate, config.bss_color, config.spatial_reuse),
        // S-MPDU leaves the A-MPDU selector at bit 28 clear.
        he_signal_a2_length: (HE_SU_A2_CONTROL_BCC as u32) | ((config.apep_length() as u32) << 11),
        power: config.data_power_primary as u32
            | ((config.data_power_alternate as u32) << 8)
            | ((config.rts_power_primary as u32) << 16)
            | ((config.rts_power_alternate as u32) << 24),
        length_control: basic_length_control_word(
            rts_rate.code(),
            HE_SMPDU_ENTRY_FLAGS,
            config.hardware_key_selector as u32,
        ),
        descriptor_count_a: 1,
        descriptor_count_b: 1,
        protection_spacing: config.protection_spacing,
    })
}

/// Build the instruction-exact basic HT queue image without touching MMIO.
///
/// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::{mac_tx_set_plcp0,
/// mac_tx_set_plcp1,mac_tx_set_htsig,mac_tx_set_len,hal_mac_tx_set_ppdu}`.
/// The direct slot represents exactly one descriptor, hence both recovered
/// descriptor-count lanes contain one. A-MPDU has a separate ownership path
/// and must not reuse this single-MPDU constructor.
pub const fn ht_q0_image(descriptor_address: u32, config: HtTxConfig) -> Option<HtQ0Image> {
    if !descriptor_address_valid(descriptor_address) || !config.valid() {
        return None;
    }
    let rate = config.rate.code();
    let rts_rate = config.rate.vendor_rts_rate();
    let channel_width_40 = match config.rate.channel_width {
        HtChannelWidth::Mhz20 => false,
        HtChannelWidth::Mhz40 => true,
    };
    // Complete mac_tx_set_htsig and mac_tx_set_plcp1 both consume descriptor
    // word1 bit 15 as the HT40 selector. The direct Rust-owned descriptor does
    // not carry the vendor metadata word, so construct its exact finite image
    // from the typed channel width.
    let descriptor_word1 = if channel_width_40 { 0x0000_8000 } else { 0 };
    Some(HtQ0Image {
        // SOURCE: HIL_VENDOR_HT20_MCS0_SINGLE_PPDU_2026_07_29. The exact
        // synchronous formatter result retains bit 22 in PLCP0. The separate
        // mac_tx_set_txop_q policy edge is not reached between lmacSetTxFrame
        // and queue publication for this vendor single-MPDU path.
        plcp0: basic_plcp0_word(descriptor_address as usize, HT_SINGLE_DESCRIPTOR_FLAGS),
        plcp1: basic_non_he_plcp1_word(
            rate,
            HT_SINGLE_DESCRIPTOR_FLAGS,
            config.hardware_key_selector,
            descriptor_word1,
            0,
        ),
        ht_signal: basic_htsig_word(rate, channel_width_40, config.length as u32),
        data_length: basic_data_length_word(rate, config.length as u32, HT_SINGLE_ENTRY_FLAGS),
        power: config.data_power_primary as u32
            | ((config.data_power_alternate as u32) << 8)
            | ((config.rts_power_primary as u32) << 16)
            | ((config.rts_power_alternate as u32) << 24),
        length_control: basic_length_control_word(
            rts_rate.code(),
            HT_SINGLE_ENTRY_FLAGS,
            config.hardware_key_selector as u32,
        ),
        descriptor_count_a: 1,
        descriptor_count_b: 1,
        protection_spacing: config.protection_spacing.hardware_value(),
    })
}

/// Build the instruction-exact basic-HT A-MPDU queue image without MMIO.
///
/// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::{mac_tx_set_plcp0,
/// mac_tx_set_plcp1,mac_tx_set_htsig,mac_tx_set_len,hal_mac_tx_set_ppdu}` and
/// `HIL_VENDOR_HT_AMPDU_PPDU`. The latter synchronously captured a two-MPDU
/// MCS7/SGI aggregate with length 0x0c2e:
/// HT-SIG=0x8f0c2e07, DATA_LENGTH=0x70400c2e and
/// LENGTH_CONTROL=0x00400244.
pub const fn ht_ampdu_q0_image(
    dma_head_address: u32,
    config: HtAmpduTxConfig,
) -> Option<HtQ0Image> {
    if !descriptor_address_valid(dma_head_address) || !config.valid() {
        return None;
    }
    let rate = config.rate.code();
    let rts_rate = config.rate.vendor_rts_rate();
    let channel_width_40 = match config.rate.channel_width {
        HtChannelWidth::Mhz20 => false,
        HtChannelWidth::Mhz40 => true,
    };
    let descriptor_word1 = if channel_width_40 { 0x0000_8000 } else { 0 };
    Some(HtQ0Image {
        // SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::
        // mac_tx_set_plcp0` as reached from the A-MPDU submit branch. Its
        // address input is `frame+0x04`, the first 12-byte DMA buffer
        // descriptor. The PP descriptor at `frame+0x34` supplies flags and
        // rate metadata but is not the hardware walker head.
        plcp0: basic_plcp0_word(dma_head_address as usize, HT_AMPDU_DESCRIPTOR_FLAGS),
        plcp1: basic_non_he_plcp1_word(
            rate,
            HT_AMPDU_DESCRIPTOR_FLAGS,
            config.hardware_key_selector,
            descriptor_word1,
            0,
        ),
        ht_signal: ht_htsig_word(rate, channel_width_40, config.aggregate_length as u32, true),
        data_length: basic_data_length_word(
            rate,
            config.aggregate_length as u32,
            HT_AMPDU_ENTRY_FLAGS,
        ),
        power: config.data_power_primary as u32
            | ((config.data_power_alternate as u32) << 8)
            | ((config.rts_power_primary as u32) << 16)
            | ((config.rts_power_alternate as u32) << 24),
        length_control: basic_length_control_word(
            rts_rate.code(),
            HT_AMPDU_ENTRY_FLAGS,
            config.hardware_key_selector as u32,
        ),
        descriptor_count_a: config.subframes,
        descriptor_count_b: config.subframes,
        protection_spacing: config.protection_spacing.hardware_value(),
    })
}

/// Build the instruction-exact bounded HE20 SU A-MPDU queue image.
///
/// SOURCE: complete `_oracles/libpp.a[hal_mac_tx.o]::{mac_tx_set_plcp0,
/// mac_tx_set_plcp1,mac_tx_set_hesig,mac_tx_set_len,
/// hal_mac_tx_set_ppdu}`, complete `pp_he.o::ppCalTxHEAMPDULength`, and
/// `HIL_VENDOR_HE20_MCS9_SU_2026_07_29`.
pub const fn he_ampdu_q0_image(
    dma_head_address: u32,
    config: HeAmpduTxConfig,
) -> Option<HeQ0Image> {
    if !descriptor_address_valid(dma_head_address) || !config.valid() {
        return None;
    }
    let rate = config.rate.code();
    let rts_rate = config.rate.vendor_rts_rate();
    let he_signal_a1 = he_su_signal_a1(config.rate, config.bss_color, config.spatial_reuse);
    Some(HeQ0Image {
        plcp0: he_ampdu_plcp0_word(dma_head_address as usize),
        plcp1: he_plcp1_word(rate, config.hardware_key_selector),
        he_signal_a1,
        he_signal_a2_length: (HE_SU_A2_CONTROL_BCC as u32)
            | ((config.aggregate_length as u32) << 11)
            | 0x1000_0000,
        power: config.data_power_primary as u32
            | ((config.data_power_alternate as u32) << 8)
            | ((config.rts_power_primary as u32) << 16)
            | ((config.rts_power_alternate as u32) << 24),
        length_control: basic_length_control_word(
            rts_rate.code(),
            HE_AMPDU_ENTRY_FLAGS,
            config.hardware_key_selector as u32,
        ),
        descriptor_count_a: config.subframes,
        descriptor_count_b: config.subframes,
        protection_spacing: config.protection_spacing,
    })
}

pub const fn legacy_q0_image(
    descriptor_address: u32,
    config: LegacyTxConfig,
) -> Option<LegacyQ0Image> {
    if !descriptor_address_valid(descriptor_address) || !config.valid() {
        return None;
    }
    // Complete `_oracles/libpp.a[pp.o]::ppTxFragmentProc` reaches its common
    // legacy setup at offsets 0x8e..0xa8 for the CCMP selector-three branch
    // at 0x22a and sets descriptor bit seven before the LMAC formatter. The
    // receiver-class bit is added independently by ppTxProtoProc.
    let descriptor_flags = 0x0000_0080
        | if config.group_receiver {
            0x0000_0002
        } else {
            0
        };
    Some(LegacyQ0Image {
        // The complete recovered formatter consumes many descriptor states.
        // This direct profile admits only its two plain legacy roots: zero for
        // an individual receiver and bit one for a group receiver.
        //
        // Complete mac_tx_set_txop_q runs after mac_tx_set_plcp0. The ordinary
        // descriptor class above retains control bit 22; keeping the second
        // edge explicit prevents the upper path from guessing its value.
        plcp0: apply_basic_txop_control_word(
            basic_plcp0_word(descriptor_address as usize, descriptor_flags),
            descriptor_flags,
        ),
        plcp1: basic_non_he_plcp1_word(
            config.rate.code(),
            0,
            config.hardware_key_selector,
            0,
            config.signal as u32,
        ),
        power: config.data_power as u32
            | ((config.rts_power_low as u32) << 16)
            | ((config.rts_power_high as u32) << 24),
        length_control: basic_length_control_word(
            config.rts_rate.code(),
            1,
            config.hardware_key_selector as u32,
        ),
    })
}

pub struct TxSlot {
    pub descriptor: Descriptor,
    state: TxSlotState,
    generation_cursor: u32,
    active: TxCookie,
    descriptor_address: u32,
    queue: LegacyTxQueue,
}

impl TxSlot {
    pub const fn new() -> Self {
        Self {
            descriptor: Descriptor::new(),
            state: TxSlotState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
            descriptor_address: 0,
            queue: LegacyTxQueue::Voice,
        }
    }

    pub fn state(&self) -> TxSlotState {
        self.state
    }

    pub fn reserve(
        &mut self,
        descriptor_address: u32,
        buffer_address: u32,
        buffer_capacity: u32,
        frame_length: u32,
    ) -> Result<TxCookie, TxError> {
        if self.state != TxSlotState::Free {
            return Err(TxError::Busy);
        }
        if !descriptor_address_valid(descriptor_address)
            || !dma_range_valid(buffer_address, buffer_capacity)
            || frame_length > buffer_capacity
        {
            return Err(TxError::Invalid);
        }
        // The low field retains allocation capacity while the high field
        // publishes the populated transfer length. They remain distinct even
        // for a direct buffer without an ESF-private prefix.
        let word0 = tx_owned_word(buffer_capacity, frame_length).ok_or(TxError::Invalid)?;
        let generation = self
            .generation_cursor
            .checked_add(1)
            .ok_or(TxError::ResetRequired)?;
        self.generation_cursor = generation;
        self.active = TxCookie(generation);
        self.descriptor_address = descriptor_address;
        self.queue = LegacyTxQueue::Voice;
        self.descriptor.publish(word0, buffer_address, 0);
        self.state = TxSlotState::Reserved;
        Ok(self.active)
    }

    /// Programs and starts one legacy q0 attempt.
    ///
    /// Pinning makes the descriptor address checked at `reserve` stable across
    /// the hardware ownership interval. The buffer must likewise remain in
    /// static/pinned DMA-visible SRAM until completion is detached.
    pub fn submit_legacy_q0<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        self.submit_legacy(hardware, cookie, LegacyTxQueue::Voice, config)
    }

    /// Programs and starts one legacy attempt on an ordinary EDCA queue.
    pub fn submit_legacy<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        // SAFETY: the method never moves the pinned slot; it changes only
        // scalar ownership fields and the device-owned descriptor words.
        let slot = unsafe { self.get_unchecked_mut() };
        if slot.state != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let actual_address = core::ptr::addr_of!(slot.descriptor).addr() as u32;
        if actual_address != slot.descriptor_address {
            return Err(TxError::Invalid);
        }
        let image = legacy_q0_image(actual_address, config).ok_or(TxError::Invalid)?;
        let index = queue.index();
        let program = MacLegacyTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            power: image.power,
            length_control: image.length_control,
            timeout: config.timeout,
            scheduler_priority: config.scheduler_priority,
            packet_priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        if !hardware.prepare_legacy_tx(index, program) {
            return Err(TxError::QueueActive);
        }

        // Publish the software owner before the final hardware edge. A fast
        // completion can therefore never observe the old Reserved state.
        slot.queue = queue;
        slot.state = TxSlotState::HardwareOwned;
        hardware.start_legacy_tx(index, image.plcp0);
        Ok(())
    }

    /// Programs and starts one non-aggregate HT MPDU.
    ///
    /// The caller must also have configured the PHY channel engine for
    /// `config.rate.channel_width` during association. This method publishes
    /// the matching HT-SIG/PLCP CBW bits, while the channel engine still owns
    /// secondary-channel placement.
    pub fn submit_ht<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtTxConfig,
    ) -> Result<(), TxError> {
        // SAFETY: the method never moves the pinned slot; it changes only
        // scalar ownership fields and the device-owned descriptor words.
        let slot = unsafe { self.get_unchecked_mut() };
        if slot.state != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let actual_address = core::ptr::addr_of!(slot.descriptor).addr() as u32;
        if actual_address != slot.descriptor_address {
            return Err(TxError::Invalid);
        }
        let image = ht_q0_image(actual_address, config).ok_or(TxError::Invalid)?;
        let index = queue.index();
        let program = MacHtTxProgram {
            plcp0: image.plcp0,
            plcp1: image.plcp1,
            ht_signal: image.ht_signal,
            data_length: image.data_length,
            power: image.power,
            length_control: image.length_control,
            descriptor_count_a: image.descriptor_count_a,
            descriptor_count_b: image.descriptor_count_b,
            protection_spacing: image.protection_spacing,
            timeout: config.timeout,
            scheduler_priority: config.scheduler_priority,
            packet_priority: config.pti,
            priority_count: config.pti_count,
            aifsn: config.aifsn,
            contention_window: config.contention_window,
            interface: config.interface,
        };
        if !hardware.prepare_ht_tx(index, program) {
            return Err(TxError::QueueActive);
        }

        slot.queue = queue;
        slot.state = TxSlotState::HardwareOwned;
        hardware.start_ht_tx(index, image.plcp0);
        Ok(())
    }

    /// Publishes the software owner immediately before the future management
    /// TX layer performs its final q0 ENABLE|VALID write under MAC-IRQ
    /// exclusion. Full q0 PPDU/rate configuration must precede this call; this
    /// method intentionally performs no incomplete hardware submission.
    pub fn mark_hardware_owned(&mut self, cookie: TxCookie) -> Result<(), TxError> {
        if self.state != TxSlotState::Reserved || cookie != self.active {
            return Err(TxError::Stale);
        }
        self.state = TxSlotState::HardwareOwned;
        Ok(())
    }

    /// Decodes and acknowledges one q0 completion. Storage stays retained in
    /// `Completed`; it is not reusable until `detach_completed` closes q0.
    pub fn acknowledge_q0_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<TxCompletion>, TxError> {
        self.acknowledge_completion(hardware)
    }

    /// Decodes and acknowledges the completion for the queue retained by the
    /// hardware-owned slot.
    pub fn acknowledge_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<TxCompletion>, TxError> {
        let index = self.queue.index();
        let Some(registers) = hardware.take_tx_completion(index) else {
            return Ok(None);
        };

        if self.state != TxSlotState::HardwareOwned {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::Stale);
        }
        self.state = TxSlotState::Completed;
        Ok(Some(decode_tx_completion(self.active, registers)))
    }

    /// Starts the recovered two-phase abort for this queue's TX-timeout edge.
    ///
    /// `migration/lmac.rs::begin_tx_timeout` forces CCA to three before its
    /// fixed 16-us settling interval. `Ok(false)` means that this queue has no
    /// timeout edge and leaves all registers untouched.
    pub fn begin_timeout_abort<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        if !hardware.begin_tx_timeout_abort(self.queue.index()) {
            return Ok(false);
        }
        Ok(true)
    }

    /// Finishes a timed-out queue abort after at least 16 us of settling.
    ///
    /// The caller owns the one timer edge between this method and
    /// [`begin_timeout_abort`](Self::begin_timeout_abort). The register order
    /// matches the recovered migration path: invalidate, release forced CCA,
    /// disable a queue that was still valid, then clear its timeout bit.
    pub fn finish_timeout_abort<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::HardwareOwned || cookie != self.active {
            return Err(TxError::Stale);
        }
        let Some(detached) = hardware.finish_tx_timeout_abort(self.queue.index()) else {
            return Err(TxError::TimeoutNotPending);
        };
        if !detached {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::DetachFailed);
        }
        self.active = TxCookie(0);
        self.descriptor_address = 0;
        self.state = TxSlotState::Free;
        Ok(())
    }

    /// Makes the completed static slot reusable after disabling q0 and exact
    /// readback. This is normal single-attempt turnover, not a global DMA
    /// release oracle for freeing the backing allocation during teardown.
    pub fn detach_completed<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        if self.state != TxSlotState::Completed || cookie != self.active {
            return Err(TxError::Stale);
        }
        if !hardware.detach_completed_tx(self.queue.index()) {
            self.state = TxSlotState::ResetRequired;
            return Err(TxError::DetachFailed);
        }
        self.active = TxCookie(0);
        self.descriptor_address = 0;
        self.state = TxSlotState::Free;
        Ok(())
    }
}

impl Default for TxSlot {
    fn default() -> Self {
        Self::new()
    }
}
