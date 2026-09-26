//! The `[ieee802154]` scenario table: IEEE 802.15.4 diagnostics.

use std::path::Path;

use hil_core::{
    context::Context,
    image::ImageClass,
    scenario::{Plan, bounded},
};
use serde::{Deserialize, Serialize};

use crate::{Result, workload::ieee802154};

/// One IEEE 802.15.4 diagnostic. Each kind implies its diagnostic image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Ieee802154Scenario {
    EventStatus(Diagnostic),
    EdEvent(Diagnostic),
    AirCheck(AirCheck),
    PeerExchange(PeerExchange),
    BackgroundMaintenance(BackgroundMaintenance),
}

/// Background PHY maintenance of the running client.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundMaintenance {
    pub boots: u8,
    pub channel: u8,
    /// Tracking periods the session runs.
    pub periods: u8,
    pub policy: MaintenancePolicy,
}

/// Shared PHY tracking admission under test.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaintenancePolicy {
    Vendor,
    Quiesced,
}

impl MaintenancePolicy {
    const fn session(self) -> oer_hil_protocol::Ieee802154SessionMaintenancePolicy {
        match self {
            Self::Vendor => oer_hil_protocol::Ieee802154SessionMaintenancePolicy::Vendor,
            Self::Quiesced => oer_hil_protocol::Ieee802154SessionMaintenancePolicy::Quiesced,
        }
    }
}

/// An exchange with the IEEE 802.15.4 reference peer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerExchange {
    pub boots: u8,
    pub channel: u8,
    /// Frames per direction.
    pub frames: u8,
}

/// The single-device on-air check.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AirCheck {
    pub boots: u8,
    pub channel: u8,
    pub cycles: u8,
    pub energy_scan_micros: u32,
    pub receive_window_millis: u32,
    pub scheduled_lead_micros: u32,
}

impl AirCheck {
    fn request(self) -> oer_hil_protocol::Ieee802154AirCheckRequest {
        oer_hil_protocol::Ieee802154AirCheckRequest {
            channel: self.channel,
            cycles: self.cycles,
            energy_scan_micros: self.energy_scan_micros,
            receive_window_millis: self.receive_window_millis,
            scheduled_lead_micros: self.scheduled_lead_micros,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub boots: u8,
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl Ieee802154Scenario {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::EventStatus(diagnostic) | Self::EdEvent(diagnostic) => {
                bounded(diagnostic.boots, 1, 20, "boots")?;
                bounded(diagnostic.poll_limit, 1, 1_000_000, "poll_limit")?;
                bounded(diagnostic.timer_threshold, 1, 1_000, "timer_threshold")
            }
            Self::BackgroundMaintenance(maintenance) => {
                bounded(maintenance.boots, 1, 20, "boots")?;
                bounded(maintenance.channel, 11, 26, "channel")?;
                bounded(maintenance.periods, 2, 30, "periods")
            }
            Self::PeerExchange(exchange) => {
                bounded(exchange.boots, 1, 20, "boots")?;
                bounded(exchange.channel, 11, 26, "channel")?;
                bounded(
                    exchange.frames,
                    1,
                    oer_hil_protocol::IEEE802154_SESSION_RECORDED_FRAMES as u8,
                    "frames",
                )
            }
            Self::AirCheck(check) => {
                bounded(check.boots, 1, 20, "boots")?;
                if check.request().validate() {
                    Ok(())
                } else {
                    Err("IEEE 802.15.4 air check bounds are outside the wire contract".into())
                }
            }
        }
    }

    pub fn plan(&self) -> Plan {
        let mut plan = Plan::target_only(match self {
            Self::EventStatus(_) => ImageClass::DiagnosticIeee802154EventStatus,
            Self::EdEvent(_) => ImageClass::DiagnosticIeee802154EdEvent,
            Self::AirCheck(_) | Self::PeerExchange(_) | Self::BackgroundMaintenance(_) => {
                ImageClass::DiagnosticIeee802154Radio
            }
        });
        plan.requirements.ieee802154_peer = matches!(self, Self::PeerExchange(_));
        plan
    }

    pub fn run(&self, output: &Path, context: &Context<'_>) -> Result<()> {
        match *self {
            Self::EventStatus(Diagnostic {
                boots,
                poll_limit,
                timer_threshold,
            }) => ieee802154::event_status::run(
                ieee802154::event_status::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
                },
                output,
                context,
            ),
            Self::EdEvent(Diagnostic {
                boots,
                poll_limit,
                timer_threshold,
            }) => ieee802154::ed_event::run(
                ieee802154::ed_event::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
                },
                output,
                context,
            ),
            Self::BackgroundMaintenance(maintenance) => ieee802154::background_maintenance::run(
                ieee802154::background_maintenance::Config {
                    boots: maintenance.boots,
                    channel: maintenance.channel,
                    policy: maintenance.policy.session(),
                    periods: maintenance.periods,
                },
                output,
                context,
            ),
            Self::PeerExchange(exchange) => ieee802154::peer_exchange::run(
                ieee802154::peer_exchange::Config {
                    boots: exchange.boots,
                    channel: exchange.channel,
                    frames: exchange.frames,
                },
                output,
                context,
            ),
            Self::AirCheck(check) => ieee802154::air_check::run(
                ieee802154::air_check::Config {
                    boots: check.boots,
                    request: check.request(),
                },
                output,
                context,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_imply_their_images_and_bound_their_polling() {
        for (kind, image) in [
            ("event-status", ImageClass::DiagnosticIeee802154EventStatus),
            ("ed-event", ImageClass::DiagnosticIeee802154EdEvent),
        ] {
            let scenario: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 10\ntimer_threshold = 2"
            ))
            .unwrap();
            scenario.validate().unwrap();
            assert_eq!(scenario.plan(), Plan::target_only(image));
            let excessive: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 0\ntimer_threshold = 2"
            ))
            .unwrap();
            assert!(excessive.validate().is_err());
        }
    }

    #[test]
    fn the_peer_exchange_needs_the_peer_and_bounds_its_frames() {
        let table = "kind = 'peer-exchange'\nboots = 1\nchannel = 15\nframes = 4";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154Radio);
        assert!(plan.requirements.ieee802154_peer);
        for invalid in ["frames = 0", "frames = 17"] {
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace("frames = 4", invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_air_check_implies_its_image_and_bounds_its_request() {
        let table = "kind = 'air-check'\nboots = 1\nchannel = 15\ncycles = 2\nenergy_scan_micros = 5000\nreceive_window_millis = 200\nscheduled_lead_micros = 20000";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        assert_eq!(
            scenario.plan(),
            Plan::target_only(ImageClass::DiagnosticIeee802154Radio)
        );
        let invalid: Ieee802154Scenario =
            toml::from_str(&table.replace("channel = 15", "channel = 27")).unwrap();
        assert!(invalid.validate().is_err());
    }
}
