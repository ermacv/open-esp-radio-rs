//! Build and launch the HIL observer with a receipt from Cargo's actual artifacts.
use crate::{Context, Result};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs, process::Command};
#[path = "../../../hil/schema/observer-artifacts.rs"]
mod artifacts;

pub fn run(ctx: &Context, args: &[OsString]) -> Result<()> {
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
    let output = Command::new(&runner).arg("--observer-build").output()?;
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
    drop(compilation);
    let status = ctx
        .command(&runner)
        .args(args)
        .env("OER_OBSERVER_RECEIPT", &receipt_path)
        .status()?;
    if !status.success() {
        return Err(format!("HIL runner exited with {status}").into());
    }
    Ok(())
}
