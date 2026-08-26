//! Ownership-bound access to the shared PHY analog-I²C master.

#![forbid(unsafe_code)]

use super::RadioPhyRegisters;

/// One of the two reviewed analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

impl RadioPhyRegisters {
    /// Install the complete reviewed PHY-I²C host map with one fresh RMW.
    pub fn configure_phy_i2c_host_map(&mut self) {
        self.peripherals
            .i2c_ana_mst
            .ana_conf2()
            .modify(|_, w| w.phy_host_map().reviewed_radio_map());
    }

    /// Publish the finite reset command for one analog-I²C host.
    pub fn pulse_phy_i2c_master_reset(&mut self, host: PhyI2cHost) {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .write(|w| w.start_or_reset().set_bit()),
        };
    }

    /// Sample the reviewed completion predicate for one host.
    pub fn phy_i2c_master_is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .read()
                .busy()
                .bit_is_set(),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .read()
                .busy()
                .bit_is_set(),
        }
    }

    /// Publish the complete complemented read mask used by the vendor leaf.
    pub fn publish_phy_i2c_read_mask(&mut self, read_mask: u16) {
        let complement = !u32::from(read_mask);
        self.peripherals.i2c_ana_mst.ana_conf1().write(|w| {
            w.read_mask_complement_low()
                .set(complement & 0x00ff_ffff)
                .read_mask_complement_high()
                .set((complement >> 24) as u8)
        });
    }

    /// Publish one complete host command in the reviewed vendor order.
    pub fn publish_phy_i2c_command(
        &mut self,
        host: PhyI2cHost,
        block: u8,
        register: u8,
        value: u8,
        write: bool,
    ) {
        match host {
            PhyI2cHost::Host0 => self.peripherals.i2c_ana_mst.i2c0_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
            PhyI2cHost::Host1 => self.peripherals.i2c_ana_mst.i2c1_ctrl().write(|w| {
                w.slave_addr()
                    .set(block)
                    .slave_reg_addr()
                    .set(register)
                    .data()
                    .set(value)
                    .read_write()
                    .bit(write)
                    .start_or_reset()
                    .set_bit()
            }),
        };
    }

    /// Sample the completed data byte from one host.
    pub fn sample_phy_i2c_result(&self, host: PhyI2cHost) -> u8 {
        match host {
            PhyI2cHost::Host0 => self
                .peripherals
                .i2c_ana_mst
                .i2c0_ctrl()
                .read()
                .data()
                .bits(),
            PhyI2cHost::Host1 => self
                .peripherals
                .i2c_ana_mst
                .i2c1_ctrl()
                .read()
                .data()
                .bits(),
        }
    }

    /// Apply all six timing RMWs in the complete vendor order.
    pub fn configure_phy_i2c_clock_selection(&mut self, selection: u32) {
        let side_guard = ((selection >> 2) & 0x1f) as u8;
        let pulse_duration = ((selection >> 1) & 0x3f) as u8;
        let registers = &self.peripherals.i2c_ana_mst;

        registers
            .i2c0_ctrl1()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .i2c0_ctrl1()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
        registers
            .i2c1_ctrl1()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .i2c1_ctrl1()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
        registers
            .hw_i2c_ctrl()
            .modify(|_, w| w.sda_side_guard().set(side_guard));
        registers
            .hw_i2c_ctrl()
            .modify(|_, w| w.scl_pulse_duration().set(pulse_duration));
    }

    /// Select register mode two, then enable it with a separate fresh RMW.
    pub fn configure_phy_i2c_master_registers(&mut self) {
        let control = self.peripherals.i2c_ana_mst.ana_conf0();
        control.modify(|_, w| w.phy_register_mode().register_mode());
        control.modify(|_, w| w.phy_register_enable().set_bit());
    }

    /// Select one of the two complete-ROM BBPLL calibration encodings.
    pub fn set_phy_i2c_bbpll_calibration(&mut self, enabled: bool) {
        self.peripherals.i2c_ana_mst.ana_conf0().modify(|_, w| {
            if enabled {
                w.bbpll_cal_mode().enabled()
            } else {
                w.bbpll_cal_mode().disabled()
            }
        });
    }

    /// Publish one recovered command-RAM entry.
    ///
    /// Returns false for an invalid index. The PAC owns the command-memory
    /// field geometry and publishes zero to every unreviewed register bit.
    pub fn write_phy_i2c_command_memory(
        &mut self,
        index: usize,
        block: u8,
        register: u8,
        value: u8,
    ) -> bool {
        if index >= 45 {
            return false;
        }
        super::svd::zero_based_field_write::phy_i2c_command_memory(
            &self.peripherals.phy_i2c_command_ram,
            index,
            block,
            register,
            value,
        );
        true
    }
}
