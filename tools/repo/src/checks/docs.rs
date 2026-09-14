//! Source-only API documentation, examples, links and capability views.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use cargo_metadata::TargetKind;
use pulldown_cmark::{BrokenLink, CowStr, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{TARGET, common};
use crate::{Context, Result, cargo, paths, process};

const OUTPUT_OWNER: &str = "oer-xtask-docs-v1\n";
const STRICT_RUSTDOC_FLAGS: &[&str] = &[
    "-D",
    "warnings",
    "-D",
    "rustdoc::unescaped-backticks",
    "-D",
    "rustdoc::redundant-explicit-links",
];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Purpose {
    PublicRustdoc,
    PrivateRustdoc,
    HostDoctest,
    McuCompileConsumer,
}

impl Purpose {
    const fn label(self) -> &'static str {
        match self {
            Self::PublicRustdoc => "public-rustdoc",
            Self::PrivateRustdoc => "private-rustdoc",
            Self::HostDoctest => "host-doctest",
            Self::McuCompileConsumer => "mcu-compile-consumer",
        }
    }
}

#[derive(Clone, Debug)]
struct Job {
    purpose: Purpose,
    configuration: common::CargoConfiguration,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Inapplicable {
    package: String,
    target: String,
    action: String,
    reason: String,
}

#[derive(Clone, Debug)]
struct CatalogGroup {
    chip: String,
    catalogs: Vec<PathBuf>,
    programs: Vec<PathBuf>,
}

struct Plan {
    source_package_count: usize,
    workspace_count: usize,
    jobs: Vec<Job>,
    inapplicable: Vec<Inapplicable>,
    documents: Vec<PathBuf>,
    catalogs: Vec<CatalogGroup>,
}

struct ToolchainIdentity {
    cargo: String,
    rustc: String,
    rustdoc: String,
    release: String,
    host: String,
}

#[derive(Debug)]
struct LinkSummary {
    documents: usize,
    local_links: usize,
    external_not_checked: usize,
    anchors: usize,
}

pub fn run(ctx: &Context, list: bool) -> Result<()> {
    let toolchain = toolchain_identity(ctx)?;
    let plan = build_plan(ctx, &toolchain.host)?;
    let output = acquire_output(ctx)?;
    let plan_value = plan_json(ctx, &plan, &toolchain)?;
    write_json(&output.join("job-plan.json"), &plan_value)?;
    if list {
        print_plan(ctx, &plan)?;
        println!(
            "docs plan: workspaces={} packages={} jobs={} documents={} catalog-groups={} inapplicable={}",
            plan.workspace_count,
            plan.source_package_count,
            plan.jobs.len(),
            plan.documents.len(),
            plan.catalogs.len(),
            plan.inapplicable.len()
        );
        return Ok(());
    }

    let report = output.join("report.json");
    if report.try_exists()? {
        fs::remove_file(&report)?;
    }
    reset_generated_outputs(&output)?;
    let before_status = repository_status(ctx)?;
    let before_locks = lock_identities(&plan)?;
    let source_sha256 = source_identity(ctx, &plan)?;

    let mut rustdoc_outputs = Vec::new();
    for job in plan.jobs.iter().filter(|job| {
        matches!(
            job.purpose,
            Purpose::PublicRustdoc | Purpose::PrivateRustdoc
        )
    }) {
        rustdoc_outputs.push(run_rustdoc(ctx, &output, job)?);
    }
    for job in plan
        .jobs
        .iter()
        .filter(|job| job.purpose == Purpose::HostDoctest)
    {
        run_doctest(ctx, &output, job)?;
    }
    for job in plan
        .jobs
        .iter()
        .filter(|job| job.purpose == Purpose::McuCompileConsumer)
    {
        run_consumer(ctx, &output, job)?;
    }

    let generated = run_catalogs(ctx, &output, &plan.catalogs)?;
    let mut documents = plan.documents.clone();
    documents.extend(generated);
    let links = check_markdown(ctx, &documents)?;

    if repository_status(ctx)? != before_status {
        return Err("docs gate changed tracked or unrelated untracked source state".into());
    }
    if lock_identities(&plan)? != before_locks {
        return Err("docs gate changed a source Cargo.lock".into());
    }
    if source_identity(ctx, &plan)? != source_sha256 {
        return Err("docs gate changed a source input".into());
    }

    let counts = purpose_counts(&plan.jobs);
    let report_value = json!({
        "schema": 1,
        "source": {
            "head": repository_head(ctx)?,
            "dirty": !before_status.is_empty(),
            "sha256": source_sha256,
        },
        "toolchain": toolchain_json(&toolchain),
        "target": TARGET,
        "coverage": {
            "workspaces": plan.workspace_count,
            "packages": plan.source_package_count,
            "configurations": unique_configuration_count(ctx, &plan.jobs)?,
            "public-rustdoc": counts.get(Purpose::PublicRustdoc.label()).copied().unwrap_or(0),
            "private-rustdoc": counts.get(Purpose::PrivateRustdoc.label()).copied().unwrap_or(0),
            "host-doctest": counts.get(Purpose::HostDoctest.label()).copied().unwrap_or(0),
            "mcu-compile-consumer": counts.get(Purpose::McuCompileConsumer.label()).copied().unwrap_or(0),
            "catalog-groups": plan.catalogs.len(),
            "catalogs": plan.catalogs.iter().map(|group| group.catalogs.len()).sum::<usize>(),
            "programs": plan.catalogs.iter().map(|group| group.programs.len()).sum::<usize>(),
            "documents": links.documents,
            "local-links": links.local_links,
            "external-not-checked": links.external_not_checked,
            "anchors": links.anchors,
            "inapplicable": plan.inapplicable.len(),
        },
        "outputs": {
            "job-plan": relative(ctx, &output.join("job-plan.json"))?,
            "public-private-rustdoc": rustdoc_outputs.iter().map(|path| relative(ctx, path)).collect::<Result<Vec<_>>>()?,
            "static-catalogs": plan.catalogs.iter().map(|group| relative(ctx, &output.join("catalogs").join(&group.chip).join("forward"))).collect::<Result<Vec<_>>>()?,
        },
        "inapplicable": plan.inapplicable.iter().map(|entry| json!({
            "package": entry.package,
            "target": entry.target,
            "action": entry.action,
            "reason": entry.reason,
        })).collect::<Vec<_>>(),
        "policy": {
            "runtime-evidence-loaded": false,
            "readiness-evaluated": false,
            "hardware-operations": false,
            "external-links": "not-checked",
        },
    });
    write_json(&report, &report_value)?;
    println!(
        "docs gate passed: workspaces={} packages={} public={} private={} doctests={} consumers={} catalogs={} programs={} documents={} local-links={} external-not-checked={} inapplicable={}",
        plan.workspace_count,
        plan.source_package_count,
        counts
            .get(Purpose::PublicRustdoc.label())
            .copied()
            .unwrap_or(0),
        counts
            .get(Purpose::PrivateRustdoc.label())
            .copied()
            .unwrap_or(0),
        counts
            .get(Purpose::HostDoctest.label())
            .copied()
            .unwrap_or(0),
        counts
            .get(Purpose::McuCompileConsumer.label())
            .copied()
            .unwrap_or(0),
        plan.catalogs
            .iter()
            .map(|group| group.catalogs.len())
            .sum::<usize>(),
        plan.catalogs
            .iter()
            .map(|group| group.programs.len())
            .sum::<usize>(),
        links.documents,
        links.local_links,
        links.external_not_checked,
        plan.inapplicable.len(),
    );
    Ok(())
}

fn build_plan(ctx: &Context, host: &str) -> Result<Plan> {
    let packages = common::source_packages(ctx)?;
    let workspace_count = packages
        .iter()
        .map(|item| &item.workspace_manifest)
        .collect::<BTreeSet<_>>()
        .len();
    let mut jobs = Vec::new();
    let mut inapplicable = BTreeSet::new();
    for item in &packages {
        let class = common::classification(&item.package)?;
        let target_triple = match documentation_target(&item.package)? {
            Some("host") => host,
            Some("chip") => TARGET,
            Some(value) => {
                return Err(format!(
                    "package {} has unknown open-radio.rustdoc-target {value}",
                    item.package.name
                )
                .into());
            }
            None => match class.platform {
                common::Platform::Chip(chip) => {
                    if chip != "esp32s31" {
                        return Err(format!(
                            "package {} has no documentation target mapping for chip {chip}",
                            item.package.name
                        )
                        .into());
                    }
                    TARGET
                }
                common::Platform::Portable | common::Platform::Host => host,
            },
        };
        let profiles = common::documentation_profiles(&item.package)?;
        let build_profile = if class.layer == "application" {
            common::CargoBuildProfile::Release
        } else {
            common::CargoBuildProfile::Dev
        };
        let mut any_documented_target = false;
        for target in &item.package.targets {
            let selector = target_selector(target);
            if !target.doc || selector.is_none() {
                inapplicable.insert(Inapplicable {
                    package: item.package.name.to_string(),
                    target: target.name.clone(),
                    action: "rustdoc".into(),
                    reason: if target.doc {
                        "non-library/binary Cargo target".into()
                    } else {
                        "Cargo target declares doc=false".into()
                    },
                });
                continue;
            }
            any_documented_target = true;
            let selector = selector.expect("checked above");
            let mut applicable_profiles = 0;
            for features in &profiles {
                if !required_features_enabled(&item.package, features, &target.required_features) {
                    continue;
                }
                applicable_profiles += 1;
                let configuration = common::CargoConfiguration {
                    manifest: item.manifest.clone(),
                    workspace_manifest: item.workspace_manifest.clone(),
                    package: item.package.name.to_string(),
                    target: target_triple.into(),
                    features: features.clone(),
                    build_profile,
                    cargo_target: selector.clone(),
                };
                jobs.push(Job {
                    purpose: Purpose::PublicRustdoc,
                    configuration: configuration.clone(),
                });
                jobs.push(Job {
                    purpose: Purpose::PrivateRustdoc,
                    configuration: configuration.clone(),
                });
                if target.doctest
                    && target_triple == host
                    && matches!(selector, common::CargoTargetSelection::Lib(_))
                {
                    jobs.push(Job {
                        purpose: Purpose::HostDoctest,
                        configuration,
                    });
                }
            }
            if applicable_profiles == 0 {
                inapplicable.insert(Inapplicable {
                    package: item.package.name.to_string(),
                    target: target.name.clone(),
                    action: "rustdoc".into(),
                    reason: format!(
                        "target requires unsupported feature selection {:?}",
                        target.required_features
                    ),
                });
            }
            if target.doctest && target_triple != host {
                inapplicable.insert(Inapplicable {
                    package: item.package.name.to_string(),
                    target: target.name.clone(),
                    action: "host-doctest".into(),
                    reason: "chip API is checked by target rustdoc and MCU compile-only consumers; no cross-target execution".into(),
                });
            } else if !target.doctest && target.doc {
                inapplicable.insert(Inapplicable {
                    package: item.package.name.to_string(),
                    target: target.name.clone(),
                    action: "host-doctest".into(),
                    reason: "Cargo target declares doctest=false".into(),
                });
            }
        }
        if !any_documented_target {
            inapplicable.insert(Inapplicable {
                package: item.package.name.to_string(),
                target: "package".into(),
                action: "rustdoc".into(),
                reason: "package has no documentable library or binary target".into(),
            });
        }
    }
    for configuration in common::example_configurations(ctx, TARGET)? {
        jobs.push(Job {
            purpose: Purpose::McuCompileConsumer,
            configuration,
        });
    }
    jobs.sort_by(|left, right| {
        (left.purpose, &left.configuration).cmp(&(right.purpose, &right.configuration))
    });
    Ok(Plan {
        source_package_count: packages.len(),
        workspace_count,
        jobs,
        inapplicable: inapplicable.into_iter().collect(),
        documents: owned_documents(ctx, &packages)?,
        catalogs: catalog_groups(ctx)?,
    })
}

fn documentation_target(package: &cargo_metadata::Package) -> Result<Option<&str>> {
    package
        .metadata
        .get("open-radio")
        .and_then(|metadata| metadata.get("rustdoc-target"))
        .map(|value| {
            value.as_str().ok_or_else(|| {
                format!(
                    "package {} open-radio.rustdoc-target must be a string",
                    package.name
                )
                .into()
            })
        })
        .transpose()
}

fn target_selector(target: &cargo_metadata::Target) -> Option<common::CargoTargetSelection> {
    if target.kind.iter().any(|kind| {
        matches!(
            kind,
            TargetKind::Lib
                | TargetKind::RLib
                | TargetKind::DyLib
                | TargetKind::CDyLib
                | TargetKind::StaticLib
                | TargetKind::ProcMacro
        )
    }) {
        Some(common::CargoTargetSelection::Lib(target.name.clone()))
    } else if target.kind.contains(&TargetKind::Bin) {
        Some(common::CargoTargetSelection::Bin(target.name.clone()))
    } else {
        None
    }
}

fn required_features_enabled(
    package: &cargo_metadata::Package,
    flags: &[String],
    required: &[String],
) -> bool {
    if required.is_empty() || flags.iter().any(|flag| flag == "--all-features") {
        return true;
    }
    let no_default = flags.iter().any(|flag| flag == "--no-default-features");
    let mut enabled = BTreeSet::<&str>::new();
    if !no_default && let Some(defaults) = package.features.get("default") {
        enabled.extend(
            defaults
                .iter()
                .filter(|feature| package.features.contains_key(feature.as_str()))
                .map(String::as_str),
        );
    }
    if let Some(index) = flags.iter().position(|flag| flag == "--features")
        && let Some(features) = flags.get(index + 1)
    {
        enabled.extend(
            features
                .split(',')
                .filter(|feature| package.features.contains_key(*feature)),
        );
    }
    required
        .iter()
        .all(|feature| enabled.contains(feature.as_str()))
}

fn toolchain_identity(ctx: &Context) -> Result<ToolchainIdentity> {
    let specification: toml::Value =
        toml::from_str(&fs::read_to_string(ctx.root.join("rust-toolchain.toml"))?)?;
    let release = specification
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("channel"))
        .and_then(toml::Value::as_str)
        .ok_or("rust-toolchain.toml lacks toolchain.channel")?
        .to_owned();
    let targets = specification
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("targets"))
        .and_then(toml::Value::as_array)
        .ok_or("rust-toolchain.toml lacks toolchain.targets")?;
    if !declares_target(targets, TARGET) {
        return Err(format!("pinned toolchain does not declare target {TARGET}").into());
    }
    let cargo_output = process::capture(ctx.cargo().arg("-Vv"))?;
    let rustc_program = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let rustdoc_program = std::env::var_os("RUSTDOC").unwrap_or_else(|| "rustdoc".into());
    let rustc_output = process::capture(ctx.command(&rustc_program).arg("-vV"))?;
    let rustdoc_output = process::capture(ctx.command(&rustdoc_program).arg("-vV"))?;
    let cargo = String::from_utf8(cargo_output.stdout)?;
    let rustc = String::from_utf8(rustc_output.stdout)?;
    let rustdoc = String::from_utf8(rustdoc_output.stdout)?;
    for (name, output) in [("cargo", &cargo), ("rustc", &rustc), ("rustdoc", &rustdoc)] {
        validate_tool_release(name, output, &release)?;
    }
    let host = rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc did not report its host target")?
        .to_owned();
    Ok(ToolchainIdentity {
        cargo: first_line(&cargo),
        rustc: first_line(&rustc),
        rustdoc: first_line(&rustdoc),
        release,
        host,
    })
}

fn declares_target(targets: &[toml::Value], required: &str) -> bool {
    targets
        .iter()
        .any(|target| target.as_str() == Some(required))
}

fn validate_tool_release(name: &str, output: &str, expected: &str) -> Result<()> {
    let actual = output
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .or_else(|| {
            output
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
        })
        .ok_or_else(|| format!("{name} did not report a release"))?;
    if actual != expected {
        return Err(
            format!("{name} release {actual} does not match pinned toolchain {expected}").into(),
        );
    }
    Ok(())
}

fn first_line(output: &str) -> String {
    output.lines().next().unwrap_or_default().to_owned()
}

fn acquire_output(ctx: &Context) -> Result<PathBuf> {
    let output = ctx.root.join("target/docs/gate");
    let owner = output.join(".owner");
    if output.try_exists()? {
        if output.is_symlink() || !output.is_dir() {
            return Err(format!(
                "docs output is not an owned directory: {}",
                output.display()
            )
            .into());
        }
        if fs::read_to_string(&owner).ok().as_deref() != Some(OUTPUT_OWNER) {
            return Err(format!(
                "docs output exists without the expected ownership marker: {}",
                output.display()
            )
            .into());
        }
    } else {
        fs::create_dir_all(&output)?;
        fs::write(&owner, OUTPUT_OWNER)?;
    }
    if !output
        .canonicalize()?
        .starts_with(&ctx.root.canonicalize()?)
    {
        return Err("docs output escaped repository".into());
    }
    Ok(output)
}

fn owned_remove_dir(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root) || path == root {
        return Err(format!("refusing to remove unowned docs path: {}", path.display()).into());
    }
    if path.try_exists()? {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(format!("rustdoc output directory missing: {}", source.display()).into());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!(
                "rustdoc output contains unsupported file type: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn link_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(format!("rustdoc resource directory missing: {}", source.display()).into());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            link_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::hard_link(entry.path(), &target)
                .or_else(|_| fs::copy(entry.path(), &target).map(|_| ()))?;
        } else {
            return Err(format!(
                "rustdoc resources contain unsupported file type: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn reset_generated_outputs(output: &Path) -> Result<()> {
    for path in [output.join("rustdoc"), output.join("catalogs")] {
        owned_remove_dir(output, &path)?;
    }
    Ok(())
}

fn strict_rustdoc(command: &mut Command) {
    command.env_remove("RUSTDOCFLAGS").env(
        "CARGO_ENCODED_RUSTDOCFLAGS",
        STRICT_RUSTDOC_FLAGS.join("\u{1f}"),
    );
}

fn run_checked(command: &mut Command) -> Result<()> {
    let output = process::output(command, None)?;
    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}:\nstdout:\n{}\nstderr:\n{}",
            command.get_program().to_string_lossy(),
            output.status,
            diagnostic_tail(&output.stdout),
            diagnostic_tail(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn diagnostic_tail(bytes: &[u8]) -> Cow<'_, str> {
    const LIMIT: usize = 64 * 1024;
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..])
}

fn workspace_cache_id(ctx: &Context, configuration: &common::CargoConfiguration) -> Result<String> {
    let identity = format!(
        "{}\n{}\n{}",
        configuration
            .workspace_manifest
            .strip_prefix(&ctx.root)?
            .display(),
        configuration.target,
        configuration.build_profile.label()
    );
    Ok(format!("{:x}", Sha256::digest(identity.as_bytes()))[..20].to_owned())
}

fn run_rustdoc(ctx: &Context, output: &Path, job: &Job) -> Result<PathBuf> {
    let configuration = &job.configuration;
    eprintln!(
        "docs {}: package={} target={} features={:?} cargo-target={}",
        job.purpose.label(),
        configuration.package,
        configuration.target,
        configuration.features,
        configuration.cargo_target.label()
    );
    let _catalog = cargo::catalog_read(
        ctx,
        configuration
            .workspace_manifest
            .parent()
            .ok_or("workspace manifest has no parent")?,
    )?;
    let cache = output
        .join("cache/rustdoc")
        .join(job.purpose.label())
        .join(workspace_cache_id(ctx, configuration)?);
    let staging = cache.join(&configuration.target).join("doc");
    let mut command = ctx.cargo();
    command
        .args(["doc", "--no-deps", "--target-dir"])
        .arg(&cache);
    apply_documentation_environment(&mut command, output)?;
    configuration.apply(&mut command);
    if job.purpose == Purpose::PrivateRustdoc {
        command.arg("--document-private-items");
    }
    strict_rustdoc(&mut command);
    run_checked(&mut command)?;
    let snapshot = output
        .join("rustdoc")
        .join(job.purpose.label())
        .join(configuration.id(&ctx.root)?);
    owned_remove_dir(output, &snapshot)?;
    snapshot_rustdoc(&staging, &snapshot, &configuration.cargo_target)?;
    Ok(snapshot)
}

fn snapshot_rustdoc(
    staging: &Path,
    snapshot: &Path,
    target: &common::CargoTargetSelection,
) -> Result<()> {
    let target_name = match target {
        common::CargoTargetSelection::Lib(name) | common::CargoTargetSelection::Bin(name) => name,
        common::CargoTargetSelection::DefaultTargets => {
            return Err("rustdoc snapshot requires an explicit Cargo target".into());
        }
    };
    let crate_name = target_name.replace('-', "_");
    copy_tree(&staging.join(&crate_name), &snapshot.join(&crate_name))?;
    let static_files = staging.join("static.files");
    if static_files.is_dir() {
        link_tree(&static_files, &snapshot.join("static.files"))?;
    }
    let source = staging.join("src").join(&crate_name);
    if source.is_dir() {
        copy_tree(&source, &snapshot.join("src").join(&crate_name))?;
    }
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), snapshot.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn apply_documentation_environment(command: &mut Command, output: &Path) -> Result<()> {
    // The bootstrap's real build embeds the packed stage-two image. Rustdoc
    // type-checks the binary but never links or executes it, so an owned empty
    // compile input satisfies include_bytes! without manufacturing firmware.
    let inputs = output.join("compile-inputs");
    fs::create_dir_all(&inputs)?;
    let stage_two = inputs.join("empty-stage-two.bin");
    if !stage_two.try_exists()? {
        fs::write(&stage_two, [])?;
    }
    command.env("PSRAM_RUNTIME_BIN", stage_two);
    Ok(())
}

fn run_doctest(ctx: &Context, output: &Path, job: &Job) -> Result<()> {
    let configuration = &job.configuration;
    let _catalog = cargo::catalog_read(
        ctx,
        configuration
            .workspace_manifest
            .parent()
            .ok_or("workspace manifest has no parent")?,
    )?;
    let cache = output
        .join("cache/doctest")
        .join(workspace_cache_id(ctx, configuration)?);
    let mut command = ctx.cargo();
    command.args(["test", "--doc", "--target-dir"]).arg(cache);
    let mut doctest_configuration = configuration.clone();
    doctest_configuration.cargo_target = common::CargoTargetSelection::DefaultTargets;
    doctest_configuration.apply(&mut command);
    strict_rustdoc(&mut command);
    run_checked(&mut command)
}

fn run_consumer(ctx: &Context, output: &Path, job: &Job) -> Result<()> {
    let configuration = &job.configuration;
    let _catalog = cargo::catalog_read(
        ctx,
        configuration
            .workspace_manifest
            .parent()
            .ok_or("workspace manifest has no parent")?,
    )?;
    let cache = output.join("consumers").join(configuration.id(&ctx.root)?);
    let mut command = ctx.cargo();
    command.args(["check", "--target-dir"]).arg(cache);
    configuration.apply(&mut command);
    run_checked(&mut command)
}

fn catalog_groups(ctx: &Context) -> Result<Vec<CatalogGroup>> {
    let mut groups = BTreeMap::<String, CatalogGroup>::new();
    for path in paths::source_files(ctx)? {
        let relative = path.strip_prefix(&ctx.root)?;
        let parts = relative.components().collect::<Vec<_>>();
        let classify = |owner: &str| {
            parts.len() == 4
                && parts[0] == Component::Normal("qualification".as_ref())
                && parts[1] == Component::Normal(owner.as_ref())
                && path
                    .extension()
                    .is_some_and(|extension| extension == "toml")
        };
        let destination = if classify("catalog") {
            Some(true)
        } else if classify("targets") {
            Some(false)
        } else {
            None
        };
        let Some(is_catalog) = destination else {
            continue;
        };
        let chip = parts[2]
            .as_os_str()
            .to_str()
            .ok_or("qualification chip directory is not Unicode")?
            .to_owned();
        let group = groups.entry(chip.clone()).or_insert_with(|| CatalogGroup {
            chip,
            catalogs: Vec::new(),
            programs: Vec::new(),
        });
        if is_catalog {
            group.catalogs.push(relative.to_owned());
        } else {
            group.programs.push(relative.to_owned());
        }
    }
    for group in groups.values_mut() {
        group.catalogs.sort();
        group.programs.sort();
        if group.catalogs.is_empty() {
            return Err(format!(
                "qualification target group {} has no source catalog",
                group.chip
            )
            .into());
        }
    }
    if groups.is_empty() {
        return Err("no qualification catalog groups found".into());
    }
    Ok(groups.into_values().collect())
}

fn qualification_binary(ctx: &Context, output: &Path) -> Result<PathBuf> {
    let target = output.join("cache/qualification");
    process::run(ctx.cargo().env("CARGO_TARGET_DIR", &target).args([
        "build",
        "--locked",
        "--offline",
        "--profile",
        "qualification",
        "--package",
        "open-esp-radio-qualification-check",
        "--bin",
        "open-esp-radio-qualification-check",
    ]))?;
    let binary = target.join("qualification").join(format!(
        "open-esp-radio-qualification-check{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !binary.is_file() {
        return Err(format!("qualification binary missing: {}", binary.display()).into());
    }
    Ok(binary)
}

fn qualification_command(
    ctx: &Context,
    binary: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<String> {
    let mut command = ctx.command(binary);
    command.args(arguments).args(["--root"]).arg(&ctx.root);
    let output = process::capture(&mut command)?;
    let stdout = String::from_utf8(output.stdout)?;
    print!("{stdout}");
    Ok(stdout)
}

fn run_catalogs(ctx: &Context, output: &Path, groups: &[CatalogGroup]) -> Result<Vec<PathBuf>> {
    let binary = qualification_binary(ctx, output)?;
    run_catalog_actions(groups, |group, action| {
        let mut arguments = vec![OsString::from("catalog")];
        match action {
            CatalogAction::CheckCatalogs => {
                arguments.push("check".into());
                for catalog in &group.catalogs {
                    arguments.push("--catalog".into());
                    arguments.push(catalog.as_os_str().to_owned());
                }
            }
            CatalogAction::CheckProgram(program) => {
                arguments.extend(["check".into(), "--manifest".into()]);
                arguments.push(program.as_os_str().to_owned());
            }
            CatalogAction::Render {
                catalogs,
                destination,
            } => {
                arguments.push("render".into());
                for catalog in catalogs {
                    arguments.push("--catalog".into());
                    arguments.push(catalog.as_os_str().to_owned());
                }
                arguments.push("--out".into());
                arguments.push(destination.as_os_str().to_owned());
            }
        }
        qualification_command(ctx, &binary, arguments).map(|_| ())
    })?;

    let mut generated = Vec::new();
    for group in groups {
        let base = output.join("catalogs").join(&group.chip);
        let forward = base.join("forward");
        let reverse = base.join("reverse");
        for name in [
            "domain-inventory.md",
            "capability-catalog.md",
            "migration-map.md",
        ] {
            let left = forward.join(name);
            let right = reverse.join(name);
            if fs::read(&left)? != fs::read(&right)? {
                return Err(format!(
                    "catalog render is input-order dependent for {}",
                    left.display()
                )
                .into());
            }
            generated.push(left);
        }
    }
    Ok(generated)
}

enum CatalogAction<'a> {
    CheckCatalogs,
    CheckProgram(&'a Path),
    Render {
        catalogs: &'a [PathBuf],
        destination: PathBuf,
    },
}

fn run_catalog_actions(
    groups: &[CatalogGroup],
    mut execute: impl FnMut(&CatalogGroup, CatalogAction<'_>) -> Result<()>,
) -> Result<()> {
    for group in groups {
        execute(group, CatalogAction::CheckCatalogs)?;
        for program in &group.programs {
            execute(group, CatalogAction::CheckProgram(program))?;
        }
        let base = PathBuf::from("target/docs/gate/catalogs").join(&group.chip);
        execute(
            group,
            CatalogAction::Render {
                catalogs: &group.catalogs,
                destination: base.join("forward"),
            },
        )?;
        let mut reversed = group.catalogs.clone();
        reversed.reverse();
        execute(
            group,
            CatalogAction::Render {
                catalogs: &reversed,
                destination: base.join("reverse"),
            },
        )?;
    }
    Ok(())
}

fn owned_documents(ctx: &Context, packages: &[common::SourcePackage]) -> Result<Vec<PathBuf>> {
    let tracked = paths::tracked_files(ctx)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut documents = BTreeSet::new();
    for path in paths::source_files(ctx)? {
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let relative = path.strip_prefix(&ctx.root)?;
        let owner_name = path.file_name().is_some_and(|name| {
            name == "README.md" || name == "FEATURES.md" || name == "OWNERSHIP.md"
        });
        if tracked.contains(&path) || relative.starts_with("docs") || owner_name {
            documents.insert(path);
        }
    }
    for item in packages {
        if let Some(readme) = &item.package.readme {
            let path = item
                .manifest
                .parent()
                .ok_or("package manifest has no parent")?
                .join(readme.as_std_path());
            if !path.is_file() {
                return Err(format!(
                    "package {} readme is missing: {}",
                    item.package.name,
                    path.display()
                )
                .into());
            }
            documents.insert(path.canonicalize()?);
        }
    }
    if documents.is_empty() {
        return Err("no owned Markdown documents found".into());
    }
    Ok(documents.into_iter().collect())
}

fn relative(ctx: &Context, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(&ctx.root)?
        .to_str()
        .ok_or("repository path is not Unicode")?
        .replace('\\', "/"))
}

fn configuration_json(ctx: &Context, configuration: &common::CargoConfiguration) -> Result<Value> {
    Ok(json!({
        "id": configuration.id(&ctx.root)?,
        "workspace": relative(ctx, &configuration.workspace_manifest)?,
        "manifest": relative(ctx, &configuration.manifest)?,
        "package": configuration.package,
        "target": configuration.target,
        "features": configuration.features,
        "build-profile": configuration.build_profile.label(),
        "cargo-target": configuration.cargo_target.label(),
    }))
}

fn toolchain_json(identity: &ToolchainIdentity) -> Value {
    json!({
        "cargo": identity.cargo,
        "rustc": identity.rustc,
        "rustdoc": identity.rustdoc,
        "release": identity.release,
        "host": identity.host,
    })
}

fn plan_json(ctx: &Context, plan: &Plan, toolchain: &ToolchainIdentity) -> Result<Value> {
    Ok(json!({
        "schema": 1,
        "toolchain": toolchain_json(toolchain),
        "target": TARGET,
        "workspaces": plan.workspace_count,
        "packages": plan.source_package_count,
        "jobs": plan.jobs.iter().map(|job| Ok(json!({
            "purpose": job.purpose.label(),
            "configuration": configuration_json(ctx, &job.configuration)?,
        }))).collect::<Result<Vec<_>>>()?,
        "documents": plan.documents.iter().map(|path| relative(ctx, path)).collect::<Result<Vec<_>>>()?,
        "catalog-groups": plan.catalogs.iter().map(|group| json!({
            "chip": group.chip,
            "catalogs": group.catalogs.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
            "programs": group.programs.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
            "actions": ["catalog-check", "program-check", "render-forward", "render-reversed"],
        })).collect::<Vec<_>>(),
        "inapplicable": plan.inapplicable.iter().map(|entry| json!({
            "package": entry.package,
            "target": entry.target,
            "action": entry.action,
            "reason": entry.reason,
        })).collect::<Vec<_>>(),
    }))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().ok_or("JSON output has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or("JSON output name is not Unicode")?,
        std::process::id()
    ));
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn print_plan(ctx: &Context, plan: &Plan) -> Result<()> {
    for job in &plan.jobs {
        println!(
            "DOC-JOB\tpurpose={}\tid={}\tmanifest={}\tpackage={}\ttarget={}\tfeatures={:?}\tprofile={}\tcargo-target={}",
            job.purpose.label(),
            job.configuration.id(&ctx.root)?,
            relative(ctx, &job.configuration.manifest)?,
            job.configuration.package,
            job.configuration.target,
            job.configuration.features,
            job.configuration.build_profile.label(),
            job.configuration.cargo_target.label(),
        );
    }
    for entry in &plan.inapplicable {
        println!(
            "DOC-N/A\tpackage={}\ttarget={}\taction={}\treason={}",
            entry.package, entry.target, entry.action, entry.reason
        );
    }
    for group in &plan.catalogs {
        println!(
            "DOC-CATALOG\tchip={}\tcatalogs={}\tprograms={}",
            group.chip,
            group.catalogs.len(),
            group.programs.len()
        );
    }
    Ok(())
}

fn purpose_counts(jobs: &[Job]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for job in jobs {
        *counts.entry(job.purpose.label()).or_default() += 1;
    }
    counts
}

fn unique_configuration_count(ctx: &Context, jobs: &[Job]) -> Result<usize> {
    Ok(jobs
        .iter()
        .map(|job| job.configuration.id(&ctx.root))
        .collect::<Result<BTreeSet<_>>>()?
        .len())
}

fn repository_head(ctx: &Context) -> Result<String> {
    Ok(
        String::from_utf8(
            process::capture(ctx.command("git").args(["rev-parse", "HEAD"]))?.stdout,
        )?
        .trim()
        .to_owned(),
    )
}

fn repository_status(ctx: &Context) -> Result<Vec<u8>> {
    Ok(process::capture(ctx.command("git").args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
    ]))?
    .stdout)
}

fn file_sha256(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn lock_identities(plan: &Plan) -> Result<BTreeMap<PathBuf, String>> {
    let mut locks = BTreeMap::new();
    for workspace in plan
        .jobs
        .iter()
        .map(|job| &job.configuration.workspace_manifest)
        .collect::<BTreeSet<_>>()
    {
        let lock = workspace
            .parent()
            .ok_or("workspace has no parent")?
            .join("Cargo.lock");
        if lock.is_file() {
            locks.insert(lock.clone(), file_sha256(&lock)?);
        }
    }
    Ok(locks)
}

fn source_identity(ctx: &Context, plan: &Plan) -> Result<String> {
    let owned_documents = plan.documents.iter().collect::<BTreeSet<_>>();
    let mut hasher = Sha256::new();
    for path in paths::source_files(ctx)? {
        let include = owned_documents.contains(&path)
            || path
                .extension()
                .is_some_and(|extension| extension == "rs" || extension == "toml")
            || path.file_name().is_some_and(|name| name == "Cargo.lock");
        if !include {
            continue;
        }
        let relative = path.strip_prefix(&ctx.root)?;
        hasher.update(relative.as_os_str().as_encoded_bytes());
        hasher.update([0]);
        hasher.update(fs::read(&path)?);
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Default)]
struct ParsedMarkdown {
    anchors: BTreeSet<String>,
    links: Vec<String>,
    undefined_references: Vec<String>,
}

fn parse_markdown(text: &str) -> ParsedMarkdown {
    let mut undefined_references = Vec::new();
    let mut callback = |broken: BrokenLink<'_>| {
        undefined_references.push(broken.reference.to_string());
        Some((CowStr::from("#"), CowStr::from("")))
    };
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let parser = Parser::new_with_broken_link_callback(text, options, Some(&mut callback));
    let mut anchors = BTreeSet::new();
    let mut duplicate_headings = BTreeMap::<String, usize>::new();
    let mut links = Vec::new();
    let mut heading: Option<(Option<String>, String)> = None;
    for event in parser {
        match event {
            Event::Start(Tag::Heading { id, .. }) => {
                heading = Some((id.map(|id| id.to_string()), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((explicit, text)) = heading.take() {
                    let base = explicit.unwrap_or_else(|| heading_slug(&text));
                    let count = duplicate_headings.entry(base.clone()).or_default();
                    let anchor = if *count == 0 {
                        base
                    } else {
                        format!("{base}-{count}")
                    };
                    *count += 1;
                    anchors.insert(anchor);
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = &mut heading {
                    heading.push_str(&text);
                }
            }
            Event::Start(Tag::Link { dest_url, .. })
            | Event::Start(Tag::Image { dest_url, .. }) => links.push(dest_url.to_string()),
            Event::Html(html) | Event::InlineHtml(html) => {
                anchors.extend(html_anchors(&html));
            }
            _ => {}
        }
    }
    ParsedMarkdown {
        anchors,
        links,
        undefined_references,
    }
}

fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() || matches!(character, '-' | '_') {
            slug.push(character);
        } else if character.is_whitespace() {
            slug.push('-');
        }
    }
    slug
}

fn html_anchors(html: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("<a") {
        let start = cursor + offset + 2;
        let Some(end_offset) = html[start..].find('>') else {
            break;
        };
        let end = start + end_offset;
        if let Ok(attributes) = std::str::from_utf8(&bytes[start..end]) {
            for name in ["id", "name"] {
                if let Some(value) = html_attribute(attributes, name) {
                    anchors.push(value);
                }
            }
        }
        cursor = end + 1;
    }
    anchors
}

fn html_attribute(attributes: &str, wanted: &str) -> Option<String> {
    let mut cursor = 0;
    let bytes = attributes.as_bytes();
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'-' | b'_'))
        {
            cursor += 1;
        }
        let name = &attributes[name_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            cursor += usize::from(cursor < bytes.len());
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = bytes
            .get(cursor)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        if quote.is_some() {
            cursor += 1;
        }
        let value_start = cursor;
        while cursor < bytes.len()
            && quote.map_or_else(
                || !bytes[cursor].is_ascii_whitespace(),
                |quote| bytes[cursor] != quote,
            )
        {
            cursor += 1;
        }
        let value = &attributes[value_start..cursor];
        if quote.is_some() && cursor < bytes.len() {
            cursor += 1;
        }
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value.to_owned());
        }
    }
    None
}

fn check_markdown(ctx: &Context, initial: &[PathBuf]) -> Result<LinkSummary> {
    let root = ctx.root.canonicalize()?;
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    let mut documents = BTreeMap::<PathBuf, ParsedMarkdown>::new();
    while let Some(path) = queue.pop_front() {
        let path = path.canonicalize()?;
        if documents.contains_key(&path) {
            continue;
        }
        if !path.starts_with(&root) {
            return Err(format!("Markdown document escaped repository: {}", path.display()).into());
        }
        let parsed = parse_markdown(&fs::read_to_string(&path)?);
        if !parsed.undefined_references.is_empty() {
            return Err(format!(
                "Markdown document {} has undefined references: {:?}",
                path.strip_prefix(&root)?.display(),
                parsed.undefined_references
            )
            .into());
        }
        for destination in &parsed.links {
            if let Some((target, _)) = local_destination(&root, &path, destination)?
                && target
                    .extension()
                    .is_some_and(|extension| extension == "md")
                && target.is_file()
                && !documents.contains_key(&target)
            {
                queue.push_back(target);
            }
        }
        documents.insert(path, parsed);
    }

    let mut local_links = 0;
    let mut external_not_checked = 0;
    for (document, parsed) in &documents {
        for destination in &parsed.links {
            let Some((target, fragment)) = local_destination(&root, document, destination)? else {
                external_not_checked += 1;
                continue;
            };
            local_links += 1;
            if !target.try_exists()? {
                return Err(format!(
                    "Markdown link from {} has missing local target {destination:?}",
                    document.strip_prefix(&root)?.display()
                )
                .into());
            }
            let canonical = target.canonicalize()?;
            if !canonical.starts_with(&root) {
                return Err(format!(
                    "Markdown link from {} escapes repository: {destination:?}",
                    document.strip_prefix(&root)?.display()
                )
                .into());
            }
            if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
                check_fragment(&root, document, &canonical, &fragment, &documents)?;
            }
        }
    }
    Ok(LinkSummary {
        documents: documents.len(),
        local_links,
        external_not_checked,
        anchors: documents
            .values()
            .map(|document| document.anchors.len())
            .sum(),
    })
}

fn local_destination(
    root: &Path,
    document: &Path,
    destination: &str,
) -> Result<Option<(PathBuf, Option<String>)>> {
    let destination = destination.trim();
    if destination.is_empty() {
        return Ok(Some((document.to_owned(), None)));
    }
    if has_url_scheme(destination) || destination.starts_with("//") {
        return Ok(None);
    }
    let (path, fragment) = destination
        .split_once('#')
        .map_or((destination, None), |(path, fragment)| {
            (path, Some(fragment))
        });
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let path = percent_decode(path)?;
    let fragment = fragment.map(percent_decode).transpose()?;
    let joined = if path.is_empty() {
        document.to_owned()
    } else if let Some(path) = path.strip_prefix('/') {
        root.join(path)
    } else {
        document
            .parent()
            .ok_or("Markdown document has no parent")?
            .join(path.as_ref())
    };
    let normalized = lexical_normalize(&joined)?;
    if !normalized.starts_with(root) {
        return Err(format!("Markdown link escapes repository: {destination:?}").into());
    }
    Ok(Some((normalized, fragment.map(Cow::into_owned))))
}

fn has_url_scheme(destination: &str) -> bool {
    let Some((scheme, _)) = destination.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn percent_decode(value: &str) -> Result<Cow<'_, str>> {
    if !value.as_bytes().contains(&b'%') {
        return Ok(Cow::Borrowed(value));
    }
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(format!("invalid percent encoding in Markdown link {value:?}").into());
            };
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Ok(Cow::Owned(String::from_utf8(output)?))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn lexical_normalize(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(
                        format!("path escapes its filesystem root: {}", path.display()).into(),
                    );
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn check_fragment(
    root: &Path,
    source: &Path,
    target: &Path,
    fragment: &str,
    documents: &BTreeMap<PathBuf, ParsedMarkdown>,
) -> Result<()> {
    if target.is_dir() {
        return Err(format!(
            "Markdown link from {} uses an anchor on directory {}",
            source.strip_prefix(root)?.display(),
            target.strip_prefix(root)?.display()
        )
        .into());
    }
    if target
        .extension()
        .is_some_and(|extension| extension == "md")
    {
        let parsed = documents.get(target).ok_or_else(|| {
            format!(
                "linked Markdown target was not parsed: {}",
                target.display()
            )
        })?;
        if !parsed.anchors.contains(fragment) {
            return Err(format!(
                "Markdown link from {} has missing anchor #{fragment} in {}",
                source.strip_prefix(root).unwrap_or(source).display(),
                target.strip_prefix(root).unwrap_or(target).display()
            )
            .into());
        }
        return Ok(());
    }
    if matches!(
        target.extension().and_then(|extension| extension.to_str()),
        Some("rs" | "toml")
    ) {
        check_source_line_fragment(target, fragment)?;
        return Ok(());
    }
    if matches!(
        target.extension().and_then(|extension| extension.to_str()),
        Some("html" | "htm")
    ) {
        let anchors = html_anchors(&fs::read_to_string(target)?);
        if anchors.iter().any(|anchor| anchor == fragment) {
            return Ok(());
        }
    }
    Err(format!(
        "Markdown link from {} has unsupported or missing fragment #{fragment} in {}",
        source.strip_prefix(root).unwrap_or(source).display(),
        target.strip_prefix(root).unwrap_or(target).display()
    )
    .into())
}

fn check_source_line_fragment(path: &Path, fragment: &str) -> Result<()> {
    let Some(lines) = fragment.strip_prefix('L') else {
        return Err(
            format!("source link fragment must use GitHub line syntax: #{fragment}").into(),
        );
    };
    let (start, end) = lines
        .split_once("-L")
        .map_or((lines, lines), |(start, end)| (start, end));
    let start: usize = start.parse()?;
    let end: usize = end.parse()?;
    let line_count = fs::read_to_string(path)?.lines().count();
    if start == 0 || end < start || end > line_count {
        return Err(format!(
            "source link fragment #{fragment} is outside {} lines in {}",
            line_count,
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
