//! Pinned hostapd preparation for the Linux HIL fixture; never owns a radio.
use crate::{Context, Result, process};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const URL: &str = "https://w1.fi/releases/hostapd-2.12.tar.gz";
const SOURCE_SHA256: &str = "f43502561c28ba47ab77e18e1a973d07361c68cc8b14178e619bd5796b70eabd";

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn verify_source(bytes: &[u8]) -> Result<()> {
    if digest(bytes) != SOURCE_SHA256 {
        return Err("hostapd archive SHA-256 mismatch".into());
    }
    Ok(())
}

pub fn build(ctx: &Context) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("the hostapd fixture build requires Linux".into());
    }
    let output = ctx.root.join("target/hil/hostapd");
    fs::create_dir_all(&output)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(output.join("build.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| "another hostapd build owns the output directory")?;
    let recipe = ctx.root.join("hil/host/linux-net/hostapd");
    let patch = fs::read(recipe.join("300-noscan.patch"))?;
    let config = fs::read(recipe.join("build.config"))?;
    let compiler = tool_version(ctx.command("cc").arg("--version"))?;
    let libraries = tool_version(ctx.command("pkg-config").args([
        "--modversion",
        "libnl-3.0",
        "libnl-genl-3.0",
        "openssl",
    ]))?;
    let inputs = serde_json::json!({"schema":1,"source":URL,"source_sha256":SOURCE_SHA256,
        "patch_sha256":digest(&patch),"config_sha256":digest(&config),"builder_sha256":digest(include_bytes!("hostapd.rs")),
        "compiler":compiler,"libraries":libraries});
    let binary = output.join("hostapd");
    let provenance = output.join("provenance.json");
    if let (Ok(old), Ok(bytes)) = (fs::read(&provenance), fs::read(&binary))
        && let Ok(old) = serde_json::from_slice::<serde_json::Value>(&old)
        && old["inputs"] == inputs
        && old["binary_sha256"] == digest(&bytes)
    {
        println!("hostapd build verified: {}", binary.display());
        return Ok(());
    }
    let archive = output.join("hostapd-2.12.tar.gz");
    if !archive.exists() {
        let download = tempfile::NamedTempFile::new_in(&output)?;
        process::run(
            ctx.command("curl")
                .args([
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "120",
                    "--output",
                ])
                .arg(download.path())
                .arg(URL),
        )?;
        verify_source(&fs::read(download.path())?)?;
        download.persist(&archive)?;
    }
    verify_source(&fs::read(&archive)?)?;
    let work = tempfile::tempdir_in(&output)?;
    process::run(
        ctx.command("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(work.path()),
    )?;
    let source = work.path().join("hostapd-2.12");
    process::run(
        ctx.command("patch")
            .current_dir(&source)
            .args(["--batch", "--fuzz=0", "-p1", "-i"])
            .arg(recipe.join("300-noscan.patch")),
    )?;
    fs::write(source.join("hostapd/.config"), config)?;
    process::run(
        ctx.command("make")
            .current_dir(source.join("hostapd"))
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .env_remove("CFLAGS")
            .env_remove("CPPFLAGS")
            .env_remove("LDFLAGS")
            .arg(format!("-j{}", std::thread::available_parallelism()?.get()))
            .args(["CC=cc", "hostapd"]),
    )?;
    let built = source.join("hostapd/hostapd");
    verify_policy(ctx, &built, work.path())?;
    let mut staged = tempfile::NamedTempFile::new_in(&output)?;
    std::io::copy(&mut fs::File::open(&built)?, &mut staged)?;
    fs::set_permissions(staged.path(), fs::metadata(&built)?.permissions())?;
    let hash = digest(&fs::read(staged.path())?);
    staged.persist(&binary)?;
    let record = serde_json::json!({"inputs":inputs,"binary_sha256":hash});
    let mut staged = tempfile::NamedTempFile::new_in(&output)?;
    serde_json::to_writer_pretty(&mut staged, &record)?;
    staged.persist(provenance)?;
    println!(
        "hostapd built and policy parser verified: {}",
        binary.display()
    );
    Ok(())
}

fn verify_policy(ctx: &Context, binary: &Path, directory: &Path) -> Result<()> {
    use oer_process::CommandExt as _;
    let config = directory.join("parser.conf");
    // An intentional final syntax error prevents all driver initialization.
    fs::write(&config, "noscan=1\nht_coex=1\noer_parser_stop=1\n")?;
    let output = ctx.command(binary).arg(&config).supervised_output()?;
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success()
        || !diagnostic.contains("unknown configuration item 'oer_parser_stop'")
        || !diagnostic.contains("1 errors found")
    {
        return Err(format!("hostapd policy parser verification failed: {diagnostic}").into());
    }
    Ok(())
}

fn tool_version(command: &mut std::process::Command) -> Result<String> {
    let output = process::output(command, Some(std::time::Duration::from_secs(10)))?;
    if !output.status.success() {
        return Err(format!(
            "hostapd build prerequisite failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_corrupt_source_before_extraction() {
        assert!(super::verify_source(b"not the pinned release").is_err());
    }
}
