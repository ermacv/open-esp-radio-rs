//! Narrow register capability for one PHY channel transaction.

use core::cell::RefMut;

use open_esp_radio_esp32s31_pac::WifiRadioRegisters;

use crate::SharedPhyAccess;

/// Temporary channel-programming borrow from the unique [`crate::Radio`] owner.
///
/// Dropping this value ends both mutable borrows; it does not consume or split
/// the radio owner. No PAC owner or generic register accessor is exposed.
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
enum ChannelRegisters<'radio> {
    Owned(&'radio mut WifiRadioRegisters),
    Published(RefMut<'radio, WifiRadioRegisters>),
}

#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
impl ChannelRegisters<'_> {
    fn get(&self) -> &WifiRadioRegisters {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }

    fn get_mut(&mut self) -> &mut WifiRadioRegisters {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }
}

pub struct RadioChannelHal<'radio, P> {
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) platform: &'radio mut P,
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    registers: ChannelRegisters<'radio>,
}

impl<'radio, P> RadioChannelHal<'radio, P> {
    pub(crate) fn from_owned(
        platform: &'radio mut P,
        registers: &'radio mut WifiRadioRegisters,
    ) -> Self {
        Self {
            platform,
            registers: ChannelRegisters::Owned(registers),
        }
    }

    pub(crate) fn from_published(
        platform: &'radio mut P,
        registers: RefMut<'radio, WifiRadioRegisters>,
    ) -> Self {
        Self {
            platform,
            registers: ChannelRegisters::Published(registers),
        }
    }
}

impl<P> crate::sealed::SharedPhyAccess for RadioChannelHal<'_, P> {
    fn pac(&self) -> &open_esp_radio_esp32s31_pac::RadioPhyRegisters {
        self.registers.get().radio_phy()
    }

    fn pac_mut(&mut self) -> &mut open_esp_radio_esp32s31_pac::RadioPhyRegisters {
        self.registers.get_mut().radio_phy_mut()
    }
}

impl<P> SharedPhyAccess for RadioChannelHal<'_, P> {}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    /// Borrow the platform through the channel transaction.
    ///
    /// Callers remain constrained by their generic platform traits; this does
    /// not expose the radio PAC owner.
    #[doc(hidden)]
    pub fn platform_mut(&mut self) -> &mut P {
        self.platform
    }

    pub fn set_agc_enabled(&mut self, enabled: bool) {
        crate::phy_agc::set_enabled(self.registers.get_mut().radio_phy_mut(), enabled);
    }

    pub fn start_frequency_switch(&mut self, frequency_index: u8) {
        crate::phy_frequency::start_channel_switch(
            self.registers.get_mut().radio_phy_mut(),
            frequency_index,
        );
    }

    pub fn clear_frequency_switch(&mut self) {
        crate::phy_frequency::clear_channel_switch(self.registers.get_mut().radio_phy_mut());
    }

    pub fn frequency_ready(&mut self) -> bool {
        crate::phy_frequency::sample_frequency_ready(self.registers.get_mut().radio_phy_mut())
    }

    pub fn configure_nrx(&mut self, frequency_mhz: u16) {
        crate::phy_frequency::configure_nrx_frequency(
            self.registers.get_mut().radio_phy_mut(),
            u32::from(frequency_mhz),
        );
    }

    pub fn configure_rx_compensation(&mut self) {
        crate::phy_agc::configure_rx_compensation(self.registers.get_mut().radio_phy_mut());
    }

    pub fn publish_tx_cap(&mut self, value: u8) {
        crate::phy_frequency::publish_tx_cap(self.registers.get_mut().radio_phy_mut(), value);
    }

    pub fn configure_channel_cbw(&mut self, cbw: u8) {
        crate::phy_frequency::configure_channel_cbw(
            self.registers.get_mut().radio_phy_mut(),
            u32::from(cbw),
        );
    }

    pub fn clear_dc_memory(&mut self) {
        crate::phy_agc::clear_dc_memory(self.registers.get_mut().radio_phy_mut());
    }

    pub fn table_memory_base_index(&self) -> u8 {
        crate::phy_memory::read_table_memory_base_index(self.registers.get().radio_phy())
    }

    pub fn program_gain_memory_entry(&mut self, words: [u32; 3], index: u8) {
        crate::phy_memory::program_gain_memory_entry(
            self.registers.get_mut().radio_phy_mut(),
            words,
            index,
        );
    }

    pub fn request_mac_stop(&mut self) {
        crate::wifi_mac::WifiMacHal::from_owned(self.registers.get_mut()).request_channel_stop();
    }

    pub fn mac_active_state(&mut self) -> u8 {
        crate::wifi_mac::WifiMacHal::from_owned(self.registers.get_mut()).channel_active_state()
    }

    pub fn restart_mac(&mut self) -> u8 {
        crate::wifi_mac::WifiMacHal::from_owned(self.registers.get_mut())
            .restart_after_channel_switch()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn set_bbpll_calibration_enabled(&mut self, enabled: bool) {
        crate::phy_i2c::configure_bbpll_calibration(self, enabled);
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn configure_bss_cbw(&mut self, cbw: u8) {
        crate::phy_frequency::configure_bss_cbw(self.registers.get_mut().radio_phy_mut(), cbw);
    }
}
