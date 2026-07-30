//! Ownership boundary for the complete cold COEX/PTI transaction.

use open_esp_radio_pac_esp32s31::ColdRadioRegisters;

/// The four OSI coexistence event numbers queried by complete cold init.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MacCoexEvent {
    Event1 = 1,
    Event3 = 3,
    Event10 = 10,
    Event15 = 15,
}

/// One byte returned by `_coex_pti_get`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacCoexPti(u8);

impl MacCoexPti {
    pub const fn from_osi_value(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// All callback samples consumed by the complete cold COEX setter graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdCoexPti {
    rx_ack: MacCoexPti,
    wifi_default: MacCoexPti,
    tb: [MacCoexPti; 7],
    beamforming: [MacCoexPti; 3],
    multi_target: [MacCoexPti; 2],
}

pub trait MacCoexPtiSource {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti;
}

impl MacColdCoexPti {
    /// Query the platform in the exact callback order of complete `hal_init`
    /// and `hal_set_ofdma_sequence_pti`.
    pub fn query<S: MacCoexPtiSource>(source: &mut S) -> Self {
        let rx_ack = source.mac_coex_pti(MacCoexEvent::Event3);
        let wifi_default = source.mac_coex_pti(MacCoexEvent::Event15);
        let tb = [
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event1),
        ];
        let beamforming = [
            source.mac_coex_pti(MacCoexEvent::Event1),
            source.mac_coex_pti(MacCoexEvent::Event3),
            source.mac_coex_pti(MacCoexEvent::Event3),
        ];
        let multi_target = [
            source.mac_coex_pti(MacCoexEvent::Event10),
            source.mac_coex_pti(MacCoexEvent::Event10),
        ];
        Self {
            rx_ack,
            wifi_default,
            tb,
            beamforming,
            multi_target,
        }
    }
}

pub trait MacColdCoexHardware {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti);
}

impl MacColdCoexHardware for ColdRadioRegisters {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti) {
        self.initialize_mac_coex(
            pti.rx_ack.value(),
            pti.wifi_default.value(),
            pti.tb.map(MacCoexPti::value),
            pti.beamforming.map(MacCoexPti::value),
            pti.multi_target.map(MacCoexPti::value),
        );
    }
}
