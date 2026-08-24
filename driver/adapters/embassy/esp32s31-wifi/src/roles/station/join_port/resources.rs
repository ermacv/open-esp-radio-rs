use open_esp_radio_ieee80211::{
    scan::ScanRecord, security::WifiSecurityMode, station::StaAssociationPreference,
};
use open_esp_radio_wifi_sta::request::StationListenInterval;

/// Driver resources borrowed for one join runner lifetime.
pub struct Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T> {
    pub(super) hardware: &'hardware mut H,
    pub(super) receive: R,
    pub(super) transmit: &'transmit mut T,
}

impl<'hardware, 'transmit, H, R, T> Esp32s31StaJoinRadio<'hardware, 'transmit, H, R, T> {
    pub const fn new(hardware: &'hardware mut H, receive: R, transmit: &'transmit mut T) -> Self {
        Self {
            hardware,
            receive,
            transmit,
        }
    }
}

/// Borrowed allocation-free parsing storage and diagnostic observer.
pub struct Esp32s31StaJoinStorage<'scratch, O> {
    pub(super) frame: &'scratch mut [u8],
    pub(super) observer: O,
}

impl<'scratch, O> Esp32s31StaJoinStorage<'scratch, O> {
    pub fn new(frame: &'scratch mut [u8], observer: O) -> Self {
        Self { frame, observer }
    }
}

/// Station and peer policy for one Authentication/Association epoch.
#[derive(Clone, Copy)]
pub struct Esp32s31StaJoinStation {
    pub(super) station_address: [u8; 6],
    pub(super) access_point: ScanRecord,
    pub(super) association_preference: StaAssociationPreference,
    pub(super) listen_interval: u16,
    pub(super) security: WifiSecurityMode,
}

impl Esp32s31StaJoinStation {
    pub const fn new(
        station_address: [u8; 6],
        access_point: ScanRecord,
        association_preference: StaAssociationPreference,
    ) -> Self {
        Self {
            station_address,
            access_point,
            association_preference,
            listen_interval: StationListenInterval::DEFAULT.get(),
            security: WifiSecurityMode::Wpa2Personal,
        }
    }

    pub const fn with_listen_interval(mut self, listen_interval: StationListenInterval) -> Self {
        self.listen_interval = listen_interval.get();
        self
    }

    pub const fn with_security(mut self, security: WifiSecurityMode) -> Self {
        self.security = security;
        self
    }
}
