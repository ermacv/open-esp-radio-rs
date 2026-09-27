//! The executable scenario catalog: the common envelope over exactly one of
//! the radio families this runner composes.

use std::path::Path;

use hil_core::{
    context::Context,
    lab::requirements::Requirements,
    scenario::{Plan, ScenarioFamily},
};
use hil_wifi::{fixture::prepared::Prepared, scenario::WifiScenario};
use serde::{Deserialize, Serialize};

use crate::Result;

/// The single family table of a scenario document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Family {
    Wifi(WifiScenario),
    Bluetooth(hil_bluetooth::scenario::BluetoothScenario),
    System(hil_system::scenario::SystemScenario),
    Ieee802154(hil_ieee802154::scenario::Ieee802154Scenario),
    Coexistence(crate::coexistence::CoexistenceScenario),
}

impl Family {
    /// The frequency ranges, in kHz, the scenario's radio work occupies; none
    /// for work that never enables a radio. Wi-Fi occupies the channel of its
    /// link in `lab` (`wifi`), IEEE 802.15.4 its channels, and Bluetooth, whose
    /// connections hop across it, the 2.4 GHz band.
    pub(crate) fn air_ranges(
        &self,
        lab: &hil_core::lab::config::LabConfig,
        wifi: hil_core::lab::link::WifiLabUse,
    ) -> Vec<(u64, u64)> {
        use hil_core::lab::lock::{BAND_2G4, Emits, Need, Spectrum};
        use hil_ieee802154::scenario::Ieee802154Scenario as Ieee802154;
        let channel = |channel: u8| Spectrum::ieee802154(channel, Need::None, Emits::None);
        let range = |spectrum: Spectrum| (spectrum.low_khz, spectrum.high_khz);
        match self {
            // The MAC-foundation probes keep RF closed.
            Self::Ieee802154(
                Ieee802154::EventStatus(_) | Ieee802154::EdEvent(_) | Ieee802154::RouteProbe(_),
            ) => Vec::new(),
            Self::Ieee802154(Ieee802154::AirCheck(scenario)) => {
                vec![range(channel(scenario.channel))]
            }
            Self::Ieee802154(Ieee802154::PeerExchange(scenario)) => {
                vec![range(channel(scenario.channel))]
            }
            Self::Ieee802154(Ieee802154::ThreadExchange(scenario)) => {
                vec![range(channel(scenario.channel))]
            }
            Self::Ieee802154(Ieee802154::BackgroundMaintenance(scenario)) => {
                vec![range(channel(scenario.channel))]
            }
            Self::Ieee802154(Ieee802154::ChannelEnergy(scenario)) => vec![
                range(channel(scenario.channel)),
                range(channel(scenario.far_channel)),
            ],
            Self::System(hil_system::scenario::SystemScenario::Watchdog {}) => Vec::new(),
            Self::Wifi(_) => vec![lab.wifi_range_khz(wifi)],
            Self::Bluetooth(_) | Self::Coexistence(_) | Self::System(_) => vec![BAND_2G4],
        }
    }
}

pub(crate) type Scenario = hil_core::scenario::Scenario<Family>;
pub(crate) type Catalog = hil_core::scenario::Catalog<Family>;

impl ScenarioFamily for Family {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Wifi(scenario) => scenario.validate(),
            Self::Bluetooth(scenario) => scenario.validate(),
            Self::System(scenario) => scenario.validate(),
            Self::Ieee802154(scenario) => scenario.validate(),
            Self::Coexistence(scenario) => scenario.validate(),
        }
    }

    fn plan(&self) -> Plan {
        match self {
            Self::Wifi(scenario) => scenario.plan(),
            Self::Bluetooth(scenario) => scenario.plan(),
            Self::System(scenario) => scenario.plan(),
            Self::Ieee802154(scenario) => scenario.plan(),
            Self::Coexistence(scenario) => scenario.plan(),
        }
    }

    fn served_by(&self, features: &oer_hil_protocol::FeatureCapabilities) -> bool {
        match self {
            Self::Bluetooth(scenario) => scenario.served_by(features),
            Self::Wifi(_) | Self::System(_) | Self::Ieee802154(_) | Self::Coexistence(_) => true,
        }
    }
}

impl Family {
    /// The catalog image the IEEE 802.15.4 reference peer board must carry.
    pub(crate) fn ieee802154_peer_image(&self) -> Option<hil_ieee802154::scenario::PeerImage> {
        match self {
            Self::Ieee802154(scenario) => scenario.peer_image(),
            Self::Wifi(_) | Self::Bluetooth(_) | Self::System(_) | Self::Coexistence(_) => None,
        }
    }

    pub(crate) fn run(
        &self,
        output: &Path,
        context: &Context<'_>,
        fixture: &Prepared,
    ) -> Result<()> {
        match self {
            Self::Wifi(scenario) => scenario.run(output, context, fixture),
            Self::Bluetooth(scenario) => scenario.run(output, context),
            Self::System(scenario) => scenario.run(output, context),
            Self::Ieee802154(scenario) => scenario.run(output, context),
            Self::Coexistence(scenario) => scenario.run(output, context),
        }
    }
}

/// The fixture services a selection needs together.
pub(crate) fn requirements(selected: &[&Scenario]) -> Requirements {
    Requirements::union(selected.iter().map(|scenario| scenario.requirements()))
}

#[cfg(test)]
mod tests;
