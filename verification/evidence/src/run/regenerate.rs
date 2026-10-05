//! Regenerate the vendor evidence shards whose recorded sources changed.
//!
//! Each shard records the digests of the sources its verdicts depend on, so a
//! shard is stale exactly when one of them changed. A shard that no longer
//! parses, such as one git left from a conflicting merge, is stale too: the
//! repository's attributes merge shards as binary files, and this task
//! rewrites them from the scenarios instead of by hand.
//!
//! This module decides which producer reruns which shard: the Blobray
//! scenario engine ([`super::scenario`]) or a host stand below
//! `verification/<chip>/host/` (its own `shard` command); `cargo xtask
//! evidence` calls it.
use crate::Result;
use crate::producer::{Producer, host_stand};
use oer_process::Checkout;
use oer_vendor_evidence_shard::{Index, store};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Shard directory of `chip`, relative to the repository root.
fn directory(root: &Path, chip: &str) -> Result<String> {
    Ok(oer_vendor_artifacts::project::Project::new(root, chip)?.evidence_shards())
}

/// The comparison probe packages of `chip` a scenario run needs.
fn probes(chip: &str) -> (String, String) {
    (
        format!("oer-{chip}-probe-radio-elf"),
        format!("oer-{chip}-probe-bluetooth-elf"),
    )
}

/// Fail when a committed shard of `directory` records a file of a report
/// package.
fn reject_report_sources(ctx: &Checkout, directory: &Path) -> Result<()> {
    let model = oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?;
    let report = crate::policy::report_packages(&model)?;
    crate::policy::reject_report_sources(directory, &report)
}

/// The typed Blobray scenarios of a chip, run through `vendor-scenario`.
struct Blobray<'a> {
    ctx: &'a Checkout,
    chip: &'a str,
    linker: PathBuf,
    output: &'a Path,
}

impl Producer for Blobray<'_> {
    fn command(&self) -> &'static str {
        oer_vendor_evidence_shard::BLOBRAY
    }

    fn owns(&self, _scenario: &str) -> bool {
        true
    }

    fn produce(&self, scenarios: &[String], index: &Path) -> Result<()> {
        let (ctx, chip) = (self.ctx, self.chip);
        super::probes::run(ctx, chip, false)?;
        let (radio, bluetooth) = probes(chip);
        let radio = super::probes::elf(ctx, &radio)?;
        // Only chips with Bluetooth scenarios build a Bluetooth probe.
        let bluetooth = super::probes::elf(ctx, &bluetooth).ok();
        // Several scenarios run concurrently under one budget in `all`.
        let runs: Vec<String> = if scenarios.len() > 1 {
            vec!["all".into()]
        } else {
            scenarios.to_vec()
        };
        for scenario in runs {
            let mut args: Vec<OsString> = vec![
                scenario.clone().into(),
                "--production".into(),
                radio.clone().into(),
                "--linker".into(),
                self.linker.as_os_str().to_owned(),
                "--output".into(),
                self.output.join(&scenario).into(),
                "--index".into(),
                index.into(),
            ];
            if let Some(bluetooth) = bluetooth
                .as_ref()
                .filter(|_| scenario == "all" || scenario == "bluetooth")
            {
                args.extend(["--bluetooth-production".into(), bluetooth.clone().into()]);
            }
            if super::scenario::run(ctx, chip, &args)? != ExitCode::SUCCESS {
                return Err(format!("vendor scenario {scenario} failed").into());
            }
        }
        Ok(())
    }
}

/// One host stand of a chip: its own workspace, whose `shard` command
/// compares every scenario and writes the stand's shard.
struct HostStand<'a> {
    ctx: &'a Checkout,
    scenario: String,
    manifest: String,
}

impl Producer for HostStand<'_> {
    fn command(&self) -> &'static str {
        oer_vendor_evidence_shard::HOST_STAND
    }

    fn owns(&self, scenario: &str) -> bool {
        scenario == self.scenario
    }

    fn produce(&self, _scenarios: &[String], index: &Path) -> Result<()> {
        super::phase::timed(&format!("stand {}", self.scenario), || {
            oer_process::run(
                oer_toolchain::cargo_in(&self.ctx.root)
                    .args(["run", "--quiet", "--manifest-path", &self.manifest, "--"])
                    .arg("shard")
                    .arg("--index")
                    .arg(index),
            )
        })?;
        Ok(())
    }
}

/// The producers of `chip`'s shards, host stands first: the Blobray
/// scenarios own every name no stand owns.
fn producers<'a>(
    ctx: &'a Checkout,
    chip: &'a str,
    linker: &Path,
    output: &'a Path,
) -> Result<Vec<Box<dyn Producer + 'a>>> {
    let model = oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?;
    let mut producers: Vec<Box<dyn Producer + 'a>> = host_stand::stands(&model, chip)?
        .into_iter()
        .map(|(scenario, manifest)| {
            Box::new(HostStand {
                ctx,
                scenario,
                manifest,
            }) as Box<dyn Producer>
        })
        .collect();
    producers.push(Box::new(Blobray {
        ctx,
        chip,
        linker: resolve(linker)?,
        output,
    }));
    Ok(producers)
}

/// Rerun `selected` scenarios of `chip` with their producers, writing their
/// shards into `index`.
fn regenerate(
    ctx: &Checkout,
    chip: &str,
    selected: Vec<String>,
    index: &Path,
    linker: &Path,
    output: &Path,
) -> Result<()> {
    let mut remaining = selected;
    for producer in producers(ctx, chip, linker, output)? {
        let (owned, rest): (Vec<String>, Vec<String>) =
            remaining.into_iter().partition(|name| producer.owns(name));
        remaining = rest;
        if !owned.is_empty() {
            producer.produce(&owned, index)?;
        }
    }
    Ok(())
}

/// Rewrite the named shards, or every stale shard when none is named.
pub fn run(
    ctx: &Checkout,
    chip: &str,
    scenarios: Vec<String>,
    linker: PathBuf,
    output: PathBuf,
) -> Result<ExitCode> {
    let relative = directory(&ctx.root, chip)?;
    let directory = ctx.root.join(&relative);
    reject_report_sources(ctx, &directory)?;
    let selected = if scenarios.is_empty() {
        store::stale(&ctx.root, Path::new(&relative))?
    } else {
        scenarios
    };
    if selected.is_empty() {
        println!("every evidence shard is current");
        return Ok(ExitCode::SUCCESS);
    }
    let before: Vec<(String, Option<Index>)> = selected
        .iter()
        .map(|name| (name.clone(), store::read(&directory, name)))
        .collect();
    println!("regenerating evidence shards: {}", selected.join(", "));
    regenerate(ctx, chip, selected, &directory, &linker, &output)?;
    for (name, old) in before {
        let new = store::read(&directory, &name);
        print!("{}", summary(&name, old.as_ref(), new.as_ref()));
    }
    print!("{}", render_untriaged(&directory, false)?);
    Ok(ExitCode::SUCCESS)
}

/// Print the chip-wide untriaged vendor locations of `chip`'s committed
/// shards, one per line.
pub fn untriaged(ctx: &Checkout, chip: &str) -> Result<ExitCode> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    print!("{}", render_untriaged(&directory, true)?);
    Ok(ExitCode::SUCCESS)
}

/// Per-function counts of the chip-wide untriaged locations of the shards
/// in `directory`, or every location when `all`.
fn render_untriaged(directory: &Path, all: bool) -> Result<String> {
    let shards = store::shards(directory)?;
    let locations = oer_vendor_evidence_shard::untriaged(&shards.iter().collect::<Vec<_>>());
    let mut functions = std::collections::BTreeMap::<&str, usize>::new();
    for location in &locations {
        *functions.entry(&location.function).or_default() += 1;
    }
    let mut text = format!(
        "chip-wide untriaged: {} locations in {} functions\n",
        locations.len(),
        functions.len()
    );
    if all {
        for location in &locations {
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
    Ok(text)
}

/// Repository files that differ between `revision` and the worktree.
fn changed_files(ctx: &Checkout, revision: &str) -> Result<Vec<PathBuf>> {
    let output = oer_process::capture(oer_process::git::command(&ctx.root).args([
        "diff",
        "--name-only",
        "-z",
        revision,
        "--",
    ]))?;
    let untracked = oer_process::capture(oer_process::git::command(&ctx.root).args([
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

/// What changed between two versions of shard `name`: its claims first,
/// then the recorded sources.
fn summary(name: &str, old: Option<&Index>, new: Option<&Index>) -> String {
    match (old, new) {
        (Some(old), Some(new)) => crate::diff::render(name, &crate::diff::differences(old, new)),
        (None, Some(_)) => format!("{name}:\n  new shard\n"),
        (_, None) => format!("{name}:\n  no readable shard\n"),
    }
}

/// Rerun every Blobray scenario shard of `chip` into `output` and fail
/// unless each equals the committed shard. Host stand shards compile vendor
/// source the check does not fetch and are not rerun.
pub fn check(
    ctx: &Checkout,
    chip: &str,
    scenarios: Vec<String>,
    changed_since: Option<String>,
    linker: PathBuf,
    output: PathBuf,
) -> Result<ExitCode> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    reject_report_sources(ctx, &directory)?;
    // An unreadable shard counts as a Blobray shard: checking it fails closed.
    let mut committed: Vec<String> = store::names(&directory)?
        .into_iter()
        .filter(|name| {
            store::read(&directory, name)
                .is_none_or(|shard| shard.command == oer_vendor_evidence_shard::BLOBRAY)
        })
        .collect();
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
                .partition(|name| match store::read(&directory, name) {
                    // An unreadable shard is checked: skipping fails closed.
                    None => true,
                    Some(shard) => store::records_any(&ctx.root, &shard, &changed),
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
    if committed.is_empty() {
        println!("no committed Blobray evidence shard of {chip} to rerun");
        return Ok(ExitCode::SUCCESS);
    }
    // Per chip, so checks of different chips keep their reruns apart.
    let rerun = ctx.root.join(&output).join(CHECK_INDEX).join(chip);
    if rerun.exists() {
        std::fs::remove_dir_all(&rerun)?;
    }
    std::fs::create_dir_all(&rerun)?;
    Blobray {
        ctx,
        chip,
        linker: resolve(&linker)?,
        output: &output,
    }
    .produce(&committed, &rerun)?;
    let differing: Vec<&String> = committed
        .iter()
        .filter(|name| {
            let (old, new) = (store::read(&directory, name), store::read(&rerun, name));
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
            "evidence shards differ from their rerun: {}; regenerate them with `cargo verification evidence --chip {chip} {}`",
            names.join(", "),
            names.join(" ")
        );
        Ok(ExitCode::FAILURE)
    }
}

/// Directory below the output root a check reruns each chip's shards into.
const CHECK_INDEX: &str = "check-index";

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
