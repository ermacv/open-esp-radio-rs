//! Linux DTM fixture preparation, isolated from ESP firmware execution.

pub(crate) mod model;

use model::{Adapter, Check};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) fn check(root: &Path, adapter: Adapter) -> crate::Result<()> {
    let _lease = crate::lab::lock::acquire_bluetooth(adapter)?;
    let directory = root.join("target/hil/fixture-checks");
    fs::create_dir_all(&directory)?;
    let output = tempfile::Builder::new()
        .prefix(&format!(
            "{}-bluetooth-",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
        ))
        .tempdir_in(directory)?
        .keep();
    let result = run_in(&output, adapter);
    crate::emit_json(
        &serde_json::from_slice::<serde_json::Value>(&fs::read(output.join("result.json"))?)?,
        true,
    )?;
    result.map(|_| ())
}

pub(crate) fn run_in(output: &Path, adapter: Adapter) -> crate::Result<Check> {
    fs::create_dir_all(output)?;
    let mut command = Command::new("sudo");
    command
        .args([
            "-n",
            "/usr/local/libexec/open-radio-bluetooth",
            "check",
            "--adapter",
            &adapter.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(output.join("helper.json"))?))
        .stderr(Stdio::from(fs::File::create(output.join("helper.stderr"))?));
    // The helper has its own bounded operations and receives SIGTERM before
    // escalation. Keep enough grace for Reset, re-registration and restoration.
    let result = (|| -> crate::Result<Check> {
        let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
            &mut command,
            Duration::from_secs(20),
        )?;
        let status = child.wait_timeout(Some(Duration::from_secs(45)))?;
        checked_report(
            status.success(),
            adapter,
            &fs::read(output.join("helper.json"))?,
        )
        .map_err(|error| format!("{error}; evidence: {}", output.display()).into())
    })();
    let report = fs::read(output.join("helper.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Check>(&bytes).ok())
        .unwrap_or_else(|| Check::new(adapter));
    let summary = serde_json::json!({
        "schema": 1, "adapter": adapter.to_string(), "output": output,
        "passed": result.is_ok(), "rf_verified": false,
        "error": result.as_ref().err().map(ToString::to_string),
        "helper": report,
    });
    crate::evidence::run::atomic_json(&output.join("result.json"), &summary)?;
    result
}

fn checked_report(success: bool, adapter: Adapter, bytes: &[u8]) -> crate::Result<Check> {
    let report: Check = serde_json::from_slice(bytes).map_err(|error| {
        format!("Bluetooth helper returned no valid report ({error}); inspect helper.stderr. Install with sudo hil/host/linux-bluetooth/install.sh after building open-radio-bluetooth")
    })?;
    if !success || !report.passed(adapter) {
        return Err(format!(
            "Bluetooth check failed or incomplete: {}",
            report.errors.join("; ")
        )
        .into());
    }
    Ok(report)
}

pub(crate) fn preflight(adapter: Adapter) -> crate::Result<()> {
    let mut command = Command::new("sudo");
    command.args([
        "-n",
        "-l",
        "/usr/local/libexec/open-radio-bluetooth",
        "check",
        "--adapter",
        &adapter.to_string(),
    ]);
    let output = oer_process::output(&mut command, Some(Duration::from_secs(5)))?;
    if !Path::new("/usr/local/libexec/open-radio-bluetooth").is_file() || !output.status.success() {
        return Err(
            "install the Bluetooth helper with sudo hil/host/linux-bluetooth/install.sh".into(),
        );
    }
    if !Path::new("/sys/class/bluetooth")
        .join(adapter.to_string())
        .exists()
    {
        return Err(format!("Bluetooth adapter {adapter} is absent").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_or_partial_helper_evidence_cannot_pass() {
        assert!(checked_report(true, Adapter(0), b"").is_err());
        let report = Check::new(Adapter(0));
        assert!(checked_report(true, Adapter(0), &serde_json::to_vec(&report).unwrap()).is_err());
    }
    #[test]
    fn helper_failure_exposes_the_controller_error() {
        let mut report = Check::new(Adapter(0));
        report.errors.push("RX rejected: command disallowed".into());
        let error =
            checked_report(false, Adapter(0), &serde_json::to_vec(&report).unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("RX rejected: command disallowed")
        );
    }
}
