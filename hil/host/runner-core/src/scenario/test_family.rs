//! A minimal scenario family for family-independent core tests.

use serde::{Deserialize, Serialize};

use super::{Plan, ScenarioFamily};
use crate::{Result, image::ImageClass};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum TestFamily {
    System(TestWorkload),
    /// A repository Wi-Fi document kept opaque: cross-process tests compare
    /// its procedure with the independent evaluator's reading of the file.
    Wifi(OpaqueWifi),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OpaqueWifi {
    image: ImageClass,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum TestWorkload {
    BootSmoke,
    /// A network workload with one optional check, for requirement and
    /// named-check tests.
    Network {
        #[serde(default)]
        station_network: bool,
        #[serde(default)]
        silence: bool,
        #[serde(default)]
        load: u32,
    },
}

impl ScenarioFamily for TestFamily {
    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn plan(&self) -> Plan {
        let workload = match self {
            Self::System(workload) => workload,
            Self::Wifi(wifi) => {
                let mut plan = Plan::target_only(wifi.image);
                plan.requirements.station_network = true;
                return plan;
            }
        };
        match workload {
            TestWorkload::BootSmoke => Plan::target_only(ImageClass::BootSmoke),
            TestWorkload::Network {
                station_network,
                silence,
                load: _,
            } => {
                let mut plan = Plan::target_only(ImageClass::Correctness);
                plan.requirements.station_network = *station_network;
                plan.checks.push("udp.rx.target-rate");
                if *silence {
                    plan.checks.push("udp.rx.maximum-silence");
                }
                plan
            }
        }
    }
}

/// Parse a test-family document.
pub(crate) fn scenario(text: &str) -> super::Scenario<TestFamily> {
    super::Scenario::from_toml(text, std::path::Path::new("test.toml")).unwrap()
}

/// A catalog with a boot scenario and two network scenarios, one of which
/// publishes an additional check.
pub(crate) fn catalog() -> super::Catalog<TestFamily> {
    super::Catalog::new(
        [
            "schema = 5\nid = \"throughput\"\ndescription = \"rate only\"\ntags = [\"he20\"]\n[system]\nkind = \"network\"\nstation_network = true\n",
            "schema = 5\nid = \"boot-smoke\"\ndescription = \"boot\"\n[system]\nkind = \"boot-smoke\"\n",
            "schema = 5\nid = \"silence\"\ndescription = \"rate and silence\"\ntags = [\"he20\"]\n[system]\nkind = \"network\"\nstation_network = true\nsilence = true\n",
        ]
        .into_iter()
        .map(scenario)
        .collect(),
    )
}
