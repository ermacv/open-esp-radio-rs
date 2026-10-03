//! Static RV32 image values: the ELF ABI, load segments and executable
//! section views. No loading or relocation.
use crate::*;
use serde::{Deserialize, Serialize};

/// ELF-declared floating-point calling convention, separate from instruction
/// coverage and from the integer analysis profile. No execution support implied.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiscvAbi {
    Ilp32,
    Ilp32f,
    Ilp32d,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSegment {
    pub address: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub flags: u32,
}

/// Borrowed executable section with structurally validated ELF data intervals.
/// Consumers must reset instruction state across each data interval.
pub struct ExecutableSectionView<'a> {
    pub section: u32,
    pub address: u32,
    pub bytes: &'a [u8],
    pub data_ranges: &'a [CodeRange],
}
