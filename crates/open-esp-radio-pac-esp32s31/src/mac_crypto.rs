//! Generated-PAC ownership for finite STA CCMP key-table transactions.

use super::{device_fence, RadioRegisters};

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
    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init`.
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &self.peripherals.wifi_mac_crypto_control;
        // SAFETY: all five complete full-word images and their order come from
        // the complete recovered leaf.
        unsafe {
            control
                .interface_control(0)
                .write_with_zero(|w| w.bits(0x0003_0000));
            control
                .interface_control(1)
                .write_with_zero(|w| w.bits(0x0003_0000));
            control.interface_control(2).write_with_zero(|w| w.bits(0));
            control.init_aux_unknown().write_with_zero(|w| w.bits(0));
            control.policy_control().write_with_zero(|w| w.bits(0));
        }
    }

    /// Install one six-word STA CCMP image into an invalid hardware entry.
    ///
    /// SOURCE: complete `_oracles/libpp.a::hal_crypto_clr_key_entry`,
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
            .bits();
        if validity & valid_bit != 0 {
            return MacKeyInstallOutcome::Occupied;
        }

        self.clear_mac_key_entry_words(index);
        let table = &self.peripherals.wifi_mac_key_table;
        for (word, value) in words.into_iter().enumerate() {
            // SAFETY: the bounded entry and six-word loop select an evidenced
            // table word, and the complete recovered leaf stores whole words.
            unsafe {
                table
                    .entry_word(usize::from(index) * KEY_ENTRY_WORDS + word)
                    .write_with_zero(|w| w.value().bits(value));
            }
        }

        let control = &self.peripherals.wifi_mac_crypto_control;
        // SAFETY: preserve the complete previously sampled validity image and
        // set only this bounded entry bit, exactly as the recovered leaf.
        unsafe {
            control
                .key_valid_bitmap()
                .write_with_zero(|w| w.bits(validity | valid_bit));
            control
                .interface_control(STA_INTERFACE)
                .write_with_zero(|w| w.bits(0x0003_0103));
        }
        let policy = control.policy_control();
        let policy_image = policy.read().bits() & 0xffc0_003f;
        // SAFETY: complete STA CCMP enable performs this preserved-image write.
        unsafe { policy.write_with_zero(|w| w.bits(policy_image)) };
        let interface = control.interface_control(STA_INTERFACE);
        let interface_image = interface.read().bits() & 0x3fff_ffff;
        // SAFETY: complete ordinary enable clears only the generated high
        // two-bit mode field after publishing the initial image.
        unsafe { interface.write_with_zero(|w| w.bits(interface_image)) };
        device_fence();

        if control.key_valid_bitmap().read().bits() & valid_bit == 0 {
            MacKeyInstallOutcome::Rejected
        } else {
            MacKeyInstallOutcome::Installed
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: u8) {
        assert!(index < KEY_ENTRY_COUNT);
        let control = &self.peripherals.wifi_mac_crypto_control;
        let validity = control.key_valid_bitmap().read().bits();
        // SAFETY: the bounded bit is the SVD-described validity entry and the
        // complete clear leaf writes the preserved bitmap image.
        unsafe {
            control
                .key_valid_bitmap()
                .write_with_zero(|w| w.bits(validity & !(1_u32 << index)));
        }
        self.clear_mac_key_entry_words(index);
        device_fence();
    }

    fn clear_mac_key_entry_words(&mut self, index: u8) {
        let table = &self.peripherals.wifi_mac_key_table;
        for word in 0..KEY_ENTRY_WORDS {
            // SAFETY: index validation and the ten-word bound reproduce the
            // complete 0x28-byte-stride clear leaf.
            unsafe {
                table
                    .entry_word(usize::from(index) * KEY_ENTRY_WORDS + word)
                    .write_with_zero(|w| w.value().bits(0));
            }
        }
    }
}
