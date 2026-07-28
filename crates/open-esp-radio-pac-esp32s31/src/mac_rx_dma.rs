//! Generated-PAC ownership for the MAC RX descriptor walker.

use super::{device_fence, RadioRegisters};

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
        self.peripherals
            .wifi_mac_rx_dma
            .rx_last_descriptor()
            .read()
            .address_low()
            .bits()
    }

    pub fn mac_rx_next_descriptor_low(&self) -> u32 {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_next_descriptor()
            .read()
            .address_low()
            .bits()
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
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .read()
            .append_descriptor_reload()
            .bit()
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
        // SAFETY: the caller validates the complete DMA address; the recovered
        // ROM leaf publishes the full address image even though the hardware
        // consumes its generated low 20-bit field.
        unsafe {
            self.peripherals
                .wifi_mac_rx_dma
                .rx_descriptor_base()
                .write_with_zero(|w| w.bits(address));
        }
    }

    pub fn publish_mac_rx_walker_enable(&mut self) {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.walker_enable().set_bit());
    }

    pub fn request_mac_rx_descriptor_reload(&mut self) {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.append_descriptor_reload().set_bit());
    }

    pub fn try_enable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        if previous.walker_enable().bit() {
            return false;
        }
        // SAFETY: this full write preserves the exact single-read ROM image.
        unsafe {
            control.write_with_zero(|w| w.bits(previous.bits() | 0x8000_0000));
        }
        device_fence();
        control.read().walker_enable().bit()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        // SAFETY: this full write preserves the exact single-read ROM image
        // while clearing only the generated walker-enable bit.
        unsafe {
            control.write_with_zero(|w| w.bits(previous.bits() & !0x8000_0000));
        }
        device_fence();
        !control.read().walker_enable().bit()
    }
}
