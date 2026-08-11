//! Ownership-bound access to the shared PHY table-memory aperture.

#![forbid(unsafe_code)]

use super::RadioRegisters;

/// One packed PBUS-memory group boundary.
///
/// Complete rev0 ROM `phy_write_pbus_mem` packs groups 0..11 into six
/// generated `GROUP_BOUNDARY` register words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PbusMemoryGroupBoundary {
    pub group: u8,
    pub first_entry: u8,
    pub last_entry: u8,
}

/// A shared PHY-memory operation did not fit the recovered layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMemoryError {
    /// Complete ROM `phy_set_pbus_mem` constructs exactly twelve groups.
    PbusGroupOutOfRange,
    /// Complete ROM `phy_write_pbus_mem` publishes a ten-bit command.
    PbusCommandOutOfRange,
}

const fn split_pbus_command(command: u16) -> Result<(u8, bool, bool), PhyMemoryError> {
    if command > 0x03ff {
        return Err(PhyMemoryError::PbusCommandOutOfRange);
    }
    Ok((
        (command & 0xff) as u8,
        command & 0x100 != 0,
        command & 0x200 != 0,
    ))
}

impl RadioRegisters {
    /// Publish one PBUS-memory entry in complete rev0 ROM access order.
    pub fn program_pbus_memory_entry(
        &mut self,
        boundary: Option<PbusMemoryGroupBoundary>,
        data: u32,
        command: u16,
    ) -> Result<(), PhyMemoryError> {
        let (memory_index, command_bit_8, command_bit_9) = split_pbus_command(command)?;

        if let Some(boundary) = boundary {
            if boundary.group >= 12 {
                return Err(PhyMemoryError::PbusGroupOutOfRange);
            }
            let pair = usize::from(boundary.group >> 1);
            let group_boundary = self.peripherals.phy_memory.group_boundary(pair);
            if boundary.group & 1 == 0 {
                group_boundary.modify(|_, w| {
                    w.even_group_first_entry()
                        .set(boundary.first_entry)
                        .even_group_last_entry()
                        .set(boundary.last_entry)
                });
            } else {
                group_boundary.modify(|_, w| {
                    w.odd_group_first_entry()
                        .set(boundary.first_entry)
                        .odd_group_last_entry()
                        .set(boundary.last_entry)
                });
            }
        }

        super::generated::phy_memory_data_0(
            &self.peripherals.phy_memory,
            super::generated::PhyMemoryData0::new(data),
        );
        self.peripherals.phy_memory.command().modify(|_, w| {
            w.memory_index()
                .set(memory_index)
                .gain_write_or_pbus_command_bit_8()
                .bit(command_bit_8)
                .pbus_command_bit_9()
                .bit(command_bit_9)
        });
        Ok(())
    }

    /// Sample the shared CFR/gain-memory base index exactly once.
    pub fn read_table_memory_base_index(&self) -> u8 {
        self.peripherals
            .phy_clock_oracle
            .table_memory_index_source()
            .read()
            .base_index()
            .bits()
    }

    /// Configure the shared CFR/gain-memory base index through one fresh RMW.
    pub fn configure_table_memory_base_index(&mut self, index: u8) {
        self.peripherals
            .phy_clock_oracle
            .table_memory_index_source()
            .modify(|_, w| w.base_index().set(index));
    }

    /// Apply complete rev0 ROM `phy_force_pwr_index` through its two ordered
    /// fresh-read field replacements.
    pub fn configure_forced_power_index(&mut self, enabled: u32, index: u32) {
        let control = self
            .peripherals
            .phy_clock_oracle
            .table_memory_index_source();
        control.modify(|_, w| w.force_power_enable().bit(enabled & 1 != 0));
        control.modify(|_, w| w.forced_power_index().set(index as u8 & 0x3f));
    }

    /// Publish one TX-CFR entry and its complete set/clear commit pulse.
    pub fn program_tx_cfr_entry(&mut self, data: u32, index: u8) {
        super::generated::phy_memory_data_0(
            &self.peripherals.phy_memory,
            super::generated::PhyMemoryData0::new(data),
        );
        self.peripherals
            .phy_memory
            .command()
            .modify(|_, w| w.memory_index().set(index));
        self.peripherals
            .phy_memory
            .command()
            .modify(|_, w| w.tx_cfr_commit().set_bit());
        self.peripherals
            .phy_memory
            .command()
            .modify(|_, w| w.tx_cfr_commit().clear_bit());
    }

    /// Publish one three-word gain-memory entry in complete ROM order.
    pub fn program_gain_memory_entry(&mut self, words: [u32; 3], index: u8) {
        let memory = &self.peripherals.phy_memory;
        super::generated::phy_memory_data_0(
            memory,
            super::generated::PhyMemoryData0::new(words[0]),
        );
        super::generated::phy_memory_data_1(
            memory,
            super::generated::PhyMemoryData1::new(words[1]),
        );
        super::generated::phy_memory_data_2(
            memory,
            super::generated::PhyMemoryData2::new(words[2]),
        );

        self.peripherals.phy_memory.command().modify(|_, w| {
            w.gain_command_low_zero_unknown()
                .set(0)
                .memory_index()
                .set(index)
                .gain_write_or_pbus_command_bit_8()
                .set_bit()
        });
    }

    /// Capture all six packed PBUS-memory group-boundary words in ROM order.
    pub fn capture_pbus_memory_boundaries(&self) -> [u32; 6] {
        [
            self.peripherals.phy_memory.group_boundary(0).read().bits(),
            self.peripherals.phy_memory.group_boundary(1).read().bits(),
            self.peripherals.phy_memory.group_boundary(2).read().bits(),
            self.peripherals.phy_memory.group_boundary(3).read().bits(),
            self.peripherals.phy_memory.group_boundary(4).read().bits(),
            self.peripherals.phy_memory.group_boundary(5).read().bits(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{PhyMemoryError, split_pbus_command};

    #[test]
    fn ten_bit_pbus_command_splits_into_generated_fields() {
        assert_eq!(split_pbus_command(0x023b), Ok((0x3b, false, true)));
        assert_eq!(
            split_pbus_command(0x0400),
            Err(PhyMemoryError::PbusCommandOutOfRange)
        );
    }
}
