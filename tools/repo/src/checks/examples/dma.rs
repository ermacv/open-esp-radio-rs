//! Compile the borrowed-DMA misuse fixture against the actual embedded API.

use super::TARGET;
use crate::{Context, Result, cargo, process};
use cargo_metadata::Message;
use std::{io::Cursor, path::PathBuf};

pub(super) fn check(ctx: &Context) -> Result<()> {
    let output = process::capture(ctx.cargo().args([
        "check",
        "--locked",
        "--offline",
        "-p",
        "open-esp-radio-esp32s31-platform-pac",
        "--features",
        "axi-gdma-mem2mem",
        "--target",
        TARGET,
        "--message-format=json",
    ]))?;
    let mut metadata = None;
    for message in Message::parse_stream(Cursor::new(&output.stdout)) {
        if let Message::CompilerArtifact(artifact) = message?
            && artifact.target.name == "open_esp_radio_esp32s31_platform_pac"
        {
            metadata = artifact
                .filenames
                .into_iter()
                .find(|file| file.extension() == Some("rmeta"));
        }
    }
    let metadata = metadata.ok_or("DMA compiler check produced no crate metadata")?;
    let target_deps = metadata.parent().ok_or("DMA metadata has no parent")?;
    let workspace = cargo::metadata_no_deps(ctx, &ctx.root.join("Cargo.toml"))?;
    let host_deps = workspace.target_directory.join("debug/deps");
    let scratch = tempfile::tempdir()?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = process::output(
        ctx.command(rustc)
            .args([
                "--edition=2024",
                "--crate-type=lib",
                "--emit=metadata",
                "--error-format=json",
                "--target",
                TARGET,
            ])
            .arg(
                ctx.root
                    .join("driver/adapters/esp-hal/esp32s31/soc/tests/ui/forget_borrowed_dma.rs"),
            )
            .arg("--extern")
            .arg(format!("open_esp_radio_esp32s31_platform_pac={metadata}"))
            .arg("-L")
            .arg(format!("dependency={target_deps}"))
            .arg("-L")
            .arg(format!("dependency={host_deps}"))
            .arg("-o")
            .arg(PathBuf::from(scratch.path()).join("fixture.rmeta")),
        None,
    )?;
    let errors = serde_json::Deserializer::from_slice(&output.stderr)
        .into_iter::<serde_json::Value>()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let codes: Vec<_> = errors
        .iter()
        .filter(|diagnostic| diagnostic["level"] == "error")
        .filter_map(|diagnostic| diagnostic["code"]["code"].as_str())
        .collect();
    if output.status.success() || codes != ["E0133"] {
        return Err(format!(
            "borrowed DMA publication must fail specifically with E0133; got {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    println!("borrowed DMA publication rejects safe forget/reuse (E0133)");
    Ok(())
}
