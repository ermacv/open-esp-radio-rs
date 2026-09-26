//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use oer_process::CommandExt as _;
use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::Result;
use hil_core::lab::config::{AccessPointConfig, StationConfig};

/// The PHY capabilities the laptop station advertises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientPhy {
    /// HT with the adapter's full capabilities.
    Ht,
    /// A Clause 18 ERP-OFDM station without HT, VHT or HE capability.
    NonHt,
}

/// The BSS the laptop joins and the station it presents there.
pub struct ClientNetwork<'a> {
    ssid: &'a str,
    passphrase: &'a str,
    /// The laptop's IPv4 address; a membership-only peer carries none.
    address: Option<String>,
    frequency_mhz: u16,
    phy: ClientPhy,
}

impl<'a> ClientNetwork<'a> {
    /// A traffic client of the target's AP.
    pub fn target_access_point(config: &'a AccessPointConfig, phy: ClientPhy) -> Self {
        let (ssid, passphrase) = config.credentials();
        Self {
            ssid,
            passphrase,
            address: Some(config.client_cidr()),
            frequency_mhz: config.frequency_mhz(),
            phy,
        }
    }

    /// A membership-only station of the laboratory AP on `channel`.
    pub fn station_fixture(station: &'a StationConfig, channel: u8, phy: ClientPhy) -> Self {
        let (ssid, passphrase) = station.credentials();
        Self {
            ssid,
            passphrase,
            address: None,
            frequency_mhz: 2407 + u16::from(channel) * 5,
            phy,
        }
    }

    fn helper_input(&self) -> Zeroizing<Vec<u8>> {
        let mut input = Zeroizing::new(Vec::new());
        for line in [
            self.ssid,
            self.passphrase,
            self.address.as_deref().unwrap_or("none"),
            &self.frequency_mhz.to_string(),
            match self.phy {
                ClientPhy::Ht => "ht",
                ClientPhy::NonHt => "non-ht",
            },
        ] {
            input.extend_from_slice(line.as_bytes());
            input.push(b'\n');
        }
        input
    }
}

pub fn doctor() -> Result<()> {
    crate::fixture::local::network_helper::doctor()
}

pub struct ControlledClient {
    restored: bool,
}

impl ControlledClient {
    pub fn connect(network: &ClientNetwork<'_>, output: &Path) -> Result<Self> {
        std::fs::create_dir_all(output)?;
        let log = std::fs::File::create(output.join("helper.log"))?;
        let input = network.helper_input();
        let owner = Self { restored: false };
        let mut child = Command::new("sudo")
            .args(["-n", crate::fixture::local::network_helper::PATH, "client"])
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .stdin(Stdio::piped())
            .spawn_owned()?;
        child
            .stdin
            .take()
            .ok_or("controlled-client helper has no stdin")?
            .write_all(&input)?;
        let status = child.wait_timeout(Some(std::time::Duration::from_secs(120)))?;
        if !status.success() {
            return Err(crate::fixture::Error::new(format!(
                "controlled-client helper failed with {status}"
            ))
            .into());
        }
        let mut control = crate::fixture::local::wpa_control::Control::connect(Path::new(
            "/run/open-radio-wpa-control/wlan0",
        ))?;
        control.record_to(std::fs::File::create(output.join("control.jsonl"))?);
        let result = control.wait_connected();
        let state = crate::fixture::local::wpa_control::field(&control.last_status, "wpa_state");
        let stage = connection_stage(state);
        std::fs::write(
            output.join("connection.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 1, "connected": result.is_ok(), "last_state": state,
                "last_observed_stage": stage, "last_event": control.last_event,
                "last_failure_event": control.last_failure_event,
                "error": result.as_ref().err().map(|error| error.to_string()),
            }))?,
        )?;
        result.map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
            format!("controlled client connection failed: {error}; last observed stage={stage}, wpa_state={state:?}, last_failure={:?}; diagnostics={}", control.last_failure_event, output.display()).into()
        })?;
        Ok(owner)
    }

    /// Return the BSSID owned by the AP under test without changing the
    /// controlled client's managed-mode lifetime.
    pub fn bssid(&self) -> Result<String> {
        let output = Command::new("iw")
            .args(["dev", "wlan0", "link"])
            .supervised_output()?;
        if !output.status.success() {
            return Err(crate::fixture::Error::new(format!(
                "cannot query controlled-client BSSID: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        parse_bssid(&String::from_utf8(output.stdout)?)
    }

    pub fn restore(mut self) -> Result<()> {
        restore_managed()?;
        self.restored = true;
        Ok(())
    }
}

fn parse_bssid(link: &str) -> Result<String> {
    let Some(bssid) = link.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Connected to ")?
            .split_whitespace()
            .next()
    }) else {
        return Err("controlled client is not associated with an AP".into());
    };
    let valid = bssid.len() == 17
        && bssid.bytes().enumerate().all(|(index, byte)| {
            index % 3 == 2 && byte == b':' || index % 3 != 2 && byte.is_ascii_hexdigit()
        });
    if !valid {
        return Err(crate::fixture::Error::new(format!(
            "controlled client reported invalid BSSID `{bssid}`"
        ))
        .into());
    }
    Ok(bssid.to_ascii_lowercase())
}

impl Drop for ControlledClient {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            if !self.restored {
                hil_core::fixture::cleanup::record("restore managed Wi-Fi", restore_managed);
            }
        });
    }
}

fn restore_managed() -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", crate::fixture::local::network_helper::PATH, "managed"])
        .supervised_status()?;
    if !status.success() {
        return Err(crate::fixture::Error::new(format!(
            "controlled-client restore failed with {status}"
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

fn connection_stage(state: Option<&str>) -> &'static str {
    match state {
        Some("SCANNING") => "discovery",
        Some("AUTHENTICATING") => "authentication",
        Some("ASSOCIATING" | "ASSOCIATED") => "association",
        Some("4WAY_HANDSHAKE" | "GROUP_HANDSHAKE") => "key-negotiation",
        Some("COMPLETED") => "connected",
        _ => "unknown",
    }
}
