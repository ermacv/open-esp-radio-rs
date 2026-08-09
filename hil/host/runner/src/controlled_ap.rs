//! Lifetime-safe access to the repository-controlled HIL access point.

use std::{fs, process::Command};

use crate::Result;

const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";
const INSTALLED_HE20_CONFIG: &str = "/etc/open-radio/hostapd-he20.conf";

/// Restores the operator's managed interface on every normal or error return.
pub(crate) struct ControlledAp;

impl ControlledAp {
    pub(crate) fn start() -> Result<Self> {
        if let Err(error) = helper_action("start-he20") {
            let _ = helper_action("managed");
            return Err(error);
        }
        Ok(Self)
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        helper_action("stop")
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        helper_action("start-he20")
    }
}

impl Drop for ControlledAp {
    fn drop(&mut self) {
        let _ = helper_action("managed");
    }
}

pub(crate) fn require_station_credentials_environment() -> Result<()> {
    let _ssid = environment_value("OPEN_RADIO_HIL_STA_SSID", "OPEN_RADIO_STA_SSID")?;
    let _passphrase = environment_value("OPEN_RADIO_HIL_STA_PASSWORD", "OPEN_RADIO_STA_PASSWORD")?;
    Ok(())
}

pub(crate) fn require_controlled_ap_credentials_environment() -> Result<()> {
    let ssid = environment_value("OPEN_RADIO_HIL_STA_SSID", "OPEN_RADIO_STA_SSID")?;
    let passphrase = environment_value("OPEN_RADIO_HIL_STA_PASSWORD", "OPEN_RADIO_STA_PASSWORD")?;
    let installed = fs::read_to_string(INSTALLED_HE20_CONFIG).map_err(|error| {
        format!(
            "cannot read installed controlled-AP profile `{INSTALLED_HE20_CONFIG}`: {error}; \
             reinstall the HIL host fixture"
        )
    })?;
    let profile_ssid = required_profile_value(&installed, "ssid")?;
    let profile_passphrase = required_profile_value(&installed, "wpa_passphrase")?;
    if ssid != profile_ssid || passphrase != profile_passphrase {
        return Err(
            "HIL network credentials do not match the installed controlled-AP profile; \
             provision the target with that profile or reinstall the HIL host fixture"
                .into(),
        );
    }
    Ok(())
}

fn environment_value(primary: &str, fallback: &str) -> Result<String> {
    std::env::var(primary)
        .or_else(|_| std::env::var(fallback))
        .map_err(|_| {
            format!(
                "controlled AP credentials must be supplied through `{primary}` or `{fallback}`"
            )
            .into()
        })
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
        return Err(format!("controlled-AP profile defines `{key}` more than once").into());
    }
    if value.is_empty() {
        return Err(format!("controlled-AP profile defines an empty `{key}`").into());
    }
    Ok(value)
}

fn helper_action(action: &str) -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", NETWORK_HELPER, action])
        .status()?;
    if !status.success() {
        return Err(format!("controlled AP helper `{action}` failed with {status}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn reads_exact_profile_values_without_accepting_comments_or_prefixes() {
        let profile = "# ssid=ignored\nssid=open-radio\nssid_suffix=ignored\nwpa_passphrase=test-passphrase\n";
        assert_eq!(
            required_profile_value(profile, "ssid").unwrap(),
            "open-radio"
        );
        assert_eq!(
            required_profile_value(profile, "wpa_passphrase").unwrap(),
            "test-passphrase"
        );
    }

    #[test]
    fn rejects_missing_empty_and_ambiguous_profile_values() {
        assert!(required_profile_value("channel=11\n", "ssid").is_err());
        assert!(required_profile_value("ssid=\n", "ssid").is_err());
        assert!(required_profile_value("ssid=one\nssid=two\n", "ssid").is_err());
    }

    #[test]
    fn installed_profile_path_is_absolute_and_not_checkout_specific() {
        assert!(Path::new(INSTALLED_HE20_CONFIG).is_absolute());
    }
}
