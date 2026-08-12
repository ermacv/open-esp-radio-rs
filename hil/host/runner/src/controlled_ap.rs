//! Lifetime-safe access to the repository-controlled HIL access point.

use std::{fs, process::Command, thread, time::Duration};

use zeroize::Zeroizing;

use crate::{
    Result,
    lab_config::{OpenWrtConfig, StationConfig, StationFixtureConfig},
    network_helper,
    scenario::PhyExpectation,
};

const INSTALLED_HE20_CONFIG: &str = "/etc/open-radio/hostapd-he20.conf";

/// Restores the selected AP frontier on every normal or error return.
pub(crate) enum ControlledAp {
    Local(PhyExpectation),
    OpenWrt(OpenWrtControlledAp),
    External,
}

pub(crate) struct OpenWrtControlledAp {
    config: OpenWrtConfig,
    radio: String,
    phy: PhyExpectation,
    stopped: bool,
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
                    return Err("the local fixture helper owns only `wlan0`".into());
                }
                require_local_ap_credentials(station, phy)?;
                let action = match phy {
                    PhyExpectation::He20 => "start-he20",
                    PhyExpectation::Ht40 => "start-ht40",
                    PhyExpectation::Ht20 => {
                        return Err("the local fixture has no qualified HT20 profile".into());
                    }
                };
                if let Err(error) = helper_action(action) {
                    let _ = helper_action("managed");
                    return Err(error);
                }
                Ok(Self::Local(phy))
            }
            StationFixtureConfig::OpenWrt(openwrt) => {
                let radio = require_openwrt_ap_credentials(station, openwrt)?;
                openwrt_action(openwrt, &radio, "up")?;
                wait_for_openwrt_interface(openwrt, phy)?;
                Ok(Self::OpenWrt(OpenWrtControlledAp {
                    config: openwrt.clone(),
                    radio,
                    phy,
                    stopped: false,
                }))
            }
            StationFixtureConfig::External => {
                require_station_credentials(station)?;
                Ok(Self::External)
            }
        }
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        match self {
            Self::Local(_) => helper_action("stop"),
            Self::OpenWrt(ap) => {
                openwrt_action(&ap.config, &ap.radio, "down")?;
                ap.stopped = true;
                Ok(())
            }
            Self::External => Err("an external station fixture cannot be stopped by HIL".into()),
        }
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        match self {
            Self::Local(phy) => helper_action(match phy {
                PhyExpectation::He20 => "start-he20",
                PhyExpectation::Ht40 => "start-ht40",
                PhyExpectation::Ht20 => {
                    return Err("the local fixture has no qualified HT20 profile".into());
                }
            }),
            Self::OpenWrt(ap) => {
                openwrt_action(&ap.config, &ap.radio, "up")?;
                wait_for_openwrt_interface(&ap.config, ap.phy)?;
                ap.stopped = false;
                Ok(())
            }
            Self::External => Err("an external station fixture cannot be restarted by HIL".into()),
        }
    }
}

impl Drop for ControlledAp {
    fn drop(&mut self) {
        match self {
            Self::Local(_) => {
                let _ = helper_action("managed");
            }
            Self::OpenWrt(ap) if ap.stopped => {
                let _ = openwrt_action(&ap.config, &ap.radio, "up");
                let _ = wait_for_openwrt_interface(&ap.config, ap.phy);
            }
            Self::OpenWrt(_) | Self::External => {}
        }
    }
}

pub(crate) fn require_station_credentials(station: &StationConfig) -> Result<()> {
    let _credentials = station.credentials();
    Ok(())
}

pub(crate) fn doctor_local() -> Result<()> {
    network_helper::doctor()
}

fn require_local_ap_credentials(station: &StationConfig, phy: PhyExpectation) -> Result<()> {
    let (ssid, passphrase) = station.credentials();
    let profile = match phy {
        PhyExpectation::He20 => INSTALLED_HE20_CONFIG,
        PhyExpectation::Ht40 => "/etc/open-radio/hostapd-ht40.conf",
        PhyExpectation::Ht20 => return Err("the local fixture has no HT20 profile".into()),
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
        return Err(
            "HIL network credentials do not match the installed controlled-AP profile; \
             provision the target with that profile or reinstall the HIL host fixture"
                .into(),
        );
    }
    Ok(())
}

fn require_openwrt_ap_credentials(
    station: &StationConfig,
    openwrt: &OpenWrtConfig,
) -> Result<String> {
    // Resolve the UCI radio from the configured credentials rather than from
    // the transient netdev. The netdev intentionally disappears while the AP
    // is down, but this stored owner must still be able to restore it.
    let script = r#"set -eu; for section in $(uci -q show wireless | sed -n 's/^wireless\.\([^.=]*\)=wifi-iface$/\1/p'); do radio=$(uci -q get wireless.$section.device || true); ssid=$(uci -q get wireless.$section.ssid || true); key=$(uci -q get wireless.$section.key || true); printf '%s\t%s\t%s\n' "$radio" "$ssid" "$key"; done"#;
    let mut output = openwrt_command(openwrt, script)?;
    if !output.status.success() {
        return Err(format!(
            "cannot read the controlled OpenWrt AP profile: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let profile = Zeroizing::new(String::from_utf8(core::mem::take(&mut output.stdout))?);
    let (ssid, passphrase) = station.credentials();
    let mut matched = None;
    for line in profile.lines() {
        let mut fields = line.split('\t');
        let radio = fields.next().unwrap_or_default();
        let profile_ssid = fields.next().unwrap_or_default();
        let profile_passphrase = fields.next().unwrap_or_default();
        if fields.next().is_some() {
            return Err("OpenWrt AP profile returned malformed data".into());
        }
        if ssid == profile_ssid && passphrase == profile_passphrase {
            if matched.is_some() || !matches!(radio, "radio0" | "radio1") {
                return Err("OpenWrt AP profile does not identify one safe radio owner".into());
            }
            matched = Some(radio.to_owned());
        }
    }
    matched.ok_or_else(|| {
        "HIL network credentials do not match the controlled OpenWrt AP profile".into()
    })
}

fn openwrt_action(config: &OpenWrtConfig, radio: &str, action: &str) -> Result<()> {
    if !matches!(radio, "radio0" | "radio1") {
        return Err("controlled OpenWrt AP has an invalid radio owner".into());
    }
    let action = match action {
        "up" => "up",
        "down" => "down",
        _ => return Err("unsupported OpenWrt AP action".into()),
    };
    let script = format!("set -eu; wifi {action} {radio}");
    let output = openwrt_command(config, &script)?;
    if !output.status.success() {
        return Err(format!(
            "controlled OpenWrt AP action failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn wait_for_openwrt_interface(config: &OpenWrtConfig, phy: PhyExpectation) -> Result<()> {
    let expected_width = match phy {
        PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
        PhyExpectation::Ht40 => 40,
    };
    for _ in 0..50 {
        let output = openwrt_command(
            config,
            &format!(
                "iw dev {} info 2>/dev/null | grep -Fq 'width: {expected_width} MHz'",
                config.wireless_interface
            ),
        )?;
        if output.status.success() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "controlled OpenWrt AP did not restore the required {expected_width} MHz interface"
    )
    .into())
}

fn openwrt_command(config: &OpenWrtConfig, script: &str) -> Result<std::process::Output> {
    Ok(Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?)
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
        .args(["-n", network_helper::PATH, action])
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
