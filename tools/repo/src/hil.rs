//! Build and launch the HIL observer with a receipt from Cargo's actual artifacts.
use crate::{Context, Result};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs, process::Command};
#[path = "../../../hil/schema/observer-artifacts.rs"]
mod artifacts;

pub fn prepare(ctx: &Context) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let compilation = artifacts::compile(&ctx.root)?;
    let executable = &compilation.executable;
    let artifacts = &compilation.artifacts;
    let bytes = fs::read(executable)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let directory = ctx.root.join("target/hil/observers").join(&hash);
    fs::create_dir_all(&directory)?;
    let runner = directory.join("runner");
    // A private copy prevents concurrent Cargo rebuilds from replacing this run's inode.
    if !runner.exists() {
        fs::write(&runner, &bytes)?;
        fs::set_permissions(&runner, fs::metadata(executable)?.permissions())?;
    }
    if fs::read(&runner)? != bytes {
        return Err("observer executable identity conflict".into());
    }
    let output = oer_process::output(Command::new(&runner).arg("--observer-build"), None)?;
    if !output.status.success() {
        return Err("cannot read executable's embedded build".into());
    }
    let mut build: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    artifacts::apply(&mut build["resolved"], artifacts)?;
    build["resolved"]["selected_profile"] = serde_json::json!(compilation.profile);
    let receipt = serde_json::json!({"executable_sha256":hash,"build":build,"artifacts":artifacts,"profile":compilation.profile});
    // Each invocation owns its receipt, including concurrent launches of the same binary.
    let mut file = tempfile::NamedTempFile::new_in(&directory)?;
    use std::io::Write;
    let bytes = serde_json::to_vec(&receipt)?;
    file.write_all(&bytes)?;
    let receipt_path = directory.join(format!("receipt-{:x}.json", Sha256::digest(&bytes)));
    if !receipt_path.exists() {
        file.persist(&receipt_path)?;
    }
    if fs::read(&receipt_path)? != bytes {
        return Err("observer receipt identity conflict".into());
    }
    let mut current = tempfile::NamedTempFile::new_in(ctx.root.join("target/hil"))?;
    current.write_all(&bytes)?;
    current.persist(ctx.root.join("target/hil/current-observer.json"))?;
    drop(compilation);
    Ok((runner, receipt_path))
}

pub fn run(ctx: &Context, args: &[OsString]) -> Result<std::process::ExitCode> {
    let (runner, receipt_path) = prepare(ctx)?;
    // Cleanup scopes in the runner have 30-second budgets and may unwind
    // multiple owned fixtures. This is a shutdown allowance, never a run timeout.
    let mut child = oer_process::owned::Child::spawn_with_shutdown_grace(
        ctx.command(&runner)
            .args(args)
            .env("OER_OBSERVER_RECEIPT", &receipt_path),
        std::time::Duration::from_secs(300),
    )?;
    Ok(exit_code(child.wait_forwarding_cancellation()?))
}

fn exit_code(status: std::process::ExitStatus) -> std::process::ExitCode {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    let code = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            128 + status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            1
        }
    });
    std::process::ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwards_nonzero_runner_status() {
        let status = oer_process::owned::Child::spawn(Command::new("sh").args(["-c", "exit 37"]))
            .unwrap()
            .wait_forwarding_cancellation()
            .unwrap();
        assert_eq!(exit_code(status), std::process::ExitCode::from(37));
    }
}
