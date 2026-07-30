//! Ownership boundary for the finite cold MAC handshake prefix.

use open_esp_radio_pac_esp32s31::ColdRadioRegisters;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacColdStartError {
    HandshakeTimedOut { samples: u32, observed: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartOutcome {
    pub handshake_samples: u32,
    pub handshake_value: u32,
}

pub trait MacColdHandshakeHardware {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError>;
}

impl MacColdHandshakeHardware for ColdRadioRegisters {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError> {
        self.begin_mac_cold_start(sample_limit)
            .map(|outcome| MacColdStartOutcome {
                handshake_samples: outcome.samples,
                handshake_value: outcome.value,
            })
            .map_err(|timeout| MacColdStartError::HandshakeTimedOut {
                samples: timeout.samples,
                observed: timeout.observed,
            })
    }
}
