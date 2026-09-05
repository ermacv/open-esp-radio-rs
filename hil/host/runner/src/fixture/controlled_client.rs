//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use std::{
    io::Write as _,
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
    pub(crate) fn connect(config: &AccessPointConfig) -> Result<Self> {
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

        let mut child = Command::new("sudo")
            .args(["-n", crate::fixture::network_helper::PATH, "client"])
            .stdin(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("controlled-client helper has no stdin")?
            .write_all(&input)?;
        let status = child.wait()?;
        if !status.success() {
            let _ = restore_managed();
            return Err(format!("controlled-client helper failed with {status}").into());
        }
        Ok(Self { restored: false })
    }

    /// Return the BSSID owned by the AP under test without changing the
    /// controlled client's managed-mode lifetime.
    pub(crate) fn bssid(&self) -> Result<String> {
        let output = Command::new("iw").args(["dev", "wlan0", "link"]).output()?;
        if !output.status.success() {
            return Err(format!(
                "cannot query controlled-client BSSID: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
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
        return Err(format!("controlled client reported invalid BSSID `{bssid}`").into());
    }
    Ok(bssid.to_ascii_lowercase())
}

impl Drop for ControlledClient {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_managed();
        }
    }
}

fn restore_managed() -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", crate::fixture::network_helper::PATH, "managed"])
        .status()?;
    if !status.success() {
        return Err(format!("controlled-client restore failed with {status}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
