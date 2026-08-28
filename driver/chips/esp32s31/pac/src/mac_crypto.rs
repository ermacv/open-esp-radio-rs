//! Generated-PAC ownership for finite Wi-Fi CCMP key-table transactions.

#![forbid(unsafe_code)]

use super::{MacInterface, MacKeyEntryIndex, WifiRadioRegisters, device_fence, svd};

const KEY_ENTRY_WORDS: usize = 10;
const PROGRAMMED_CCMP_WORDS: usize = 6;
const KEY_ENTRY_COUNT: usize = 25;

fn key_entry_validity(control: &svd::WifiMacCryptoControl) -> [bool; KEY_ENTRY_COUNT] {
    let validity = control.key_valid_bitmap().read();
    [
        validity.entry_0_valid().bit(),
        validity.entry_1_valid().bit(),
        validity.entry_2_valid().bit(),
        validity.entry_3_valid().bit(),
        validity.entry_4_valid().bit(),
        validity.entry_5_valid().bit(),
        validity.entry_6_valid().bit(),
        validity.entry_7_valid().bit(),
        validity.entry_8_valid().bit(),
        validity.entry_9_valid().bit(),
        validity.entry_10_valid().bit(),
        validity.entry_11_valid().bit(),
        validity.entry_12_valid().bit(),
        validity.entry_13_valid().bit(),
        validity.entry_14_valid().bit(),
        validity.entry_15_valid().bit(),
        validity.entry_16_valid().bit(),
        validity.entry_17_valid().bit(),
        validity.entry_18_valid().bit(),
        validity.entry_19_valid().bit(),
        validity.entry_20_valid().bit(),
        validity.entry_21_valid().bit(),
        validity.entry_22_valid().bit(),
        validity.entry_23_valid().bit(),
        validity.entry_24_valid().bit(),
    ]
}

fn publish_key_entry_validity(
    control: &svd::WifiMacCryptoControl,
    validity: [bool; KEY_ENTRY_COUNT],
) {
    super::svd::zero_based_field_write::publish_mac_crypto_key_valid_entries(
        control,
        validity[0],
        validity[1],
        validity[2],
        validity[3],
        validity[4],
        validity[5],
        validity[6],
        validity[7],
        validity[8],
        validity[9],
        validity[10],
        validity[11],
        validity[12],
        validity[13],
        validity[14],
        validity[15],
        validity[16],
        validity[17],
        validity[18],
        validity[19],
        validity[20],
        validity[21],
        validity[22],
        validity[23],
        validity[24],
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKeyInstallOutcome {
    Installed,
    Occupied,
    Rejected,
}

impl WifiRadioRegisters {
    /// Report whether one bounded hardware key-table entry is currently valid.
    pub fn mac_key_entry_is_valid(&self, index: MacKeyEntryIndex) -> bool {
        key_entry_validity(&self.peripherals.wifi_mac.wifi_mac_crypto_control)[index.get() as usize]
    }

    /// Establish the common cold hardware-crypto bypass state.
    ///
    /// SOURCE: complete pinned
    /// `libpp.a[hal_crypto.o]::hal_crypto_init`.
    pub fn initialize_mac_crypto_bypass(&mut self) {
        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
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
        index: MacKeyEntryIndex,
        words: &[u32; PROGRAMMED_CCMP_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_ccmp_key_entry(MacInterface::Station, index, words)
    }

    /// Install one six-word AP CCMP image into an invalid hardware entry.
    ///
    /// SOURCE: complete `wDev_Insert_KeyEntry` passes interface one to the
    /// same `hal_crypto_enable` transaction. AP group keys occupy logical
    /// slots 1..=4; the first associated AP peer observed hardware slot 8.
    pub fn install_ap_ccmp_key_entry(
        &mut self,
        index: MacKeyEntryIndex,
        words: &[u32; PROGRAMMED_CCMP_WORDS],
    ) -> MacKeyInstallOutcome {
        self.install_ccmp_key_entry(MacInterface::AccessPoint, index, words)
    }

    fn install_ccmp_key_entry(
        &mut self,
        interface: MacInterface,
        index: MacKeyEntryIndex,
        words: &[u32; PROGRAMMED_CCMP_WORDS],
    ) -> MacKeyInstallOutcome {
        let index = index.get();
        let interface = interface.bits() as usize;
        let mut validity = key_entry_validity(&self.peripherals.wifi_mac.wifi_mac_crypto_control);
        if validity[index as usize] {
            return MacKeyInstallOutcome::Occupied;
        }

        self.clear_mac_key_entry_words(index);
        let table = &self.peripherals.wifi_mac.wifi_mac_key_table;
        for (word, value) in words.iter().copied().enumerate() {
            super::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                index as usize * KEY_ENTRY_WORDS + word,
                value,
            );
        }

        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
        validity[index as usize] = true;
        publish_key_entry_validity(control, validity);
        super::svd::zero_based_field_write::mac_crypto_interface_control(
            control,
            interface,
            0x0003_0103,
            0,
        );
        control
            .policy_control()
            .modify(|_, w| w.ordinary_enable_clear_unknown().set(0));
        control
            .interface_control(interface)
            .modify(|_, w| w.mode_high_unknown().set(0));
        device_fence();

        if !key_entry_validity(control)[index as usize] {
            MacKeyInstallOutcome::Rejected
        } else {
            MacKeyInstallOutcome::Installed
        }
    }

    /// Invalidate and zero all ten words of one hardware key entry.
    pub fn clear_mac_key_entry(&mut self, index: MacKeyEntryIndex) {
        let index = index.get();
        let control = &self.peripherals.wifi_mac.wifi_mac_crypto_control;
        let mut validity = key_entry_validity(control);
        validity[index as usize] = false;
        publish_key_entry_validity(control, validity);
        self.clear_mac_key_entry_words(index);
        device_fence();
    }

    fn clear_mac_key_entry_words(&mut self, index: u32) {
        let table = &self.peripherals.wifi_mac.wifi_mac_key_table;
        for word in 0..KEY_ENTRY_WORDS {
            super::svd::zero_based_field_write::mac_key_table_entry_word(
                table,
                index as usize * KEY_ENTRY_WORDS + word,
                0,
            );
        }
    }
}
