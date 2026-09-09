//! Interactive provisioning hands the foreground terminal directly to sudo.

use std::{path::Path, process::Command};

use crate::Result;

pub(crate) fn run(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // All downloads, patching and compilation happen without root, before
        // the installer takes terminal control for its narrow system writes.
        oer_process::run(
            Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .current_dir(root)
                .args(["xtask", "build", "hostapd"]),
        )?;

        oer_process::run(
            Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .current_dir(root)
                .args([
                    "build",
                    "--locked",
                    "-p",
                    "open-esp-radio-hil-runner",
                    "--bin",
                    "open-radio-probe",
                    "--target-dir",
                    "target/hil/fixture-build",
                ]),
        )?;
        // This is a terminal handoff, not a supervised background workload.
        // exec preserves the foreground process group and standard streams so
        // sudo owns password echo, input and job control. No HIL owners have
        // been acquired; sudo also becomes the command's exit-status authority.
        let error = Command::new("sudo")
            .arg(root.join("hil/host/linux-net/install.sh"))
            .exec();
        Err(format!("cannot start the Linux fixture installer: {error}").into())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err("the Linux fixture installer requires a Unix host".into())
    }
}
