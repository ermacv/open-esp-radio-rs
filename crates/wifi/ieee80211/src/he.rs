//! Allocation-free parsing for the bounded 2.4-GHz HE20 capability prefix.
//!
//! This module does not enable HE transmission. It owns the stateless peer
//! representation recovered from the former migration runtime. Register
//! programming belongs to the chip MAC/PAC boundary.

pub const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
pub const HE_OPERATION_EXTENSION_ID: u8 = 36;
pub const HE_CAPABILITIES_IE_MIN_LEN: usize = 24;
pub const HE_OPERATION_IE_MIN_LEN: usize = 9;

/// HE resource-unit widths supported by the S31 HE20 profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeResourceUnit {
    Ru26,
    Ru52,
    Ru106,
    #[default]
    Ru242,
}

/// One decoded 21-bit non-MU-MIMO HE-SIG-B user field.
///
/// SOURCE\[BLOB_LIBPP_DBG_DUMP_MUSIGB_NON_MIMO]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_musigb_non_mimo`, size `0x6e`.
/// The blob loads one caller-owned word and names bits 10:0 STA-ID, 13:11
/// NSTS, 14 beamformed, 18:15 MCS, 19 DCM and 20 coding. STA-ID `0x7fe`
/// takes a terminal special branch and none of the remaining bits are read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMuSigBNonMimoUser {
    NonMuMimo,
    Scheduled {
        station_id: u16,
        nsts: u8,
        beamformed: bool,
        mcs: u8,
        dcm: bool,
        ldpc: bool,
    },
}

impl HeMuSigBNonMimoUser {
    pub const fn decode(word: u32) -> Self {
        let station_id = (word & 0x07ff) as u16;
        if station_id == 0x07fe {
            Self::NonMuMimo
        } else {
            Self::Scheduled {
                station_id,
                nsts: ((word >> 11) & 0x07) as u8,
                beamformed: word & (1 << 14) != 0,
                mcs: ((word >> 15) & 0x0f) as u8,
                dcm: word & (1 << 19) != 0,
                ldpc: word & (1 << 20) != 0,
            }
        }
    }
}

/// One decoded 21-bit MU-MIMO HE-SIG-B user field.
///
/// SOURCE\[BLOB_LIBPP_DBG_DUMP_MUSIGB_MIMO]: complete
/// `_oracles/libpp.a[hal_debug.o]::dbg_dump_musigb_mimo`, size `0x4a`.
/// The blob names bits 10:0 STA-ID, 14:11 spatial configuration, 18:15 MCS,
/// bit 19 reserved and bit 20 coding. The reserved bit is deliberately not
/// exposed as a writable semantic field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeMuSigBMimoUser {
    pub station_id: u16,
    pub spatial_configuration: u8,
    pub mcs: u8,
    pub ldpc: bool,
}

impl HeMuSigBMimoUser {
    pub const fn decode(word: u32) -> Self {
        Self {
            station_id: (word & 0x07ff) as u16,
            spatial_configuration: ((word >> 11) & 0x0f) as u8,
            mcs: ((word >> 15) & 0x0f) as u8,
            ldpc: word & (1 << 20) != 0,
        }
    }
}

// SOURCE[ROM_REV0_MU_MIMO_SPECIAL_CFG]: `_oracles/esp32s31_rev0_rom.elf`
// SHA-256 d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542,
// symbols `mu_mimo_special_cfg_user_num_2` through `_8` at
// 0x2f84fee8, 0x2f84fe80, 0x2f84fe28, 0x2f84fdf0, 0x2f84fdd0,
// 0x2f84fdc0 and 0x2f84fdb8. Each configuration occupies the exact
// eight-byte ROM stride; zero tail bytes are retained below.
const HE_MU_MIMO_NSTS_2: [[u8; 8]; 10] = [
    [1, 1, 0, 0, 0, 0, 0, 0],
    [2, 1, 0, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0, 0],
    [4, 1, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0],
    [3, 2, 0, 0, 0, 0, 0, 0],
    [4, 2, 0, 0, 0, 0, 0, 0],
    [3, 3, 0, 0, 0, 0, 0, 0],
    [4, 3, 0, 0, 0, 0, 0, 0],
    [4, 4, 0, 0, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_3: [[u8; 8]; 13] = [
    [1, 1, 1, 0, 0, 0, 0, 0],
    [2, 1, 1, 0, 0, 0, 0, 0],
    [3, 1, 1, 0, 0, 0, 0, 0],
    [4, 1, 1, 0, 0, 0, 0, 0],
    [2, 2, 1, 0, 0, 0, 0, 0],
    [3, 2, 1, 0, 0, 0, 0, 0],
    [4, 2, 1, 0, 0, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0, 0],
    [4, 3, 1, 0, 0, 0, 0, 0],
    [2, 2, 2, 0, 0, 0, 0, 0],
    [3, 2, 2, 0, 0, 0, 0, 0],
    [4, 2, 2, 0, 0, 0, 0, 0],
    [3, 3, 2, 0, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_4: [[u8; 8]; 11] = [
    [1, 1, 1, 1, 0, 0, 0, 0],
    [2, 1, 1, 1, 0, 0, 0, 0],
    [3, 1, 1, 1, 0, 0, 0, 0],
    [4, 1, 1, 1, 0, 0, 0, 0],
    [2, 2, 1, 1, 0, 0, 0, 0],
    [3, 2, 1, 1, 0, 0, 0, 0],
    [4, 2, 1, 1, 0, 0, 0, 0],
    [3, 3, 1, 1, 0, 0, 0, 0],
    [2, 2, 2, 1, 0, 0, 0, 0],
    [3, 2, 2, 1, 0, 0, 0, 0],
    [2, 2, 2, 2, 0, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_5: [[u8; 8]; 7] = [
    [1, 1, 1, 1, 1, 0, 0, 0],
    [2, 1, 1, 1, 1, 0, 0, 0],
    [3, 1, 1, 1, 1, 0, 0, 0],
    [4, 1, 1, 1, 1, 0, 0, 0],
    [2, 2, 1, 1, 1, 0, 0, 0],
    [3, 2, 1, 1, 1, 0, 0, 0],
    [2, 2, 2, 1, 1, 0, 0, 0],
];
const HE_MU_MIMO_NSTS_6: [[u8; 8]; 4] = [
    [1, 1, 1, 1, 1, 1, 0, 0],
    [2, 1, 1, 1, 1, 1, 0, 0],
    [3, 1, 1, 1, 1, 1, 0, 0],
    [2, 2, 1, 1, 1, 1, 0, 0],
];
const HE_MU_MIMO_NSTS_7: [[u8; 8]; 2] = [[1, 1, 1, 1, 1, 1, 1, 0], [2, 1, 1, 1, 1, 1, 1, 0]];
const HE_MU_MIMO_NSTS_8: [[u8; 8]; 1] = [[1, 1, 1, 1, 1, 1, 1, 1]];

/// A validated HE MU-MIMO spatial-configuration encoding.
///
/// SOURCE\[BLOB_LIBPP_MUMIMO_SPATIAL_CFG_GET_NSTS]: complete
/// `_oracles/libpp.a[test_hal_rx_mu.o]::{mumimo_spatial_cfg_get_nsts,
/// mumimo_spatial_cfg_get_nsts_tot}`, sizes `0x10e` and `0x44`. The first
/// function selects one of the seven ROM tables above with an eight-byte
/// stride; the second sums exactly `user_count` entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeMuMimoSpatialConfiguration {
    user_count: u8,
    encoding: u8,
    nsts: &'static [u8; 8],
}

impl HeMuMimoSpatialConfiguration {
    pub fn try_new(user_count: u8, encoding: u8) -> Option<Self> {
        let nsts = configuration_record(user_count, encoding)?;
        Some(Self {
            user_count,
            encoding,
            nsts,
        })
    }

    pub const fn user_count(self) -> u8 {
        self.user_count
    }

    pub const fn encoding(self) -> u8 {
        self.encoding
    }

    pub fn nsts_for_user(self, user_index: u8) -> Option<u8> {
        if user_index >= self.user_count {
            return None;
        }
        Some(self.nsts[usize::from(user_index)])
    }

    pub fn total_nsts(self) -> u8 {
        self.nsts[..usize::from(self.user_count)]
            .iter()
            .copied()
            .sum()
    }
}

fn configuration_record(user_count: u8, encoding: u8) -> Option<&'static [u8; 8]> {
    let encoding = usize::from(encoding);
    match user_count {
        2 => HE_MU_MIMO_NSTS_2.get(encoding),
        3 => HE_MU_MIMO_NSTS_3.get(encoding),
        4 => HE_MU_MIMO_NSTS_4.get(encoding),
        5 => HE_MU_MIMO_NSTS_5.get(encoding),
        6 => HE_MU_MIMO_NSTS_6.get(encoding),
        7 => HE_MU_MIMO_NSTS_7.get(encoding),
        8 => HE_MU_MIMO_NSTS_8.get(encoding),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMuSigBUser {
    NonMimo(HeMuSigBNonMimoUser),
    Mimo(HeMuSigBMimoUser),
}

impl HeMuSigBUser {
    pub const fn decode(word: u32, compressed_sig_b: bool) -> Self {
        if compressed_sig_b {
            Self::Mimo(HeMuSigBMimoUser::decode(word))
        } else {
            Self::NonMimo(HeMuSigBNonMimoUser::decode(word))
        }
    }
}

// SOURCE[ROM_REV0_SIGB_COMMON_RU_ALLOCATION]:
// `_oracles/esp32s31_rev0_rom.elf`, SHA-256
// d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542,
// symbols `sigb_common_ru_allocation` at 0x2f84ff38 (144 bytes) and
// `sigb_ru_allocation_user_num` at 0x2f84ffc8 (16 bytes). Each row is the
// exact nine-byte ROM stride; zero tail entries are retained.
const HE_MU_SIG_B_COMMON_RU_TONES: [[u8; 9]; 16] = [
    [26, 26, 26, 26, 26, 26, 26, 26, 26],
    [26, 26, 26, 26, 26, 26, 26, 52, 0],
    [26, 26, 26, 26, 26, 52, 26, 26, 0],
    [26, 26, 26, 26, 26, 52, 52, 0, 0],
    [26, 26, 52, 26, 26, 26, 26, 26, 0],
    [26, 26, 52, 26, 26, 26, 52, 0, 0],
    [26, 26, 52, 26, 52, 26, 26, 0, 0],
    [26, 26, 52, 26, 52, 52, 0, 0, 0],
    [52, 26, 26, 26, 26, 26, 26, 26, 26],
    [52, 26, 26, 26, 26, 26, 52, 0, 0],
    [52, 26, 26, 26, 52, 26, 26, 0, 0],
    [52, 26, 26, 26, 52, 52, 0, 0, 0],
    [52, 52, 26, 26, 26, 26, 26, 0, 0],
    [52, 52, 26, 26, 26, 52, 0, 0, 0],
    [52, 52, 26, 52, 26, 26, 0, 0, 0],
    [52, 52, 26, 52, 52, 0, 0, 0, 0],
];
const HE_MU_SIG_B_COMMON_RU_USER_COUNTS: [u8; 16] =
    [9, 8, 8, 7, 8, 7, 7, 6, 8, 7, 7, 6, 7, 6, 6, 5];

/// A failure to decode the complete blob's HE-SIG-B RU Allocation domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBRuAllocationError {
    ReservedEncoding,
    UnsupportedRuType,
}

/// One user's RU view selected by an HE-SIG-B RU Allocation encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBRuUser {
    pub zero_based_position: u8,
    pub resource_unit: HeResourceUnit,
    /// The exact numeric `multiplexed` output produced by the blob helper.
    ///
    /// It is deliberately not collapsed to `bool`: several allocation
    /// classes return values through seven.
    pub multiplexed: u8,
}

/// A validated HE20 HE-SIG-B RU Allocation encoding.
///
/// SOURCE\[BLOB_LIBPP_GET_USER_NUM]: complete `_oracles/libpp.a
/// [test_hal_rx_mu.o]::get_user_num`, size `0x2e2`, object SHA-256
/// `b9891fdcbbb104e6ee466426916efec6782f8062d7c725fc955c210f4f522d75`.
/// Complete caller `test_nonmimo_update_user_info` passes the RU Allocation
/// byte and zero-based user position, then logs the two output bytes as
/// `ru_size` and `multiplexed`. The helper indexes the two exact ROM objects
/// above only for encodings 0 through 15 and computes every other class with
/// bounded branches. RU types four and five are rejected here because the
/// complete adjacent `rutype2str` supports only types zero through three and
/// the S31 non-AP profile is HE20.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBRuAllocation {
    encoding: u8,
    user_count: u8,
}

impl He20MuSigBRuAllocation {
    pub const fn try_new(encoding: u8) -> Result<Self, He20MuSigBRuAllocationError> {
        let user_count = match encoding {
            0..=15 => HE_MU_SIG_B_COMMON_RU_USER_COUNTS[encoding as usize],
            16..=39 => (encoding & 0x07) + 3,
            40..=47 | 64..=79 => (encoding & 0x07) + 6,
            48..=63 | 80..=95 => (encoding & 0x07) + 5,
            96..=111 => (encoding & 0x07) + 4,
            112 => 4,
            128..=191 => ((encoding >> 2) & 0x03) + (encoding & 0x03) + 2,
            192..=199 => (encoding & 0x07) + 1,
            200..=216 => return Err(He20MuSigBRuAllocationError::UnsupportedRuType),
            _ => return Err(He20MuSigBRuAllocationError::ReservedEncoding),
        };
        Ok(Self {
            encoding,
            user_count,
        })
    }

    pub const fn encoding(self) -> u8 {
        self.encoding
    }

    pub const fn user_count(self) -> u8 {
        self.user_count
    }

    pub const fn user(self, zero_based_position: u8) -> Option<He20MuSigBRuUser> {
        if zero_based_position >= self.user_count {
            return None;
        }

        let resource_unit = match self.encoding {
            0..=15 => {
                match HE_MU_SIG_B_COMMON_RU_TONES[self.encoding as usize]
                    [zero_based_position as usize]
                {
                    26 => HeResourceUnit::Ru26,
                    52 => HeResourceUnit::Ru52,
                    _ => return None,
                }
            }
            16..=23 => {
                if zero_based_position > 1 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru52
                }
            }
            24..=39 => {
                if zero_based_position == 0 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru52
                }
            }
            40..=47 => {
                if zero_based_position == 5 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            48..=55 => match zero_based_position {
                2 => HeResourceUnit::Ru52,
                4 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            56..=63 => match zero_based_position {
                0 => HeResourceUnit::Ru52,
                4 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            64..=79 => {
                if zero_based_position == 0 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            80..=87 => {
                if zero_based_position == 0 || zero_based_position == 4 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            88..=95 => {
                if zero_based_position == 0 || zero_based_position == 2 {
                    HeResourceUnit::Ru106
                } else {
                    HeResourceUnit::Ru26
                }
            }
            96..=103 => match zero_based_position {
                0 | 1 => HeResourceUnit::Ru52,
                3 => HeResourceUnit::Ru106,
                _ => HeResourceUnit::Ru26,
            },
            104..=111 => match zero_based_position {
                0 => HeResourceUnit::Ru106,
                1 => HeResourceUnit::Ru26,
                _ => HeResourceUnit::Ru52,
            },
            112 => HeResourceUnit::Ru52,
            128..=191 => HeResourceUnit::Ru106,
            192..=199 => HeResourceUnit::Ru242,
            _ => return None,
        };

        let multiplexed = match self.encoding {
            0..=15 | 112 => 0,
            16..=111 | 192..=199 => self.encoding & 0x07,
            128..=191 => {
                let first_group_users = ((self.encoding >> 2) & 0x03) + 1;
                let users_in_group = if zero_based_position < first_group_users {
                    first_group_users
                } else {
                    (self.encoding & 0x03) + 1
                };
                if users_in_group >= 2 { 1 } else { 0 }
            }
            _ => return None,
        };

        Some(He20MuSigBRuUser {
            zero_based_position,
            resource_unit,
            multiplexed,
        })
    }
}

/// Number of common-information bits before the first user in an HE20
/// non-MU-MIMO complete HE-SIG-B stream.
pub const HE20_MU_SIG_B_COMMON_BITS: u16 = 18;
/// Number of bits in one HE-SIG-B user field.
pub const HE_MU_SIG_B_USER_BITS: u16 = 21;
/// Number of bits occupied by a complete pair of user fields and its
/// intervening CRC/tail.
pub const HE20_MU_SIG_B_USER_PAIR_BITS: u16 = 52;
/// Maximum number of users in the blob's complete non-MU-MIMO HE20 parser.
pub const HE20_MU_SIG_B_MAX_USERS: u8 = 9;
/// Maximum number of users retained by the blob's compressed/MU-MIMO parser.
pub const HE20_MU_SIG_B_MIMO_MAX_USERS: u8 = 4;

fn read_complete_sig_b_user(complete_bytes: &[u8], bit_offset: u16) -> u32 {
    let mut raw = 0_u32;
    for output_bit in 0..HE_MU_SIG_B_USER_BITS {
        let source_bit = bit_offset + output_bit;
        let source_byte = complete_bytes[usize::from(source_bit / 8)];
        raw |= u32::from((source_byte >> (source_bit % 8)) & 1) << output_bit;
    }
    raw
}

/// A failure to construct a bounded HE20 non-MU-MIMO complete-SIG-B view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBNonMimoStreamError {
    BitLengthBeforeFirstUser,
    CompleteBytesTooShort,
    IncompleteUserField,
    TooManyUsers,
}

/// Why the common RU Allocation and complete HE20 user stream disagree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBRuStreamError {
    Allocation(He20MuSigBRuAllocationError),
    UserCountMismatch {
        stream_users: u8,
        allocation_users: u8,
    },
}

/// One user recovered from the complete HE20 non-MU-MIMO HE-SIG-B stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBNonMimoEntry {
    pub index: u8,
    pub bit_offset: u16,
    pub raw: u32,
    pub user: HeMuSigBNonMimoUser,
}

/// Allocation-free iterator over an HE20 non-MU-MIMO complete HE-SIG-B stream.
///
/// SOURCE\[BLOB_LIBPP_TEST_RX_PARSE_NONMUMIMO_COMPLETE_SIGB]: complete
/// `_oracles/libpp.a[test_hal_rx_mu_sigb.o]::
/// test_rx_parse_nonmumimo_complete_sigb`, size `0x3e4`, from archive SHA-256
/// `f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`.
/// The RISC-V body copies the complete bytes from RX offset `0x38`, calls
/// `test_get_nonmumimo_common`, and extracts the tested HE20 user words at
/// absolute bit offsets `18,39,70,91,122,143,174,195,226`. These are exactly
/// `18 + pair * 52 + {0,21}`. Its user-count expression is
/// `(remaining / 52) * 2 + (remaining % 52 != 0)`, and the unrolled body stops
/// after user eight.
///
/// This type intentionally says HE20 and non-MU-MIMO. The same oracle proves
/// different common lengths for wider bandwidth selectors, while
/// `test_rx_parse_mumimo_complete_sigb` uses a configuration-dependent layout.
/// Neither is silently folded onto this iterator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBNonMimoUsers<'a> {
    complete_bytes: &'a [u8],
    bit_length: u16,
    user_count: u8,
    next_user: u8,
}

impl<'a> He20MuSigBNonMimoUsers<'a> {
    pub fn try_new(
        complete_bytes: &'a [u8],
        bit_length: u16,
    ) -> Result<Self, He20MuSigBNonMimoStreamError> {
        let remaining = bit_length
            .checked_sub(HE20_MU_SIG_B_COMMON_BITS)
            .ok_or(He20MuSigBNonMimoStreamError::BitLengthBeforeFirstUser)?;
        let required_bytes = usize::from(bit_length).div_ceil(8);
        if complete_bytes.len() < required_bytes {
            return Err(He20MuSigBNonMimoStreamError::CompleteBytesTooShort);
        }

        let complete_pairs = remaining / HE20_MU_SIG_B_USER_PAIR_BITS;
        let partial_pair_bits = remaining % HE20_MU_SIG_B_USER_PAIR_BITS;
        if partial_pair_bits != 0 && partial_pair_bits < HE_MU_SIG_B_USER_BITS {
            return Err(He20MuSigBNonMimoStreamError::IncompleteUserField);
        }
        let user_count = complete_pairs
            .checked_mul(2)
            .and_then(|count| count.checked_add(u16::from(partial_pair_bits != 0)))
            .ok_or(He20MuSigBNonMimoStreamError::TooManyUsers)?;
        if user_count > u16::from(HE20_MU_SIG_B_MAX_USERS) {
            return Err(He20MuSigBNonMimoStreamError::TooManyUsers);
        }

        Ok(Self {
            complete_bytes,
            bit_length,
            user_count: user_count as u8,
            next_user: 0,
        })
    }

    pub const fn bit_length(&self) -> u16 {
        self.bit_length
    }

    pub const fn user_count(&self) -> u8 {
        self.user_count
    }

    /// Decodes the first HE20 common-information RU Allocation byte and
    /// requires it to describe the same number of users as this stream.
    ///
    /// SOURCE\[BLOB_LIBPP_TEST_GET_NONMUMIMO_COMMON]: complete
    /// `_oracles/libpp.a[test_hal_rx_mu_sigb.o]::
    /// test_get_nonmumimo_common`, size `0xf6`, object SHA-256
    /// `a141879bd5f92710941861f83a968182ef71f83c0f258b7381576c5e8acf4b1e`.
    /// Bandwidth selector zero loads byte zero as the sole RU Allocation and
    /// returns an 18-bit common prefix.
    pub fn ru_allocation(self) -> Result<He20MuSigBRuAllocation, He20MuSigBRuStreamError> {
        let allocation = He20MuSigBRuAllocation::try_new(self.complete_bytes[0])
            .map_err(He20MuSigBRuStreamError::Allocation)?;
        if allocation.user_count() != self.user_count {
            return Err(He20MuSigBRuStreamError::UserCountMismatch {
                stream_users: self.user_count,
                allocation_users: allocation.user_count(),
            });
        }
        Ok(allocation)
    }

    fn read_user_word(&self, index: u8) -> (u16, u32) {
        let pair = u16::from(index / 2);
        let within_pair = if index & 1 == 0 {
            0
        } else {
            HE_MU_SIG_B_USER_BITS
        };
        let bit_offset =
            HE20_MU_SIG_B_COMMON_BITS + pair * HE20_MU_SIG_B_USER_PAIR_BITS + within_pair;
        let raw = read_complete_sig_b_user(self.complete_bytes, bit_offset);
        (bit_offset, raw)
    }
}

impl Iterator for He20MuSigBNonMimoUsers<'_> {
    type Item = He20MuSigBNonMimoEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_user >= self.user_count {
            return None;
        }
        let index = self.next_user;
        let (bit_offset, raw) = self.read_user_word(index);
        self.next_user += 1;
        Some(He20MuSigBNonMimoEntry {
            index,
            bit_offset,
            raw,
            user: HeMuSigBNonMimoUser::decode(raw),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.user_count - self.next_user);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for He20MuSigBNonMimoUsers<'_> {}
impl core::iter::FusedIterator for He20MuSigBNonMimoUsers<'_> {}

/// A failure to construct the bounded HE20 compressed/MU-MIMO SIG-B view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBMimoStreamError {
    UserCountOutOfRange,
    CompleteBytesTooShort,
    IncompleteUserField,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20MuSigBMimoSpatialError {
    UnsupportedUserCountOrEncoding,
    InconsistentEncoding,
}

/// One MU-MIMO user recovered from a complete compressed HE20 SIG-B stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBMimoEntry {
    pub index: u8,
    pub bit_offset: u16,
    pub raw: u32,
    pub user: HeMuSigBMimoUser,
}

/// Allocation-free iterator over the blob's HE20 compressed/MU-MIMO layout.
///
/// SOURCE\[BLOB_LIBPP_TEST_RX_PARSE_MUMIMO_COMPLETE_SIGB]: complete
/// `_oracles/libpp.a[test_hal_rx_mu_sigb.o]::
/// test_rx_parse_mumimo_complete_sigb`, size `0x20c`, from archive SHA-256
/// `f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`.
/// The body derives a one-based user count from HE-SIG-A1 bits 21:18, copies
/// 16 complete-SIG-B bytes and extracts at most four 21-bit words at exact
/// bit offsets `0,21,52,105`. The third and fourth paths require total bit
/// lengths above 72 and 135 respectively. The non-linear fourth offset is
/// retained deliberately instead of imposing the non-MIMO pair geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20MuSigBMimoUsers<'a> {
    complete_bytes: &'a [u8],
    bit_length: u16,
    user_count: u8,
    next_user: u8,
}

impl<'a> He20MuSigBMimoUsers<'a> {
    const USER_BIT_OFFSETS: [u16; HE20_MU_SIG_B_MIMO_MAX_USERS as usize] = [0, 21, 52, 105];
    // The first two bounds are the safe completion of the fields the blob
    // reads. The last two are the blob's explicit `> 72` and `> 135` guards.
    const REQUIRED_BIT_LENGTHS: [u16; HE20_MU_SIG_B_MIMO_MAX_USERS as usize] = [21, 42, 73, 136];

    pub fn try_new(
        complete_bytes: &'a [u8],
        bit_length: u16,
        user_count: u8,
    ) -> Result<Self, He20MuSigBMimoStreamError> {
        if user_count == 0 || user_count > HE20_MU_SIG_B_MIMO_MAX_USERS {
            return Err(He20MuSigBMimoStreamError::UserCountOutOfRange);
        }
        let required_bytes = usize::from(bit_length).div_ceil(8);
        if complete_bytes.len() < required_bytes {
            return Err(He20MuSigBMimoStreamError::CompleteBytesTooShort);
        }
        if bit_length < Self::REQUIRED_BIT_LENGTHS[usize::from(user_count - 1)] {
            return Err(He20MuSigBMimoStreamError::IncompleteUserField);
        }
        Ok(Self {
            complete_bytes,
            bit_length,
            user_count,
            next_user: 0,
        })
    }

    pub const fn bit_length(&self) -> u16 {
        self.bit_length
    }

    pub const fn user_count(&self) -> u8 {
        self.user_count
    }

    /// Validates the shared spatial configuration exactly as the blob test
    /// does after extracting the per-user words.
    pub fn spatial_configuration(
        self,
    ) -> Result<HeMuMimoSpatialConfiguration, He20MuSigBMimoSpatialError> {
        let mut users = self;
        let encoding = users
            .next()
            .map(|entry| entry.user.spatial_configuration)
            .ok_or(He20MuSigBMimoSpatialError::UnsupportedUserCountOrEncoding)?;
        if users.any(|entry| entry.user.spatial_configuration != encoding) {
            return Err(He20MuSigBMimoSpatialError::InconsistentEncoding);
        }
        HeMuMimoSpatialConfiguration::try_new(self.user_count, encoding)
            .ok_or(He20MuSigBMimoSpatialError::UnsupportedUserCountOrEncoding)
    }
}

impl Iterator for He20MuSigBMimoUsers<'_> {
    type Item = He20MuSigBMimoEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_user >= self.user_count {
            return None;
        }
        let index = self.next_user;
        let bit_offset = Self::USER_BIT_OFFSETS[usize::from(index)];
        let raw = read_complete_sig_b_user(self.complete_bytes, bit_offset);
        self.next_user += 1;
        Some(He20MuSigBMimoEntry {
            index,
            bit_offset,
            raw,
            user: HeMuSigBMimoUser::decode(raw),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.user_count - self.next_user);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for He20MuSigBMimoUsers<'_> {}
impl core::iter::FusedIterator for He20MuSigBMimoUsers<'_> {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeMcsNssSupport {
    Mcs0To7,
    Mcs0To9,
    Mcs0To11,
    NotSupported,
}

impl HeMcsNssSupport {
    const fn from_map(map: u16, spatial_stream: u8) -> Self {
        let shift = spatial_stream.saturating_sub(1) as u32 * 2;
        match (map >> shift) & 0x03 {
            0 => Self::Mcs0To7,
            1 => Self::Mcs0To9,
            2 => Self::Mcs0To11,
            _ => Self::NotSupported,
        }
    }

    pub const fn supports_mcs9(self) -> bool {
        matches!(self, Self::Mcs0To9 | Self::Mcs0To11)
    }
}

/// Maximum modulation constellation advertised for HE DCM.
///
/// The values are the two-bit HE PHY capability encoding, not an S31-private
/// enum. Keeping `NotSupported` distinct is important: a HE peer is not
/// necessarily able to receive DCM merely because it supports ordinary HE SU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum HeDcmConstellation {
    #[default]
    NotSupported = 0,
    Bpsk = 1,
    Qpsk = 2,
    Qam16 = 3,
}

impl HeDcmConstellation {
    const fn from_encoding(encoding: u8) -> Self {
        match encoding & 0x03 {
            0 => Self::NotSupported,
            1 => Self::Bpsk,
            2 => Self::Qpsk,
            _ => Self::Qam16,
        }
    }

    pub const fn supports_bpsk(self) -> bool {
        !matches!(self, Self::NotSupported)
    }

    pub const fn supports_qpsk(self) -> bool {
        matches!(self, Self::Qpsk | Self::Qam16)
    }

    pub const fn supports_16qam(self) -> bool {
        matches!(self, Self::Qam16)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Capabilities {
    pub receive_mcs_map: u16,
    pub transmit_mcs_map: u16,
    pub receive_nss1: HeMcsNssSupport,
    pub transmit_nss1: HeMcsNssSupport,
    /// The peer can receive the optional HE SU 1x HE-LTF / 0.8-us GI form.
    ///
    /// SOURCE\[LINUX_IEEE80211_HE_PHY_CAP1_GI_2026_07_29]: Linux v6.12
    /// `include/linux/ieee80211.h` names HE PHY capability byte 1 bit `0x40`
    /// `HE_LTF_AND_GI_FOR_HE_PPDUS_0_8US`. The S31 oracle's ordinary
    /// `ppSelectTxFormat` never emits GI/LTF selector zero, while HIL against a
    /// peer with this bit clear rejected selector zero for MCS0 through MCS9
    /// and accepted selectors one through three.
    pub one_ltf_800ns_gi: bool,
    /// The peer can decode LDPC coding in an HE payload.
    ///
    /// SOURCE\[BLOB_LIBNET80211_HE_CAP_LDPC]: complete
    /// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_parse_hecap`
    /// (size `0x2d8`) reads HE PHY capability byte one at element offset ten,
    /// shifts it by five and masks one before publishing the decoded field in
    /// its complete capability diagnostic. The same function subsequently
    /// copies the complete 24-byte bounded prefix into peer state. This is
    /// independent of the peer's DCM receive constellation.
    pub ldpc_coding_in_payload: bool,
    /// The peer can transmit HE STBC below 80 MHz.
    ///
    /// HE PHY capability byte 2 bit 2. For the S31 non-AP role this is the
    /// peer capability required before attempting a controlled downlink RX
    /// STBC qualification.
    pub stbc_transmit_under_80_mhz: bool,
    /// The peer can receive HE STBC below 80 MHz.
    ///
    /// SOURCE\[BLOB_LIBNET80211_HE_CAP_STBC]: complete
    /// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` copies
    /// `g_phy_cap_rx_stbc` into HE PHY capability byte 2 bit 3. Complete
    /// `esp_wifi_enable_rx_stbc` owns that one-byte capability flag and the
    /// corresponding interface-state bits.
    pub stbc_receive_under_80_mhz: bool,
    /// Maximum DCM constellation the peer can transmit.
    ///
    /// HE PHY capability byte 3 bits 1:0. This is useful for the open RX
    /// policy, but is deliberately separate from [`Self::dcm_receive`].
    pub dcm_transmit: HeDcmConstellation,
    /// Maximum DCM constellation the peer can receive.
    ///
    /// SOURCE\[LINUX_IEEE80211_HE_PHY_CAP3_DCM_2026_07_29]: Linux
    /// `include/linux/ieee80211-he.h` names HE PHY capability byte 3 bits
    /// 4:3 `DCM_MAX_CONST_RX`. `_oracles/libpp.a[trc.o]::rcGetDCMMaxRate`
    /// independently maps the same four capability levels to disabled,
    /// BPSK/MCS0, QPSK/MCS1 and 16-QAM/MCS3 for the vendor BCC path.
    pub dcm_receive: HeDcmConstellation,
    /// The peer can send SU beamforming feedback in a Trigger frame response.
    pub triggered_su_beamforming_feedback: bool,
    /// The peer can send partial-bandwidth MU feedback in a Trigger response.
    pub triggered_mu_beamforming_partial_bandwidth_feedback: bool,
    /// The peer can send CQI feedback in a Trigger frame response.
    ///
    /// HE PHY capability byte 6 bit 4. This is distinct from
    /// [`Self::non_triggered_cqi_feedback`].
    pub triggered_cqi_feedback: bool,
    /// The peer can send CQI feedback without a Trigger frame.
    ///
    /// HE PHY capability byte 9 bit 1.
    pub non_triggered_cqi_feedback: bool,
}

impl He20Capabilities {
    pub const fn supports_bidirectional_mcs9(self) -> bool {
        self.receive_nss1.supports_mcs9() && self.transmit_nss1.supports_mcs9()
    }

    pub const fn supports_one_ltf_800ns_gi(self) -> bool {
        self.one_ltf_800ns_gi
    }

    pub const fn supports_ldpc_coding_in_payload(self) -> bool {
        self.ldpc_coding_in_payload
    }

    pub const fn dcm_receive_constellation(self) -> HeDcmConstellation {
        self.dcm_receive
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20Operation {
    pub bss_color: u8,
    pub bss_color_enabled: bool,
    pub partial_bss_color: bool,
    pub basic_mcs_nss_map: u16,
}

impl He20Operation {
    /// Return the BSS color published in an HE-SIG-A1 transmit vector.
    ///
    /// SOURCE: complete `_oracles/libnet80211.a[ieee80211_he.o]::
    /// ieee80211_parse_heopr` passes HE Operation byte-six bit seven inverted
    /// as the enable argument, bit six as the partial-color argument and bits
    /// 5:0 as the color to `hal_he_set_bss_color`. Complete
    /// `_oracles/libpp.a[hal_mac_ctl.o]::hal_he_get_bss_color` returns zero
    /// whenever that enable bit is clear. Keeping the advertised numeric
    /// color separate from this effective transmit value prevents a disabled
    /// BSS color such as `0xae` from becoming active color 46.
    pub const fn effective_bss_color(self) -> u8 {
        if self.bss_color_enabled {
            self.bss_color
        } else {
            0
        }
    }
}

/// Bounded HE20 peer state consumed by a chip-specific register backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct He20PeerState {
    pub capability_prefix: [u8; HE_CAPABILITIES_IE_MIN_LEN],
    pub max_rate_code: u8,
    pub packet_padding_eight_us: u8,
    pub operation_parameters: u32,
    pub bss_color_information: u8,
    pub basic_mcs_nss_map: u16,
    pub rts_threshold: Option<u16>,
    /// Raw HE Operation `ER-SU-Disable` bit.
    ///
    /// SOURCE: complete
    /// `_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr`
    /// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
    /// peer-state bit 10 and passes it unchanged to `hal_he_set_ersu`.
    pub extended_range_single_user_disabled: bool,
}

impl He20PeerState {
    pub const fn extended_range_single_user_permitted(self) -> bool {
        !self.extended_range_single_user_disabled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeElementError {
    WrongElement,
    LengthMismatch,
    TooShort,
    WrongExtension,
}

fn validate_extension(
    element: &[u8],
    extension: u8,
    minimum_len: usize,
) -> Result<&[u8], HeElementError> {
    if element.first().copied() != Some(255) {
        return Err(HeElementError::WrongElement);
    }
    let Some(declared_len) = element.get(1).copied() else {
        return Err(HeElementError::TooShort);
    };
    if usize::from(declared_len).checked_add(2) != Some(element.len()) {
        return Err(HeElementError::LengthMismatch);
    }
    if element.len() < minimum_len {
        return Err(HeElementError::TooShort);
    }
    if element.get(2).copied() != Some(extension) {
        return Err(HeElementError::WrongExtension);
    }
    Ok(element)
}

pub fn parse_he20_capabilities(element: &[u8]) -> Result<He20Capabilities, HeElementError> {
    let element = validate_extension(
        element,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let receive_mcs_map = u16::from_le_bytes([element[20], element[21]]);
    let transmit_mcs_map = u16::from_le_bytes([element[22], element[23]]);
    Ok(He20Capabilities {
        receive_mcs_map,
        transmit_mcs_map,
        receive_nss1: HeMcsNssSupport::from_map(receive_mcs_map, 1),
        transmit_nss1: HeMcsNssSupport::from_map(transmit_mcs_map, 1),
        one_ltf_800ns_gi: element[10] & 0x40 != 0,
        ldpc_coding_in_payload: element[10] & 0x20 != 0,
        stbc_transmit_under_80_mhz: element[11] & 0x04 != 0,
        stbc_receive_under_80_mhz: element[11] & 0x08 != 0,
        dcm_transmit: HeDcmConstellation::from_encoding(element[12]),
        dcm_receive: HeDcmConstellation::from_encoding(element[12] >> 3),
        triggered_su_beamforming_feedback: element[15] & 0x04 != 0,
        triggered_mu_beamforming_partial_bandwidth_feedback: element[15] & 0x08 != 0,
        triggered_cqi_feedback: element[15] & 0x10 != 0,
        non_triggered_cqi_feedback: element[18] & 0x02 != 0,
    })
}

pub fn parse_he20_operation(element: &[u8]) -> Result<He20Operation, HeElementError> {
    let element = validate_extension(element, HE_OPERATION_EXTENSION_ID, HE_OPERATION_IE_MIN_LEN)?;
    let bss_color_information = element[6];
    Ok(He20Operation {
        bss_color: bss_color_information & 0x3f,
        bss_color_enabled: bss_color_information & 0x80 == 0,
        partial_bss_color: bss_color_information & 0x40 != 0,
        basic_mcs_nss_map: u16::from_le_bytes([element[7], element[8]]),
    })
}

/// Recover the peer fields installed by the pinned HE capability and
/// operation parsers.
///
/// Primary evidence: complete pinned
/// `_oracles/libnet80211.a[ieee80211_he.o]::{ieee80211_parse_hecap,
/// ieee80211_parse_heopr}` bodies and their format strings. This function is
/// deliberately pure; corresponding MMIO transforms are tracked separately
/// in the S31 MAC.
pub fn parse_he20_peer_state(
    capability: &[u8],
    operation: &[u8],
) -> Result<He20PeerState, HeElementError> {
    let capability = validate_extension(
        capability,
        HE_CAPABILITIES_EXTENSION_ID,
        HE_CAPABILITIES_IE_MIN_LEN,
    )?;
    let operation = validate_extension(
        operation,
        HE_OPERATION_EXTENSION_ID,
        HE_OPERATION_IE_MIN_LEN,
    )?;

    let mut capability_prefix = [0_u8; HE_CAPABILITIES_IE_MIN_LEN];
    capability_prefix.copy_from_slice(&capability[..HE_CAPABILITIES_IE_MIN_LEN]);
    let max_rate_code = if capability[20] & 0x03 == 0 { 172 } else { 229 };
    let packet_padding_eight_us = if capability[15] & 0x80 == 0 {
        capability[18] >> 6
    } else {
        let ppe0 = *capability.get(24).ok_or(HeElementError::TooShort)?;
        let ppe1 = *capability.get(25).ok_or(HeElementError::TooShort)?;
        let ppet8 = ((ppe1 & 0x03) << 1) | (ppe0 >> 7);
        if ppe0 & 0x08 != 0 && ppe1 & 0x1c == 0x1c && ppet8 == 0 {
            2
        } else {
            0
        }
    };

    let operation_parameters =
        u32::from(operation[3]) | (u32::from(operation[4]) << 8) | (u32::from(operation[5]) << 16);
    let encoded_rts_threshold =
        (u16::from(operation[4] & 0x3f) << 4) | u16::from(operation[3] >> 4);
    let rts_threshold =
        (!matches!(encoded_rts_threshold, 0 | 0x03ff)).then_some(encoded_rts_threshold);

    Ok(He20PeerState {
        capability_prefix,
        max_rate_code,
        packet_padding_eight_us,
        operation_parameters,
        bss_color_information: operation[6],
        basic_mcs_nss_map: u16::from_le_bytes([operation[7], operation[8]]),
        rts_threshold,
        extended_range_single_user_disabled: operation[5] & 0x01 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bits(bytes: &mut [u8], bit_offset: u16, bit_width: u16, value: u32) {
        for input_bit in 0..bit_width {
            let destination_bit = bit_offset + input_bit;
            let destination = &mut bytes[usize::from(destination_bit / 8)];
            let mask = 1 << (destination_bit % 8);
            if value & (1 << input_bit) != 0 {
                *destination |= mask;
            } else {
                *destination &= !mask;
            }
        }
    }

    #[test]
    fn parses_single_stream_mcs9_capability_without_optional_tails() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[20..22].copy_from_slice(&0xfffd_u16.to_le_bytes());
        element[22..24].copy_from_slice(&0xfffd_u16.to_le_bytes());
        let capability = parse_he20_capabilities(&element).unwrap();
        assert_eq!(capability.receive_nss1, HeMcsNssSupport::Mcs0To9);
        assert_eq!(capability.transmit_nss1, HeMcsNssSupport::Mcs0To9);
        assert!(capability.supports_bidirectional_mcs9());
        assert!(!capability.supports_one_ltf_800ns_gi());
        assert_eq!(
            capability.dcm_receive_constellation(),
            HeDcmConstellation::NotSupported
        );
    }

    #[test]
    fn decodes_complete_non_mimo_sig_b_user_and_terminal_sentinel() {
        let word = 0x10_0000 | 0x08_0000 | (9 << 15) | (1 << 14) | (5 << 11) | 0x234;
        assert_eq!(
            HeMuSigBNonMimoUser::decode(word),
            HeMuSigBNonMimoUser::Scheduled {
                station_id: 0x234,
                nsts: 5,
                beamformed: true,
                mcs: 9,
                dcm: true,
                ldpc: true,
            }
        );
        assert_eq!(
            HeMuSigBNonMimoUser::decode(0x001f_f7fe),
            HeMuSigBNonMimoUser::NonMuMimo
        );
    }

    #[test]
    fn decodes_complete_mimo_sig_b_user_without_exposing_reserved_bit() {
        let word = 0x10_0000 | 0x08_0000 | (7 << 15) | (12 << 11) | 0x345;
        assert_eq!(
            HeMuSigBMimoUser::decode(word),
            HeMuSigBMimoUser {
                station_id: 0x345,
                spatial_configuration: 12,
                mcs: 7,
                ldpc: true,
            }
        );
    }

    #[test]
    fn decodes_exact_rom_backed_he_sig_b_ru_allocations() {
        for (encoding, expected_users) in [9, 8, 8, 7, 8, 7, 7, 6, 8, 7, 7, 6, 7, 6, 6, 5]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                He20MuSigBRuAllocation::try_new(encoding as u8)
                    .unwrap()
                    .user_count(),
                expected_users
            );
        }

        let all_ru26 = He20MuSigBRuAllocation::try_new(0).unwrap();
        assert_eq!(all_ru26.user_count(), 9);
        for position in 0..all_ru26.user_count() {
            assert_eq!(
                all_ru26.user(position),
                Some(He20MuSigBRuUser {
                    zero_based_position: position,
                    resource_unit: HeResourceUnit::Ru26,
                    multiplexed: 0,
                })
            );
        }

        let mixed = He20MuSigBRuAllocation::try_new(15).unwrap();
        let expected = [
            HeResourceUnit::Ru52,
            HeResourceUnit::Ru52,
            HeResourceUnit::Ru26,
            HeResourceUnit::Ru52,
            HeResourceUnit::Ru52,
        ];
        for (position, resource_unit) in expected.into_iter().enumerate() {
            assert_eq!(
                mixed.user(position as u8).unwrap().resource_unit,
                resource_unit
            );
        }
        assert_eq!(mixed.user(mixed.user_count()), None);
    }

    #[test]
    fn decodes_every_computed_he20_sig_b_ru_class_boundary() {
        let cases = [
            (23, 0, 10, HeResourceUnit::Ru52, 7),
            (23, 2, 10, HeResourceUnit::Ru106, 7),
            (24, 0, 3, HeResourceUnit::Ru106, 0),
            (24, 1, 3, HeResourceUnit::Ru52, 0),
            (47, 5, 13, HeResourceUnit::Ru106, 7),
            (48, 2, 5, HeResourceUnit::Ru52, 0),
            (48, 4, 5, HeResourceUnit::Ru106, 0),
            (56, 0, 5, HeResourceUnit::Ru52, 0),
            (79, 0, 13, HeResourceUnit::Ru106, 7),
            (80, 4, 5, HeResourceUnit::Ru106, 0),
            (88, 2, 5, HeResourceUnit::Ru106, 0),
            (96, 1, 4, HeResourceUnit::Ru52, 0),
            (96, 3, 4, HeResourceUnit::Ru106, 0),
            (104, 0, 4, HeResourceUnit::Ru106, 0),
            (104, 1, 4, HeResourceUnit::Ru26, 0),
            (104, 2, 4, HeResourceUnit::Ru52, 0),
            (112, 3, 4, HeResourceUnit::Ru52, 0),
            (128, 0, 2, HeResourceUnit::Ru106, 0),
            (129, 0, 3, HeResourceUnit::Ru106, 0),
            (129, 1, 3, HeResourceUnit::Ru106, 1),
            (191, 7, 8, HeResourceUnit::Ru106, 1),
            (199, 7, 8, HeResourceUnit::Ru242, 7),
        ];
        for (encoding, position, user_count, resource_unit, multiplexed) in cases {
            let allocation = He20MuSigBRuAllocation::try_new(encoding).unwrap();
            assert_eq!(allocation.encoding(), encoding);
            assert_eq!(allocation.user_count(), user_count);
            assert_eq!(
                allocation.user(position),
                Some(He20MuSigBRuUser {
                    zero_based_position: position,
                    resource_unit,
                    multiplexed,
                })
            );
        }
    }

    #[test]
    fn rejects_reserved_and_non_he20_sig_b_ru_types() {
        for encoding in [113, 127, 217, 255] {
            assert_eq!(
                He20MuSigBRuAllocation::try_new(encoding),
                Err(He20MuSigBRuAllocationError::ReservedEncoding)
            );
        }
        for encoding in [200, 207, 208, 216] {
            assert_eq!(
                He20MuSigBRuAllocation::try_new(encoding),
                Err(He20MuSigBRuAllocationError::UnsupportedRuType)
            );
        }
    }

    #[test]
    fn iterates_three_he20_non_mimo_users_across_pair_crc_tail_gap() {
        let words = [
            (1 << 20) | (3 << 15) | 0x123,
            (1 << 19) | (5 << 15) | 0x456,
            (1 << 14) | (7 << 15) | 0x321,
        ];
        let mut complete = [0_u8; 13];
        write_bits(&mut complete, 18, 21, words[0]);
        write_bits(&mut complete, 39, 21, words[1]);
        write_bits(&mut complete, 70, 21, words[2]);

        // 18 common + 52 for the first pair + 21 for the final user + ten
        // final CRC/tail bits. The blob user-count expression sees three.
        let mut users = He20MuSigBNonMimoUsers::try_new(&complete, 101).unwrap();
        assert_eq!(users.user_count(), 3);
        assert_eq!(users.len(), 3);
        for (expected_index, expected_offset, expected_word) in
            [(0, 18, words[0]), (1, 39, words[1]), (2, 70, words[2])]
        {
            let entry = users.next().unwrap();
            assert_eq!(entry.index, expected_index);
            assert_eq!(entry.bit_offset, expected_offset);
            assert_eq!(entry.raw, expected_word);
            assert_eq!(entry.user, HeMuSigBNonMimoUser::decode(expected_word));
        }
        assert_eq!(users.next(), None);
        assert_eq!(users.len(), 0);
    }

    #[test]
    fn rejects_truncated_or_out_of_domain_he20_complete_streams() {
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&[], 17),
            Err(He20MuSigBNonMimoStreamError::BitLengthBeforeFirstUser)
        );
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&[0; 11], 101),
            Err(He20MuSigBNonMimoStreamError::CompleteBytesTooShort)
        );
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&[0; 12], 90),
            Err(He20MuSigBNonMimoStreamError::IncompleteUserField)
        );
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&[0; 35], 278),
            Err(He20MuSigBNonMimoStreamError::TooManyUsers)
        );
    }

    #[test]
    fn binds_he20_common_ru_allocation_to_the_complete_user_count() {
        let mut four_users = [0_u8; 16];
        four_users[0] = 112;
        let users = He20MuSigBNonMimoUsers::try_new(&four_users, 122).unwrap();
        let allocation = users.ru_allocation().unwrap();
        assert_eq!(allocation.encoding(), 112);
        assert_eq!(allocation.user_count(), 4);

        four_users[0] = 0;
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&four_users, 122)
                .unwrap()
                .ru_allocation(),
            Err(He20MuSigBRuStreamError::UserCountMismatch {
                stream_users: 4,
                allocation_users: 9,
            })
        );
        four_users[0] = 200;
        assert_eq!(
            He20MuSigBNonMimoUsers::try_new(&four_users, 122)
                .unwrap()
                .ru_allocation(),
            Err(He20MuSigBRuStreamError::Allocation(
                He20MuSigBRuAllocationError::UnsupportedRuType
            ))
        );
    }

    #[test]
    fn iterates_the_four_non_linear_compressed_mimo_user_offsets() {
        let words = [
            (1 << 20) | (1 << 15) | (4 << 11) | 0x111,
            (2 << 15) | (4 << 11) | 0x222,
            (1 << 20) | (3 << 15) | (4 << 11) | 0x333,
            (4 << 15) | (4 << 11) | 0x444,
        ];
        let mut complete = [0_u8; 17];
        for (offset, word) in [0, 21, 52, 105].into_iter().zip(words) {
            write_bits(&mut complete, offset, 21, word);
        }

        let mut users = He20MuSigBMimoUsers::try_new(&complete, 136, 4).unwrap();
        let spatial = users.spatial_configuration().unwrap();
        assert_eq!(spatial.user_count(), 4);
        assert_eq!(spatial.encoding(), 4);
        assert_eq!(
            [0, 1, 2, 3].map(|index| spatial.nsts_for_user(index).unwrap()),
            [2, 2, 1, 1]
        );
        assert_eq!(spatial.total_nsts(), 6);
        assert_eq!(users.len(), 4);
        for (expected_offset, expected_word) in [0, 21, 52, 105].into_iter().zip(words) {
            let entry = users.next().unwrap();
            assert_eq!(entry.bit_offset, expected_offset);
            assert_eq!(entry.raw, expected_word);
            assert_eq!(entry.user, HeMuSigBMimoUser::decode(expected_word));
        }
        assert_eq!(users.next(), None);

        write_bits(&mut complete, 21, 21, words[1] ^ (1 << 11));
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&complete, 136, 4)
                .unwrap()
                .spatial_configuration(),
            Err(He20MuSigBMimoSpatialError::InconsistentEncoding)
        );
    }

    #[test]
    fn rejects_invalid_compressed_mimo_counts_lengths_and_storage() {
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&[0; 17], 136, 0),
            Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
        );
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&[0; 17], 136, 5),
            Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
        );
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&[0; 16], 136, 4),
            Err(He20MuSigBMimoStreamError::CompleteBytesTooShort)
        );
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&[0; 17], 135, 4),
            Err(He20MuSigBMimoStreamError::IncompleteUserField)
        );
        assert_eq!(
            He20MuSigBMimoUsers::try_new(&[0; 9], 72, 3),
            Err(He20MuSigBMimoStreamError::IncompleteUserField)
        );
        assert_eq!(HeMuMimoSpatialConfiguration::try_new(1, 0), None);
        assert_eq!(HeMuMimoSpatialConfiguration::try_new(4, 11), None);
        assert_eq!(
            HeMuMimoSpatialConfiguration::try_new(8, 0)
                .unwrap()
                .total_nsts(),
            8
        );
    }

    #[test]
    fn parses_optional_one_ltf_800ns_gi_capability() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[10] = 0x40;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert!(capability.supports_one_ltf_800ns_gi());
        assert!(!capability.supports_ldpc_coding_in_payload());
    }

    #[test]
    fn parses_payload_ldpc_independently_from_gi_and_dcm() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[10] = 0x20;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert!(!capability.supports_one_ltf_800ns_gi());
        assert!(capability.supports_ldpc_coding_in_payload());
        assert_eq!(
            capability.dcm_receive_constellation(),
            HeDcmConstellation::NotSupported
        );
    }

    #[test]
    fn parses_independent_dcm_transmit_and_receive_constellations() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        // Peer TX: QPSK (bits 1:0 = 2); peer RX: 16-QAM
        // (bits 4:3 = 3). NSS stays one when bits 2 and 5 are clear.
        element[12] = 0x1a;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qpsk);
        assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
        assert!(capability.dcm_receive.supports_bpsk());
        assert!(capability.dcm_receive.supports_qpsk());
        assert!(capability.dcm_receive.supports_16qam());
    }

    #[test]
    fn parses_stbc_and_independent_cqi_feedback_capabilities() {
        let mut element = [0_u8; 24];
        element[..3].copy_from_slice(&[255, 22, 35]);
        element[11] = 0x0c;
        element[15] = 0x1c;
        element[18] = 0x02;
        let capability = parse_he20_capabilities(&element).unwrap();
        assert!(capability.stbc_transmit_under_80_mhz);
        assert!(capability.stbc_receive_under_80_mhz);
        assert!(capability.triggered_su_beamforming_feedback);
        assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
        assert!(capability.triggered_cqi_feedback);
        assert!(capability.non_triggered_cqi_feedback);
    }

    #[test]
    fn decodes_the_vendor_s31_sta_stbc_and_cqi_advertisement() {
        let capability = [
            0xff, 0x16, 0x23, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d,
            0xc0, 0x1f, 0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
        ];
        let capability = parse_he20_capabilities(&capability).unwrap();
        assert!(!capability.stbc_transmit_under_80_mhz);
        assert!(capability.stbc_receive_under_80_mhz);
        assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qam16);
        assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
        assert!(capability.triggered_su_beamforming_feedback);
        assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
        assert!(capability.triggered_cqi_feedback);
        assert!(capability.non_triggered_cqi_feedback);
    }

    #[test]
    fn parses_disabled_partial_bss_color() {
        let element = [255, 7, 36, 0, 0, 0, 0xc5, 0xfd, 0xff];
        let operation = parse_he20_operation(&element).unwrap();
        assert_eq!(operation.bss_color, 5);
        assert!(!operation.bss_color_enabled);
        assert!(operation.partial_bss_color);
        assert_eq!(operation.effective_bss_color(), 0);
        assert_eq!(operation.basic_mcs_nss_map, 0xfffd);
    }

    #[test]
    fn disabled_color_matches_vendor_effective_tx_color() {
        let element = [255, 7, 36, 0, 0, 0, 0xae, 0xfd, 0xff];
        let operation = parse_he20_operation(&element).unwrap();
        assert_eq!(operation.bss_color, 46);
        assert!(!operation.bss_color_enabled);
        assert!(!operation.partial_bss_color);
        assert_eq!(operation.effective_bss_color(), 0);
    }

    #[test]
    fn recovers_vendor_ap_he20_peer_state() {
        let capability = [
            0xff, 0x1a, 0x23, 0x05, 0x00, 0x18, 0x12, 0x00, 0x10, 0x22, 0x20, 0x02, 0xc0, 0x0f,
            0x41, 0x95, 0x08, 0x00, 0xcc, 0x00, 0xfa, 0xff, 0xfa, 0xff, 0x19, 0x1c, 0xc7, 0x71,
        ];
        let operation = [0xff, 0x07, 0x24, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];
        let state = parse_he20_peer_state(&capability, &operation).unwrap();
        assert_eq!(state.max_rate_code, 229);
        assert_eq!(state.packet_padding_eight_us, 2);
        assert_eq!(state.operation_parameters, 0x01_0004);
        assert_eq!(state.bss_color_information, 27);
        assert_eq!(state.basic_mcs_nss_map, 0xfffc);
        assert_eq!(state.rts_threshold, None);
        assert!(state.extended_range_single_user_disabled);
        assert!(!state.extended_range_single_user_permitted());
        assert!(
            !parse_he20_capabilities(&capability)
                .unwrap()
                .supports_one_ltf_800ns_gi()
        );
        assert_eq!(
            parse_he20_capabilities(&capability)
                .unwrap()
                .dcm_receive_constellation(),
            HeDcmConstellation::NotSupported
        );
    }
}
