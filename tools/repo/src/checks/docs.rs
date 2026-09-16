//! Source-only API documentation, examples, links and capability views.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use cargo_metadata::TargetKind;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

mod markdown;
use markdown::check_markdown;

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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone)]
struct Plan {
    source_package_count: usize,
    workspace_count: usize,
    /// All source coverage obligations, including equivalent Cargo profiles.
    jobs: Vec<Job>,
    /// Distinct invocations selected without merging distinct feature graphs.
    execution_jobs: Vec<Job>,
    requirement_map: Vec<(Job, Job)>,
    inapplicable: Vec<Inapplicable>,
    documents: Vec<PathBuf>,
    catalogs: Vec<CatalogGroup>,
}

/// Explicit coverage: a partial documentation check is never a full gate.
#[derive(Clone, Debug)]
pub enum Scope {
    Static,
    Packages { names: Vec<String>, private: bool },
    Full,
}

impl Scope {
    fn label(&self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Packages { .. } => "packages",
            Self::Full => "full",
        }
    }

    fn output_name(&self) -> &'static str {
        match self {
            Self::Full => "gate",
            _ => self.label(),
        }
    }

    fn as_json(&self) -> Value {
        match self {
            Self::Packages { names, private } => json!({
                "mode": self.label(), "packages": names, "private": private,
                "full-gate": false,
            }),
            _ => json!({"mode": self.label(), "full-gate": matches!(self, Self::Full)}),
        }
    }
}

fn select_plan(plan: &mut Plan, scope: &Scope) -> Result<()> {
    if let Scope::Packages { names, .. } = scope {
        if names.is_empty() {
            return Err("package documentation requires at least one package".into());
        }
        for name in names {
            if !plan
                .jobs
                .iter()
                .any(|job| job.configuration.package == *name)
            {
                return Err(format!(
                    "package {name:?} has no documentation jobs in this repository"
                )
                .into());
            }
        }
    }
    let include = |job: &Job| match scope {
        Scope::Full => true,
        Scope::Static => false,
        Scope::Packages { names, private } => {
            names.contains(&job.configuration.package)
                && (*private || job.purpose != Purpose::PrivateRustdoc)
        }
    };
    plan.jobs.retain(include);
    plan.execution_jobs.retain(include);
    plan.requirement_map.retain(|(job, _)| include(job));
    plan.inapplicable.retain(|entry| match scope {
        Scope::Full => true,
        Scope::Static => false,
        Scope::Packages { names, .. } => names.contains(&entry.package),
    });
    Ok(())
}

fn executable_documentation_features(
    package: &cargo_metadata::Package,
    requested: &[String],
) -> Vec<String> {
    let only_empty_default =
        package.features.len() == 1 && package.features.get("default").is_some_and(Vec::is_empty);
    if package.features.is_empty() || only_empty_default {
        Vec::new()
    } else {
        requested.to_vec()
    }
}

fn record_requirement(
    mapped: &mut Vec<(Job, Job)>,
    purpose: Purpose,
    configuration: common::CargoConfiguration,
    package: Option<&cargo_metadata::Package>,
) {
    let requirement = Job {
        purpose,
        configuration,
    };
    let mut execution = requirement.clone();
    if let Some(package) = package {
        execution.configuration.features =
            executable_documentation_features(package, &execution.configuration.features);
    }
    mapped.push((requirement, execution));
}

fn validate_verified_consumers(
    plan: &Plan,
    verified: &BTreeSet<common::CargoConfiguration>,
) -> Result<()> {
    let planned = plan
        .execution_jobs
        .iter()
        .filter(|job| job.purpose == Purpose::McuCompileConsumer)
        .map(|job| job.configuration.clone())
        .collect::<BTreeSet<_>>();
    if !verified.is_subset(&planned) {
        return Err("source-only example evidence does not match planned MCU consumers".into());
    }
    Ok(())
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

/// Keep Cargo execution separate from catalog leases and HTML export so
/// cold, warm and one-source-change measurements remain comparable.
#[derive(Clone, Copy, Default)]
struct JobTiming {
    metadata_us: u64,
    cargo_us: u64,
    snapshot_us: u64,
    total_us: u64,
}

struct RustdocOutput {
    verified: PathBuf,
    snapshot: Option<PathBuf>,
}

impl JobTiming {
    fn as_json(self) -> Value {
        json!({
            "metadata-us": self.metadata_us,
            "cargo-us": self.cargo_us,
            "snapshot-us": self.snapshot_us,
            "total-us": self.total_us,
        })
    }
}

fn elapsed_us(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn stage_timing(label: &str, duration_us: u64) {
    eprintln!("docs timing: stage={label} total-us={duration_us}");
}

fn job_timing(ctx: &Context, job: &Job, timing: JobTiming) -> Result<Value> {
    let id = job.configuration.id(&ctx.root)?;
    eprintln!(
        "docs timing: job={} id={id} metadata-us={} cargo-us={} snapshot-us={} total-us={}",
        job.purpose.label(),
        timing.metadata_us,
        timing.cargo_us,
        timing.snapshot_us,
        timing.total_us,
    );
    Ok(json!({
        "purpose": job.purpose.label(),
        "configuration-id": id,
        "verification": "docs-cargo",
        "timing": timing.as_json(),
    }))
}

fn reused_consumer_timing(ctx: &Context, job: &Job) -> Result<Value> {
    let id = job.configuration.id(&ctx.root)?;
    eprintln!(
        "docs timing: job={} id={id} metadata-us=0 cargo-us=0 snapshot-us=0 total-us=0 verification=source-only/examples",
        job.purpose.label(),
    );
    Ok(json!({
        "purpose": job.purpose.label(),
        "configuration-id": id,
        "verification": "source-only/examples",
        "timing": JobTiming::default().as_json(),
    }))
}

/// Full checkpoint entry point used by source-only orchestration.
pub fn run(ctx: &Context, list: bool, export_html: bool, workers: usize) -> Result<()> {
    run_with_consumers(ctx, list, export_html, workers, &BTreeSet::new())
}

pub fn run_selected(
    ctx: &Context,
    scope: Scope,
    list: bool,
    export_html: bool,
    workers: usize,
) -> Result<()> {
    if export_html && matches!(scope, Scope::Static) {
        return Err("--export-html requires --full or --package".into());
    }
    run_scoped(ctx, scope, list, export_html, workers, &BTreeSet::new())
}

pub fn run_with_consumers(
    ctx: &Context,
    list: bool,
    export_html: bool,
    workers: usize,
    verified_consumers: &BTreeSet<common::CargoConfiguration>,
) -> Result<()> {
    run_scoped(
        ctx,
        Scope::Full,
        list,
        export_html,
        workers,
        verified_consumers,
    )
}

fn run_scoped(
    ctx: &Context,
    scope: Scope,
    list: bool,
    export_html: bool,
    workers: usize,
    verified_consumers: &BTreeSet<common::CargoConfiguration>,
) -> Result<()> {
    if !(1..=2).contains(&workers) {
        return Err("docs rustdoc workers must be one or two".into());
    }
    let gate_start = Instant::now();
    let stage_start = Instant::now();
    let toolchain = toolchain_identity(ctx)?;
    let toolchain_us = elapsed_us(stage_start);
    stage_timing("toolchain", toolchain_us);
    let stage_start = Instant::now();
    let mut plan = build_plan(ctx, &toolchain.host)?;
    select_plan(&mut plan, &scope)?;
    validate_verified_consumers(&plan, verified_consumers)?;
    let plan_us = elapsed_us(stage_start);
    stage_timing("plan-metadata", plan_us);
    let stage_start = Instant::now();
    let output = acquire_scoped_output(ctx, scope.output_name())?;
    let mut plan_value = plan_json(ctx, &plan, &toolchain)?;
    plan_value["scope"] = scope.as_json();
    write_json(&output.join("job-plan.json"), &plan_value)?;
    let output_us = elapsed_us(stage_start);
    stage_timing("output-and-plan-json", output_us);
    if list {
        print_plan(ctx, &plan)?;
        println!(
            "docs {} plan: workspaces={} packages={} requirements={} executions={} documents={} catalog-groups={} inapplicable={}",
            scope.label(),
            plan.workspace_count,
            plan.source_package_count,
            plan.jobs.len(),
            plan.execution_jobs.len(),
            plan.documents.len(),
            plan.catalogs.len(),
            plan.inapplicable.len()
        );
        return Ok(());
    }

    let stage_start = Instant::now();
    let report = output.join("report.json");
    if report.try_exists()? {
        fs::remove_file(&report)?;
    }
    reset_generated_outputs(&output)?;
    let before_status = repository_status(ctx)?;
    let before_locks = lock_identities(&plan)?;
    let source_sha256 = source_identity(ctx, &plan)?;
    let reset_us = elapsed_us(stage_start);
    stage_timing("reset-and-source-identity", reset_us);

    let mut rustdoc_outputs = Vec::new();
    let mut html_snapshots = Vec::new();
    let mut job_timings = Vec::new();
    let stage_start = Instant::now();
    let rustdoc_jobs = plan
        .execution_jobs
        .iter()
        .filter(|job| {
            matches!(
                job.purpose,
                Purpose::PublicRustdoc | Purpose::PrivateRustdoc
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    for (job, rendered, timing) in
        run_rustdoc_jobs(ctx, &output, &rustdoc_jobs, export_html, workers)?
    {
        rustdoc_outputs.push(rendered.verified);
        if let Some(snapshot) = rendered.snapshot {
            html_snapshots.push(snapshot);
        }
        job_timings.push(job_timing(ctx, &job, timing)?);
    }
    let rustdoc_us = elapsed_us(stage_start);
    stage_timing("rustdoc", rustdoc_us);
    let stage_start = Instant::now();
    for job in plan
        .execution_jobs
        .iter()
        .filter(|job| job.purpose == Purpose::HostDoctest)
    {
        let timing = run_doctest_measured(ctx, &output, job)?;
        job_timings.push(job_timing(ctx, job, timing)?);
    }
    let doctest_us = elapsed_us(stage_start);
    stage_timing("host-doctest", doctest_us);
    let stage_start = Instant::now();
    let mut reused_consumers = 0;
    for job in plan
        .execution_jobs
        .iter()
        .filter(|job| job.purpose == Purpose::McuCompileConsumer)
    {
        if verified_consumers.contains(&job.configuration) {
            reused_consumers += 1;
            job_timings.push(reused_consumer_timing(ctx, job)?);
        } else {
            let timing = run_consumer_measured(ctx, &output, job)?;
            job_timings.push(job_timing(ctx, job, timing)?);
        }
    }
    let consumer_us = elapsed_us(stage_start);
    stage_timing("mcu-consumer", consumer_us);

    let stage_start = Instant::now();
    let generated = run_catalogs(ctx, &output, &plan.catalogs)?;
    let catalogs_us = elapsed_us(stage_start);
    stage_timing("catalogs", catalogs_us);
    let stage_start = Instant::now();
    let mut documents = plan.documents.clone();
    documents.extend(generated);
    let links = check_markdown(ctx, &documents)?;
    let markdown_us = elapsed_us(stage_start);
    stage_timing("markdown-links", markdown_us);

    let stage_start = Instant::now();
    if repository_status(ctx)? != before_status {
        return Err("docs gate changed tracked or unrelated untracked source state".into());
    }
    if lock_identities(&plan)? != before_locks {
        return Err("docs gate changed a source Cargo.lock".into());
    }
    if source_identity(ctx, &plan)? != source_sha256 {
        return Err("docs gate changed a source input".into());
    }
    let integrity_us = elapsed_us(stage_start);
    stage_timing("source-integrity", integrity_us);

    let stage_start = Instant::now();
    let counts = purpose_counts(&plan.jobs);
    let execution_counts = purpose_counts(&plan.execution_jobs);
    let report_value = json!({
        "schema": 2,
        "scope": scope.as_json(),
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
            "requirements": plan.jobs.len(),
            "mapped-requirements": plan.requirement_map.len(),
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
        "execution": {
            "jobs": plan.execution_jobs.len(),
            "public-rustdoc": execution_counts.get(Purpose::PublicRustdoc.label()).copied().unwrap_or(0),
            "private-rustdoc": execution_counts.get(Purpose::PrivateRustdoc.label()).copied().unwrap_or(0),
            "host-doctest": execution_counts.get(Purpose::HostDoctest.label()).copied().unwrap_or(0),
            "mcu-compile-consumer": execution_counts.get(Purpose::McuCompileConsumer.label()).copied().unwrap_or(0),
            "mcu-compile-consumer-example-evidence": reused_consumers,
            "local-cargo-jobs": plan.execution_jobs.len() - reused_consumers,
            "rustdoc-workers": workers,
        },
        "requirement-map": plan.requirement_map.iter().map(|(requirement, execution)| {
            requirement_mapping_json(ctx, requirement, execution)
        }).collect::<Result<Vec<_>>>()?,
        "outputs": {
            "job-plan": relative(ctx, &output.join("job-plan.json"))?,
            "verified-rustdoc": rustdoc_outputs.iter().map(|path| relative(ctx, path)).collect::<Result<Vec<_>>>()?,
            "exported-html-snapshots": html_snapshots.iter().map(|path| relative(ctx, path)).collect::<Result<Vec<_>>>()?,
            "html-snapshot-export": export_html,
            "static-catalogs": plan.catalogs.iter().map(|group| relative(ctx, &output.join("catalogs").join(&group.chip).join("forward"))).collect::<Result<Vec<_>>>()?,
        },
        "timings": {
            "unit": "microseconds",
            "jobs": job_timings,
            "stages": {
                "toolchain": toolchain_us,
                "plan-metadata": plan_us,
                "output-and-plan-json": output_us,
                "reset-and-source-identity": reset_us,
                "rustdoc": rustdoc_us,
                "host-doctest": doctest_us,
                "mcu-consumer": consumer_us,
                "catalogs": catalogs_us,
                "markdown-links": markdown_us,
                "source-integrity": integrity_us,
            },
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
    stage_timing("report-write", elapsed_us(stage_start));
    stage_timing("total", elapsed_us(gate_start));
    println!(
        "docs {} passed: workspaces={} packages={} public={} private={} doctests={} consumers={} catalogs={} programs={} documents={} local-links={} external-not-checked={} inapplicable={}",
        scope.label(),
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
    let mut mapped = Vec::new();
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
                if !required_features_enabled(&item.package, features, &target.required_features)? {
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
                record_requirement(
                    &mut mapped,
                    Purpose::PublicRustdoc,
                    configuration.clone(),
                    Some(&item.package),
                );
                record_requirement(
                    &mut mapped,
                    Purpose::PrivateRustdoc,
                    configuration.clone(),
                    Some(&item.package),
                );
                if target.doctest
                    && target_triple == host
                    && matches!(selector, common::CargoTargetSelection::Lib(_))
                {
                    record_requirement(
                        &mut mapped,
                        Purpose::HostDoctest,
                        configuration,
                        Some(&item.package),
                    );
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
        record_requirement(
            &mut mapped,
            Purpose::McuCompileConsumer,
            configuration,
            None,
        );
    }
    mapped.sort_by(|left, right| left.0.cmp(&right.0));
    let jobs = mapped.iter().map(|(job, _)| job.clone()).collect();
    let execution_jobs = mapped
        .iter()
        .map(|(_, job)| job.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(Plan {
        source_package_count: packages.len(),
        workspace_count,
        jobs,
        execution_jobs,
        requirement_map: mapped,
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
) -> Result<bool> {
    if required.is_empty() {
        return Ok(true);
    }

    let mut no_default = false;
    let mut all_features = false;
    let mut roots = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        match flags[index].as_str() {
            "--no-default-features" => no_default = true,
            "--all-features" => all_features = true,
            "--features" => {
                index += 1;
                let selection = flags.get(index).ok_or_else(|| {
                    format!(
                        "package {} documentation profile has --features without a value",
                        package.name
                    )
                })?;
                for feature in selection.split(',') {
                    if feature.is_empty() || !package.features.contains_key(feature) {
                        return Err(format!(
                            "package {} documentation profile selects unsupported local feature {feature:?}",
                            package.name
                        )
                        .into());
                    }
                    roots.push(feature.to_owned());
                }
            }
            flag => {
                return Err(format!(
                    "package {} documentation profile contains unsupported Cargo feature flag {flag:?}",
                    package.name
                )
                .into());
            }
        }
        index += 1;
    }

    if all_features {
        roots.extend(package.features.keys().cloned());
    } else if !no_default && package.features.contains_key("default") {
        roots.push("default".into());
    }

    let mut enabled = BTreeSet::new();
    while let Some(feature) = roots.pop() {
        if !enabled.insert(feature.clone()) {
            continue;
        }
        let Some(edges) = package.features.get(&feature) else {
            continue;
        };
        for edge in edges {
            // `dep:name`, `name/feature` and `name?/feature` select dependency
            // state, not another local feature. Cargo metadata exposes an
            // implicit optional-dependency feature as its own key when that
            // feature is selectable, so only exact local keys are traversed.
            if edge.starts_with("dep:") || edge.contains('/') {
                continue;
            }
            if package.features.contains_key(edge) {
                roots.push(edge.clone());
            }
        }
    }

    Ok(required.iter().all(|feature| enabled.contains(feature)))
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

#[cfg(test)]
fn acquire_output(ctx: &Context) -> Result<PathBuf> {
    acquire_scoped_output(ctx, "gate")
}

fn acquire_scoped_output(ctx: &Context, name: &str) -> Result<PathBuf> {
    let output = ctx.root.join("target/docs").join(name);
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

fn run_rustdoc_jobs(
    ctx: &Context,
    output: &Path,
    jobs: &[Job],
    export_html: bool,
    workers: usize,
) -> Result<Vec<(Job, RustdocOutput, JobTiming)>> {
    if !(1..=2).contains(&workers) || jobs.iter().collect::<BTreeSet<_>>().len() != jobs.len() {
        return Err("rustdoc worker plan must be bounded and contain distinct jobs".into());
    }
    if workers == 1 {
        return jobs
            .iter()
            .map(|job| {
                let (rendered, timing) = run_rustdoc_measured(ctx, output, job, export_html)?;
                Ok((job.clone(), rendered, timing))
            })
            .collect();
    }

    // A worker owns one Cargo target-dir group at a time. Catalog readers may
    // overlap, but no two commands can mutate the same rustdoc cache staging.
    let mut groups =
        BTreeMap::<(PathBuf, Purpose, String, common::CargoBuildProfile), Vec<Job>>::new();
    for job in jobs {
        groups
            .entry((
                job.configuration.workspace_manifest.clone(),
                job.purpose,
                job.configuration.target.clone(),
                job.configuration.build_profile,
            ))
            .or_default()
            .push(job.clone());
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left[0].cmp(&right[0]))
    });
    let queue = Mutex::new(VecDeque::from(groups));
    let canceled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<(Job, Result<(RustdocOutput, JobTiming)>)>();
    let completed = thread::scope(|scope| {
        for _ in 0..workers {
            let sender = sender.clone();
            let queue = &queue;
            let canceled = &canceled;
            scope.spawn(move || {
                while !canceled.load(Ordering::Acquire) {
                    let Some(group) = queue.lock().expect("rustdoc queue poisoned").pop_front()
                    else {
                        break;
                    };
                    for job in group {
                        if canceled.load(Ordering::Acquire) {
                            break;
                        }
                        let result = run_rustdoc_measured(ctx, output, &job, export_html);
                        if result.is_err() {
                            canceled.store(true, Ordering::Release);
                        }
                        if sender.send((job, result)).is_err() {
                            return;
                        }
                    }
                }
            });
        }
        drop(sender);
        receiver.into_iter().collect::<Vec<_>>()
    });
    let mut results = BTreeMap::new();
    let mut first_error = None;
    for (job, result) in completed {
        match result {
            Ok(output) => {
                results.insert(job, output);
            }
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    if results.len() != jobs.len() {
        return Err("rustdoc workers did not verify every planned execution".into());
    }
    jobs.iter()
        .map(|job| {
            let (rendered, timing) = results.remove(job).ok_or("rustdoc worker result missing")?;
            Ok((job.clone(), rendered, timing))
        })
        .collect()
}

fn run_rustdoc_measured(
    ctx: &Context,
    output: &Path,
    job: &Job,
    export_html: bool,
) -> Result<(RustdocOutput, JobTiming)> {
    let total_start = Instant::now();
    let metadata_start = Instant::now();
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
    let metadata_us = elapsed_us(metadata_start);
    let cargo_start = Instant::now();
    run_checked(&mut command)?;
    let cargo_us = elapsed_us(cargo_start);
    let verified = staging.join(target_crate_name(&configuration.cargo_target)?);
    if !verified.join("index.html").is_file() {
        return Err(format!("Cargo rustdoc output missing: {}", verified.display()).into());
    }
    let (snapshot, snapshot_us) = if export_html {
        let snapshot_start = Instant::now();
        let snapshot = output
            .join("rustdoc")
            .join(job.purpose.label())
            .join(configuration.id(&ctx.root)?);
        owned_remove_dir(output, &snapshot)?;
        snapshot_rustdoc(&staging, &snapshot, &configuration.cargo_target)?;
        (Some(snapshot), elapsed_us(snapshot_start))
    } else {
        (None, 0)
    };
    Ok((
        RustdocOutput { verified, snapshot },
        JobTiming {
            metadata_us,
            cargo_us,
            snapshot_us,
            total_us: elapsed_us(total_start),
        },
    ))
}

#[cfg(test)]
fn run_rustdoc(ctx: &Context, output: &Path, job: &Job) -> Result<PathBuf> {
    run_rustdoc_measured(ctx, output, job, true)?
        .0
        .snapshot
        .ok_or_else(|| "fixture rustdoc snapshot missing".into())
}

fn target_crate_name(target: &common::CargoTargetSelection) -> Result<String> {
    match target {
        common::CargoTargetSelection::Lib(name) | common::CargoTargetSelection::Bin(name) => {
            Ok(name.replace('-', "_"))
        }
        common::CargoTargetSelection::DefaultTargets => {
            Err("rustdoc requires an explicit Cargo target".into())
        }
    }
}

fn snapshot_rustdoc(
    staging: &Path,
    snapshot: &Path,
    target: &common::CargoTargetSelection,
) -> Result<()> {
    let crate_name = target_crate_name(target)?;
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

fn run_doctest_measured(ctx: &Context, output: &Path, job: &Job) -> Result<JobTiming> {
    let total_start = Instant::now();
    let metadata_start = Instant::now();
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
    let metadata_us = elapsed_us(metadata_start);
    let cargo_start = Instant::now();
    run_checked(&mut command)?;
    Ok(JobTiming {
        metadata_us,
        cargo_us: elapsed_us(cargo_start),
        snapshot_us: 0,
        total_us: elapsed_us(total_start),
    })
}

#[cfg(test)]
fn run_doctest(ctx: &Context, output: &Path, job: &Job) -> Result<()> {
    run_doctest_measured(ctx, output, job).map(|_| ())
}

fn run_consumer_measured(ctx: &Context, output: &Path, job: &Job) -> Result<JobTiming> {
    let total_start = Instant::now();
    let metadata_start = Instant::now();
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
    let metadata_us = elapsed_us(metadata_start);
    let cargo_start = Instant::now();
    run_checked(&mut command)?;
    Ok(JobTiming {
        metadata_us,
        cargo_us: elapsed_us(cargo_start),
        snapshot_us: 0,
        total_us: elapsed_us(total_start),
    })
}

#[cfg(test)]
fn run_consumer(ctx: &Context, output: &Path, job: &Job) -> Result<()> {
    run_consumer_measured(ctx, output, job).map(|_| ())
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

fn qualification_binary(ctx: &Context) -> Result<PathBuf> {
    // All scopes use the same catalog tool; avoid recompiling it per scope.
    let target = ctx.root.join("target");
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
    let binary = qualification_binary(ctx)?;
    run_catalog_actions(output, groups, |group, action| {
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
    output: &Path,
    groups: &[CatalogGroup],
    mut execute: impl FnMut(&CatalogGroup, CatalogAction<'_>) -> Result<()>,
) -> Result<()> {
    for group in groups {
        execute(group, CatalogAction::CheckCatalogs)?;
        for program in &group.programs {
            execute(group, CatalogAction::CheckProgram(program))?;
        }
        let base = output.join("catalogs").join(&group.chip);
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

fn requirement_mapping_json(ctx: &Context, requirement: &Job, execution: &Job) -> Result<Value> {
    Ok(json!({
        "requirement": {
            "purpose": requirement.purpose.label(),
            "configuration-id": requirement.configuration.id(&ctx.root)?,
        },
        "execution": {
            "purpose": execution.purpose.label(),
            "configuration-id": execution.configuration.id(&ctx.root)?,
        },
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
        "schema": 2,
        "toolchain": toolchain_json(toolchain),
        "target": TARGET,
        "workspaces": plan.workspace_count,
        "packages": plan.source_package_count,
        "jobs": plan.jobs.iter().map(|job| Ok(json!({
            "purpose": job.purpose.label(),
            "configuration": configuration_json(ctx, &job.configuration)?,
        }))).collect::<Result<Vec<_>>>()?,
        "executions": plan.execution_jobs.iter().map(|job| Ok(json!({
            "purpose": job.purpose.label(),
            "configuration": configuration_json(ctx, &job.configuration)?,
        }))).collect::<Result<Vec<_>>>()?,
        "requirement-map": plan.requirement_map.iter().map(|(requirement, execution)| {
            requirement_mapping_json(ctx, requirement, execution)
        }).collect::<Result<Vec<_>>>()?,
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
    for job in &plan.execution_jobs {
        println!(
            "DOC-EXEC\tpurpose={}\tid={}\tpackage={}\ttarget={}\tfeatures={:?}\tcargo-target={}",
            job.purpose.label(),
            job.configuration.id(&ctx.root)?,
            job.configuration.package,
            job.configuration.target,
            job.configuration.features,
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

#[cfg(test)]
mod tests;
