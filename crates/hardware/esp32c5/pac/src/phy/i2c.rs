//! Ownership-bound access to the analog register I2C master the PHY drives.
//!
//! The libphy leaves `phy_chip_i2c_readReg` and `phy_chip_i2c_writeReg`
//! busy-wait on bit 25 of the host command word. This owner does not
//! reproduce those loops: it separates command publication from completion
//! observation so an outer owner can wait and inspect the host once.
//!
//! SOURCE: reviewed evidence `C5_BLOB_LIBPHY_PHY_I2C` (ESP32-C5
//! `libphy.a[phy_i2c.o]`: `phy_chip_i2c_readReg`, `phy_chip_i2c_readReg_org`,
//! `phy_chip_i2c_writeReg`, `phy_get_i2c_hostid_`, `phy_get_i2c_read_mask_`),
//! corroborated by `ESP_IDF_4D59230D_C5_I2C_ANA_MST`.

#![forbid(unsafe_code)]

use crate::svd;

/// One of the two analog-register command hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cHost {
    Host0,
    Host1,
}

/// Validated identity of one analog block the libphy leaves accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cBlock {
    code: u8,
}

/// First and last block with a read-mask entry in `phy_get_i2c_read_mask_`.
const FIRST_BLOCK: u8 = 0x61;
const LAST_BLOCK: u8 = 0x6f;

/// `CSWTCH.81`: the one-hot read-mask quarter of blocks 0x61 to 0x6F.
const READ_MASK_QUARTERS: [u16; 15] = [
    0x0040, 0x0000, 0x0004, 0x0000, 0x0400, 0x0020, 0x0001, 0x0008, 0x0080, 0x0010, 0x0002, 0x0800,
    0x0100, 0x0200, 0x1000,
];

/// Blocks 0x63 to 0x6E that `phy_get_i2c_hostid_` assigns to host 1, as bit
/// `block - 0x63`.
const HOST1_BLOCKS: u16 = 0x09ad;

impl PhyI2cBlock {
    /// Convert the raw block byte of the vendor ABI.
    pub const fn from_vendor_abi(code: u8) -> Option<Self> {
        if code >= FIRST_BLOCK && code <= LAST_BLOCK {
            Some(Self { code })
        } else {
            None
        }
    }

    pub const fn code(self) -> u8 {
        self.code
    }

    /// The host `phy_get_i2c_hostid_` selects for the block as called.
    pub const fn host(self) -> PhyI2cHost {
        let offset = self.code.wrapping_sub(0x63);
        if offset <= 11 && HOST1_BLOCKS & (1 << offset) != 0 {
            PhyI2cHost::Host1
        } else {
            PhyI2cHost::Host0
        }
    }

    /// Low 24 bits of the complemented read mask `phy_chip_i2c_readReg_org`
    /// publishes for the block as called; the high byte is 0xFF.
    pub const fn read_mask_complement_low(self) -> u32 {
        let mask = (READ_MASK_QUARTERS[(self.code - FIRST_BLOCK) as usize] as u32) << 2;
        !mask & 0x00ff_ffff
    }

    /// The block code the command carries: both leaves address block 0x65
    /// when called with block 0x62, after choosing its host and read mask.
    pub const fn command_code(self) -> u8 {
        if self.code == 0x62 { 0x65 } else { self.code }
    }
}

/// Validated address of one analog byte register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cAddress {
    block: PhyI2cBlock,
    register: u8,
}

impl PhyI2cAddress {
    pub const fn new(block: PhyI2cBlock, register: u8) -> Self {
        Self { block, register }
    }

    pub const fn block(self) -> PhyI2cBlock {
        self.block
    }

    pub const fn register(self) -> u8 {
        self.register
    }
}

/// A command is still owned by its host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cAccessError {
    Busy,
}

/// Unique owner of the analog register I2C master.
#[must_use = "dropping the analog I2C owner loses its register authority"]
pub struct PhyI2cRegisters {
    peripherals: svd::peripheral_ownership::PhyI2cPeripherals,
    /// The host map was rewritten for a command that has not started yet
    /// because its host was busy. The map image is the same for every block,
    /// so a retry only polls the host, as the vendor leaves do.
    host_map_pending: bool,
}

impl PhyI2cRegisters {
    pub(crate) const fn new(peripherals: svd::peripheral_ownership::PhyI2cPeripherals) -> Self {
        Self {
            peripherals,
            host_map_pending: false,
        }
    }

    /// Rewrite the host map once per command and return the block's host.
    fn select_host_once(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        if self.host_map_pending {
            block.host()
        } else {
            self.host_map_pending = true;
            self.configure_and_select_host(block)
        }
    }

    fn master(&self) -> &svd::I2cAnaMst {
        &self.peripherals.i2c_ana_mst
    }

    /// Replace the host-selection field and return the block's host, as
    /// `phy_get_i2c_hostid_` does on every access.
    pub fn configure_and_select_host(&mut self, block: PhyI2cBlock) -> PhyI2cHost {
        crate::generated::configure_phy_i2c_host_map(self.master());
        block.host()
    }

    /// Whether a host is executing a command.
    pub fn is_busy(&self, host: PhyI2cHost) -> bool {
        match host {
            PhyI2cHost::Host0 => svd::field_read::observe_phy_i2c_host0_busy(self.master()),
            PhyI2cHost::Host1 => svd::field_read::observe_phy_i2c_host1_busy(self.master()),
        }
    }

    fn publish(&mut self, host: PhyI2cHost, address: PhyI2cAddress, value: u8, write: bool) {
        self.host_map_pending = false;
        let code = address.block.command_code();
        match host {
            PhyI2cHost::Host0 => svd::zero_based_field_write::publish_phy_i2c_host0_command(
                self.master(),
                code,
                address.register,
                value,
                write,
                true,
            ),
            PhyI2cHost::Host1 => svd::zero_based_field_write::publish_phy_i2c_host1_command(
                self.master(),
                code,
                address.register,
                value,
                write,
                true,
            ),
        }
    }

    /// Start one read: select the host, publish the block's read mask and
    /// the read command.
    ///
    /// # Errors
    ///
    /// The block's host is busy; the host map has been rewritten once for
    /// this command, and a retry only polls the host.
    pub fn try_start_read(&mut self, address: PhyI2cAddress) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.block);
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        svd::zero_based_field_write::publish_phy_i2c_read_mask(
            self.master(),
            address.block.read_mask_complement_low(),
            0xff,
        );
        self.publish(host, address, 0, false);
        Ok(())
    }

    /// Sample the byte of a completed read of the same address.
    ///
    /// # Errors
    ///
    /// The host is still busy.
    pub fn try_finish_read(&self, address: PhyI2cAddress) -> Result<u8, PhyI2cAccessError> {
        let host = address.block.host();
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        Ok(match host {
            PhyI2cHost::Host0 => svd::field_read::observe_phy_i2c_host0_data(self.master()),
            PhyI2cHost::Host1 => svd::field_read::observe_phy_i2c_host1_data(self.master()),
        })
    }

    /// Start one write once the block's host is idle.
    ///
    /// # Errors
    ///
    /// The block's host is busy; the host map has been rewritten once for
    /// this command, and a retry only polls the host.
    pub fn try_start_write(
        &mut self,
        address: PhyI2cAddress,
        value: u8,
    ) -> Result<(), PhyI2cAccessError> {
        let host = self.select_host_once(address.block);
        if self.is_busy(host) {
            return Err(PhyI2cAccessError::Busy);
        }
        self.publish(host, address, value, true);
        Ok(())
    }

    /// Observe the completion of a write of the same address.
    ///
    /// # Errors
    ///
    /// The host is still busy.
    pub fn try_finish_write(&self, address: PhyI2cAddress) -> Result<(), PhyI2cAccessError> {
        if self.is_busy(address.block.host()) {
            Err(PhyI2cAccessError::Busy)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blocks ESP-IDF's `regi2c_impl.c` names agree with the libphy
    /// tables: the same one-hot read-mask bit and the same host.
    #[test]
    fn the_public_regi2c_blocks_agree_with_the_libphy_tables() {
        // (block, ANA_CONF1 read-mask bit, ANA_CONF2 selection bit set)
        for (code, mask_bit, selects_host0) in [
            (0x6a, 6, false),
            (0x66, 7, false),
            (0x61, 8, true),
            (0x69, 9, true),
            (0x6d, 10, true),
        ] {
            let block = PhyI2cBlock::from_vendor_abi(code).expect("reviewed block");
            assert_eq!(
                block.read_mask_complement_low(),
                !(1 << mask_bit) & 0x00ff_ffff,
                "{code:#x}"
            );
            let host = if selects_host0 {
                PhyI2cHost::Host0
            } else {
                PhyI2cHost::Host1
            };
            assert_eq!(block.host(), host, "{code:#x}");
            // The reviewed host map sets exactly the selection bits of the
            // host-0 blocks (bits 8..=12 of ANA_CONF2, field offset 4).
            let selection = 0x19c1_u32 << 4;
            let bit = match code {
                0x6a => 8,
                0x66 => 9,
                0x61 => 10,
                0x69 => 11,
                _ => 12,
            };
            assert_eq!(selection & (1 << bit) != 0, selects_host0, "{code:#x}");
        }
    }

    #[test]
    fn block_0x62_is_addressed_as_0x65_with_its_own_host_and_mask() {
        let block = PhyI2cBlock::from_vendor_abi(0x62).expect("block");
        assert_eq!(block.command_code(), 0x65);
        assert_eq!(block.host(), PhyI2cHost::Host0);
        assert_eq!(block.read_mask_complement_low(), 0x00ff_ffff);
        let target = PhyI2cBlock::from_vendor_abi(0x65).expect("block");
        assert_eq!(target.command_code(), 0x65);
        assert_eq!(target.host(), PhyI2cHost::Host1);
    }

    #[test]
    fn blocks_outside_the_mask_table_are_rejected() {
        assert!(PhyI2cBlock::from_vendor_abi(0x60).is_none());
        assert!(PhyI2cBlock::from_vendor_abi(0x70).is_none());
        assert_eq!(
            PhyI2cBlock::from_vendor_abi(0x6e).map(PhyI2cBlock::host),
            Some(PhyI2cHost::Host1)
        );
        assert_eq!(
            PhyI2cBlock::from_vendor_abi(0x6f).map(PhyI2cBlock::host),
            Some(PhyI2cHost::Host0)
        );
    }
}
