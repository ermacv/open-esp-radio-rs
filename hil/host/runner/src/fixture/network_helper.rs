//! One versioned contract for the root-owned laptop radio helper.

use std::process::Command;

use crate::Result;

pub(crate) const PATH: &str = "/usr/local/sbin/open-radio-net";
const REQUIRED_CAPABILITIES: &str =
    "schema=5 station_ap=he20,ht40 client=1 observer=ht40 managed=1";

pub(crate) fn doctor() -> Result<()> {
    let output = Command::new("sudo")
        .args(["-n", PATH, "capabilities"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "laptop radio helper is unavailable through non-interactive sudo: {}",
            output.status
        )
        .into());
    }
    require_capabilities(&String::from_utf8(output.stdout)?)
}

fn require_capabilities(capabilities: &str) -> Result<()> {
    if capabilities.trim() != REQUIRED_CAPABILITIES {
        return Err(format!(
            "installed laptop radio helper is incompatible: expected `{REQUIRED_CAPABILITIES}`, got `{}`; reinstall it from this checkout",
            capabilities.trim()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
