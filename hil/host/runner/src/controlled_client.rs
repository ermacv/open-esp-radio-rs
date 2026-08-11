//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::{Result, lab_config::AccessPointConfig};

const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";
const REQUIRED_CAPABILITIES: &[u8] = b"schema=1 client=1 managed=1\n";

pub(crate) fn doctor() -> Result<()> {
    let output = Command::new("sudo")
        .args(["-n", NETWORK_HELPER, "capabilities"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "controlled-client helper is unavailable through non-interactive sudo: {}",
            output.status
        )
        .into());
    }
    if output.stdout != REQUIRED_CAPABILITIES {
        return Err(format!(
            "controlled-client helper has incompatible capabilities: expected `{}`, got `{}`",
            String::from_utf8_lossy(REQUIRED_CAPABILITIES).trim(),
            String::from_utf8_lossy(&output.stdout).trim(),
        )
        .into());
    }
    Ok(())
}

pub(crate) struct ControlledClient {
    restored: bool,
}

impl ControlledClient {
    pub(crate) fn connect(config: &AccessPointConfig) -> Result<Self> {
        let (ssid, passphrase) = config.credentials();
        let mut input = Zeroizing::new(Vec::with_capacity(
            ssid.len() + passphrase.len() + config.client_cidr().len() + 3,
        ));
        input.extend_from_slice(ssid.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(passphrase.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(config.client_cidr().as_bytes());
        input.push(b'\n');

        let mut child = Command::new("sudo")
            .args(["-n", NETWORK_HELPER, "client"])
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

    pub(crate) fn restore(mut self) -> Result<()> {
        restore_managed()?;
        self.restored = true;
        Ok(())
    }
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
        .args(["-n", NETWORK_HELPER, "managed"])
        .status()?;
    if !status.success() {
        return Err(format!("controlled-client restore failed with {status}").into());
    }
    Ok(())
}
