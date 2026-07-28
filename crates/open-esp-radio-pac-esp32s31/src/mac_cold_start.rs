//! Generated-PAC ownership for the cold MAC handshake prefix.

use super::RadioRegisters;

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

impl RadioRegisters {
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
        let handshake = self.peripherals.wifi_mac_cold_handshake.control();
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

        let interrupt = &self.peripherals.wifi_mac_interrupt;
        // SAFETY: both are complete full-register images from the recovered
        // prefix; CLEAR is write-only and accepts the sampled event bitmap.
        unsafe {
            interrupt
                .enable()
                .write_with_zero(|w| w.event_mask().bits(0));
            interrupt
                .clear()
                .write_with_zero(|w| w.events().bits(u32::MAX));
        }

        Ok(MacColdHandshakeOutcome { samples, value })
    }
}
