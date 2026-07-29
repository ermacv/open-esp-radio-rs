//! Allocation-free IEEE 802.11ax Trigger and HE-control field parsing.
//!
//! This module deliberately describes bytes received over the air. It does
//! not describe ESP32-S31 MMIO. The primary implementation oracle is the
//! complete pinned `_oracles/libpp.a[hal_debug.o]` member:
//!
//! - `dbg_dump_trig_common_info`;
//! - `dbg_dump_trig_user_ru` and `dbg_dump_trig_user_ss`;
//! - `dbg_dump_trig_basic_dependent`, `dbg_dump_trig_bfrp_dependent`,
//!   `dbg_dump_trig_mubar_dependent` and `dbg_dump_trig_nfrp_user`;
//! - `dbg_dump_trs_control` and `dbg_dump_uph_control`.
//!
//! Every extraction below reproduces the loads, shifts and masks in those
//! functions. Derived units such as dBm are exposed separately from their
//! wire encodings.

pub const TRIGGER_COMMON_INFO_LEN: usize = 8;
pub const TRIGGER_MAC_HEADER_LEN: usize = 16;
pub const TRIGGER_FRAME_MIN_LEN: usize = TRIGGER_MAC_HEADER_LEN + TRIGGER_COMMON_INFO_LEN;
pub const TRIGGER_USER_INFO_LEN: usize = 5;
pub const TRIGGER_BASIC_DEPENDENT_LEN: usize = 1;
pub const TRIGGER_BFRP_DEPENDENT_LEN: usize = 1;
pub const TRIGGER_MU_BAR_DEPENDENT_LEN: usize = 4;
pub const TRIGGER_NFRP_USER_INFO_LEN: usize = 5;
pub const HE_CONTROL_INFO_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerParseError {
    Truncated { required: usize },
    NotTriggerFrame,
    UnsupportedUserLayout { trigger_type: TriggerType },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerType {
    Basic,
    BeamformingReportPoll,
    MultiUserBlockAckRequest,
    MultiUserRequestToSend,
    BufferStatusReportPoll,
    GroupcastMultiUserBlockAckRequest,
    BandwidthQueryReportPoll,
    NgpaFeedbackReportPoll,
    Reserved(u8),
}

impl TriggerType {
    pub const fn from_encoding(value: u8) -> Self {
        match value & 0x0f {
            0 => Self::Basic,
            1 => Self::BeamformingReportPoll,
            2 => Self::MultiUserBlockAckRequest,
            3 => Self::MultiUserRequestToSend,
            4 => Self::BufferStatusReportPoll,
            5 => Self::GroupcastMultiUserBlockAckRequest,
            6 => Self::BandwidthQueryReportPoll,
            7 => Self::NgpaFeedbackReportPoll,
            value => Self::Reserved(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerGiLtf {
    OneLtf800Ns,
    TwoLtf800Ns,
    TwoLtf1600Ns,
    FourLtf3200Ns,
}

impl TriggerGiLtf {
    const fn from_encoding(value: u8) -> Self {
        match value & 0x03 {
            0 => Self::OneLtf800Ns,
            1 => Self::TwoLtf800Ns,
            2 => Self::TwoLtf1600Ns,
            _ => Self::FourLtf3200Ns,
        }
    }
}

/// HE resource-unit widths represented by the S31 narrow-RU tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeResourceUnit {
    Ru26,
    Ru52,
    Ru106,
    #[default]
    Ru242,
}

/// One raw Trigger RU allocation classified by the complete blob helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerRuAllocation {
    /// A supported narrow RU and its one-based index in the printed group.
    Narrow {
        resource_unit: HeResourceUnit,
        one_based_index: u8,
    },
    /// The blob labels every encoding at or above 69 as `484 OFDM`.
    ///
    /// This is retained for diagnostics but is outside the ESP32-S31
    /// 20-MHz-only non-AP transmit profile.
    WiderThan242,
}

impl TriggerRuAllocation {
    /// Classify the seven-bit Trigger RU allocation without guessing gaps.
    ///
    /// SOURCE[BLOB_LIBPP_HAL_UTILITIES_RU2STR]: complete
    /// `_oracles/libpp.a[hal_utilities.o]::ru2str` (size `0x8c`) formats
    /// 0..8 as 26-tone index `raw+1`, 37..40 as 52-tone index `raw-36`,
    /// 53..54 as 106-tone index `raw-52`, 61..62 as 242-tone index
    /// `raw-60`, and values at least 69 as `484 OFDM`. The intervening
    /// encodings do not produce a fresh valid index and remain `None`.
    pub const fn from_encoding(raw: u8) -> Option<Self> {
        match raw {
            0..=8 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru26,
                one_based_index: raw + 1,
            }),
            37..=40 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru52,
                one_based_index: raw - 36,
            }),
            53..=54 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru106,
                one_based_index: raw - 52,
            }),
            61..=62 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru242,
                one_based_index: raw - 60,
            }),
            69..=0x7f => Some(Self::WiderThan242),
            _ => None,
        }
    }

    pub const fn resource_unit(self) -> Option<HeResourceUnit> {
        match self {
            Self::Narrow { resource_unit, .. } => Some(resource_unit),
            Self::WiderThan242 => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerTargetRssi {
    Dbm(i8),
    Reserved,
}

impl TriggerTargetRssi {
    const fn from_user_encoding(value: u8) -> Self {
        if value == 0x7f {
            Self::Reserved
        } else {
            Self::Dbm(value as i8 - 110)
        }
    }

    const fn from_trs_encoding(value: u8) -> Self {
        if value == 0x1f {
            Self::Reserved
        } else {
            Self::Dbm((value as i8 - 45) * 2)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerApTxPower {
    Dbm(i8),
    Reserved,
}

impl TriggerApTxPower {
    const fn from_common_encoding(value: u8) -> Self {
        Self::Dbm(value as i8 - 20)
    }

    const fn from_trs_encoding(value: u8) -> Self {
        if value == 0x1f {
            Self::Reserved
        } else {
            Self::Dbm((value as i8 - 10) * 2)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerCommonInfo {
    pub trigger_type: TriggerType,
    pub uplink_length: u16,
    pub more_trigger_frames: bool,
    pub carrier_sense_required: bool,
    pub uplink_bandwidth_encoding: u8,
    pub gi_ltf: TriggerGiLtf,
    pub mu_mimo_ltf_mode: bool,
    pub he_ltf_symbols_and_midamble_periodicity: u8,
    pub uplink_stbc: bool,
    pub ldpc_extra_symbol_segment: bool,
    pub ap_tx_power_encoding: u8,
    pub ap_tx_power: TriggerApTxPower,
    pub pre_fec_padding_factor_encoding: u8,
    pub pre_fec_padding_factor: u8,
    pub packet_extension_disambiguity: bool,
    pub uplink_spatial_reuse: u16,
    pub doppler: bool,
    pub uplink_he_sig_a2_reserved: u16,
    pub trailing_reserved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerFrame<'a> {
    pub frame_control: u16,
    pub duration: u16,
    pub receiver_address: [u8; 6],
    pub transmitter_address: [u8; 6],
    pub common: TriggerCommonInfo,
    /// User Info fields and any trailing padding.
    pub user_info_and_padding: &'a [u8],
}

impl<'a> TriggerFrame<'a> {
    /// Iterate over bounded User Info fields without allocation.
    ///
    /// SOURCE[BLOB_LIBNET80211_TEST_RX_PARSE_TRIG]: complete
    /// `_oracles/libnet80211.a[test_rx_trig.o]::esp_test_rx_parse_trig`
    /// (size `0x1d6`) advances Basic users by six bytes, MU-BAR users by
    /// nine, BFRP/MU-RTS/BSRP/BQRP users by five and treats NFRP as one
    /// terminal five-byte user. Its instruction-exact padding test requires
    /// both AID12 `0xfff` and RU allocation `0x7f`.
    ///
    /// The vendor test traps for Groupcast MU-BAR and has no bounded layout
    /// for reserved Trigger types. This iterator reports those layouts as an
    /// error instead of guessing or reproducing the trap.
    pub const fn users(&self) -> TriggerUserIterator<'a> {
        TriggerUserIterator::new(self.common.trigger_type, self.user_info_and_padding)
    }
}

/// One borrowed Trigger User Info field and its type-dependent suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserField<'a> {
    /// The common five-byte User Info field.
    pub user_info: &'a [u8],
    /// Basic contributes one byte and MU-BAR contributes four; other
    /// instruction-proven layouts expose an empty slice.
    pub dependent_info: &'a [u8],
}

impl TriggerUserField<'_> {
    pub const fn aid12(&self) -> u16 {
        u16::from_le_bytes([self.user_info[0], self.user_info[1] & 0x0f])
    }

    pub const fn ru_allocation(&self) -> u8 {
        (self.user_info[1] >> 5) | ((self.user_info[2] & 0x0f) << 3)
    }

    pub const fn classified_ru_allocation(&self) -> Option<TriggerRuAllocation> {
        TriggerRuAllocation::from_encoding(self.ru_allocation())
    }
}

/// Allocation-free, fail-closed iterator over Trigger User Info fields.
#[derive(Clone, Debug)]
pub struct TriggerUserIterator<'a> {
    trigger_type: TriggerType,
    remaining: &'a [u8],
    padding: &'a [u8],
    finished: bool,
}

impl<'a> TriggerUserIterator<'a> {
    const fn new(trigger_type: TriggerType, bytes: &'a [u8]) -> Self {
        Self {
            trigger_type,
            remaining: bytes,
            padding: &[],
            finished: false,
        }
    }

    /// Return the padding sentinel and bytes after it once iteration stops.
    ///
    /// Before the iterator reaches the sentinel this is an empty slice.
    pub const fn padding(&self) -> &'a [u8] {
        self.padding
    }

    fn fail(
        &mut self,
        error: TriggerParseError,
    ) -> Option<Result<TriggerUserField<'a>, TriggerParseError>> {
        self.finished = true;
        Some(Err(error))
    }
}

impl<'a> Iterator for TriggerUserIterator<'a> {
    type Item = Result<TriggerUserField<'a>, TriggerParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished || self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < TRIGGER_USER_INFO_LEN {
            return self.fail(TriggerParseError::Truncated {
                required: TRIGGER_USER_INFO_LEN,
            });
        }

        let aid12 = u16::from(self.remaining[0]) | (u16::from(self.remaining[1] & 0x0f) << 8);
        let ru_allocation = (self.remaining[1] >> 5) | ((self.remaining[2] & 0x0f) << 3);
        if aid12 == 0x0fff && ru_allocation == 0x7f {
            self.padding = self.remaining;
            self.remaining = &[];
            self.finished = true;
            return None;
        }

        let (dependent_len, terminal) = match self.trigger_type {
            TriggerType::Basic => (TRIGGER_BASIC_DEPENDENT_LEN, false),
            TriggerType::BeamformingReportPoll
            | TriggerType::MultiUserRequestToSend
            | TriggerType::BufferStatusReportPoll
            | TriggerType::BandwidthQueryReportPoll => (0, false),
            TriggerType::MultiUserBlockAckRequest => (TRIGGER_MU_BAR_DEPENDENT_LEN, false),
            TriggerType::NgpaFeedbackReportPoll => (0, true),
            TriggerType::GroupcastMultiUserBlockAckRequest | TriggerType::Reserved(_) => {
                return self.fail(TriggerParseError::UnsupportedUserLayout {
                    trigger_type: self.trigger_type,
                });
            }
        };
        let required = TRIGGER_USER_INFO_LEN + dependent_len;
        if self.remaining.len() < required {
            return self.fail(TriggerParseError::Truncated { required });
        }

        let (field, rest) = self.remaining.split_at(required);
        let user = TriggerUserField {
            user_info: &field[..TRIGGER_USER_INFO_LEN],
            dependent_info: &field[TRIGGER_USER_INFO_LEN..],
        };
        if terminal {
            self.padding = rest;
            self.remaining = &[];
            self.finished = true;
        } else {
            self.remaining = rest;
        }
        Some(Ok(user))
    }
}

/// Admit a complete Trigger control MPDU whose FCS has already been removed.
///
/// The 16-byte MAC header boundary is the one consumed by the hardware
/// Trigger diagnostics before the complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_common_info` field. The frame
/// control check uses IEEE 802.11 type/subtype bits and allows legal flag bits
/// in the second byte.
pub fn parse_trigger_frame(frame: &[u8]) -> Result<TriggerFrame<'_>, TriggerParseError> {
    require(frame, TRIGGER_FRAME_MIN_LEN)?;
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & 0x00fc != 0x0024 {
        return Err(TriggerParseError::NotTriggerFrame);
    }
    let mut receiver_address = [0; 6];
    receiver_address.copy_from_slice(&frame[4..10]);
    let mut transmitter_address = [0; 6];
    transmitter_address.copy_from_slice(&frame[10..16]);
    Ok(TriggerFrame {
        frame_control,
        duration: u16::from_le_bytes([frame[2], frame[3]]),
        receiver_address,
        transmitter_address,
        common: parse_trigger_common_info(&frame[TRIGGER_MAC_HEADER_LEN..TRIGGER_FRAME_MIN_LEN])?,
        user_info_and_padding: &frame[TRIGGER_FRAME_MIN_LEN..],
    })
}

/// Parse the fixed eight-byte Trigger Common Info field.
///
/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_COMMON_INFO]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_common_info`. The blob reads
/// two little-endian words and extracts bits 0:62 exactly as represented here;
/// bit 63 is retained as `trailing_reserved` even though the debug printer
/// omits it.
pub fn parse_trigger_common_info(bytes: &[u8]) -> Result<TriggerCommonInfo, TriggerParseError> {
    require(bytes, TRIGGER_COMMON_INFO_LEN)?;
    let bits = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let ap_tx_power_encoding = field(bits, 28, 0x3f) as u8;
    let pre_fec_padding_factor_encoding = field(bits, 34, 0x03) as u8;
    Ok(TriggerCommonInfo {
        trigger_type: TriggerType::from_encoding(field(bits, 0, 0x0f) as u8),
        uplink_length: field(bits, 4, 0x0fff) as u16,
        more_trigger_frames: bit(bits, 16),
        carrier_sense_required: bit(bits, 17),
        uplink_bandwidth_encoding: field(bits, 18, 0x03) as u8,
        gi_ltf: TriggerGiLtf::from_encoding(field(bits, 20, 0x03) as u8),
        mu_mimo_ltf_mode: bit(bits, 22),
        he_ltf_symbols_and_midamble_periodicity: field(bits, 23, 0x07) as u8,
        uplink_stbc: bit(bits, 26),
        ldpc_extra_symbol_segment: bit(bits, 27),
        ap_tx_power_encoding,
        ap_tx_power: TriggerApTxPower::from_common_encoding(ap_tx_power_encoding),
        pre_fec_padding_factor_encoding,
        pre_fec_padding_factor: if pre_fec_padding_factor_encoding == 0 {
            4
        } else {
            pre_fec_padding_factor_encoding
        },
        packet_extension_disambiguity: bit(bits, 36),
        uplink_spatial_reuse: field(bits, 37, 0xffff) as u16,
        doppler: bit(bits, 53),
        uplink_he_sig_a2_reserved: field(bits, 54, 0x01ff) as u16,
        trailing_reserved: bit(bits, 63),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserRuInfo {
    pub aid12: u16,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub coding_type: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub number_of_ra_ru: u8,
    pub more_ra_ru: bool,
    pub target_rssi_encoding: u8,
    pub target_rssi: TriggerTargetRssi,
    pub reserved: bool,
}

/// Parse the random-access RU form of a five-byte Trigger User Info field.
///
/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_USER_RU]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_user_ru`.
pub fn parse_trigger_user_ru(bytes: &[u8]) -> Result<TriggerUserRuInfo, TriggerParseError> {
    require(bytes, TRIGGER_USER_INFO_LEN)?;
    let target_rssi_encoding = bytes[4] & 0x7f;
    Ok(TriggerUserRuInfo {
        aid12: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        ru_allocation_region: bytes[1] & 0x10 != 0,
        ru_allocation: (bytes[1] >> 5) | ((bytes[2] & 0x0f) << 3),
        coding_type: bytes[2] & 0x10 != 0,
        mcs: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        dcm: bytes[3] & 0x02 != 0,
        number_of_ra_ru: (bytes[3] >> 2) & 0x1f,
        more_ra_ru: bytes[3] & 0x80 != 0,
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(target_rssi_encoding),
        reserved: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserSpatialStreamInfo {
    pub aid12: u16,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub coding_type: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub starting_spatial_stream_encoding: u8,
    pub starting_spatial_stream: u8,
    pub spatial_stream_count_encoding: u8,
    pub spatial_stream_count: u8,
    pub target_rssi_encoding: u8,
    pub target_rssi: TriggerTargetRssi,
    pub reserved: bool,
}

/// Parse the scheduled spatial-stream form of Trigger User Info.
///
/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_USER_SS]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_user_ss`. The blob suppresses
/// this form for AID12 `0xfff`; this bounded parser preserves the bytes and
/// leaves that semantic decision to the caller.
pub fn parse_trigger_user_spatial_stream(
    bytes: &[u8],
) -> Result<TriggerUserSpatialStreamInfo, TriggerParseError> {
    require(bytes, TRIGGER_USER_INFO_LEN)?;
    let starting_spatial_stream_encoding = (bytes[3] >> 2) & 0x07;
    let spatial_stream_count_encoding = bytes[3] >> 5;
    let target_rssi_encoding = bytes[4] & 0x7f;
    Ok(TriggerUserSpatialStreamInfo {
        aid12: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        ru_allocation_region: bytes[1] & 0x10 != 0,
        ru_allocation: (bytes[1] >> 5) | ((bytes[2] & 0x0f) << 3),
        coding_type: bytes[2] & 0x10 != 0,
        mcs: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        dcm: bytes[3] & 0x02 != 0,
        starting_spatial_stream_encoding,
        starting_spatial_stream: starting_spatial_stream_encoding + 1,
        spatial_stream_count_encoding,
        spatial_stream_count: spatial_stream_count_encoding + 1,
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(target_rssi_encoding),
        reserved: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerBasicDependentInfo {
    pub mpdu_mu_spacing_factor: u8,
    pub tid_aggregation_limit: u8,
    pub reserved: bool,
    pub preferred_access_category: u8,
}

/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_BASIC_DEPENDENT]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_basic_dependent`.
pub fn parse_trigger_basic_dependent(
    bytes: &[u8],
) -> Result<TriggerBasicDependentInfo, TriggerParseError> {
    require(bytes, TRIGGER_BASIC_DEPENDENT_LEN)?;
    Ok(TriggerBasicDependentInfo {
        mpdu_mu_spacing_factor: bytes[0] & 0x03,
        tid_aggregation_limit: (bytes[0] >> 2) & 0x07,
        reserved: bytes[0] & 0x20 != 0,
        preferred_access_category: bytes[0] >> 6,
    })
}

/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_BFRP_DEPENDENT]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_bfrp_dependent`.
pub fn parse_trigger_bfrp_dependent(bytes: &[u8]) -> Result<u8, TriggerParseError> {
    require(bytes, TRIGGER_BFRP_DEPENDENT_LEN)?;
    Ok(bytes[0])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerMuBarDependentInfo {
    pub bar_control: u16,
    pub ack_policy: bool,
    pub bar_type: u8,
    pub tid: u8,
    pub bar_information: u16,
    pub starting_sequence_number: u16,
}

/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_MUBAR_DEPENDENT]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_mubar_dependent`.
pub fn parse_trigger_mu_bar_dependent(
    bytes: &[u8],
) -> Result<TriggerMuBarDependentInfo, TriggerParseError> {
    require(bytes, TRIGGER_MU_BAR_DEPENDENT_LEN)?;
    let bar_control = u16::from_le_bytes([bytes[0], bytes[1]]);
    let bar_information = u16::from_le_bytes([bytes[2], bytes[3]]);
    Ok(TriggerMuBarDependentInfo {
        bar_control,
        ack_policy: bar_control & 0x0001 != 0,
        bar_type: ((bar_control >> 1) & 0x0f) as u8,
        tid: (bar_control >> 12) as u8,
        bar_information,
        starting_sequence_number: bar_information >> 4,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerNfrpUserInfo {
    pub starting_aid: u16,
    pub reserved_9: u16,
    pub feedback_type: u8,
    pub reserved_7: u8,
    pub uplink_target_rssi: u8,
    pub multiplexing: bool,
}

/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_NFRP_USER]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trig_nfrp_user`.
pub fn parse_trigger_nfrp_user(bytes: &[u8]) -> Result<TriggerNfrpUserInfo, TriggerParseError> {
    require(bytes, TRIGGER_NFRP_USER_INFO_LEN)?;
    Ok(TriggerNfrpUserInfo {
        starting_aid: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        reserved_9: u16::from(bytes[1] >> 4) | (u16::from(bytes[2] & 0x1f) << 4),
        feedback_type: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        reserved_7: bytes[3] >> 1,
        uplink_target_rssi: bytes[4] & 0x7f,
        multiplexing: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerResponseSchedulingControl {
    pub control_id: u8,
    pub uplink_data_symbols: u8,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub ap_tx_power_encoding: u8,
    pub ap_tx_power: TriggerApTxPower,
    pub uplink_target_rssi_encoding: u8,
    pub uplink_target_rssi: TriggerTargetRssi,
    pub mcs: u8,
    pub trailing_reserved: bool,
}

/// Parse the 32-bit Trigger Response Scheduling HE-control information.
///
/// SOURCE[BLOB_LIBPP_DBG_DUMP_TRS_CONTROL]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_trs_control`.
pub fn parse_trigger_response_scheduling_control(
    bytes: &[u8],
) -> Result<TriggerResponseSchedulingControl, TriggerParseError> {
    let bits = parse_he_control_word(bytes)?;
    let ap_tx_power_encoding = field(u64::from(bits), 19, 0x1f) as u8;
    let uplink_target_rssi_encoding = field(u64::from(bits), 24, 0x1f) as u8;
    Ok(TriggerResponseSchedulingControl {
        control_id: field(u64::from(bits), 0, 0x3f) as u8,
        uplink_data_symbols: field(u64::from(bits), 6, 0x1f) as u8,
        ru_allocation_region: bit(u64::from(bits), 11),
        ru_allocation: field(u64::from(bits), 12, 0x7f) as u8,
        ap_tx_power_encoding,
        ap_tx_power: TriggerApTxPower::from_trs_encoding(ap_tx_power_encoding),
        uplink_target_rssi_encoding,
        uplink_target_rssi: TriggerTargetRssi::from_trs_encoding(uplink_target_rssi_encoding),
        mcs: field(u64::from(bits), 29, 0x03) as u8,
        trailing_reserved: bit(u64::from(bits), 31),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UplinkPowerHeadroomControl {
    pub control_id: u8,
    pub uplink_power_headroom: u8,
    pub minimum_transmit_power: bool,
    pub reserved: u8,
    pub unparsed_upper_bits: u32,
}

/// Parse the fields printed from a 32-bit UPH HE-control information word.
///
/// SOURCE[BLOB_LIBPP_DBG_DUMP_UPH_CONTROL]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_uph_control`. The blob only names
/// bits 6:13; bits 14:31 remain explicitly unparsed instead of being guessed.
pub fn parse_uplink_power_headroom_control(
    bytes: &[u8],
) -> Result<UplinkPowerHeadroomControl, TriggerParseError> {
    let bits = parse_he_control_word(bytes)?;
    Ok(UplinkPowerHeadroomControl {
        control_id: (bits & 0x3f) as u8,
        uplink_power_headroom: ((bits >> 6) & 0x1f) as u8,
        minimum_transmit_power: bits & (1 << 11) != 0,
        reserved: ((bits >> 12) & 0x03) as u8,
        unparsed_upper_bits: bits >> 14,
    })
}

fn parse_he_control_word(bytes: &[u8]) -> Result<u32, TriggerParseError> {
    require(bytes, HE_CONTROL_INFO_LEN)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

const fn require(bytes: &[u8], required: usize) -> Result<(), TriggerParseError> {
    if bytes.len() < required {
        Err(TriggerParseError::Truncated { required })
    } else {
        Ok(())
    }
}

const fn field(bits: u64, shift: u32, mask: u64) -> u64 {
    (bits >> shift) & mask
}

const fn bit(bits: u64, shift: u32) -> bool {
    field(bits, shift, 1) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_common_info_field_across_the_word_boundary() {
        let bits = 0_u64
            | 4
            | (0xabc << 4)
            | (1 << 16)
            | (1 << 17)
            | (2 << 18)
            | (3 << 20)
            | (1 << 22)
            | (5 << 23)
            | (1 << 26)
            | (1 << 27)
            | (42 << 28)
            | (2 << 34)
            | (1 << 36)
            | (0xbeef << 37)
            | (1 << 53)
            | (0x155 << 54)
            | (1 << 63);
        let common = parse_trigger_common_info(&bits.to_le_bytes()).unwrap();
        assert_eq!(common.trigger_type, TriggerType::BufferStatusReportPoll);
        assert_eq!(common.uplink_length, 0xabc);
        assert!(common.more_trigger_frames);
        assert!(common.carrier_sense_required);
        assert_eq!(common.uplink_bandwidth_encoding, 2);
        assert_eq!(common.gi_ltf, TriggerGiLtf::FourLtf3200Ns);
        assert!(common.mu_mimo_ltf_mode);
        assert_eq!(common.he_ltf_symbols_and_midamble_periodicity, 5);
        assert!(common.uplink_stbc);
        assert!(common.ldpc_extra_symbol_segment);
        assert_eq!(common.ap_tx_power_encoding, 42);
        assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(22));
        assert_eq!(common.pre_fec_padding_factor_encoding, 2);
        assert_eq!(common.pre_fec_padding_factor, 2);
        assert!(common.packet_extension_disambiguity);
        assert_eq!(common.uplink_spatial_reuse, 0xbeef);
        assert!(common.doppler);
        assert_eq!(common.uplink_he_sig_a2_reserved, 0x155);
        assert!(common.trailing_reserved);
    }

    #[test]
    fn admits_trigger_mac_header_and_borrows_user_tail() {
        let mut frame = [0_u8; 29];
        frame[0] = 0x24;
        frame[1] = 0x08;
        frame[2..4].copy_from_slice(&0x1234_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        frame[10..16].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        frame[16] = 1;
        frame[24..].copy_from_slice(&[13, 14, 15, 16, 17]);
        let trigger = parse_trigger_frame(&frame).unwrap();
        assert_eq!(trigger.frame_control, 0x0824);
        assert_eq!(trigger.duration, 0x1234);
        assert_eq!(trigger.receiver_address, [1, 2, 3, 4, 5, 6]);
        assert_eq!(trigger.transmitter_address, [7, 8, 9, 10, 11, 12]);
        assert_eq!(
            trigger.common.trigger_type,
            TriggerType::BeamformingReportPoll
        );
        assert_eq!(trigger.user_info_and_padding, [13, 14, 15, 16, 17]);
    }

    #[test]
    fn iterates_basic_users_and_stops_at_the_exact_blob_padding_marker() {
        let bytes = [
            0x34, 0x02, 0x00, 0x00, 0x01, 0xa5, // AID 0x234 + Basic suffix
            0x56, 0x04, 0x00, 0x00, 0x02, 0x5a, // AID 0x456 + Basic suffix
            0xff, 0xef, 0x0f, 0x00, 0x00, // AID 0xfff + RU allocation 0x7f
            0xaa, 0xbb,
        ];
        let mut users = TriggerUserIterator::new(TriggerType::Basic, &bytes);

        let first = users.next().unwrap().unwrap();
        assert_eq!(first.aid12(), 0x234);
        assert_eq!(first.user_info.len(), TRIGGER_USER_INFO_LEN);
        assert_eq!(first.dependent_info, [0xa5]);

        let second = users.next().unwrap().unwrap();
        assert_eq!(second.aid12(), 0x456);
        assert_eq!(second.dependent_info, [0x5a]);

        assert_eq!(users.next(), None);
        assert_eq!(users.padding(), &bytes[12..]);
    }

    #[test]
    fn applies_the_instruction_proven_user_strides() {
        let mu_bar = [
            0x34, 0x02, 0x00, 0x00, 0x01, 0x05, 0xa0, 0x30, 0x12, 0x56, 0x04, 0x00, 0x00, 0x02,
            0x01, 0xb0, 0x40, 0x23,
        ];
        let mut users = TriggerUserIterator::new(TriggerType::MultiUserBlockAckRequest, &mu_bar);
        let first = users.next().unwrap().unwrap();
        assert_eq!(first.aid12(), 0x234);
        assert_eq!(first.dependent_info, [0x05, 0xa0, 0x30, 0x12]);
        let second = users.next().unwrap().unwrap();
        assert_eq!(second.aid12(), 0x456);
        assert_eq!(second.dependent_info, [0x01, 0xb0, 0x40, 0x23]);
        assert_eq!(users.next(), None);

        let bfrp = [0x34, 0x02, 0x00, 0x00, 0x01];
        let only = TriggerUserIterator::new(TriggerType::BeamformingReportPoll, &bfrp)
            .next()
            .unwrap()
            .unwrap();
        assert!(only.dependent_info.is_empty());
    }

    #[test]
    fn classifies_only_the_ru_groups_with_valid_blob_indices() {
        for (raw, resource_unit, one_based_index) in [
            (0, HeResourceUnit::Ru26, 1),
            (8, HeResourceUnit::Ru26, 9),
            (37, HeResourceUnit::Ru52, 1),
            (40, HeResourceUnit::Ru52, 4),
            (53, HeResourceUnit::Ru106, 1),
            (54, HeResourceUnit::Ru106, 2),
            (61, HeResourceUnit::Ru242, 1),
            (62, HeResourceUnit::Ru242, 2),
        ] {
            assert_eq!(
                TriggerRuAllocation::from_encoding(raw),
                Some(TriggerRuAllocation::Narrow {
                    resource_unit,
                    one_based_index,
                })
            );
        }
        for raw in [9, 36, 41, 52, 55, 60, 63, 68] {
            assert_eq!(TriggerRuAllocation::from_encoding(raw), None);
        }
        assert_eq!(
            TriggerRuAllocation::from_encoding(69),
            Some(TriggerRuAllocation::WiderThan242)
        );
        assert_eq!(
            TriggerRuAllocation::from_encoding(127),
            Some(TriggerRuAllocation::WiderThan242)
        );
        assert_eq!(TriggerRuAllocation::from_encoding(128), None);

        let bytes = [0x34, 0xa2, 0x06, 0x00, 0x01];
        let field = TriggerUserField {
            user_info: &bytes,
            dependent_info: &[],
        };
        assert_eq!(field.ru_allocation(), 53);
        assert_eq!(
            field.classified_ru_allocation(),
            Some(TriggerRuAllocation::Narrow {
                resource_unit: HeResourceUnit::Ru106,
                one_based_index: 1,
            })
        );
    }

    #[test]
    fn nfrp_is_one_terminal_user_and_retains_following_padding() {
        let bytes = [0x34, 0x02, 0x00, 0x00, 0x01, 0xaa, 0xbb];
        let mut users = TriggerUserIterator::new(TriggerType::NgpaFeedbackReportPoll, &bytes);
        assert_eq!(users.next().unwrap().unwrap().aid12(), 0x234);
        assert_eq!(users.next(), None);
        assert_eq!(users.padding(), [0xaa, 0xbb]);
    }

    #[test]
    fn unsupported_and_truncated_user_layouts_fail_closed() {
        let user = [0_u8; TRIGGER_USER_INFO_LEN];
        let mut unsupported =
            TriggerUserIterator::new(TriggerType::GroupcastMultiUserBlockAckRequest, &user);
        assert_eq!(
            unsupported.next(),
            Some(Err(TriggerParseError::UnsupportedUserLayout {
                trigger_type: TriggerType::GroupcastMultiUserBlockAckRequest,
            }))
        );
        assert_eq!(unsupported.next(), None);

        let mut truncated =
            TriggerUserIterator::new(TriggerType::Basic, &[0; TRIGGER_USER_INFO_LEN]);
        assert_eq!(
            truncated.next(),
            Some(Err(TriggerParseError::Truncated { required: 6 }))
        );
        assert_eq!(truncated.next(), None);
    }

    #[test]
    fn rejects_other_control_subtypes() {
        let mut frame = [0_u8; TRIGGER_FRAME_MIN_LEN];
        frame[0] = 0xd4;
        assert_eq!(
            parse_trigger_frame(&frame),
            Err(TriggerParseError::NotTriggerFrame)
        );
    }

    #[test]
    fn zero_pre_fec_encoding_means_factor_four_in_the_blob() {
        let common = parse_trigger_common_info(&[0; 8]).unwrap();
        assert_eq!(common.pre_fec_padding_factor_encoding, 0);
        assert_eq!(common.pre_fec_padding_factor, 4);
        assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(-20));
    }

    #[test]
    fn parses_ru_and_spatial_stream_views_of_user_info() {
        let bytes = [0xa5, 0xbb, 0xd6, 0xeb, 0x52];
        let ru = parse_trigger_user_ru(&bytes).unwrap();
        assert_eq!(ru.aid12, 0xba5);
        assert!(ru.ru_allocation_region);
        assert_eq!(ru.ru_allocation, 53);
        assert!(ru.coding_type);
        assert_eq!(ru.mcs, 14);
        assert!(ru.dcm);
        assert_eq!(ru.number_of_ra_ru, 26);
        assert!(ru.more_ra_ru);
        assert_eq!(ru.target_rssi, TriggerTargetRssi::Dbm(-28));
        assert!(!ru.reserved);

        let ss = parse_trigger_user_spatial_stream(&bytes).unwrap();
        assert_eq!(ss.starting_spatial_stream_encoding, 2);
        assert_eq!(ss.starting_spatial_stream, 3);
        assert_eq!(ss.spatial_stream_count_encoding, 7);
        assert_eq!(ss.spatial_stream_count, 8);
    }

    #[test]
    fn target_rssi_reserved_encoding_stays_distinct_from_dbm() {
        let user = parse_trigger_user_ru(&[0, 0, 0, 0, 0x7f]).unwrap();
        assert_eq!(user.target_rssi, TriggerTargetRssi::Reserved);
        let user = parse_trigger_user_ru(&[0, 0, 0, 0, 0]).unwrap();
        assert_eq!(user.target_rssi, TriggerTargetRssi::Dbm(-110));
    }

    #[test]
    fn parses_all_dependent_user_forms() {
        let basic = parse_trigger_basic_dependent(&[0xd6]).unwrap();
        assert_eq!(basic.mpdu_mu_spacing_factor, 2);
        assert_eq!(basic.tid_aggregation_limit, 5);
        assert!(!basic.reserved);
        assert_eq!(basic.preferred_access_category, 3);

        assert_eq!(parse_trigger_bfrp_dependent(&[0xa5]).unwrap(), 0xa5);

        let mu_bar = parse_trigger_mu_bar_dependent(&[0x05, 0xa0, 0x30, 0x12]).unwrap();
        assert_eq!(mu_bar.bar_control, 0xa005);
        assert!(mu_bar.ack_policy);
        assert_eq!(mu_bar.bar_type, 2);
        assert_eq!(mu_bar.tid, 10);
        assert_eq!(mu_bar.bar_information, 0x1230);
        assert_eq!(mu_bar.starting_sequence_number, 0x123);

        let nfrp = parse_trigger_nfrp_user(&[0x34, 0xa2, 0xb5, 0x6b, 0xd5]).unwrap();
        assert_eq!(nfrp.starting_aid, 0x234);
        assert_eq!(nfrp.reserved_9, 0x15a);
        assert_eq!(nfrp.feedback_type, 13);
        assert_eq!(nfrp.reserved_7, 0x35);
        assert_eq!(nfrp.uplink_target_rssi, 0x55);
        assert!(nfrp.multiplexing);
    }

    #[test]
    fn parses_trs_and_uph_control_without_hiding_unowned_bits() {
        let trs_bits = 0x2a_u32
            | (17 << 6)
            | (1 << 11)
            | (61 << 12)
            | (15 << 19)
            | (20 << 24)
            | (2 << 29)
            | (1 << 31);
        let trs = parse_trigger_response_scheduling_control(&trs_bits.to_le_bytes()).unwrap();
        assert_eq!(trs.control_id, 0x2a);
        assert_eq!(trs.uplink_data_symbols, 17);
        assert!(trs.ru_allocation_region);
        assert_eq!(trs.ru_allocation, 61);
        assert_eq!(trs.ap_tx_power, TriggerApTxPower::Dbm(10));
        assert_eq!(trs.uplink_target_rssi, TriggerTargetRssi::Dbm(-50));
        assert_eq!(trs.mcs, 2);
        assert!(trs.trailing_reserved);

        let uph_bits = 0x15_u32 | (23 << 6) | (1 << 11) | (2 << 12) | (0x2aa << 14);
        let uph = parse_uplink_power_headroom_control(&uph_bits.to_le_bytes()).unwrap();
        assert_eq!(uph.control_id, 0x15);
        assert_eq!(uph.uplink_power_headroom, 23);
        assert!(uph.minimum_transmit_power);
        assert_eq!(uph.reserved, 2);
        assert_eq!(uph.unparsed_upper_bits, 0x2aa);
    }

    #[test]
    fn every_parser_rejects_a_truncated_field() {
        assert_eq!(
            parse_trigger_common_info(&[0; 7]),
            Err(TriggerParseError::Truncated { required: 8 })
        );
        assert_eq!(
            parse_trigger_user_ru(&[0; 4]),
            Err(TriggerParseError::Truncated { required: 5 })
        );
        assert_eq!(
            parse_trigger_response_scheduling_control(&[0; 3]),
            Err(TriggerParseError::Truncated { required: 4 })
        );
    }
}
