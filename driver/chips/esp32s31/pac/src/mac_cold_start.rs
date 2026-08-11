//! Generated-PAC ownership for the cold MAC handshake prefix.

#![forbid(unsafe_code)]

use super::{ColdRadioRegisters, MacInterruptMask};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeOutcome {
    pub samples: u32,
    pub value: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdHandshakeTimeout {
    pub samples: u32,
    pub observed: u32,
}

impl ColdRadioRegisters {
    /// Request cold MAC initialization, wait for READY, then mask and clear
    /// every MAC event.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`, offsets
    /// `0x00..0x3a`. The blob waits forever; this source-owned form adds the
    /// caller-supplied finite sample limit without changing the successful
    /// hardware order.
    pub fn begin_mac_cold_start(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdHandshakeOutcome, MacColdHandshakeTimeout> {
        let handshake = self.registers.peripherals.wifi_mac_cold_handshake.control();
        handshake.modify(|_, w| w.request().set_bit());

        let mut samples = 0;
        let value = loop {
            let value = handshake.read().bits();
            if value & 1 != 0 {
                break value;
            }
            samples += 1;
            if samples >= sample_limit {
                return Err(MacColdHandshakeTimeout {
                    samples,
                    observed: value,
                });
            }
        };

        let interrupt = &self.interrupts.wifi_mac_interrupt;
        super::generated::mac_interrupt_enable(interrupt, MacInterruptMask::NONE);
        super::generated::mac_interrupt_clear(
            interrupt,
            super::generated::MacInterruptClearImage::new(u32::MAX),
        );

        Ok(MacColdHandshakeOutcome { samples, value })
    }
}
