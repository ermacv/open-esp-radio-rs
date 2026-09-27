//! Regenerate the vendor evidence shards whose recorded sources changed.
//!
//! Each shard records the digests of the sources its verdicts depend on, so a
//! shard is stale exactly when one of them changed. A shard that no longer
//! parses, such as one git left from a conflicting merge, is stale too: the
//! repository's attributes merge shards as binary files, and this task
//! rewrites them from the scenarios instead of by hand.
#[path = "../../../verification/schema/scenario-evidence.rs"]
#[allow(
    dead_code,
    reason = "this task reads only shard identities and source digests"
)]
mod scenario_evidence;

use crate::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Shard directory of `chip`, relative to the repository root.
fn directory(root: &Path, chip: &str) -> Result<String> {
    Ok(crate::chips::Chip::new(root, chip)?.evidence_shards())
}

/// A stand that compiles pinned vendor source on the host and compares the
/// production engine with it scenario by scenario.
struct Stand {
    /// The chip whose shards it writes.
    chip: &'static str,
    /// Shard name.
    scenario: &'static str,
    manifest: &'static str,
    /// Artifact source of the vendor files it compiles.
    source: &'static str,
    /// The production entry every scenario drives.
    production: &'static str,
}

const STANDS: &[Stand] = &[Stand {
    chip: "esp32s31",
    scenario: "ieee802154-host",
    manifest: "verification/esp32s31/host/ieee802154/Cargo.toml",
    source: "esp-idf",
    production: "oer_ieee802154_engine::engine",
}];

/// Shared schema sources every shard depends on.
const SCHEMA_SOURCES: &str = "verification/schema";

/// The comparison probe packages of `chip` a scenario run needs.
fn probes(chip: &str) -> (String, String) {
    (
        format!("oer-{chip}-probe-radio-elf"),
        format!("oer-{chip}-probe-bluetooth-elf"),
    )
}

/// Scenario names of the shards in `directory` that are stale or unreadable.
pub fn stale(root: &Path, directory: &Path) -> Result<Vec<String>> {
    let mut names = vec![];
    for entry in std::fs::read_dir(root.join(directory))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(scenario_evidence::SHARD_EXTENSION) {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("shard without a name")?
            .to_owned();
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<scenario_evidence::Index>(&text).ok())
            .is_some_and(|shard| shard.scenario == name && shard.is_current(root));
        if !current {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Rewrite the named shards, or every stale shard when none is named.
/// Stands write their shards directly; Blobray scenarios run through
/// `vendor-scenario`.
pub fn run(
    ctx: &Context,
    chip: &str,
    scenarios: Vec<String>,
    linker: PathBuf,
    limit_mode: String,
    output: PathBuf,
) -> Result<ExitCode> {
    let directory = directory(&ctx.root, chip)?;
    let directory = Path::new(&directory);
    let selected = if scenarios.is_empty() {
        stale(&ctx.root, directory)?
    } else {
        scenarios
    };
    if selected.is_empty() {
        println!("every evidence shard is current");
        return Ok(ExitCode::SUCCESS);
    }
    let linker = resolve(&linker)?;
    println!("regenerating evidence shards: {}", selected.join(", "));
    let (stands, selected): (Vec<String>, Vec<String>) = selected
        .into_iter()
        .partition(|name| STANDS.iter().any(|s| s.chip == chip && s.scenario == name));
    for name in &stands {
        let stand = STANDS
            .iter()
            .find(|s| s.chip == chip && s.scenario == name)
            .expect("partitioned stand");
        let shard = stand_shard(ctx, chip, stand)?;
        write(&ctx.root.join(directory), &shard)?;
    }
    if selected.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    crate::checks::vendor::run(ctx, chip, false)?;
    let (radio, bluetooth) = probes(chip);
    let radio = crate::checks::vendor::elf(ctx, &radio)?;
    let bluetooth = crate::checks::vendor::elf(ctx, &bluetooth)?;
    // Several scenarios run concurrently under one budget in `all`.
    let runs: Vec<String> = if selected.len() > 1 {
        vec!["all".into()]
    } else {
        selected
    };
    for scenario in runs {
        let mut args: Vec<OsString> = vec![
            scenario.clone().into(),
            "--production".into(),
            radio.clone().into(),
            "--linker".into(),
            linker.clone().into(),
            "--output".into(),
            output.join(&scenario).into(),
            "--limit-mode".into(),
            limit_mode.clone().into(),
            "--index".into(),
            ctx.root.join(directory).into(),
        ];
        if scenario == "all" || scenario == "bluetooth" {
            args.extend(["--bluetooth-production".into(), bluetooth.clone().into()]);
        }
        let code = crate::vendor_scenario::run(ctx, chip, &args)?;
        if code != ExitCode::SUCCESS {
            return Ok(code);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Run `stand`'s every scenario and build its shard; any scenario that is
/// not MATCH fails without a shard.
fn stand_shard(ctx: &Context, chip: &str, stand: &Stand) -> Result<scenario_evidence::Index> {
    let run = |args: &[&str]| -> Result<String> {
        let output = ctx
            .command("cargo")
            .args(["run", "--quiet", "--manifest-path", stand.manifest, "--"])
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "{} {}: {}",
                stand.scenario,
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(output.stdout)?)
    };
    let mut entries = vec![];
    for name in run(&["list"])?.lines().filter(|l| !l.is_empty()) {
        let verdict = run(&["compare", name])?;
        if verdict.lines().next() != Some("MATCH") {
            return Err(format!("{} {name}: {verdict}", stand.scenario).into());
        }
        entries.push(scenario_evidence::Entry {
            suite: stand.scenario.into(),
            source: stand.source.into(),
            symbol: name.into(),
            production: stand.production.into(),
            verdict: scenario_evidence::MATCH.into(),
            cases: 1,
            reviews: vec![],
            coverage: None,
            observation: None,
            state: None,
        });
    }
    let inputs = crate::vendor_fetch::pinned(ctx, chip)?
        .into_iter()
        .filter(|a| a.source == stand.source)
        .map(|a| {
            use sha2::Digest;
            let bytes = std::fs::read(&a.path)?;
            Ok((a.id, format!("{:x}", sha2::Sha256::digest(bytes))))
        })
        .collect::<Result<_>>()?;
    let mut directories = local_closure(ctx, stand.manifest)?;
    directories.push(PathBuf::from(SCHEMA_SOURCES));
    directories.sort();
    directories.dedup();
    let sources = directories
        .into_iter()
        .map(|path| {
            Ok(scenario_evidence::SourceDigest {
                sha256: scenario_evidence::digest_directory(&ctx.root, &path)?,
                path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let shard = scenario_evidence::Index {
        schema: scenario_evidence::SCHEMA,
        command: scenario_evidence::COMMAND.into(),
        target: chip.into(),
        scenario: stand.scenario.into(),
        inputs,
        sources,
        entries,
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    shard.validate(chip)?;
    Ok(shard)
}

/// Directories of the path packages `manifest`'s package depends on,
/// including itself, relative to the repository root.
fn local_closure(ctx: &Context, manifest: &str) -> Result<Vec<PathBuf>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(ctx.root.join(manifest))
        .other_options(["--offline".to_owned(), "--locked".to_owned()])
        .exec()?;
    let resolve = metadata.resolve.as_ref().ok_or("cargo metadata resolve")?;
    let root = resolve.root.as_ref().ok_or("cargo metadata root")?;
    let mut pending = vec![root.clone()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = resolve
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or("unresolved package")?;
        pending.extend(node.dependencies.iter().cloned());
    }
    let mut directories = vec![];
    for package in &metadata.packages {
        if package.source.is_none() && seen.contains(&package.id) {
            let directory = package
                .manifest_path
                .parent()
                .ok_or("manifest directory")?
                .as_std_path()
                .canonicalize()?;
            directories.push(
                directory
                    .strip_prefix(ctx.root.canonicalize()?)?
                    .to_path_buf(),
            );
        }
    }
    Ok(directories)
}

/// Write `shard` as its scenario's file in `directory`.
fn write(directory: &Path, shard: &scenario_evidence::Index) -> Result<()> {
    let path = directory.join(format!(
        "{}.{}",
        shard.scenario,
        scenario_evidence::SHARD_EXTENSION
    ));
    let mut bytes = serde_json::to_vec_pretty(shard)?;
    bytes.push(b'\n');
    std::fs::write(&path, bytes)?;
    println!("evidence shard {}", path.display());
    Ok(())
}

/// `program` itself when it names a path, otherwise its first match on
/// `PATH`.
fn resolve(program: &Path) -> Result<PathBuf> {
    if program.components().count() > 1 {
        return Ok(program.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("{} is not on PATH", program.display()).into())
}

#[cfg(test)]
mod tests;
