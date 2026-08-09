#![no_std]
#![deny(unsafe_code)]
#![cfg_attr(not(feature = "qualification"), allow(unused_variables))]

//! Concrete ESP32-S31 Embassy radio composition.
//!
//! [`new`] returns one application radio root and the sole owner-holding
//! runner. Board firmware owns credentials, IP policy and sockets; it does not
//! assemble PAC, DMA, ISR or role transactions.

#[cfg(feature = "qualification")]
macro_rules! qualification_event {
    ($($argument:tt)*) => { esp_println::println!($($argument)*) };
}

#[cfg(not(feature = "qualification"))]
macro_rules! qualification_event {
    ($($argument:tt)*) => {{}};
}

mod connected;
mod monitor;
mod station;

#[cfg(feature = "qualification")]
pub use connected::Esp32s31QualificationSnapshot;
pub use connected::Esp32s31WifiDevice;
pub use monitor::Esp32s31MonitorFrames;
pub use station::{
    Esp32s31NewError, Esp32s31Radio, Esp32s31RadioError, Esp32s31RadioInitialization,
    Esp32s31RadioRunner, Esp32s31Wifi, Esp32s31WifiControl, new,
};

/// Board-derived radio identity. Reading eFuse remains an application
/// responsibility; credentials are supplied separately to `start_station`.
pub struct Esp32s31RadioConfig {
    pub(crate) station_mac: open_esp_radio::WifiMacAddress,
    pub(crate) access_point_mac: open_esp_radio::WifiMacAddress,
    pub(crate) calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
    pub(crate) initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
    pub(crate) calibration_record:
        Option<open_esp_radio::esp32s31::phy::phy_cold::PhyCalibrationRecord>,
    pub(crate) maximum_tx_power_quarter_dbm: Option<i8>,
    #[cfg(feature = "qualification")]
    pub(crate) qualification: Option<Esp32s31QualificationHooks>,
}

impl Esp32s31RadioConfig {
    pub const fn new(
        station_mac: open_esp_radio::WifiMacAddress,
        access_point_mac: open_esp_radio::WifiMacAddress,
        calibration: open_esp_radio::esp32s31::phy::PhyCalibrationIdentity,
        initial_channel: open_esp_radio::wifi::ieee80211::channel::WifiChannel,
    ) -> Self {
        Self {
            station_mac,
            access_point_mac,
            calibration,
            initial_channel,
            calibration_record: None,
            maximum_tx_power_quarter_dbm: None,
            #[cfg(feature = "qualification")]
            qualification: None,
        }
    }

    /// Supply a caller-owned retained PHY calibration record. The driver
    /// validates its embedded identity before deciding whether it is reusable.
    pub fn with_calibration_record(
        mut self,
        record: open_esp_radio::esp32s31::phy::phy_cold::PhyCalibrationRecord,
    ) -> Self {
        self.calibration_record = Some(record);
        self
    }

    /// Apply the board/regulatory TX ceiling to the calibrated power profile.
    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = Some(maximum);
        self
    }

    /// Attach value-only, non-blocking qualification observers. This API does
    /// not exist in production builds and grants no register or owner access.
    #[cfg(feature = "qualification")]
    pub const fn with_qualification_hooks(mut self, hooks: Esp32s31QualificationHooks) -> Self {
        self.qualification = Some(hooks);
        self
    }
}

/// Optional HIL observers compiled only into qualification firmware.
#[cfg(feature = "qualification")]
#[derive(Clone, Copy)]
pub struct Esp32s31QualificationHooks {
    pub rx_pipeline: &'static dyn open_esp_radio_esp32s31_wifi_embassy::rx_pipeline_observer::RxPipelineObserver,
    pub aggregate_tx: &'static dyn open_esp_radio_esp32s31_wifi_embassy::aggregate_tx_observer::AggregateTxObserver,
}
