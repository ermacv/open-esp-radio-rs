//! Offline planning and unprivileged preparation before foreground sudo handoff.

use std::{path::Path, process::Command};

use oer_hil_fixture_install::{INSTALLER_BINARY, Provider, build_plan, prepare};

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
        require_unprivileged(rustix::process::geteuid().as_raw())?;
        // All downloads, patching and compilation happen without root, before
        // the installer takes terminal control for its narrow system writes.
        if provider == Provider::LinuxNet {
            super::hostapd::build(root)?;
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
        build.current_dir(root).args([
            "build",
            "--locked",
            "-p",
            "oer-hil-fixture-install",
            "-p",
            "oer-hil-fixture",
        ]);
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
        // Queue behind the runs using the provider; runs queued later wait
        // for the installation. Nothing supervises this lease: the operator
        // may take a while to answer sudo, and a partial installation is
        // worse than a late one.
        eprintln!(
            "hil-arbiter: queueing for {}",
            hil_core::fixture::software::resource(provider)
        );
        let grant = hil_core::fixture::software::install_grant(provider)?;
        // sudo runs in this foreground process group with the terminal's
        // standard streams, so it owns password echo, input and job control,
        // and its exit status is the command's.
        let status = Command::new("sudo")
            .arg(
                root.join("target/hil/fixture-build/debug")
                    .join(INSTALLER_BINARY),
            )
            .args(["--provider", provider.as_str(), "--bundle"])
            .arg(bundle)
            .status()
            .map_err(|error| format!("cannot start the Linux fixture installer: {error}"))?;
        drop(grant);
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
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
