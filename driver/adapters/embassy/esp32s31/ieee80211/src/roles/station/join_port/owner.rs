use open_esp_radio_esp32s31_wifi_mac::init::{
    StaLinkRxPolicyHardware, configure_sta_link_receive_policy,
};

use super::{Esp32s31StaJoinRadio, Esp32s31StaJoinStation, Esp32s31StaJoinStorage};

/// Complete production ESP32-S31 STA join port.
pub struct Esp32s31StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, O> {
    pub(super) radio: Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T>,
    pub(super) storage: Esp32s31StaJoinStorage<'scratch, O>,
    pub(super) station: Esp32s31StaJoinStation,
}

#[cfg_attr(test, allow(dead_code))]
impl<'hardware, 'transmit, 'scratch, H, R, T, O>
    Esp32s31StaJoinPort<'hardware, 'transmit, 'scratch, H, R, T, O>
{
    pub const fn new(
        radio: Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T>,
        storage: Esp32s31StaJoinStorage<'scratch, O>,
        station: Esp32s31StaJoinStation,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
        }
    }

    pub fn into_receive(self) -> R {
        self.radio.receive
    }

    /// Install the selected peer address into the pre-connected RX filter.
    pub fn prepare_authentication(&mut self)
    where
        H: StaLinkRxPolicyHardware,
    {
        configure_sta_link_receive_policy(self.radio.hardware, self.station.access_point.bssid);
    }
}
