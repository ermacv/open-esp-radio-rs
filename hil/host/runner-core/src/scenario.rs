//! Family-agnostic HIL scenario envelope and catalog.
//!
//! A scenario document is a common header plus exactly one radio-family
//! table. The family owns its typed workload, its validation and the
//! [`Plan`] projection every family-independent consumer reads: the firmware
//! image, laboratory requirements, target initialization and named checks.

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    Result,
    image::ImageClass,
    lab::{link::WifiLabUse, requirements::Requirements},
    session::Settings,
};

mod catalog;
pub mod identity;

pub use catalog::Catalog;

pub const SCENARIO_SCHEMA: u16 = 5;

/// Whether review may transfer a whole-scenario observation across images.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransferPolicy {
    /// Permit a reviewed functional transfer with unchanged relevant inputs.
    #[default]
    UnchangedFunctionalContract,
    /// Require the same application bytes for this whole-scenario guarantee.
    IdenticalImage,
}

/// Identity and review policy shared by every scenario family.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub schema: u16,
    pub id: String,
    pub description: String,
    #[serde(default = "one_repetition")]
    pub repetitions: u8,
    /// Review policy for the whole scenario; execution is unaffected.
    #[serde(default)]
    pub transfer: TransferPolicy,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Chips whose device under test runs the scenario; the esp32s31 when
    /// the document names none.
    #[serde(
        default = "default_targets",
        skip_serializing_if = "is_default_targets"
    )]
    pub targets: Vec<String>,
    /// Why the scenario cannot run on the current firmware. The runner
    /// refuses to select it, before any image build, with this reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
    /// Sample the program counter of the image's harts during the
    /// workload's measured window. It is part of the procedure: a profiled
    /// scenario is a diagnostic, never a throughput or timing measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRequest>,
}

/// A scenario's request for a program-counter profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ProfileRequest {
    pub harts: ProfileHarts,
    pub period_us: u32,
}

/// The harts a profile samples.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileHarts {
    Both,
    Core0,
    Core1,
}

impl ProfileRequest {
    /// Periods the sampler supports: short enough to resolve a window,
    /// long enough that sampling stays a small part of each hart's time.
    pub const PERIOD_US: std::ops::RangeInclusive<u32> = 100..=100_000;

    /// The protocol's arming command.
    pub fn control(self) -> oer_hil_protocol::telemetry::ProfileControl {
        oer_hil_protocol::telemetry::ProfileControl::Arm {
            harts: match self.harts {
                ProfileHarts::Both => oer_hil_protocol::telemetry::ProfileHarts::Both,
                ProfileHarts::Core0 => oer_hil_protocol::telemetry::ProfileHarts::Core0,
                ProfileHarts::Core1 => oer_hil_protocol::telemetry::ProfileHarts::Core1,
            },
            period_us: self.period_us,
        }
    }
}

const fn one_repetition() -> u8 {
    1
}

fn default_targets() -> Vec<String> {
    vec![crate::lab::config::DEFAULT_TARGET.to_owned()]
}

fn is_default_targets(targets: &[String]) -> bool {
    targets == default_targets()
}

const HEADER_FIELDS: [&str; 9] = [
    "schema",
    "id",
    "description",
    "repetitions",
    "transfer",
    "tags",
    "targets",
    "unsupported",
    "profile",
];

impl Header {
    /// Read the family-independent identity of a recorded scenario snapshot.
    ///
    /// Recorded runs keep the schema they executed under; only the identity
    /// fields shared by every schema are interpreted here.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let serde_json::Value::Object(mut document) = serde_json::from_slice(bytes)? else {
            return Err("scenario snapshot is not a table".into());
        };
        document.retain(|key, _| HEADER_FIELDS.contains(&key.as_str()));
        let header: Self = serde_json::from_value(serde_json::Value::Object(document))?;
        if !valid_id(&header.id) {
            return Err(format!("invalid recorded scenario id `{}`", header.id).into());
        }
        Ok(header)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != SCENARIO_SCHEMA {
            return Err(format!(
                "scenario schema {} is unsupported (expected {SCENARIO_SCHEMA})",
                self.schema
            )
            .into());
        }
        if !valid_id(&self.id) {
            return Err(format!("invalid scenario id `{}`", self.id).into());
        }
        if self.targets.is_empty() || self.targets.iter().any(|chip| !valid_id(chip)) {
            return Err(format!("scenario `{}` names no valid target chip", self.id).into());
        }
        if self.description.trim().is_empty() {
            return Err("scenario description is empty".into());
        }
        if self
            .unsupported
            .as_ref()
            .is_some_and(|reason| reason.trim().is_empty())
        {
            return Err(format!("scenario `{}` gives an empty unsupported reason", self.id).into());
        }
        if let Some(profile) = self.profile {
            let range = ProfileRequest::PERIOD_US;
            bounded(
                profile.period_us,
                *range.start(),
                *range.end(),
                "profile.period-us",
            )?;
            // Sampling perturbs timing, so a profile never shapes a
            // performance or qualification figure.
            if self
                .tags
                .iter()
                .any(|tag| tag == "performance" || tag == "qualification")
            {
                return Err(format!(
                    "scenario `{}` profiles a performance or qualification scenario; profile a \
                     diagnostic copy of it instead",
                    self.id
                )
                .into());
            }
        }
        bounded(self.repetitions, 1, 20, "repetitions")
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// What family-independent execution, planning and evidence need to know.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub image: ImageClass,
    pub requirements: Requirements,
    pub settings: Settings,
    /// Named observations the workload publishes. Their acceptance limits
    /// remain in the family's criteria; listing a check is not a verdict.
    pub checks: Vec<&'static str>,
    pub wifi: WifiLabUse,
}

impl Plan {
    /// A plan for a scenario that uses no laboratory service beyond the
    /// target's serial port and firmware.
    pub fn target_only(image: ImageClass) -> Self {
        Self {
            image,
            requirements: Requirements::default(),
            settings: Settings::default(),
            checks: Vec::new(),
            wifi: WifiLabUse::default(),
        }
    }
}

/// The typed family table of a scenario document.
///
/// Implementations deserialize from a map with exactly one family key, so a
/// document naming no family or several families is rejected.
pub trait ScenarioFamily: Clone + Debug + Eq + Serialize + DeserializeOwned {
    /// Validate value ranges and relations inside the family table.
    fn validate(&self) -> Result<()>;

    fn plan(&self) -> Plan;

    /// Whether an image of the planned class that reports `capabilities` can
    /// run this scenario. The class alone suffices unless the family names
    /// a role the class's image may omit.
    fn served_by(&self, _capabilities: &crate::session::DeviceCapabilities) -> bool {
        true
    }
}

/// One validated scenario document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scenario<F> {
    pub header: Header,
    pub family: F,
    source: PathBuf,
}

impl<F: ScenarioFamily> Scenario<F> {
    /// Parse and validate one TOML document.
    pub fn from_toml(text: &str, source: &Path) -> Result<Self> {
        let document: toml::Table =
            toml::from_str(text).map_err(|error| format!("{}: {error}", source.display()))?;
        let value = serde_json::to_value(document)?;
        let mut scenario =
            Self::from_value(value).map_err(|error| format!("{}: {error}", source.display()))?;
        scenario.source = source.to_owned();
        scenario
            .validate()
            .map_err(|error| format!("{}: {error}", source.display()))?;
        Ok(scenario)
    }

    /// Read a recorded scenario snapshot.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let scenario = Self::from_value(serde_json::from_slice(bytes)?)?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn from_value(value: serde_json::Value) -> Result<Self> {
        let serde_json::Value::Object(mut document) = value else {
            return Err("scenario document is not a table".into());
        };
        let mut header = serde_json::Map::new();
        for key in HEADER_FIELDS {
            if let Some(value) = document.remove(key) {
                header.insert(key.to_owned(), value);
            }
        }
        Ok(Self {
            header: serde_json::from_value(serde_json::Value::Object(header))?,
            family: serde_json::from_value(serde_json::Value::Object(document))?,
            source: PathBuf::new(),
        })
    }

    pub fn validate(&self) -> Result<()> {
        self.header.validate()?;
        self.family.validate()?;
        if self.header.profile.is_some() && !self.image().samples_program_counter() {
            return Err(format!(
                "scenario {} requests a profile, but its image {} does not sample the program \
                 counter",
                self.header.id,
                self.image().id()
            )
            .into());
        }
        // The station-exit image admits air observers beside saturated
        // traffic. An observer must never shape a qualification or gated
        // figure, so only diagnostic scenarios may select it.
        if self.image() == ImageClass::DiagnosticStationExit {
            let tagged = |tag: &str| self.header.tags.iter().any(|known| known == tag);
            if !tagged("diagnostic") || tagged("qualification") || tagged("performance") {
                return Err(format!(
                    "scenario {} selects {} without being a diagnostic-only scenario",
                    self.header.id,
                    ImageClass::DiagnosticStationExit.id()
                )
                .into());
            }
        }
        Ok(())
    }

    pub fn id(&self) -> &str {
        &self.header.id
    }

    pub fn repetitions(&self) -> u8 {
        self.header.repetitions
    }

    pub fn plan(&self) -> Plan {
        self.family.plan()
    }

    pub fn image(&self) -> ImageClass {
        self.family.plan().image
    }

    pub fn requirements(&self) -> Requirements {
        self.family.plan().requirements
    }

    pub fn source(&self) -> &Path {
        &self.source
    }
}

/// Scenarios serialize as the document they were read from: the header
/// fields beside the single family table.
impl<F: ScenarioFamily> Serialize for Scenario<F> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let (serde_json::Value::Object(mut document), serde_json::Value::Object(family)) = (
            serde_json::to_value(&self.header).map_err(S::Error::custom)?,
            serde_json::to_value(&self.family).map_err(S::Error::custom)?,
        ) else {
            return Err(S::Error::custom("scenario parts must serialize as tables"));
        };
        document.extend(family);
        document.serialize(serializer)
    }
}

/// Reject a value outside `minimum..=maximum`, naming its field.
pub fn bounded<T>(value: T, minimum: T, maximum: T, field: &str) -> Result<()>
where
    T: PartialOrd + std::fmt::Display,
{
    if value < minimum || value > maximum {
        return Err(format!("{field}={value} is outside {minimum}..={maximum}").into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_family;
