//! Safe generated-PAC ownership for the MAC RX descriptor walker.

#![forbid(unsafe_code)]

use open_esp_radio_dma::StableDmaRange;

use super::{RadioRegisters, device_fence, svd};

/// Read-only hardware frontier of the MAC RX descriptor walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxDmaSnapshot {
    pub walker_enabled: bool,
    pub reload_pending: bool,
    pub descriptor_base: u32,
    pub next_descriptor_low: u32,
    pub last_descriptor_low: u32,
}

#[inline(always)]
pub(crate) fn set_walker_enabled(registers: &svd::WifiMacRxDma, enabled: bool) {
    registers.rx_control().modify(|_, writer| {
        if enabled {
            writer.walker_enable().set_bit()
        } else {
            writer.walker_enable().clear_bit()
        }
    });
}

#[inline(always)]
pub(crate) fn read_last_descriptor(registers: &svd::WifiMacRxDma) -> u32 {
    registers.rx_last_descriptor().read().bits()
}

#[inline(always)]
pub(crate) fn read_next_descriptor(registers: &svd::WifiMacRxDma) -> u32 {
    registers.rx_next_descriptor().read().bits()
}

#[inline(always)]
pub(crate) fn write_descriptor_base(registers: &svd::WifiMacRxDma, address: u32) {
    svd::full_register_write::rx_descriptor_base(registers, address);
}

#[inline(always)]
pub(crate) fn read_last_descriptor_address(registers: &svd::WifiMacRxDma) -> u32 {
    let low = registers.rx_last_descriptor().read().address_low().bits();
    let high = registers
        .rx_descriptor_high_window()
        .read()
        .address_high()
        .bits();
    low | (u32::from(high) << 20)
}

#[inline(always)]
pub(crate) fn descriptor_reload_pending(registers: &svd::WifiMacRxDma) -> bool {
    registers
        .rx_control()
        .read()
        .append_descriptor_reload()
        .bit()
}

#[inline(always)]
pub(crate) fn request_descriptor_reload(registers: &svd::WifiMacRxDma) {
    registers
        .rx_control()
        .modify(|_, writer| writer.append_descriptor_reload().set_bit());
}

impl RadioRegisters {
    /// Snapshot the complete published RX walker frontier without changing it.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        let dma = &self.peripherals.wifi_mac_rx_dma;
        let control = dma.rx_control().read();
        MacRxDmaSnapshot {
            walker_enabled: control.walker_enable().bit(),
            reload_pending: control.append_descriptor_reload().bit(),
            descriptor_base: dma.rx_descriptor_base().read().bits(),
            next_descriptor_low: dma.rx_next_descriptor().read().bits() & 0x000f_ffff,
            last_descriptor_low: dma.rx_last_descriptor().read().bits() & 0x000f_ffff,
        }
    }

    /// Initialize the RX buffer geometry without publishing a descriptor.
    ///
    /// SOURCE: first four RMWs of complete pinned
    /// `libpp.a[hal_mac.o]::mac_rxbuf_init`. Its final descriptor-base store
    /// is deliberately excluded because the RX ring owner publishes it later.
    pub fn initialize_mac_rx_buffer_prefix(&mut self) {
        let dma = &self.peripherals.wifi_mac_rx_dma;
        dma.rx_buffer_limit_unknown()
            .modify(|_, w| w.low_unknown().set(0x000f_ffff));
        dma.rx_buffer_base_unknown()
            .modify(|_, w| w.low_unknown().set(4));
        dma.rx_descriptor_high_window()
            .modify(|_, w| w.address_high().set(0x02f0));
        dma.rx_cold_control_unknown()
            .modify(|_, w| w.cold_low_unknown().set(0));
    }

    pub fn mac_rx_last_descriptor_low(&self) -> u32 {
        read_last_descriptor(&self.peripherals.wifi_mac_rx_dma) & 0x000f_ffff
    }

    pub fn mac_rx_next_descriptor_low(&self) -> u32 {
        read_next_descriptor(&self.peripherals.wifi_mac_rx_dma) & 0x000f_ffff
    }

    /// Reconstruct the complete last descriptor pointer exactly as the
    /// vendor leaf does. Ring ownership normally needs only the low index,
    /// but this form keeps the high-window composition available without a
    /// duplicated handwritten register transaction.
    pub fn mac_rx_last_descriptor_address(&self) -> u32 {
        read_last_descriptor_address(&self.peripherals.wifi_mac_rx_dma)
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
        descriptor_reload_pending(&self.peripherals.wifi_mac_rx_dma)
    }

    /// Select the address window carried by a retained descriptor range.
    pub fn set_mac_rx_descriptor_high_window(
        &mut self,
        binding: &StableDmaRange<'_>,
        address_high: u16,
    ) {
        assert!(address_high <= 0x0fff);
        assert_eq!(address_high, (binding.start() >> 20) as u16);
        self.peripherals
            .wifi_mac_rx_dma
            .rx_descriptor_high_window()
            .modify(|_, w| w.address_high().set(address_high));
    }

    /// Publish a descriptor head contained by the retained DMA range.
    pub fn write_mac_rx_descriptor_base(&mut self, binding: &StableDmaRange<'_>, address: u32) {
        assert!(binding.contains(address, 1));
        write_descriptor_base(&self.peripherals.wifi_mac_rx_dma, address);
    }

    /// Start the walker only while its descriptor backing authority is held.
    pub fn publish_mac_rx_walker_enable(&mut self, _binding: &StableDmaRange<'_>) {
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, true);
    }

    /// Ask the live walker to follow newly published descriptor links.
    pub fn request_mac_rx_descriptor_reload(&mut self, _binding: &StableDmaRange<'_>) {
        request_descriptor_reload(&self.peripherals.wifi_mac_rx_dma);
    }

    /// Start and confirm the walker while descriptor authority is retained.
    pub fn try_enable_mac_rx_walker(&mut self, _binding: &StableDmaRange<'_>) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        if previous.walker_enable().bit() {
            return false;
        }
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, true);
        device_fence();
        control.read().walker_enable().bit()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, false);
        device_fence();
        !control.read().walker_enable().bit()
    }
}
