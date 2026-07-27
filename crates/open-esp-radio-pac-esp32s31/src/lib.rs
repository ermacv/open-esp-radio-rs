#![no_std]

use core::ptr::{read_volatile, write_volatile};

pub mod mac;
pub mod power;

/// One PAC-described 32-bit MMIO register.
///
/// The address is intentionally private: downstream crates can use registers
/// described by this PAC but cannot manufacture new MMIO addresses.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Register32 {
    address: usize,
}

impl Register32 {
    pub(crate) const fn new(address: usize) -> Self {
        Self { address }
    }

    /// Numeric address for diagnostics and host-side register models.
    pub const fn address(self) -> usize {
        self.address
    }
}

/// Unique logical owner of the ESP32-S31 radio register regions.
///
/// Named [`Register32`] values localize addresses in this crate. Higher layers
/// retain semantic sequencing, but volatile pointer access stays here.
pub struct RadioRegisters {
    _private: (),
}

impl RadioRegisters {
    /// Claim radio MMIO when the caller has established unique ownership.
    ///
    /// # Safety
    ///
    /// No other live owner may mutate the radio through raw pointers, ROM,
    /// vendor code, or another `RadioRegisters` value.
    pub const unsafe fn steal() -> Self {
        Self { _private: () }
    }

    pub const fn contains(address: usize) -> bool {
        matches!(
            address,
            0x2010_0000..=0x2010_ffff
                | 0x2058_0000..=0x2058_ffff
                | 0x2070_0000..=0x2071_ffff
                | 0x2080_0000..=0x2081_ffff
        )
    }

    /// Read one PAC-described 32-bit register.
    pub fn read32(&self, register: Register32) -> u32 {
        // SAFETY: only this crate constructs `Register32`, and
        // `RadioRegisters` represents the unique live radio MMIO owner.
        unsafe { read_volatile(register.address as *const u32) }
    }

    /// Write one PAC-described 32-bit register.
    pub fn write32(&mut self, register: Register32, value: u32) {
        // SAFETY: only this crate constructs `Register32`, and the mutable
        // borrow serializes writes through the unique live radio owner.
        unsafe { write_volatile(register.address as *mut u32, value) }
    }

    /// Perform a finite read/modify/write on a PAC-described register.
    pub fn modify32(&mut self, register: Register32, clear_mask: u32, set_bits: u32) -> u32 {
        let previous = self.read32(register);
        let next = (previous & !clear_mask) | (set_bits & clear_mask);
        self.write32(register, next);
        previous
    }

    /// Order device-memory accesses at a descriptor or interrupt boundary.
    pub fn fence(&mut self) {
        #[cfg(target_arch = "riscv32")]
        // SAFETY: this instruction only orders memory and device accesses.
        unsafe {
            core::arch::asm!("fence iorw, iorw")
        }

        #[cfg(not(target_arch = "riscv32"))]
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }

    // Temporary compatibility for PHY leaves that have not yet moved to
    // PAC-described registers. New target paths must use the typed methods.

    /// Read one evidenced 32-bit radio register.
    ///
    /// # Safety
    ///
    /// `address` must be aligned and identify a readable register.
    pub unsafe fn read(&self, address: usize) -> u32 {
        debug_assert!(Self::contains(address));
        unsafe { read_volatile(address as *const u32) }
    }

    /// Write one evidenced 32-bit radio register.
    ///
    /// # Safety
    ///
    /// `address` must be aligned and identify a writable register, and the
    /// value must obey that register's hardware contract.
    pub unsafe fn write(&mut self, address: usize, value: u32) {
        debug_assert!(Self::contains(address));
        unsafe { write_volatile(address as *mut u32, value) }
    }

    /// Perform a finite read/modify/write transaction.
    ///
    /// # Safety
    ///
    /// The register must permit read/modify/write with the supplied masks.
    pub unsafe fn replace_bits(&mut self, address: usize, clear_mask: u32, set_bits: u32) -> u32 {
        let previous = unsafe { self.read(address) };
        let next = (previous & !clear_mask) | (set_bits & clear_mask);
        unsafe { self.write(address, next) };
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::{mac, power, RadioRegisters, Register32};

    fn assert_valid(register: Register32) {
        assert!(RadioRegisters::contains(register.address()));
        assert_eq!(register.address() & 3, 0);
    }

    #[test]
    fn every_power_register_belongs_to_a_known_mmio_region() {
        for register in power::ALL {
            assert_valid(register);
        }
    }

    #[test]
    fn hp_modem_region_is_part_of_the_radio_capability() {
        assert!(RadioRegisters::contains(power::hp_modem::CTRL0.address()));
        assert!(RadioRegisters::contains(power::hp_modem::CONF.address()));
        assert!(!RadioRegisters::contains(0x2057_ffff));
    }

    #[test]
    fn indexed_mac_registers_are_bounded_and_aligned() {
        for group in [
            &mac::init::INTERFACE_ADDRESS_LOW[..],
            &mac::init::INTERFACE_ADDRESS_HIGH,
            &mac::init::RX_FILTER,
            &mac::init::BSSID_HIGH,
            &mac::init::RX_QUEUE_DEFAULT,
            &mac::init::HE_PROTECTION,
            &mac::init::HE_QUEUE_CONTROL,
            &mac::init::LAST_RX_BUFFER,
            &mac::init::CRYPTO_BYPASS,
        ] {
            for &register in group {
                assert_valid(register);
            }
        }

        for index in 0..mac::init::HE_SCRATCH_COUNT {
            assert_valid(mac::init::he_scratch(index).unwrap());
        }
        assert!(mac::init::he_scratch(mac::init::HE_SCRATCH_COUNT).is_none());

        for index in 0..mac::init::ANTENNA_CONTROL_COUNT {
            assert_valid(mac::init::antenna_control(index).unwrap());
        }
        assert!(mac::init::antenna_control(mac::init::ANTENNA_CONTROL_COUNT).is_none());
    }

    #[test]
    fn mac_init_aliases_share_canonical_register_identities() {
        assert_eq!(mac::init::R_4098, mac::RX_CSI_CONFIG);
        assert_eq!(mac::init::RX_SNIFFER_CONTROL, mac::init::RX_FILTER[3]);
        assert_eq!(mac::init::HE_PROTECTION[0], mac::TX_Q0_PROTECTION);
        assert_eq!(mac::init::HE_QUEUE_CONTROL[0], mac::TX_Q0_PPDU_CONTROL);
        assert_eq!(
            mac::init::antenna_control(0),
            Some(mac::TX_Q0_LENGTH_CONTROL)
        );
    }
}
