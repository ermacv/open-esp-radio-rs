mod engineering;
mod hash_cache;
mod hil;
mod inventory;
mod model;
mod planning;
mod report;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use model::{CatalogView, QUALIFICATION_SCHEMA, Qualification};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage: cargo qualification <status|next> (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification plan --manifest PATH [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification <validate|evaluate|gate> --manifest PATH [--root PATH] [--json-report PATH]\n       cargo qualification hil-evidence (--manifest PATH | --hil-target TARGET) [--run RUN_ID ...] [--root PATH]\n       cargo qualification catalog check (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--root PATH]\n       cargo qualification catalog render (--manifest PATH | --catalog PATH [--catalog PATH ...]) --out DIRECTORY [--root PATH]\n\nstatus --details expands scopes, limits, links and observations.\nstatus and next read declarations (--catalog) or saved evidence (--manifest); they never run hardware, tests or vendor analysis. --capability selects a capability and its dependency context, not a rerun plan.\n--catalog validates/renders selected catalogs and their transitive imports without vendor evidence or HIL runs.\nhil-evidence records the qualifying HIL observations of the program's runs, or only of the --run runs, as tracked shards bound to their firmware and observer sources.\n--manifest check also validates program selection, dependency closure, and the declared required-set policy without loading evidence; render additionally emits the evaluator-derived program view.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Next,
    Plan,
    Validate,
    Evaluate,
    Gate,
    CatalogCheck,
    CatalogRender,
    HilEvidence,
}

impl Command {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "status" => Ok(Self::Status),
            "next" => Ok(Self::Next),
            "plan" => Ok(Self::Plan),
            "validate" => Ok(Self::Validate),
            "evaluate" => Ok(Self::Evaluate),
            "gate" => Ok(Self::Gate),
            "hil-evidence" => Ok(Self::HilEvidence),
            _ => Err(format!("unknown qualification command {value:?}").into()),
        }
    }
}

#[derive(Debug)]
struct Arguments {
    command: Command,
    manifest: Option<PathBuf>,
    catalogs: Vec<PathBuf>,
    root: PathBuf,
    json_report: Option<PathBuf>,
    output_directory: Option<PathBuf>,
    capability: Option<String>,
    details: bool,
    /// `hil-evidence`: the HIL target whose programs name the run and
    /// evidence directories, instead of one `--manifest`.
    hil_target: Option<String>,
    /// `hil-evidence --run`: record only these runs' observations.
    runs: Vec<String>,
}

fn take_value(arguments: &[String], index: &mut usize, option: &str) -> Result<PathBuf> {
    let value = arguments
        .get(*index + 1)
        .ok_or_else(|| format!("{option} requires a value"))?;
    *index += 2;
    Ok(PathBuf::from(value))
}

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<Arguments> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let command_name = arguments.first().ok_or("missing qualification command")?;
    let (command, mut index) = if command_name == "catalog" {
        let operation = arguments.get(1).ok_or("missing catalog operation")?;
        let command = match operation.as_str() {
            "check" => Command::CatalogCheck,
            "render" => Command::CatalogRender,
            _ => return Err(format!("unknown catalog operation {operation:?}").into()),
        };
        (command, 2)
    } else {
        (Command::parse(command_name)?, 1)
    };
    let mut manifest = None;
    let mut catalogs = Vec::new();
    let mut root = None;
    let mut json_report = None;
    let mut output_directory = None;
    let mut capability = None;
    let mut details = false;
    let mut hil_target = None;
    let mut runs = Vec::new();
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--details" => {
                if details {
                    return Err("duplicate --details".into());
                }
                details = true;
                index += 1;
            }
            "--capability" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--capability requires a value")?
                    .clone();
                index += 2;
                if capability.replace(value).is_some() {
                    return Err("duplicate --capability".into());
                }
            }
            "--run" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--run requires a run ID")?
                    .clone();
                index += 2;
                if command != Command::HilEvidence {
                    return Err("--run is accepted by hil-evidence".into());
                }
                runs.push(value);
            }
            "--hil-target" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--hil-target requires a value")?
                    .clone();
                index += 2;
                if command != Command::HilEvidence || hil_target.replace(value).is_some() {
                    return Err("--hil-target is accepted once, by hil-evidence".into());
                }
            }
            "--manifest" => {
                let value = take_value(&arguments, &mut index, "--manifest")?;
                if manifest.replace(value).is_some() {
                    return Err("duplicate --manifest".into());
                }
            }
            "--catalog" => {
                let value = take_value(&arguments, &mut index, "--catalog")?;
                catalogs.push(value);
            }
            "--root" => {
                let value = take_value(&arguments, &mut index, "--root")?;
                if root.replace(value).is_some() {
                    return Err("duplicate --root".into());
                }
            }
            "--json-report" => {
                let value = take_value(&arguments, &mut index, "--json-report")?;
                if json_report.replace(value).is_some() {
                    return Err("duplicate --json-report".into());
                }
            }
            "--out" => {
                let value = take_value(&arguments, &mut index, "--out")?;
                if output_directory.replace(value).is_some() {
                    return Err("duplicate --out".into());
                }
            }
            option => return Err(format!("unknown option {option:?}").into()),
        }
    }
    if matches!(command, Command::CatalogCheck | Command::CatalogRender) && json_report.is_some() {
        return Err("catalog commands do not accept --json-report".into());
    }
    if capability.is_some() && !matches!(command, Command::Status | Command::Next | Command::Plan) {
        return Err("--capability is only accepted by status, next and plan".into());
    }
    if details && command != Command::Status {
        return Err("--details is only accepted by status".into());
    }
    if !matches!(command, Command::CatalogRender) && output_directory.is_some() {
        return Err("--out is only accepted by catalog render".into());
    }
    if command == Command::CatalogRender && output_directory.is_none() {
        return Err("catalog render requires --out".into());
    }
    if matches!(
        command,
        Command::CatalogCheck | Command::CatalogRender | Command::Status | Command::Next
    ) {
        if manifest.is_some() == !catalogs.is_empty() {
            return Err("command requires exactly one of --manifest or --catalog".into());
        }
    } else if command == Command::HilEvidence {
        if manifest.is_some() == hil_target.is_some() {
            return Err("hil-evidence requires exactly one of --manifest or --hil-target".into());
        }
    } else if manifest.is_none() {
        return Err("missing --manifest".into());
    } else if !catalogs.is_empty() {
        return Err("--catalog is only accepted by catalog commands".into());
    }
    Ok(Arguments {
        command,
        manifest,
        catalogs,
        root: root.unwrap_or(env::current_dir()?),
        json_report,
        output_directory,
        capability,
        details,
        hil_target,
        runs,
    })
}

fn execute(arguments: Arguments) -> Result<()> {
    if matches!(
        arguments.command,
        Command::Status | Command::Next | Command::Plan
    ) {
        let map = if let Some(manifest) = &arguments.manifest {
            let path = arguments.root.join(manifest);
            let program = Qualification::load_and_evaluate(&path, &arguments.root)?;
            engineering::ProjectMap::from_program(&program, arguments.capability.as_deref())?
        } else {
            let catalog = CatalogView::load(&arguments.root, &arguments.catalogs)?;
            engineering::ProjectMap::from_catalog(&catalog, arguments.capability.as_deref())?
        };
        if arguments.command == Command::Plan {
            let plan = planning::Plan::from_map(&map)?;
            if let Some(path) = &arguments.json_report {
                report::write_serialized(&plan, &arguments.root.join(path))?;
            }
            println!("{}", serde_json::to_string_pretty(&plan)?);
            return Ok(());
        }
        map.print(arguments.command == Command::Next, arguments.details);
        if let Some(path) = &arguments.json_report {
            report::write_serialized(&map, &arguments.root.join(path))?;
        }
        return Ok(());
    }
    if matches!(
        arguments.command,
        Command::CatalogCheck | Command::CatalogRender
    ) && !arguments.catalogs.is_empty()
    {
        let catalog = CatalogView::load(&arguments.root, &arguments.catalogs)?;
        if let Some(path) = arguments.output_directory.as_deref() {
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                arguments.root.join(path)
            };
            inventory::write_static(&catalog, &path, &arguments.root)?;
        }
        println!(
            "CATALOG-STATIC-VALID\tschema={}\tcatalogs={}\tcapabilities={}\tinventory-items={}\treferences={}",
            model::CAPABILITY_CATALOG_SCHEMA,
            catalog.sources.len(),
            catalog.capabilities.len(),
            catalog.items.len(),
            catalog.references.len()
        );
        return Ok(());
    }
    if let Some(target) = &arguments.hil_target {
        let manifest = model::hil_program(&arguments.root, target)?;
        return record_hil_evidence(
            &arguments.root.join(manifest),
            &arguments.root,
            &arguments.runs,
        );
    }
    let manifest = arguments.manifest.as_ref().ok_or("missing --manifest")?;
    let manifest_path = if manifest.is_absolute() {
        manifest.clone()
    } else {
        arguments.root.join(manifest)
    };
    if arguments.command == Command::HilEvidence {
        return record_hil_evidence(&manifest_path, &arguments.root, &arguments.runs);
    }
    if arguments.command == Command::CatalogCheck {
        let catalog = CatalogView::load_for_program(&arguments.root, &manifest_path)?;
        println!(
            "CATALOG-STATIC-VALID\tprogram-schema={QUALIFICATION_SCHEMA}\tcatalogs={}\tcapabilities={}\tinventory-items={}\treferences={}",
            catalog.sources.len(),
            catalog.capabilities.len(),
            catalog.items.len(),
            catalog.references.len()
        );
        return Ok(());
    }
    let qualification = Qualification::load_and_evaluate(&manifest_path, &arguments.root)?;
    if matches!(
        arguments.command,
        Command::CatalogCheck | Command::CatalogRender
    ) && qualification.catalog_sources.is_empty()
    {
        return Err("qualification program does not select a capability catalog".into());
    }
    report::print(&qualification);
    if let Some(path) = arguments.json_report.as_deref() {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            arguments.root.join(path)
        };
        report::write_json(&qualification, &path)?;
    }
    if let Some(path) = arguments.output_directory.as_deref() {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            arguments.root.join(path)
        };
        inventory::write(&qualification, &path, &arguments.root)?;
    }
    if arguments.command == Command::Gate && !qualification.all_required_ready() {
        return Err(format!(
            "qualification gate rejected target {}: {}/{} required capabilities are ready",
            qualification.target,
            qualification.ready_count(),
            qualification.capabilities.len()
        )
        .into());
    }
    if arguments.command == Command::Validate {
        println!(
            "VALID\ttarget={}\tschema={QUALIFICATION_SCHEMA}",
            qualification.target
        );
    }
    Ok(())
}

fn record_hil_evidence(
    manifest: &std::path::Path,
    root: &std::path::Path,
    runs: &[String],
) -> Result<()> {
    let runs = (!runs.is_empty()).then(|| {
        runs.iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    });
    let evidence = Qualification::record_hil_evidence(manifest, root, runs.as_ref())?;
    println!("HIL-EVIDENCE\tshards={}", evidence.recorded.len());
    for scenario in &evidence.recorded {
        println!("HIL-SHARD\t{scenario}");
    }
    // One line per scenario of each requested run: what became of it and why.
    for (run, scenario, verdict) in &evidence.verdicts {
        println!(
            "HIL-EVIDENCE-RUN\t{run}\t{}\t{}\t{}",
            if scenario.is_empty() { "-" } else { scenario },
            verdict.id(),
            verdict.detail()
        );
    }
    Ok(())
}

fn is_help(arguments: &[String]) -> bool {
    matches!(arguments, [value] if matches!(value.as_str(), "--help" | "-h" | "help"))
        || matches!(arguments, [catalog, value] if catalog == "catalog" && matches!(value.as_str(), "--help" | "-h" | "help"))
}

fn main() -> ExitCode {
    let raw_arguments = env::args().skip(1).collect::<Vec<_>>();
    if is_help(&raw_arguments) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    // Recorded shards enter the repository; hash every file they bind.
    if raw_arguments
        .first()
        .is_some_and(|command| command == "hil-evidence")
    {
        hash_cache::disable();
    }
    let arguments = match parse_arguments(raw_arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("{USAGE}\nerror: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = execute(arguments);
    hash_cache::save();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
