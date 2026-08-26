//! Ownership-bound access to the shared PHY table-memory aperture.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

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

/// One complete gain-memory transaction encoded only inside the PAC.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PhyGainMemoryEntry {
    words: [u32; 3],
    index: u8,
}

impl core::fmt::Debug for PhyGainMemoryEntry {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PhyGainMemoryEntry(..)")
    }
}

impl PhyGainMemoryEntry {
    /// Encode one generated receive-gain entry from semantic calibration data.
    ///
    /// `dc` contains the I and Q calibration components in that order.
    pub const fn generated_receive(
        index: u8,
        dc: [u16; 2],
        index_dc: [u16; 2],
        auxiliary: u16,
        encoded_gain: u32,
        mixer_digital_gain: u8,
        parameter_002: u8,
    ) -> Self {
        let [dc_i, dc_q] = dc;
        Self {
            words: [
                ((dc_i as u32) << 31)
                    | ((dc_q as u32) << 13)
                    | ((index_dc[1] as u32) << 22)
                    | ((auxiliary as u32) & 0x1fff),
                (((encoded_gain >> 4) & 0x7f) << 20)
                    | ((encoded_gain & 7) << 17)
                    | ((index_dc[0] as u32) << 8)
                    | ((dc_i as u32) >> 1)
                    | ((mixer_digital_gain as u32) << 29),
                ((parameter_002 >> 6) as u32)
                    | (((encoded_gain >> 15) & 7) << 5)
                    | (((encoded_gain >> 12) & 7) << 2),
            ],
            index,
        }
    }

    /// Encode one fixed-form receive-table initialization entry.
    pub const fn receive_table(index: u8, parameter_002: u8) -> Self {
        Self {
            words: [
                0x4020_0000,
                0x0201_0080 | ((parameter_002 as u32) << 29),
                ((parameter_002 >> 6) as u32) | 0x0000_00fc,
            ],
            index,
        }
    }

    /// Encode one transmit-gain entry from the reviewed table components.
    pub const fn transmit(
        index: u8,
        gain_72: u16,
        gain_64: u16,
        gain_32: u8,
        seed: [u16; 4],
        config: u16,
    ) -> Self {
        let [seed_0, seed_1, seed_2, seed_3] = seed;
        let gain_72 = gain_72 as u32;
        let gain_64 = gain_64 as u32;
        Self {
            words: [
                ((config as u32) & 0x1fff)
                    | ((seed_2 as u32) << 22)
                    | ((seed_1 as u32) << 31)
                    | ((seed_3 as u32) << 13),
                ((seed_0 as u32) << 8)
                    | ((seed_1 as u32) >> 1)
                    | (((gain_64 >> 6) & 0xff) << 17)
                    | ((gain_72 & 7) << 31)
                    | ((gain_64 & 0x3f) << 20)
                    | 0x1000_0000,
                ((gain_72 & 7) >> 1)
                    | ((gain_72 >> 1) & 0x1c)
                    | ((gain_32 as u32) << 15)
                    | 0x0000_7f80,
            ],
            index,
        }
    }
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

impl RadioPhyRegisters {
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
    pub fn program_gain_memory_entry(&mut self, entry: PhyGainMemoryEntry) {
        let memory = &self.peripherals.phy_memory;
        super::generated::phy_memory_data_0(
            memory,
            super::generated::PhyMemoryData0::new(entry.words[0]),
        );
        super::generated::phy_memory_data_1(
            memory,
            super::generated::PhyMemoryData1::new(entry.words[1]),
        );
        super::generated::phy_memory_data_2(
            memory,
            super::generated::PhyMemoryData2::new(entry.words[2]),
        );

        self.peripherals.phy_memory.command().modify(|_, w| {
            w.gain_command_low_zero_unknown()
                .set(0)
                .memory_index()
                .set(entry.index)
                .gain_write_or_pbus_command_bit_8()
                .set_bit()
        });
    }
}
