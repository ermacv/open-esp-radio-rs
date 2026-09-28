mod anchors;
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

const USAGE: &str = "usage: cargo qualification <status|next> (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification plan --manifest PATH [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification <validate|evaluate|gate> --manifest PATH [--root PATH] [--json-report PATH]\n       cargo qualification hil-evidence (--manifest PATH | --hil-target TARGET) [--run RUN_ID ...] [--root PATH]\n       cargo qualification catalog check (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--root PATH]\n       cargo qualification catalog render (--manifest PATH | --catalog PATH [--catalog PATH ...]) --out DIRECTORY [--root PATH]\n       cargo qualification catalog anchors --catalog PATH [--catalog PATH ...] [--changed FILE ...] [--root PATH]\n\nstatus --details expands scopes, limits, links and observations.\nstatus and next read declarations (--catalog) or saved evidence (--manifest); they never run hardware, tests or vendor analysis. --capability selects a capability and its dependency context, not a rerun plan.\n--catalog validates/renders selected catalogs and their transitive imports without vendor evidence or HIL runs.\nhil-evidence records the qualifying HIL observations of the program's runs, or only of the --run runs, as tracked shards bound to their firmware and observer sources.\ncatalog anchors checks the `// CAPABILITY: <id>` comments in code against every selected catalog entry (pass all catalogs: an anchor naming an unselected entry is unknown) and lists the entries anchored in --changed files.\n--manifest check also validates program selection, dependency closure, and the declared required-set policy without loading evidence; render additionally emits the evaluator-derived program view.";

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
    CatalogAnchors,
    HilEvidence,
}

impl Command {
    const ALL: [Self; 10] = [
        Self::Status,
        Self::Next,
        Self::Plan,
        Self::Validate,
        Self::Evaluate,
        Self::Gate,
        Self::HilEvidence,
        Self::CatalogCheck,
        Self::CatalogRender,
        Self::CatalogAnchors,
    ];

    /// The words that name the command after `cargo qualification`.
    fn words(self) -> &'static [&'static str] {
        match self {
            Self::Status => &["status"],
            Self::Next => &["next"],
            Self::Plan => &["plan"],
            Self::Validate => &["validate"],
            Self::Evaluate => &["evaluate"],
            Self::Gate => &["gate"],
            Self::HilEvidence => &["hil-evidence"],
            Self::CatalogCheck => &["catalog", "check"],
            Self::CatalogRender => &["catalog", "render"],
            Self::CatalogAnchors => &["catalog", "anchors"],
        }
    }

    /// Every option the command accepts: the parser rejects the others, and
    /// `__command-tree` prints these for the documentation check.
    fn options(self) -> &'static [&'static str] {
        const REPORTS: &[&str] = &["--manifest", "--root", "--json-report"];
        match self {
            Self::Status => &[
                "--manifest",
                "--catalog",
                "--capability",
                "--details",
                "--root",
                "--json-report",
            ],
            Self::Next => &[
                "--manifest",
                "--catalog",
                "--capability",
                "--root",
                "--json-report",
            ],
            Self::Plan => &["--manifest", "--capability", "--root", "--json-report"],
            Self::Validate | Self::Evaluate | Self::Gate => REPORTS,
            Self::HilEvidence => &["--manifest", "--hil-target", "--run", "--root"],
            Self::CatalogCheck => &["--manifest", "--catalog", "--root"],
            Self::CatalogRender => &["--manifest", "--catalog", "--out", "--root"],
            Self::CatalogAnchors => &["--catalog", "--changed", "--root"],
        }
    }

    fn find(arguments: &[String]) -> Result<Self> {
        let first = arguments.first().ok_or("missing qualification command")?;
        if let Some(command) = Self::ALL.into_iter().find(|command| {
            let words = command.words();
            arguments.len() >= words.len() && words.iter().zip(arguments).all(|(w, a)| w == a)
        }) {
            return Ok(command);
        }
        if first != "catalog" {
            return Err(format!("unknown qualification command {first:?}").into());
        }
        match arguments.get(1) {
            None => Err("missing catalog operation".into()),
            Some(operation) => Err(format!("unknown catalog operation {operation:?}").into()),
        }
    }
}

/// The command tree of `cargo qualification`: each command path with its
/// subcommands and the long options it accepts.
fn command_tree() -> Vec<oer_command_tree::CommandNode> {
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| word.to_string())
            .collect::<Vec<_>>()
    };
    let mut nodes = vec![oer_command_tree::CommandNode {
        path: words(&["qualification"]),
        subcommands: Vec::new(),
        flags: Vec::new(),
        forwards: false,
    }];
    for command in Command::ALL {
        let mut path = words(&["qualification"]);
        for word in command.words() {
            let parent = nodes
                .iter_mut()
                .find(|node| node.path == path)
                .expect("every parent is listed before its children");
            if !parent.subcommands.iter().any(|known| known == word) {
                parent.subcommands.push(word.to_string());
            }
            path.push(word.to_string());
            if !nodes.iter().any(|node| node.path == path) {
                nodes.push(oer_command_tree::CommandNode {
                    path: path.clone(),
                    subcommands: Vec::new(),
                    flags: Vec::new(),
                    forwards: false,
                });
            }
        }
        let leaf = nodes
            .iter_mut()
            .find(|node| node.path == path)
            .expect("the command was just listed");
        leaf.flags = words(command.options());
    }
    nodes
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
    /// `catalog anchors --changed`: repository-relative files an edit touched.
    changed: Vec<PathBuf>,
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
    let command = Command::find(&arguments)?;
    let mut index = command.words().len();
    let mut manifest = None;
    let mut catalogs = Vec::new();
    let mut root = None;
    let mut json_report = None;
    let mut output_directory = None;
    let mut capability = None;
    let mut details = false;
    let mut hil_target = None;
    let mut runs = Vec::new();
    let mut changed = Vec::new();
    while index < arguments.len() {
        let option = arguments[index].as_str();
        if !command.options().contains(&option) {
            return Err(if Command::ALL
                .iter()
                .any(|command| command.options().contains(&option))
            {
                format!("{option} is not accepted by {}", command.words().join(" "))
            } else {
                format!("unknown option {option:?}")
            }
            .into());
        }
        match option {
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
                runs.push(value);
            }
            "--hil-target" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--hil-target requires a value")?
                    .clone();
                index += 2;
                if hil_target.replace(value).is_some() {
                    return Err("duplicate --hil-target".into());
                }
            }
            "--changed" => {
                let value = take_value(&arguments, &mut index, "--changed")?;
                changed.push(value);
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
    if command == Command::CatalogRender && output_directory.is_none() {
        return Err("catalog render requires --out".into());
    }
    match command {
        Command::CatalogAnchors if catalogs.is_empty() => {
            return Err("catalog anchors requires --catalog".into());
        }
        Command::Status | Command::Next | Command::CatalogCheck | Command::CatalogRender
            if manifest.is_some() == !catalogs.is_empty() =>
        {
            return Err("command requires exactly one of --manifest or --catalog".into());
        }
        Command::HilEvidence if manifest.is_some() == hil_target.is_some() => {
            return Err("hil-evidence requires exactly one of --manifest or --hil-target".into());
        }
        Command::Plan | Command::Validate | Command::Evaluate | Command::Gate
            if manifest.is_none() =>
        {
            return Err("missing --manifest".into());
        }
        _ => {}
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
        changed,
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
    if arguments.command == Command::CatalogAnchors {
        let catalog = CatalogView::load(&arguments.root, &arguments.catalogs)?;
        return anchors::run(&catalog, &arguments.root, &arguments.changed);
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
    if oer_command_tree::requested() {
        println!("{}", oer_command_tree::json(&command_tree()));
        return ExitCode::SUCCESS;
    }
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
