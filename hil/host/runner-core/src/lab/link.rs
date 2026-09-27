//! Wi-Fi link vocabulary shared by the laboratory configuration and the
//! scenarios that select a fixture link.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhyExpectation {
    He20,
    Ht20,
    Ht40,
}

impl PhyExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::He20 => "he20",
            Self::Ht20 => "ht20",
            Self::Ht40 => "ht40",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HtGuardIntervalExpectation {
    #[default]
    Any,
    Long,
    Short,
}

impl HtGuardIntervalExpectation {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Long => "long",
            Self::Short => "short",
        }
    }
}

/// Management frame protection the station fixture's access point offers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementFrameProtection {
    #[default]
    Disabled,
    /// The access point is capable of protection (MFPC).
    Optional,
    /// The access point requires protection (MFPC and MFPR).
    Required,
}

impl ManagementFrameProtection {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }

    /// Whether a station capable of protection negotiates it with this
    /// access point.
    pub const fn negotiated(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// How one scenario uses the shared Wi-Fi laboratory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiLabUse {
    /// The link the station fixture must provide, when the scenario names one.
    pub link: Option<PhyExpectation>,
    /// Management frame protection of the station fixture's access point.
    pub management_frame_protection: ManagementFrameProtection,
    /// The target runs the access point; the fixture follows its channel.
    pub access_point: bool,
}
