//! Compute a chip's derived vendor evidence index for the checkout.
//!
//! Each shard records the digests of the sources its verdicts depend on, so a
//! shard is stale exactly when one of them changed; the index is untracked
//! derived data, computed whole or for named scenarios.
//!
//! This module decides which producer computes which shard: the Blobray
//! scenario engine ([`super::scenario`]) or a host stand below
//! `verification/<chip>/host/` (its own `shard` command);
//! `cargo verification evidence` calls it.
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

/// Fail when a shard of the index `directory` records a file of a report
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

/// Compute `chip`'s derived evidence index ([`directory`]) for the checkout:
/// every shard when no scenario is named, after fetching the pins and
/// building the firmware inputs from their catalog recipes, else only the
/// named shards. Fails when a scenario does not match.
pub fn run(
    ctx: &Checkout,
    chip: &str,
    scenarios: Vec<String>,
    linker: Option<PathBuf>,
    output: PathBuf,
) -> Result<ExitCode> {
    // Ubuntu's LLD 18 fails these RV32 links: the toolchain's LLD is the default.
    let linker = match linker {
        Some(linker) => linker,
        None => oer_toolchain::lld()?,
    };
    let relative = directory(&ctx.root, chip)?;
    let directory = ctx.root.join(&relative);
    if scenarios.is_empty() {
        oer_vendor_artifacts::fetch_vendor_sources(&ctx.root, chip)?;
        for image in oer_vendor_artifacts::firmware_images(&ctx.root, chip)? {
            oer_image::esp_idf::catalog::build(&ctx.root, &image)?;
        }
        if directory.exists() {
            std::fs::remove_dir_all(&directory)?;
        }
        std::fs::create_dir_all(&directory)?;
        let mut every = host_stand::stands(
            &oer_repo::Model::load(&oer_repo::Repo::from_git(&ctx.root)?)?,
            chip,
        )?
        .into_keys()
        .collect::<Vec<_>>();
        // The Blobray producer runs every one of its scenarios as `all`.
        every.push("all".into());
        println!("computing the evidence index of {chip} into {relative}");
        regenerate(ctx, chip, every, &directory, &linker, &output)?;
        for name in store::names(&directory)? {
            print!(
                "{}",
                summary(&name, None, store::read(&directory, &name).as_ref())
            );
        }
    } else {
        std::fs::create_dir_all(&directory)?;
        let before: Vec<(String, Option<Index>)> = scenarios
            .iter()
            .map(|name| (name.clone(), store::read(&directory, name)))
            .collect();
        println!("regenerating evidence shards: {}", scenarios.join(", "));
        regenerate(ctx, chip, scenarios, &directory, &linker, &output)?;
        for (name, old) in before {
            let new = store::read(&directory, &name);
            print!("{}", summary(&name, old.as_ref(), new.as_ref()));
        }
    }
    reject_report_sources(ctx, &directory)?;
    print!("{}", render_untriaged(&directory, false)?);
    Ok(ExitCode::SUCCESS)
}

/// Compare `chip`'s computed index with `base`, the index `main` computed at
/// the change's merge base: print what each scenario's shard says
/// differently, then every vendor root the base claims and the index no
/// longer does ([`crate::diff::lost_roots`]); whether any is lost. A root the
/// chip's reviewed retirements name ([`Retired`]) is accepted, and one the
/// index still claims is an error.
pub fn compare(ctx: &Checkout, chip: &str, base: &Path) -> Result<bool> {
    let directory = ctx.root.join(directory(&ctx.root, chip)?);
    let mut names = store::names(base)?;
    names.extend(store::names(&directory)?);
    names.sort();
    names.dedup();
    for name in &names {
        let (old, new) = (store::read(base, name), store::read(&directory, name));
        let changed = match (&old, &new) {
            (Some(old), Some(new)) => !crate::diff::differences(old, new).is_empty(),
            _ => old.is_some() != new.is_some(),
        };
        if changed {
            print!("{}", summary(name, old.as_ref(), new.as_ref()));
        }
    }
    let (old, new) = (claimed_roots(base)?, claimed_roots(&directory)?);
    let retired = Retired::load(&ctx.root, chip)?;
    retired.check(chip, &new)?;
    let lost = crate::diff::lost_roots(&old, &new, &retired.roots);
    for (source, symbol) in &lost {
        println!(
            "{chip}: vendor root {source}::{symbol} is no longer claimed; if that is intended, retire it in {}",
            retired.path
        );
    }
    Ok(!lost.is_empty())
}

/// The vendor roots every shard of the index `directory` claims, read from
/// each entry's `source` and `symbol` only: an index an earlier format wrote
/// still names its roots, so a change of the shard format stays comparable.
/// A file without them makes the comparison impossible rather than an index
/// that claims nothing.
fn claimed_roots(directory: &Path) -> Result<std::collections::BTreeSet<crate::diff::Root>> {
    #[derive(serde::Deserialize)]
    struct Shard {
        entries: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        source: String,
        symbol: String,
    }
    let mut roots = std::collections::BTreeSet::new();
    for name in store::names(directory)? {
        let path = store::path(directory, &name);
        let shard: Shard = serde_json::from_slice(&std::fs::read(&path)?).map_err(|error| {
            format!(
                "{}: names no claimed vendor roots ({error}); the indexes cannot be compared",
                path.display()
            )
        })?;
        roots.extend(shard.entries.into_iter().map(|e| (e.source, e.symbol)));
    }
    Ok(roots)
}

/// A chip's reviewed retirements of vendor roots its index stopped claiming
/// on purpose, each with its reason.
#[derive(Debug)]
struct Retired {
    path: String,
    roots: std::collections::BTreeSet<crate::diff::Root>,
}

impl Retired {
    /// The retirements of `chip`'s project below `root`; none without the file.
    fn load(root: &Path, chip: &str) -> Result<Self> {
        let path = oer_vendor_artifacts::project::Project::new(root, chip)?.retired_roots();
        match std::fs::read_to_string(root.join(&path)) {
            Ok(text) => Self::parse(path, &text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                path,
                roots: Default::default(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn parse(path: String, text: &str) -> Result<Self> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            retired: Vec<Retirement>,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Retirement {
            source: String,
            symbol: String,
            reason: String,
        }
        let file: File = toml::from_str(text).map_err(|e| format!("{path}: {e}"))?;
        let mut roots = std::collections::BTreeSet::new();
        for retirement in file.retired {
            if retirement.reason.trim().is_empty() {
                return Err(format!("{path}: every retirement gives its reason").into());
            }
            roots.insert((retirement.source, retirement.symbol));
        }
        Ok(Self { path, roots })
    }

    /// An error when a retired root is still claimed: the retirement is wrong.
    fn check(
        &self,
        chip: &str,
        claimed: &std::collections::BTreeSet<crate::diff::Root>,
    ) -> Result<()> {
        match self.roots.iter().find(|root| claimed.contains(*root)) {
            Some((source, symbol)) => Err(format!(
                "{chip}: the retired vendor root {source}::{symbol} is still claimed; remove its retirement from {}",
                self.path
            )
            .into()),
            None => Ok(()),
        }
    }
}

/// Print the chip-wide untriaged vendor locations of `chip`'s derived
/// evidence index, one per line.
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

/// What changed between two versions of shard `name`: its claims first,
/// then the recorded sources.
fn summary(name: &str, old: Option<&Index>, new: Option<&Index>) -> String {
    match (old, new) {
        (Some(old), Some(new)) => crate::diff::render(name, &crate::diff::differences(old, new)),
        (None, Some(_)) => format!("{name}:\n  new shard\n"),
        (_, None) => format!("{name}:\n  no readable shard\n"),
    }
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
mod tests {
    use super::*;

    fn retired(text: &str) -> Result<Retired> {
        Retired::parse("decisions/retired.toml".into(), text)
    }

    #[test]
    fn a_retirement_names_its_root_and_reason_and_no_more() {
        let parsed =
            retired("[[retired]]\nsource = \"archive\"\nsymbol = \"old\"\nreason = \"replaced\"\n")
                .unwrap();
        let root = ("archive".to_owned(), "old".to_owned());
        assert!(parsed.roots.contains(&root));
        assert!(retired("").unwrap().roots.is_empty());
        let unreasoned = "[[retired]]\nsource = \"archive\"\nsymbol = \"old\"\nreason = \" \"\n";
        assert!(
            retired(unreasoned)
                .unwrap_err()
                .to_string()
                .contains("reason")
        );
        let unknown = "[[retired]]\nsource = \"a\"\nsymbol = \"b\"\nreason = \"c\"\nwhy = 1\n";
        assert!(retired(unknown).is_err());
        // A retired root the index still claims is a wrong retirement.
        let claimed = std::collections::BTreeSet::from([root]);
        let error = parsed.check("chip", &claimed).unwrap_err().to_string();
        assert!(error.contains("archive::old is still claimed"), "{error}");
        assert!(parsed.check("chip", &Default::default()).is_ok());
    }

    #[test]
    fn a_base_shard_of_another_format_still_names_its_roots() {
        let directory = tempfile::tempdir().unwrap();
        assert!(claimed_roots(directory.path()).unwrap().is_empty());
        // An earlier format: another schema, a field this one no longer has.
        let earlier = r#"{"schema": 1, "gone": true, "entries": [
            {"source": "archive", "symbol": "set_chan", "gone": 0}]}"#;
        std::fs::write(store::path(directory.path(), "radio"), earlier).unwrap();
        let roots = claimed_roots(directory.path()).unwrap();
        assert_eq!(roots, [("archive".into(), "set_chan".into())].into());
        std::fs::write(store::path(directory.path(), "radio"), r#"{"schema": 1}"#).unwrap();
        let error = claimed_roots(directory.path()).unwrap_err().to_string();
        assert!(error.contains("cannot be compared"), "{error}");
    }
}
