//! Production interrupt-port glue for the ESP32-S31 HAL interrupt owner.

use oer_esp32s31_hal::ieee802154::mac::{
    Ieee802154EventMask, Ieee802154EventObservationError, Ieee802154InterruptOwner,
    Ieee802154InterruptSnapshot, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortReasonObservation,
};

use crate::{InterruptPort, InterruptSnapshot};

impl InterruptSnapshot for Ieee802154InterruptSnapshot {
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

impl InterruptPort for Ieee802154InterruptOwner {
    type Snapshot = Ieee802154InterruptSnapshot;

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
