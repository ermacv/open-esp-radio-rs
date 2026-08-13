//! Scoped ownership of the laptop Wi-Fi interface as one AP test client.

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::{Result, lab_config::AccessPointConfig, network_helper};

pub(crate) fn doctor() -> Result<()> {
    network_helper::doctor()
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
            .args(["-n", network_helper::PATH, "client"])
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
        .args(["-n", network_helper::PATH, "managed"])
        .status()?;
    if !status.success() {
        return Err(format!("controlled-client restore failed with {status}").into());
    }
    Ok(())
}
