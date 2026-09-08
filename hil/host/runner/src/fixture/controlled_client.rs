//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use oer_process::CommandExt as _;
use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::{Result, lab::config::AccessPointConfig};

pub(crate) fn doctor() -> Result<()> {
    crate::fixture::network_helper::doctor()
}

pub(crate) struct ControlledClient {
    restored: bool,
}

impl ControlledClient {
    pub(crate) fn connect(config: &AccessPointConfig, output: &Path) -> Result<Self> {
        std::fs::create_dir_all(output)?;
        let log = std::fs::File::create(output.join("helper.log"))?;
        let (ssid, passphrase) = config.credentials();
        let frequency_mhz = config.frequency_mhz();
        let mut input = Zeroizing::new(Vec::with_capacity(
            ssid.len() + passphrase.len() + config.client_cidr().len() + 9,
        ));
        input.extend_from_slice(ssid.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(passphrase.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(config.client_cidr().as_bytes());
        input.push(b'\n');
        input.extend_from_slice(frequency_mhz.to_string().as_bytes());
        input.push(b'\n');

        let owner = Self { restored: false };
        let mut child = Command::new("sudo")
            .args(["-n", crate::fixture::network_helper::PATH, "client"])
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
        let mut control =
            super::wpa_control::Control::connect(Path::new("/run/open-radio-wpa-control/wlan0"))?;
        control.record_to(std::fs::File::create(output.join("control.jsonl"))?);
        let result = control.wait_connected();
        let state = super::wpa_control::field(&control.last_status, "wpa_state");
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
    pub(crate) fn bssid(&self) -> Result<String> {
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

    pub(crate) fn restore(mut self) -> Result<()> {
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
                crate::fixture::cleanup::record("restore managed Wi-Fi", restore_managed);
            }
        });
    }
}

fn restore_managed() -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", crate::fixture::network_helper::PATH, "managed"])
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
