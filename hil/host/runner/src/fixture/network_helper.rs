//! One versioned contract for the root-owned laptop radio helper.

use oer_process::CommandExt as _;
use std::process::Command;

use crate::Result;

pub(crate) const PATH: &str = "/usr/local/sbin/open-radio-net";
pub(crate) const ASSOCIATION_TIMEOUT: i32 = 10;
const REQUIRED_CAPABILITIES: &str =
    "schema=6 station_ap=he20,ht40 client=1 observer=ht40 managed=1";

/// Validate the installed command protocol before a selected run can flash or
/// reset the DUT. System-only and remote-only workloads do not need this helper.
pub(crate) fn require_for(
    lab: &crate::lab::config::LabConfig,
    required: crate::lab::requirements::Requirements,
) -> Result<()> {
    if required.local_radio()
        || (required.station_network
            && matches!(
                lab.station_fixture,
                crate::lab::config::StationFixtureConfig::LocalLinux(_)
            ))
    {
        doctor()?;
    }
    Ok(())
}

pub(crate) fn doctor() -> Result<()> {
    let output = Command::new("sudo")
        .args(["-n", PATH, "capabilities"])
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "laptop radio helper is unavailable through non-interactive sudo: {}",
            output.status
        ))
        .into());
    }
    require_capabilities(&String::from_utf8(output.stdout)?)
}

fn require_capabilities(capabilities: &str) -> Result<()> {
    if capabilities.trim() != REQUIRED_CAPABILITIES {
        return Err(crate::fixture::Error::new(format!(
            "installed laptop radio helper is incompatible: expected `{REQUIRED_CAPABILITIES}`, got `{}`; reinstall it from this checkout",
            capabilities.trim()
        )).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
