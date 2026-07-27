//! Owned access to the shared ESP32-S31 PHY table-memory aperture.

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
    let command_value = phy_memory::command::MEMORY_COMMAND
        .checked_value(u32::from(command))
        .ok_or(PhyMemoryError::PbusCommandOutOfRange)?;
    if let Some(boundary) = boundary {
        let (pair, mask, value) = encoded_boundary(boundary)?;
        registers.modify32(phy_memory::GROUP_BOUNDARY[pair], mask, value);
    }
    registers.write32(phy_memory::DATA_0, data);
    registers.modify32(
        phy_memory::COMMAND,
        phy_memory::command::MEMORY_COMMAND.mask(),
        command_value,
    );
    Ok(())
}

/// Capture all six packed PBUS-memory group-boundary words.
///
/// Basis: complete rev0 ROM `phy_save_pbus_reg` at `0x2f82_4602`, size
/// `0x32`. It performs exactly six consecutive reads from `0x2010_0854`
/// through `0x2010_0868`; Rust returns them to its unique state owner instead
/// of storing through ROM's global `phy_param` pointer.
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
}
