//! Diagnostic-only AP observation owner.

use crate::roles::access_point::Esp32s31AccessPointControlObservation;
use open_esp_radio_esp32s31_wifi_ap::{
    engine::Esp32s31ApEngineObservation, mac::Esp32s31ApMacObservation,
};

/// Value-only AP protocol evidence emitted at the terminal owner edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessPointTerminalObservation {
    pub control: Esp32s31AccessPointControlObservation,
    pub mac: Esp32s31ApMacObservation,
    pub engine: Esp32s31ApEngineObservation,
}

/// Non-owning terminal observer. Implementations receive facts after the AP
/// protocol is quiescent and cannot influence scheduling or hardware state.
pub trait AccessPointTerminalObserver: Sync {
    fn observe(&self, observation: AccessPointTerminalObservation);
}

/// External storage for accumulated AP observations for one role epoch.
///
/// The AP processor borrows this storage exclusively and returns it at the
/// terminal owner edge. The large value therefore never becomes part of the
/// active/parked protocol state machine. Functional RX progress never depends
/// on this value.
#[derive(Default)]
pub struct AccessPointObservationStorage {
    pub(crate) observation: Esp32s31AccessPointControlObservation,
}

impl AccessPointObservationStorage {
    #[cfg(feature = "diagnostics")]
    pub(crate) fn reset(&mut self) {
        self.observation = Esp32s31AccessPointControlObservation::default();
    }
}
