//! The product's single connected-station maintenance request owner.

use oer_esp32s31_ieee80211_runtime::roles::station::maintenance::Requests;
pub use oer_esp32s31_ieee80211_runtime::roles::station::maintenance::{
    PauseError, PauseOperation, PauseReport, PauseTimeline, TrackingConfig, TrackingReport,
    TrackingStatus, timeline,
};

pub(super) static REQUESTS: Requests = Requests::new();

/// Stop and resume the standalone connected datapath without reassociation.
/// Access only checks the physical handoff. Tracking also executes currently due
/// PHY work; a missing tracking outcome means no work was due. Neither operation
/// grants shared RF access. Only one request may run in a connected standalone STA.
pub async fn station_pause_round_trip(
    operation: PauseOperation,
) -> Result<PauseReport, PauseError> {
    REQUESTS.request(operation).await
}

/// Enable or disable observation-driven tracking for this connected STA epoch.
/// Each selected operation still performs the full physical pause/restoration.
/// No dynamic configuration survives leaving the connected role. The next
/// epoch starts from the policy carried by `RadioConfig`.
pub fn configure_station_tracking(config: Option<TrackingConfig>) -> Result<(), PauseError> {
    REQUESTS.configure_tracking(config)
}

/// Ask the enabled service to acquire a new PHY sensor observation. This is
/// an event, not a supplied temperature, calibration completion or RF grant.
pub fn request_station_temperature_observation() -> Result<(), PauseError> {
    REQUESTS.request_temperature_observation()
}

pub fn station_tracking_status() -> TrackingStatus {
    REQUESTS.tracking_status()
}

pub fn station_tracking_report() -> TrackingReport {
    REQUESTS.tracking_report()
}
