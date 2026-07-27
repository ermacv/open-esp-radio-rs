//! Owned access to the shared ESP32-S31 PHY table-memory aperture.

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_pac_esp32s31::power::phy_clock_oracle;
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_pac_esp32s31::power::phy_memory;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_pac_esp32s31::RadioRegisters;

/// One packed PBUS-memory group boundary.
///
/// Basis: complete rev0 ROM `phy_write_pbus_mem` at `0x2f82_4634`. The body
/// packs the first and last entry indices for groups 0..11 into six words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PbusMemoryGroupBoundary {
    pub group: u8,
    pub first_entry: u8,
    pub last_entry: u8,
}

/// A shared PHY-memory operation did not fit the instruction-evidenced layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMemoryError {
    /// Complete ROM `phy_set_pbus_mem` constructs exactly twelve groups.
    PbusGroupOutOfRange,
    /// Complete ROM `phy_write_pbus_mem` publishes a ten-bit command.
    PbusCommandOutOfRange,
}

#[cfg(any(target_arch = "riscv32", test))]
fn encoded_boundary(
    boundary: PbusMemoryGroupBoundary,
) -> Result<(usize, u32, u32), PhyMemoryError> {
    if boundary.group >= 12 {
        return Err(PhyMemoryError::PbusGroupOutOfRange);
    }
    let odd = boundary.group & 1 != 0;
    let pair = usize::from(boundary.group >> 1);
    let (first, last) = if odd {
        (
            phy_memory::group_boundary::ODD_GROUP_FIRST_ENTRY,
            phy_memory::group_boundary::ODD_GROUP_LAST_ENTRY,
        )
    } else {
        (
            phy_memory::group_boundary::EVEN_GROUP_FIRST_ENTRY,
            phy_memory::group_boundary::EVEN_GROUP_LAST_ENTRY,
        )
    };
    let mask = first.mask() | last.mask();
    let value = first
        .checked_value(u32::from(boundary.first_entry))
        .unwrap_or(0)
        | last
            .checked_value(u32::from(boundary.last_entry))
            .unwrap_or(0);
    Ok((pair, mask, value))
}

#[cfg(any(target_arch = "riscv32", test))]
fn encoded_pbus_command(command: u16) -> Result<(u32, u32), PhyMemoryError> {
    if command > 0x03ff {
        return Err(PhyMemoryError::PbusCommandOutOfRange);
    }
    let index = phy_memory::command::MEMORY_INDEX
        .checked_value(u32::from(command & 0xff))
        .unwrap_or(0);
    let bit_8 = if command & 0x100 != 0 {
        phy_memory::command::GAIN_WRITE_OR_PBUS_COMMAND_BIT_8.mask()
    } else {
        0
    };
    let bit_9 = if command & 0x200 != 0 {
        phy_memory::command::PBUS_COMMAND_BIT_9.mask()
    } else {
        0
    };
    Ok((
        phy_memory::command::MEMORY_INDEX.mask()
            | phy_memory::command::GAIN_WRITE_OR_PBUS_COMMAND_BIT_8.mask()
            | phy_memory::command::PBUS_COMMAND_BIT_9.mask(),
        index | bit_8 | bit_9,
    ))
}

#[cfg(any(target_arch = "riscv32", test))]
fn encoded_tx_cfr_index(index: u8) -> (u32, u32) {
    let field = phy_memory::command::MEMORY_INDEX;
    (
        field.mask(),
        field.checked_value(u32::from(index)).unwrap_or(0),
    )
}

#[cfg(any(target_arch = "riscv32", test))]
fn encoded_gain_command(index: u8) -> (u32, u32) {
    let low = phy_memory::command::GAIN_COMMAND_LOW_ZERO_UNKNOWN;
    let memory_index = phy_memory::command::MEMORY_INDEX;
    let write = phy_memory::command::GAIN_WRITE_OR_PBUS_COMMAND_BIT_8;
    (
        low.mask() | memory_index.mask() | write.mask(),
        memory_index.checked_value(u32::from(index)).unwrap_or(0) | write.mask(),
    )
}

#[cfg(any(target_arch = "riscv32", test))]
fn encoded_table_memory_base_index(index: u8) -> (u32, u32) {
    let field = phy_clock_oracle::table_memory_index_source::BASE_INDEX;
    (
        field.mask(),
        field.checked_value(u32::from(index)).unwrap_or(0),
    )
}

/// Publish one entry of the PBUS-memory table.
///
/// Basis: complete rev0 ROM `phy_write_pbus_mem` at `0x2f82_4634`, size
/// `0x16a`. On the first entry of a group it performs one packed-boundary
/// RMW, then writes `DATA_0`, then replaces `COMMAND.MEMORY_COMMAND`.
/// The method preserves that exact access order and performs no polling.
#[cfg(target_arch = "riscv32")]
pub fn program_pbus_memory_entry(
    registers: &mut RadioRegisters,
    boundary: Option<PbusMemoryGroupBoundary>,
    data: u32,
    command: u16,
) -> Result<(), PhyMemoryError> {
    let (command_mask, command_value) = encoded_pbus_command(command)?;
    if let Some(boundary) = boundary {
        let (pair, mask, value) = encoded_boundary(boundary)?;
        registers.modify32(phy_memory::GROUP_BOUNDARY[pair], mask, value);
    }
    registers.write32(phy_memory::DATA_0, data);
    registers.modify32(phy_memory::COMMAND, command_mask, command_value);
    Ok(())
}

/// Sample the shared CFR/gain-memory base index once.
///
/// Basis: complete S31 `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem` and
/// `phy_set_tx_gain_mem_new`. Both read
/// `PHY_CLOCK_ORACLE.TABLE_MEMORY_INDEX_SOURCE.BASE_INDEX` exactly once
/// before publishing any table entry.
#[cfg(target_arch = "riscv32")]
pub fn read_table_memory_base_index(registers: &RadioRegisters) -> u8 {
    phy_clock_oracle::table_memory_index_source::BASE_INDEX
        .extract(registers.read32(phy_clock_oracle::TABLE_MEMORY_INDEX_SOURCE)) as u8
}

/// Configure the shared CFR/gain-memory base index.
///
/// Basis: complete rev0 ROM `phy_fe_reg_init` at `0x2f82_7740`. Its fifth
/// fresh-read RMW replaces exactly
/// `PHY_CLOCK_ORACLE.TABLE_MEMORY_INDEX_SOURCE.BASE_INDEX` with `0xa0`.
/// Complete S31 CFR and gain publishers later sample that same byte.
#[cfg(target_arch = "riscv32")]
pub fn configure_table_memory_base_index(registers: &mut RadioRegisters, index: u8) {
    let (mask, value) = encoded_table_memory_base_index(index);
    registers.modify32(phy_clock_oracle::TABLE_MEMORY_INDEX_SOURCE, mask, value);
}

/// Publish one TX-CFR memory entry and its complete commit pulse.
///
/// Basis: complete S31 `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem`, size
/// `0x76`. The body writes `DATA_0`, replaces only index bits 18:11, then
/// performs fresh-read set and clear RMW operations on commit bit 21.
#[cfg(target_arch = "riscv32")]
pub fn program_tx_cfr_entry(registers: &mut RadioRegisters, data: u32, index: u8) {
    registers.write32(phy_memory::DATA_0, data);
    let (index_mask, index_value) = encoded_tx_cfr_index(index);
    registers.modify32(phy_memory::COMMAND, index_mask, index_value);
    let commit = phy_memory::command::TX_CFR_COMMIT.mask();
    registers.modify32(phy_memory::COMMAND, commit, commit);
    registers.modify32(phy_memory::COMMAND, commit, 0);
}

/// Publish one three-word gain-memory entry.
///
/// Basis: complete rev0 ROM `phy_write_gain_mem` at `0x2f82_74f0`, size
/// `0x2a`. It writes all three data words in order, clears command bits 10:0,
/// writes the index in bits 18:11, sets gain-write bit 19, and preserves
/// bits 31:20 in one final RMW.
#[cfg(target_arch = "riscv32")]
pub fn program_gain_memory_entry(registers: &mut RadioRegisters, words: [u32; 3], index: u8) {
    registers.write32(phy_memory::DATA_0, words[0]);
    registers.write32(phy_memory::DATA_1, words[1]);
    registers.write32(phy_memory::DATA_2, words[2]);

    let (mask, value) = encoded_gain_command(index);
    registers.modify32(phy_memory::COMMAND, mask, value);
}

/// Capture all six packed PBUS-memory group-boundary words.
///
/// Basis: complete rev0 ROM `phy_save_pbus_reg` at `0x2f82_4602`, size
/// `0x32`. It performs exactly six consecutive reads of
/// `PHY_MEMORY.GROUP_BOUNDARY[0..6]`; Rust returns them to its unique state
/// owner instead of storing through ROM's global `phy_param` pointer.
#[cfg(target_arch = "riscv32")]
pub fn capture_pbus_memory_boundaries(registers: &RadioRegisters) -> [u32; 6] {
    [
        registers.read32(phy_memory::GROUP_BOUNDARY[0]),
        registers.read32(phy_memory::GROUP_BOUNDARY[1]),
        registers.read32(phy_memory::GROUP_BOUNDARY[2]),
        registers.read32(phy_memory::GROUP_BOUNDARY[3]),
        registers.read32(phy_memory::GROUP_BOUNDARY[4]),
        registers.read32(phy_memory::GROUP_BOUNDARY[5]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_twelve_groups_map_to_six_packed_words() {
        for group in 0..12 {
            let boundary = PbusMemoryGroupBoundary {
                group,
                first_entry: group * 3,
                last_entry: group * 3 + 2,
            };
            let (pair, mask, value) = encoded_boundary(boundary).unwrap();
            assert_eq!(pair, usize::from(group >> 1));
            let shift = if group & 1 == 0 { 0 } else { 16 };
            assert_eq!(mask, 0xffff << shift);
            assert_eq!(
                value,
                (u32::from(group * 3) | (u32::from(group * 3 + 2) << 8)) << shift
            );
        }
    }

    #[test]
    fn thirteenth_group_is_rejected() {
        assert_eq!(
            encoded_boundary(PbusMemoryGroupBoundary {
                group: 12,
                first_entry: 0,
                last_entry: 0,
            }),
            Err(PhyMemoryError::PbusGroupOutOfRange)
        );
    }

    #[test]
    fn pbus_command_uses_all_ten_evidenced_bits() {
        let (mask, value) = encoded_pbus_command(0x023b).unwrap();
        assert_eq!(mask, 0x001f_f800);
        assert_eq!(value, 0x0011_d800);
        assert_eq!(
            encoded_pbus_command(0x0400),
            Err(PhyMemoryError::PbusCommandOutOfRange)
        );
    }

    #[test]
    fn tx_cfr_replaces_only_the_eight_bit_index() {
        assert_eq!(encoded_tx_cfr_index(0x00), (0x0007_f800, 0));
        assert_eq!(encoded_tx_cfr_index(0xa5), (0x0007_f800, 0x0005_2800));
        assert_eq!(phy_memory::command::TX_CFR_COMMIT.mask(), 0x0020_0000);
    }

    #[test]
    fn gain_command_matches_the_complete_rom_leaf() {
        assert_eq!(encoded_gain_command(0x00), (0x000f_ffff, 0x0008_0000));
        assert_eq!(encoded_gain_command(0xa5), (0x000f_ffff, 0x000d_2800));
    }

    #[test]
    fn table_memory_base_index_is_exactly_the_high_byte() {
        assert_eq!(
            encoded_table_memory_base_index(0xa0),
            (0xff00_0000, 0xa000_0000)
        );
    }
}
