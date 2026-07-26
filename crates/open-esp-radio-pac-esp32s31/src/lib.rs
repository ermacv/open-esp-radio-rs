#![no_std]

use core::ptr::{read_volatile, write_volatile};

/// Unique logical owner of the ESP32-S31 radio register regions.
///
/// This is the initial PAC boundary. Named register blocks and fields will
/// replace raw addresses as their meanings are proven.
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
                | 0x2070_0000..=0x2071_ffff
                | 0x2080_0000..=0x2081_ffff
        )
    }

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
