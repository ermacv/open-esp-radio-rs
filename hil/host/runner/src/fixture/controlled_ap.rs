//! Lifetime-safe access to the repository-controlled HIL access point.

use oer_process::CommandExt as _;
use std::{fs, process::Command};

use crate::{
    Result,
    lab::config::{StationConfig, StationFixtureConfig},
    scenario::PhyExpectation,
};

const INSTALLED_HE20_CONFIG: &str = "/etc/open-radio/hostapd-he20.conf";

/// Restores the selected AP frontier on every normal or error return.
pub(crate) enum ControlledAp {
    Local(PhyExpectation),
    OpenWrt(Box<super::openwrt_ap::AccessPoint>),
    External,
}

impl ControlledAp {
    pub(crate) fn start(
        station: &StationConfig,
        fixture: &StationFixtureConfig,
        phy: PhyExpectation,
    ) -> Result<Self> {
        match fixture {
            StationFixtureConfig::LocalLinux(config) => {
                if config.interface != "wlan0" {
                    return Err(crate::fixture::Error::new(
                        "the local fixture helper owns only `wlan0`",
                    )
                    .into());
                }
                require_local_ap_credentials(station, phy)
                    .map_err(crate::fixture::Error::context)?;
                let action = match phy {
                    PhyExpectation::He20 => "start-he20",
                    PhyExpectation::Ht40 => "start-ht40",
                    PhyExpectation::Ht20 => {
                        return Err(crate::fixture::Error::new(
                            "the local fixture has no qualified HT20 profile",
                        )
                        .into());
                    }
                };
                let owner = Self::Local(phy);
                helper_action(action)?;
                Ok(owner)
            }
            StationFixtureConfig::OpenWrt(openwrt) => Ok(Self::OpenWrt(Box::new(
                super::openwrt_ap::AccessPoint::start(openwrt, station, phy)
                    .map_err(super::Error::context)?,
            ))),
            StationFixtureConfig::External(_) => {
                require_station_credentials(station)?;
                Ok(Self::External)
            }
        }
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        match self {
            Self::Local(_) => helper_action("stop"),
            Self::OpenWrt(ap) => ap.stop(),
            Self::External => Err(crate::fixture::Error::new(
                "an external station fixture cannot be stopped by HIL",
            )
            .into()),
        }
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        match self {
            Self::Local(phy) => helper_action(match phy {
                PhyExpectation::He20 => "start-he20",
                PhyExpectation::Ht40 => "start-ht40",
                PhyExpectation::Ht20 => {
                    return Err(crate::fixture::Error::new(
                        "the local fixture has no qualified HT20 profile",
                    )
                    .into());
                }
            }),
            Self::OpenWrt(ap) => ap.restart(),
            Self::External => Err(crate::fixture::Error::new(
                "an external station fixture cannot be restarted by HIL",
            )
            .into()),
        }
    }
}

impl Drop for ControlledAp {
    fn drop(&mut self) {
        oer_process::cleanup(|| match self {
            Self::Local(_) => {
                crate::fixture::cleanup::record("restore managed Wi-Fi", || {
                    helper_action("managed")
                });
            }
            Self::OpenWrt(_) | Self::External => {}
        });
    }
}

pub(crate) fn require_station_credentials(station: &StationConfig) -> Result<()> {
    let _credentials = station.credentials();
    Ok(())
}

pub(crate) fn doctor_local() -> Result<()> {
    crate::fixture::network_helper::doctor()
}

fn require_local_ap_credentials(station: &StationConfig, phy: PhyExpectation) -> Result<()> {
    let (ssid, passphrase) = station.credentials();
    let profile = match phy {
        PhyExpectation::He20 => INSTALLED_HE20_CONFIG,
        PhyExpectation::Ht40 => "/etc/open-radio/hostapd-ht40.conf",
        PhyExpectation::Ht20 => {
            return Err(crate::fixture::Error::new("the local fixture has no HT20 profile").into());
        }
    };
    let installed = fs::read_to_string(profile).map_err(|error| {
        format!(
            "cannot read installed controlled-AP profile `{profile}`: {error}; \
             reinstall the HIL host fixture"
        )
    })?;
    let profile_ssid = required_profile_value(&installed, "ssid")?;
    let profile_passphrase = required_profile_value(&installed, "wpa_passphrase")?;
    if ssid != profile_ssid || passphrase != profile_passphrase {
        return Err(crate::fixture::Error::new(
            "HIL network credentials do not match the installed controlled-AP profile; \
             provision the target with that profile or reinstall the HIL host fixture",
        )
        .into());
    }
    Ok(())
}

fn required_profile_value<'a>(profile: &'a str, key: &str) -> Result<&'a str> {
    let mut values = profile.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        line.split_once('=')
            .filter(|(candidate, _)| candidate.trim() == key)
            .map(|(_, value)| value.trim())
    });
    let value = values
        .next()
        .ok_or_else(|| format!("controlled-AP profile is missing `{key}`"))?;
    if values.next().is_some() {
        return Err(crate::fixture::Error::new(format!(
            "controlled-AP profile defines `{key}` more than once"
        ))
        .into());
    }
    if value.is_empty() {
        return Err(crate::fixture::Error::new(format!(
            "controlled-AP profile defines an empty `{key}`"
        ))
        .into());
    }
    Ok(value)
}

fn helper_action(action: &str) -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", crate::fixture::network_helper::PATH, action])
        .supervised_status()?;
    if !status.success() {
        return Err(crate::fixture::Error::new(format!(
            "controlled AP helper `{action}` failed with {status}"
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
