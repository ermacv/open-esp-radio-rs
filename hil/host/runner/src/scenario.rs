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
        }
    }

    fn plan(&self) -> Plan {
        match self {
            Self::Wifi(scenario) => scenario.plan(),
            Self::Bluetooth(scenario) => scenario.plan(),
            Self::System(scenario) => scenario.plan(),
            Self::Ieee802154(scenario) => scenario.plan(),
        }
    }

    fn validate_control(&self, control: &Self) -> Result<()> {
        match (self, control) {
            (Self::Wifi(experiment), Self::Wifi(control)) => experiment.validate_control(control),
            _ => Err("only Wi-Fi scenarios define a controlled comparison".into()),
        }
    }
}

impl Family {
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
        }
    }
}

/// The fixture services a selection needs together.
pub(crate) fn requirements(selected: &[&Scenario]) -> Requirements {
    Requirements::union(selected.iter().map(|scenario| scenario.requirements()))
}

#[cfg(test)]
mod tests;
