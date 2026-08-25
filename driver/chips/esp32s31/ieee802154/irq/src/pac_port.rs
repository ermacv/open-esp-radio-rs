//! Production interrupt-port glue for the restricted ESP32-S31 PAC owner.

use open_esp_radio_esp32s31_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSnapshot as PacInterruptSnapshot,
};

use crate::{Ieee802154InterruptPort, Ieee802154InterruptSnapshot};

impl Ieee802154InterruptSnapshot for PacInterruptSnapshot {
    #[inline]
    fn raw_event_bits(&self) -> u16 {
        self.events().bits()
    }

    #[inline]
    fn raw_rx_abort_reason_code(&self) -> u8 {
        self.rx_abort_reason_code().unwrap_or(0)
    }

    #[inline]
    fn raw_tx_abort_reason_code(&self) -> u8 {
        self.tx_abort_reason_code().unwrap_or(0)
    }

    #[inline]
    fn ed_rss_code(&self) -> i8 {
        self.ed_rss_code().unwrap_or(0)
    }

    #[inline]
    fn cca_busy(&self) -> bool {
        self.cca_busy().unwrap_or(false)
    }
}

impl Ieee802154InterruptPort for PacInterruptRegisters {
    type Snapshot = PacInterruptSnapshot;

    #[inline]
    fn status(&mut self) -> Self::Snapshot {
        self.sample_interrupt()
    }

    #[inline]
    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_interrupt(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::Ieee802154InterruptRegisters;

    use crate::{Ieee802154InterruptPort, Ieee802154InterruptSnapshot};

    #[test]
    fn restricted_pac_owner_satisfies_the_production_port_contract() {
        fn require_port<Port: Ieee802154InterruptPort>()
        where
            Port::Snapshot: Ieee802154InterruptSnapshot,
        {
        }

        require_port::<Ieee802154InterruptRegisters>();
    }
}
