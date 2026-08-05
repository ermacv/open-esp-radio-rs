//! Generated-PAC ownership for the MAC RX descriptor walker.

use super::{RadioRegisters, device_fence, generated};

impl RadioRegisters {
    /// Initialize the RX buffer geometry without publishing a descriptor.
    ///
    /// SOURCE: first four RMWs of complete pinned
    /// `libpp.a[hal_mac.o]::mac_rxbuf_init`. Its final descriptor-base store
    /// is deliberately excluded because the RX ring owner publishes it later.
    pub fn initialize_mac_rx_buffer_prefix(&mut self) {
        let dma = &self.peripherals.wifi_mac_rx_dma;
        dma.rx_buffer_limit_unknown()
            .modify(|_, w| unsafe { w.low_unknown().bits(0x000f_ffff) });
        dma.rx_buffer_base_unknown()
            .modify(|_, w| unsafe { w.low_unknown().bits(4) });
        dma.rx_descriptor_high_window()
            .modify(|_, w| unsafe { w.address_high().bits(0x02f0) });
        dma.rx_cold_control_unknown()
            .modify(|_, w| unsafe { w.cold_low_unknown().bits(0) });
    }

    pub fn mac_rx_last_descriptor_low(&self) -> u32 {
        generated::hal_mac_rx_read_rxdscrlast::generated_hal_mac_rx_read_rxdscrlast(
            &self.peripherals.wifi_mac_rx_dma,
        ) & 0x000f_ffff
    }

    pub fn mac_rx_next_descriptor_low(&self) -> u32 {
        generated::hal_mac_rx_read_rxdscrnext::generated_hal_mac_rx_read_rxdscrnext(
            &self.peripherals.wifi_mac_rx_dma,
        ) & 0x000f_ffff
    }

    /// Reconstruct the complete last descriptor pointer exactly as the
    /// vendor leaf does. Ring ownership normally needs only the low index,
    /// but this form keeps the high-window composition available without a
    /// duplicated handwritten register transaction.
    pub fn mac_rx_last_descriptor_address(&self) -> u32 {
        generated::hal_mac_rx_get_last_dscr::generated_hal_mac_rx_get_last_dscr(
            &self.peripherals.wifi_mac_rx_dma,
        )
    }

    pub fn mac_rx_walker_enabled(&self) -> bool {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .read()
            .walker_enable()
            .bit()
    }

    pub fn mac_rx_reload_pending(&self) -> bool {
        generated::hal_mac_rx_is_dscr_reload::generated_hal_mac_rx_is_dscr_reload(
            &self.peripherals.wifi_mac_rx_dma,
        ) != 0
    }

    pub fn set_mac_rx_descriptor_high_window(&mut self, address_high: u16) {
        assert!(address_high <= 0x0fff);
        // SAFETY: the assertion proves the value fits the generated 12-bit
        // field. The RMW preserves the low 20 bits exactly as the ROM leaf.
        self.peripherals
            .wifi_mac_rx_dma
            .rx_descriptor_high_window()
            .modify(|_, w| unsafe { w.address_high().bits(address_high) });
    }

    pub fn write_mac_rx_descriptor_base(&mut self, address: u32) {
        let _ = generated::hal_mac_rx_set_base::generated_hal_mac_rx_set_base(
            &self.peripherals.wifi_mac_rx_dma,
            address,
        );
    }

    pub fn publish_mac_rx_walker_enable(&mut self) {
        let _ = generated::hal_mac_rx_enable::generated_hal_mac_rx_enable(
            &self.peripherals.wifi_mac_rx_dma,
            0,
        );
    }

    pub fn request_mac_rx_descriptor_reload(&mut self) {
        let _ = generated::hal_mac_rx_set_dscr_reload::generated_hal_mac_rx_set_dscr_reload(
            &self.peripherals.wifi_mac_rx_dma,
            0,
        );
    }

    pub fn try_enable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        if previous.walker_enable().bit() {
            return false;
        }
        let _ = generated::hal_mac_rx_enable::generated_hal_mac_rx_enable(
            &self.peripherals.wifi_mac_rx_dma,
            0,
        );
        device_fence();
        control.read().walker_enable().bit()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let _ = generated::hal_mac_rx_disable::generated_hal_mac_rx_disable(
            &self.peripherals.wifi_mac_rx_dma,
            0,
        );
        device_fence();
        !control.read().walker_enable().bit()
    }
}
