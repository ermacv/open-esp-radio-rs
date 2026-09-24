//! Explicit compiler diagnostics through the same image constructor, in a fresh
//! output tree. No fixture, qualification run, flash or implicit normal-build cost.
use super::*;

fn output(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let result = Command::new(program)
        .current_dir(root)
        .args(args)
        .supervised_output()?;
    if !result.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn clean_commit(root: &Path) -> Result<String> {
    if !output(
        root,
        "git",
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err("mono capture requires committed source, including new source files".into());
    }
    output(root, "git", &["rev-parse", "HEAD"])
}

pub(super) fn configure(command: &mut Command, output: &Path) -> Result<()> {
    let stats = output.join("mono-stats");
    let path = stats.to_str().ok_or("mono output path is not UTF-8")?;
    if path.chars().any(char::is_whitespace) {
        return Err("mono RUSTFLAGS output path cannot contain whitespace".into());
    }
    // The image constructor already applied its required stack flags. Preserve
    // them and expose exactly the actual diagnostic runtime command.
    let flags = command
        .get_envs()
        .find(|(key, _)| *key == "RUSTFLAGS")
        .and_then(|(_, value)| value)
        .ok_or("runtime stack flags are missing")?
        .to_str()
        .ok_or("runtime flags are not UTF-8")?;
    let flags = format!("{flags} -Zdump-mono-stats={path} -Zdump-mono-stats-format=json");
    fs::create_dir(&stats)?;
    command.env("RUSTFLAGS", &flags);
    crate::evidence::run::atomic_json(
        &output.join("mono-command.json"),
        &serde_json::json!({
            "program": command.get_program(),
            "args": command.get_args().collect::<Vec<_>>(),
            "environment": command.get_envs().collect::<Vec<_>>(),
        }),
    )
}

fn identity(path: &Path) -> Result<serde_json::Value> {
    let bytes = fs::read(path)?;
    Ok(
        serde_json::json!({"path":path,"size_bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes))}),
    )
}

pub(crate) fn capture(root: &Path, class: ImageClass) -> Result<()> {
    for name in [
        "ESP_HAL_ROOT",
        "EMBASSY_ROOT",
        "OPEN_RADIO_XARXA_ROOT",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        if env::var_os(name).is_some() {
            return Err(format!(
                "mono capture requires the pinned compiler and immutable dependencies; unset {name}"
            )
            .into());
        }
    }
    let commit = clean_commit(root)?;
    let parent = root.join("target/hil/esp32s31/mono");
    fs::create_dir_all(&parent)?;
    // Fresh Cargo directories force compiler output for this invocation rather
    // than silently reusing an old dump or a cached successful build.
    let directory = tempfile::Builder::new()
        .prefix(class.id())
        .tempdir_in(&parent)?
        .keep();
    let rustc = output(root, "rustc", &["-vV"])?;
    let cargo = output(root, "cargo", &["-V"])?;
    crate::evidence::run::atomic_json(
        &directory.join("capture.json"),
        &serde_json::json!({
            "schema":1,"status":"building","commit":commit,"class":class.id(),"rustc":rustc,"cargo":cargo,
            "scope":"runtime compiler estimates; no linked-byte attribution or HIL qualification",
        }),
    )?;
    let built = build_resolved(
        root,
        class,
        Integration::UpstreamXarxa,
        LocalOverrides::default(),
        Some(&directory),
        false,
        true,
    );
    let artifacts = built?;
    if clean_commit(root)? != commit {
        return Err("source identity changed during mono capture".into());
    }
    let mut inputs =
        fs::read_dir(directory.join("mono-stats"))?.collect::<std::io::Result<Vec<_>>>()?;
    inputs.sort_by_key(fs::DirEntry::file_name);
    let mut files = Vec::new();
    for entry in inputs {
        if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|s| s != "json") {
            return Err("unexpected compiler mono output; inspect the retained capture".into());
        }
        let report = open_esp_radio_memory_report::analyze_mono(&entry.path())?;
        files.push(serde_json::json!({"identity":identity(&entry.path())?,
            "compiler_output_empty":report.compiler_output_empty,
            "definitions":report.definitions.len()}));
    }
    if files.is_empty() {
        return Err("compiler produced no mono JSON; capture is incomplete".into());
    }
    if !files
        .iter()
        .any(|file| file["definitions"].as_u64().unwrap_or(0) > 0)
    {
        return Err("compiler produced no definition estimates; capture is incomplete".into());
    }
    let linked = open_esp_radio_memory_report::analyze_code(&artifacts.runtime_elf)?;
    crate::evidence::run::atomic_json(&directory.join("linked-code.json"), &linked)?;
    let report = serde_json::json!({"schema":1,"status":"complete","commit":commit,"class":class.id(),
        "target":TARGET,"rustc":rustc,"cargo":cargo,
        "scope":"runtime compiler estimates; no linked-byte attribution or HIL qualification",
        "command":identity(&directory.join("mono-command.json"))?,"files":files,
        "runtime_elf":identity(&artifacts.runtime_elf)?,
        "runtime_lock":identity(&artifacts.effective_embedded_lock)?,
        "bootstrap_lock":identity(&artifacts.effective_bootstrap_lock)?,
        "linked_code":identity(&directory.join("linked-code.json"))?,
    });
    crate::evidence::run::atomic_json(&directory.join("capture.json"), &report)?;
    crate::emit_json(
        &serde_json::json!({"capture":directory.join("capture.json"),"compiler_files":files.len()}),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_preserves_stack_flags_and_refuses_to_reuse_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("cargo");
        command.env("RUSTFLAGS", "-Z emit-stack-sizes -D large-assignments");
        configure(&mut command, dir.path()).unwrap();
        let flags = command
            .get_envs()
            .find(|(k, _)| *k == "RUSTFLAGS")
            .unwrap()
            .1
            .unwrap()
            .to_str()
            .unwrap();
        assert!(flags.starts_with("-Z emit-stack-sizes -D large-assignments"));
        assert!(flags.ends_with("-Zdump-mono-stats-format=json"));
        assert!(configure(&mut command, dir.path()).is_err());
        let no_flags = tempfile::tempdir().unwrap();
        assert!(configure(&mut Command::new("cargo"), no_flags.path()).is_err());
    }
}
