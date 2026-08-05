//! Generated-PAC ownership for finite STA CCMP key-table transactions.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence};

const KEY_ENTRY_COUNT: u8 = 25;
const KEY_ENTRY_WORDS: usize = 10;
const PROGRAMMED_CCMP_WORDS: usize = 6;
const STA_INTERFACE: usize = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKeyInstallOutcome {
    Installed,
    Occupied,
    Rejected,
}

impl RadioRegisters {
    /// Report whether one bounded hardware key-table entry is currently valid.
    pub fn mac_key_entry_is_valid(&self, index: u8) -> Option<bool> {
        if index >= KEY_ENTRY_COUNT {
            return None;
        }
        let validity = self
            .peripherals
            .wifi_mac_crypto_control
            .key_valid_bitmap()
            .read()
            .valid_entries()
            .bits();
        Some(validity & (1_u32 << index) != 0)
    }

    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init`.
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &self.peripherals.wifi_mac_crypto_control;
        super::svd::zero_based_field_write::mac_crypto_interface_control(
            control,
            0,
            0x0003_0000,
            0,
        );
        super::svd::zero_based_field_write::mac_crypto_interface_control(
            control,
            1,
            0x0003_0000,
            0,
        );
        super::svd::zero_based_field_write::mac_crypto_interface_control(control, 2, 0, 0);
        super::svd::zero_register_write::clear_mac_crypto_init_aux(control);
        super::svd::zero_register_write::clear_mac_crypto_policy(control);
    }

    /// Install one six-word STA CCMP image into an invalid hardware entry.
    ///
    /// SOURCE: complete `libpp.a::hal_crypto_clr_key_entry`,
    /// `hal_crypto_set_key_entry`, `hal_crypto_is_key_valid`, and the reachable
    /// `hal_crypto_enable(STA, CCMP, true, false)` branch.
    pub fn install_sta_ccmp_key_entry(
        &mut self,
        index: u8,
        words: [u32; PROGRAMMED_CCMP_WORDS],
    ) -> MacKeyInstallOutcome {
        assert!(index < KEY_ENTRY_COUNT);
        let valid_bit = 1_u32 << index;
        let validity = self
            .peripherals
            .wifi_mac_crypto_control
            .key_valid_bitmap()
            .read()
            .valid_entries()
            .bits();
        if validity & valid_bit != 0 {
            return MacKeyInstallOutcome::Occupied;
        }

        self.clear_mac_key_entry_words(index);
        let table = &self.peripherals.wifi_mac_key_table;
        for (word, value) in words.into_iter().enumerate() {
            super::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                usize::from(index) * KEY_ENTRY_WORDS + word,
                value,
            );
        }

        let control = &self.peripherals.wifi_mac_crypto_control;
        super::svd::zero_based_field_write::mac_crypto_key_valid_bitmap(
            control,
            validity | valid_bit,
        );
        super::svd::zero_based_field_write::mac_crypto_interface_control(
            control,
            STA_INTERFACE,
            0x0003_0103,
            0,
        );
        control
            .policy_control()
            .modify(|_, w| w.ordinary_enable_clear_unknown().set(0));
        control
            .interface_control(STA_INTERFACE)
            .modify(|_, w| w.mode_high_unknown().set(0));
        device_fence();

        if control.key_valid_bitmap().read().valid_entries().bits() & valid_bit == 0 {
            MacKeyInstallOutcome::Rejected
        } else {
            MacKeyInstallOutcome::Installed
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: u8) {
        assert!(index < KEY_ENTRY_COUNT);
        let control = &self.peripherals.wifi_mac_crypto_control;
        let validity = control.key_valid_bitmap().read().valid_entries().bits();
        super::svd::zero_based_field_write::mac_crypto_key_valid_bitmap(
            control,
            validity & !(1_u32 << index),
        );
        self.clear_mac_key_entry_words(index);
        device_fence();
    }

    fn clear_mac_key_entry_words(&mut self, index: u8) {
        let table = &self.peripherals.wifi_mac_key_table;
        for word in 0..KEY_ENTRY_WORDS {
            super::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                usize::from(index) * KEY_ENTRY_WORDS + word,
                0,
            );
        }
    }
}
