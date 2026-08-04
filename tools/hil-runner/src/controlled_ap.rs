//! Lifetime-safe access to the repository-controlled HIL access point.

use std::process::Command;

use crate::Result;

const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";

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

pub(crate) fn require_credentials_environment() -> Result<()> {
    let has_ssid = std::env::var_os("OPEN_RADIO_HIL_STA_SSID").is_some()
        || std::env::var_os("OPEN_RADIO_STA_SSID").is_some();
    let has_password = std::env::var_os("OPEN_RADIO_HIL_STA_PASSWORD").is_some()
        || std::env::var_os("OPEN_RADIO_STA_PASSWORD").is_some();
    if !has_ssid || !has_password {
        return Err(
            "controlled AP credentials must be supplied through the normal HIL environment".into(),
        );
    }
    Ok(())
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
