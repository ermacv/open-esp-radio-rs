//! The pinned dependencies a local checkout can replace in a snapshot.

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dependency_parses_from_its_id_only() {
        for dependency in Dependency::ALL {
            assert_eq!(dependency.id().parse::<Dependency>(), Ok(dependency));
        }
        assert!("tokio".parse::<Dependency>().is_err());
    }
}
