//! The A/B experiment a run belongs to, recorded in its manifest.
//!
//! `cargo hil ab` runs every repetition of an arm with [`EXPERIMENT_ENV`]
//! naming the experiment, the arm and the arm's variant; the runner copies it
//! into the run manifest, so the variant a figure was measured on stays with
//! the run however long after the comparison it is read.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Names the experiment a runner process runs for, as JSON.
pub const EXPERIMENT_ENV: &str = "OER_HIL_EXPERIMENT";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub id: String,
    pub arm: Arm,
    pub variant: Variant,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arm {
    A,
    B,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
        })
    }
}

/// What an arm builds: a commit of the repository and the local checkouts
/// that replace its pinned dependencies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// The repository commit, resolved.
    pub commit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<DependencyOverride>,
}

/// A local checkout that replaces a pinned dependency, as the
/// `ESP_HAL_ROOT`, `EMBASSY_ROOT` and `OPEN_RADIO_XARXA_ROOT` roots do.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyOverride {
    pub dependency: Dependency,
    pub path: PathBuf,
    /// The checkout's commit when the experiment started.
    pub commit: String,
    /// Whether it had uncommitted changes; the source snapshot keeps them.
    pub dirty: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dependency {
    EspHal,
    Embassy,
    Xarxa,
}

impl Dependency {
    pub const ALL: [Self; 3] = [Self::EspHal, Self::Embassy, Self::Xarxa];

    /// The name the variant syntax and the source snapshot use.
    pub const fn id(self) -> &'static str {
        match self {
            Self::EspHal => "esp-hal",
            Self::Embassy => "embassy",
            Self::Xarxa => "xarxa",
        }
    }

    /// The environment variable that names the override's root.
    pub const fn root_env(self) -> &'static str {
        match self {
            Self::EspHal => "ESP_HAL_ROOT",
            Self::Embassy => "EMBASSY_ROOT",
            Self::Xarxa => "OPEN_RADIO_XARXA_ROOT",
        }
    }
}

impl std::str::FromStr for Dependency {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|dependency| dependency.id() == value)
            .ok_or_else(|| format!("`{value}` is not esp-hal, embassy or xarxa"))
    }
}

impl Experiment {
    /// The experiment this process runs for, if [`EXPERIMENT_ENV`] names one.
    pub fn from_environment() -> Result<Option<Self>> {
        match std::env::var(EXPERIMENT_ENV) {
            Ok(json) => Ok(Some(serde_json::from_str(&json).map_err(|error| {
                format!("{EXPERIMENT_ENV} is not an experiment: {error}")
            })?)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_experiment_round_trips_through_its_environment_form() {
        let experiment = Experiment {
            id: String::from("1790-1"),
            arm: Arm::B,
            variant: Variant {
                commit: String::from("abc"),
                overrides: vec![DependencyOverride {
                    dependency: Dependency::Xarxa,
                    path: PathBuf::from("/src/xarxa"),
                    commit: String::from("ea9385e"),
                    dirty: false,
                }],
            },
        };
        let json = serde_json::to_string(&experiment).unwrap();
        assert!(json.contains(r#""arm":"b""#), "{json}");
        assert!(json.contains(r#""dependency":"xarxa""#), "{json}");
        assert_eq!(
            serde_json::from_str::<Experiment>(&json).unwrap(),
            experiment
        );
        assert_eq!("embassy".parse::<Dependency>(), Ok(Dependency::Embassy));
        assert!("tokio".parse::<Dependency>().is_err());
    }
}
