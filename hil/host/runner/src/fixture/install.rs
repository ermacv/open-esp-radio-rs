//! Offline planning and unprivileged preparation before foreground sudo handoff.

use std::{path::Path, process::Command};

use open_esp_radio_hil_runner::fixture_install::{INSTALLER_BINARY, Provider, build_plan, prepare};

use crate::Result;

pub(crate) fn run(
    root: &Path,
    provider: Provider,
    dry_run: bool,
    adapters: &[String],
) -> Result<()> {
    let plan = build_plan(root, provider, adapters)?;
    if dry_run {
        return crate::emit_json(&plan, true);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt as _;
        require_unprivileged(unsafe { libc::geteuid() })?;
        // All downloads, patching and compilation happen without root, before
        // the installer takes terminal control for its narrow system writes.
        if provider == Provider::LinuxNet {
            oer_process::run(
                Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                    .current_dir(root)
                    .args(["xtask", "build", "hostapd"]),
            )?;
        }

        let provider_binaries: &[&str] = match provider {
            Provider::LinuxNet => &[
                "open-radio-net-launcher",
                "open-radio-probe-launcher",
                "open-radio-probe",
            ],
            Provider::LinuxBluetooth => &["open-radio-bluetooth-launcher", "open-radio-bluetooth"],
        };
        let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        build
            .current_dir(root)
            .args(["build", "--locked", "-p", "open-esp-radio-hil-runner"]);
        for binary in provider_binaries {
            build.args(["--bin", binary]);
        }
        build.args([
            "--bin",
            INSTALLER_BINARY,
            "--target-dir",
            "target/hil/fixture-build",
        ]);
        oer_process::run(&mut build)?;
        let operator = current_operator()?;
        let bundle = prepare(root, provider, &operator, &plan.allowed_bluetooth_adapters)?;
        // This is a terminal handoff, not a supervised background workload.
        // exec preserves the foreground process group and standard streams so
        // sudo owns password echo, input and job control. No HIL owners have
        // been acquired; sudo also becomes the command's exit-status authority.
        let error = Command::new("sudo")
            .arg(
                root.join("target/hil/fixture-build/debug")
                    .join(INSTALLER_BINARY),
            )
            .args(["--provider", provider.as_str(), "--bundle"])
            .arg(bundle)
            .exec();
        Err(format!("cannot start the Linux fixture installer: {error}").into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, provider, adapters, plan);
        Err("fixture installation requires Linux".into())
    }
}

#[cfg(target_os = "linux")]
fn current_operator() -> Result<String> {
    let output = Command::new("/usr/bin/id").arg("-un").output()?;
    if !output.status.success() {
        return Err("cannot resolve the non-root fixture operator".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn require_unprivileged(uid: u32) -> Result<()> {
    if uid == 0 {
        return Err(
            "run cargo hil fixture install as the non-root operator; sudo is used only for the final apply"
                .into(),
        );
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn elevated_invocation_is_rejected_before_preparation() {
        assert!(super::require_unprivileged(0).is_err());
        assert!(super::require_unprivileged(1000).is_ok());
    }
}
