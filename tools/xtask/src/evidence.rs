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
    reason = "this task reads only shard identities, digests and claims"
)]
pub(crate) mod scenario_evidence;

use crate::{Context, Result};
use std::collections::BTreeSet;
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
    let before: Vec<(String, Option<scenario_evidence::Index>)> = selected
        .iter()
        .map(|name| (name.clone(), read_shard(&ctx.root.join(directory), name)))
        .collect();
    if selected.is_empty() {
        println!("every evidence shard is current");
        return Ok(ExitCode::SUCCESS);
    }
    println!("regenerating evidence shards: {}", selected.join(", "));
    let code = regenerate(
        ctx,
        chip,
        selected,
        &ctx.root.join(directory),
        &resolve(&linker)?,
        &limit_mode,
        &output,
    )?;
    if code == ExitCode::SUCCESS {
        for (name, old) in before {
            let new = read_shard(&ctx.root.join(directory), &name);
            print!("{}", summary(&name, old.as_ref(), new.as_ref()));
        }
        print!(
            "{}",
            render_untriaged(&chip_untriaged(&ctx.root.join(directory))?, false)
        );
    }
    Ok(code)
}

/// Print the chip-wide untriaged vendor locations of `chip`'s committed
/// shards, one per line.
pub fn untriaged(ctx: &Context, chip: &str) -> Result<ExitCode> {
    let directory = directory(&ctx.root, chip)?;
    print!(
        "{}",
        render_untriaged(&chip_untriaged(&ctx.root.join(directory))?, true)
    );
    Ok(ExitCode::SUCCESS)
}

/// Untriaged vendor locations of every shard in `directory` that no other
/// scenario covers: a location another shard's closures contain the
/// function of is covered there unless that shard lists it untriaged too.
fn chip_untriaged(directory: &Path) -> Result<BTreeSet<scenario_evidence::Location>> {
    let mut shards = vec![];
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some(scenario_evidence::SHARD_EXTENSION) {
            continue;
        }
        if let Some(shard) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| read_shard(directory, stem))
        {
            shards.push(shard);
        }
    }
    Ok(untriaged_everywhere(&shards))
}

fn untriaged_everywhere(
    shards: &[scenario_evidence::Index],
) -> BTreeSet<scenario_evidence::Location> {
    let untriaged: Vec<BTreeSet<&scenario_evidence::Location>> = shards
        .iter()
        .map(|shard| shard.untriaged.iter().collect())
        .collect();
    let mut remaining = BTreeSet::new();
    for (index, shard) in shards.iter().enumerate() {
        for location in &shard.untriaged {
            let covered_elsewhere = shards.iter().enumerate().any(|(other, candidate)| {
                other != index
                    && candidate.functions.contains(&location.function)
                    && !untriaged[other].contains(location)
            });
            if !covered_elsewhere {
                remaining.insert(location.clone());
            }
        }
    }
    remaining
}

/// Per-function counts of `locations`, or every location when `all`.
fn render_untriaged(locations: &BTreeSet<scenario_evidence::Location>, all: bool) -> String {
    let mut functions = std::collections::BTreeMap::<&str, usize>::new();
    for location in locations {
        *functions.entry(&location.function).or_default() += 1;
    }
    let mut text = format!(
        "chip-wide untriaged: {} locations in {} functions\n",
        locations.len(),
        functions.len()
    );
    if all {
        for location in locations {
            text += &format!(
                "  {}+{:#x} {:?}\n",
                location.function, location.offset, location.kind
            );
        }
    } else {
        let mut ranked: Vec<_> = functions.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (function, count) in ranked {
            text += &format!("  {count:4} {function}\n");
        }
    }
    text
}

/// Repository files that differ between `revision` and the worktree.
fn changed_files(ctx: &Context, revision: &str) -> Result<Vec<PathBuf>> {
    let output = crate::process::capture(ctx.command("git").args([
        "diff",
        "--name-only",
        "-z",
        revision,
        "--",
    ]))?;
    let untracked = crate::process::capture(ctx.command("git").args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .chain(untracked.stdout.split(|b| *b == 0))
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

/// Whether `shard` records one of `changed`: a recorded file itself, a
/// file below a recorded directory, or a changed decision file whose
/// decisions that apply to the shard differ from those it recorded.
fn records_any(root: &Path, shard: &scenario_evidence::Index, changed: &[PathBuf]) -> bool {
    shard.sources.iter().any(|source| {
        changed
            .iter()
            .any(|path| path == &source.path || path.starts_with(&source.path))
    }) || shard
        .dependence
        .coverage_decisions
        .as_ref()
        .is_some_and(|decisions| {
            changed.contains(&decisions.path)
                && !scenario_evidence::CoverageDecisions::read(root, &decisions.path)
                    .is_ok_and(|file| file.applicable_digest(&shard.functions) == decisions.sha256)
        })
}

/// The committed shard `name` in `directory`, when it parses.
fn read_shard(directory: &Path, name: &str) -> Option<scenario_evidence::Index> {
    let file = format!("{name}.{}", scenario_evidence::SHARD_EXTENSION);
    std::fs::read_to_string(directory.join(file))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// What changed between two versions of shard `name`: its claims first,
/// then the recorded sources.
fn summary(
    name: &str,
    old: Option<&scenario_evidence::Index>,
    new: Option<&scenario_evidence::Index>,
) -> String {
    match (old, new) {
        (Some(old), Some(new)) => {
            crate::evidence_diff::render(name, &crate::evidence_diff::differences(old, new))
        }
        (None, Some(_)) => format!("{name}:\n  new shard\n"),
        (_, None) => format!("{name}:\n  no readable shard\n"),
    }
}

/// Rerun every Blobray scenario shard of `chip` into `output` and fail
/// unless each equals the committed shard. Stand shards run on hardware
/// and are not rerun.
pub fn check(
    ctx: &Context,
    chip: &str,
    scenarios: Vec<String>,
    changed_since: Option<String>,
    linker: PathBuf,
    limit_mode: String,
    output: PathBuf,
) -> Result<ExitCode> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    let mut committed = vec![];
    for entry in std::fs::read_dir(&directory)? {
        let path = entry?.path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) == Some(scenario_evidence::SHARD_EXTENSION)
            && !STANDS.iter().any(|s| s.chip == chip && s.scenario == name)
        {
            committed.push(name.to_owned());
        }
    }
    committed.sort();
    // Named scenarios narrow the check; each must have a committed shard.
    if !scenarios.is_empty() {
        if let Some(unknown) = scenarios.iter().find(|s| !committed.contains(s)) {
            return Err(
                format!("{unknown} has no committed Blobray evidence shard for {chip}").into(),
            );
        }
        committed.retain(|name| scenarios.contains(name));
    }
    if let Some(revision) = &changed_since {
        let changed = changed_files(ctx, revision)?;
        let (touched, skipped): (Vec<String>, Vec<String>) =
            committed
                .into_iter()
                .partition(|name| match read_shard(&directory, name) {
                    // An unreadable shard is checked: skipping fails closed.
                    None => true,
                    Some(shard) => records_any(&ctx.root, &shard, &changed),
                });
        for name in &skipped {
            println!(
                "{name}: skipped, no recorded source changed in {revision}..HEAD or the worktree"
            );
        }
        committed = touched;
        if committed.is_empty() {
            println!("every evidence shard skipped: no recorded source changed");
            return Ok(ExitCode::SUCCESS);
        }
    }
    // Per chip, so checks of different chips keep their reruns apart.
    let rerun = ctx.root.join(&output).join(CHECK_INDEX).join(chip);
    if rerun.exists() {
        std::fs::remove_dir_all(&rerun)?;
    }
    std::fs::create_dir_all(&rerun)?;
    let code = regenerate(
        ctx,
        chip,
        committed.clone(),
        &rerun,
        &resolve(&linker)?,
        &limit_mode,
        &output,
    )?;
    if code != ExitCode::SUCCESS {
        return Ok(code);
    }
    let differing: Vec<&String> = committed
        .iter()
        .filter(|name| {
            let (old, new) = (read_shard(&directory, name), read_shard(&rerun, name));
            let differs = !matches!((&old, &new), (Some(a), Some(b)) if a == b);
            if differs {
                eprint!("{}", summary(name, old.as_ref(), new.as_ref()));
            }
            differs
        })
        .collect();
    if differing.is_empty() {
        println!("every rerun evidence shard equals its committed shard");
        Ok(ExitCode::SUCCESS)
    } else {
        let names: Vec<&str> = differing.iter().map(|s| s.as_str()).collect();
        eprintln!(
            "evidence shards differ from their rerun: {}; regenerate them with `cargo xtask evidence --chip {chip} {}`",
            names.join(", "),
            names.join(" ")
        );
        Ok(ExitCode::FAILURE)
    }
}

/// Directory below the output root a check reruns each chip's shards into.
const CHECK_INDEX: &str = "check-index";

/// Rerun `selected` scenarios of `chip`, writing their shards into `index`.
fn regenerate(
    ctx: &Context,
    chip: &str,
    selected: Vec<String>,
    index: &Path,
    linker: &Path,
    limit_mode: &str,
    output: &Path,
) -> Result<ExitCode> {
    let directory = index;
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
    // Only chips with Bluetooth scenarios build a Bluetooth probe.
    let bluetooth = crate::checks::vendor::elf(ctx, &bluetooth).ok();
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
            linker.as_os_str().to_owned(),
            "--output".into(),
            output.join(&scenario).into(),
            "--limit-mode".into(),
            limit_mode.into(),
            "--index".into(),
            ctx.root.join(directory).into(),
        ];
        if let Some(bluetooth) = bluetooth
            .as_ref()
            .filter(|_| scenario == "all" || scenario == "bluetooth")
        {
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
        dependence: scenario_evidence::Dependence::whole_closure(STAND_CLOSURE),
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

/// Why a stand shard tracks its whole source closure.
const STAND_CLOSURE: &str =
    "a stand scenario runs on hardware, outside the executions files attribute";

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
