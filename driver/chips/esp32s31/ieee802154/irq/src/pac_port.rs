//! Production interrupt-port glue for the restricted ESP32-S31 PAC owner.

use open_esp_radio_esp32s31_pac::{
    Ieee802154EventMask, Ieee802154EventObservationError,
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSnapshot as PacInterruptSnapshot, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortReasonObservation,
};

use crate::{InterruptPort, InterruptSnapshot};

impl InterruptSnapshot for PacInterruptSnapshot {
    #[inline]
    fn event_classification(&self) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        self.event_classification()
    }

    #[inline]
    fn rx_abort_reason(&self) -> Option<Ieee802154RxAbortReasonObservation> {
        self.rx_abort_reason()
    }

    #[inline]
    fn tx_abort_reason(&self) -> Option<Ieee802154TxAbortReasonObservation> {
        self.tx_abort_reason()
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

impl InterruptPort for PacInterruptRegisters {
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
mod tests;
