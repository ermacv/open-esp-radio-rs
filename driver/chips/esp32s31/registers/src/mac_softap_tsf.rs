//! Safe generated-PAC ownership for the recovered SoftAP TSF stop edge.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence};

impl RadioRegisters {
    /// Stop the SoftAP TSF domain using the complete vendor leaf transaction.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::hal_disable_softap_tsf`
    /// clears bits 31 and 30 of `0x2010_d860` with one read-modify-write.
    /// Bit 31 is the TSF enable. Bit 30 is deliberately left unnamed by the
    /// recovered SVD because no complete enable leaf proves its semantics.
    pub fn stop_softap_tsf(&mut self) {
        self.peripherals
            .wifi_mac_aux_tsf_control
            .softap_control()
            .modify(|_, writer| {
                writer
                    .tsf_enable()
                    .clear_bit()
                    .high_control_unknown()
                    .clear_bit()
            });
        device_fence();
    }

    /// Return the SoftAP control image for diagnostics and HIL comparison.
    pub fn softap_tsf_control_image(&self) -> u32 {
        self.peripherals
            .wifi_mac_aux_tsf_control
            .softap_control()
            .read()
            .bits()
    }
}
