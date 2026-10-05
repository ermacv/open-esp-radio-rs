mod anchors;
mod engineering;
mod hil;
mod inventory;
mod model;
mod planning;
mod report;

use std::{env, error::Error, path::PathBuf, process::ExitCode, sync::OnceLock};

use oer_durable::digests::DigestCache;

use model::{CatalogView, QUALIFICATION_SCHEMA, Qualification};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// The SHA-256 digests of evidence files, remembered across evaluations.
///
/// Sealed HIL bundles are immutable, yet every evaluation hashed all of their
/// files again; `oer_durable::digests` keeps each file's digest by its
/// identity and times in the user's cache directory. Set
/// `OER_QUALIFICATION_HASH_CACHE=0` to hash every file.
fn digests() -> &'static DigestCache {
    static DIGESTS: OnceLock<DigestCache> = OnceLock::new();
    DIGESTS.get_or_init(|| {
        let disabled =
            cfg!(test) || env::var("OER_QUALIFICATION_HASH_CACHE").is_ok_and(|value| value == "0");
        DigestCache::open(
            (!disabled)
                .then(|| {
                    oer_durable::xdg::path(
                        oer_durable::xdg::Base::Cache,
                        "qualification/sha256.json",
                    )
                    .ok()
                })
                .flatten(),
        )
    })
}

/// What the commands read and decide, below the generated help.
const DETAILS: &str = "\
status and next read declarations (--catalog) or saved evidence (--manifest); they never run hardware, tests or vendor analysis. --capability selects a capability and its dependency context, not a rerun plan.
--catalog validates/renders selected catalogs and their transitive imports without vendor evidence or HIL runs.
hil-evidence records the qualifying HIL observations of the program's runs, or only of the --run runs, or of the checkout's pending runs (--pending, which then leaves the pending list), as tracked shards bound to their firmware and observer sources.
catalog anchors checks the `// CAPABILITY: <id>` comments in code against every selected catalog entry (pass all catalogs: an anchor naming an unselected entry is unknown) and lists the entries anchored in --changed files.
--manifest check also validates program selection, dependency closure, and the declared required-set policy without loading evidence; render additionally emits the evaluator-derived program view.";

/// The command an invocation runs, as `execute` dispatches it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Next,
    Plan,
    Validate,
    Evaluate,
    CatalogCheck,
    CatalogRender,
    CatalogAnchors,
    HilEvidence,
}

/// `cargo qualification`: the one definition of its command line, which
/// parses the arguments and which `__command-tree` walks.
#[derive(clap::Parser)]
#[command(
    name = "cargo qualification",
    about = "The readiness authority: capability declarations and independent evidence",
    after_help = DETAILS
)]
struct Cli {
    #[command(subcommand)]
    command: CommandCli,
}

/// What `status`, `next` and the catalog commands read: a program, or
/// catalogs and their transitive imports.
#[derive(clap::Args)]
struct Selection {
    /// A qualification program.
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// A capability catalog; repeatable.
    #[arg(long = "catalog")]
    catalogs: Vec<PathBuf>,
}

/// The checkout and the machine-readable report of a command.
#[derive(clap::Args)]
struct Reporting {
    /// The checkout; default: the current directory.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Also write the result as JSON here.
    #[arg(long)]
    json_report: Option<PathBuf>,
}

/// The checkout a command reads.
#[derive(clap::Args)]
struct RootOnly {
    /// The checkout; default: the current directory.
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
enum CommandCli {
    /// Declarations or saved evidence of a selection; never runs hardware,
    /// tests or vendor analysis.
    Status {
        #[command(flatten)]
        selection: Selection,
        /// A capability and its dependency context, not a rerun plan.
        #[arg(long)]
        capability: Option<String>,
        /// Expand scopes, limits, links and observations.
        #[arg(long)]
        details: bool,
        #[command(flatten)]
        reporting: Reporting,
    },
    /// The next work of a selection.
    Next {
        #[command(flatten)]
        selection: Selection,
        #[arg(long)]
        capability: Option<String>,
        #[command(flatten)]
        reporting: Reporting,
    },
    /// The plan of a program.
    Plan {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        capability: Option<String>,
        #[command(flatten)]
        reporting: Reporting,
    },
    /// Validate a program.
    Validate {
        #[arg(long)]
        manifest: PathBuf,
        #[command(flatten)]
        reporting: Reporting,
    },
    /// Evaluate a program.
    Evaluate {
        #[arg(long)]
        manifest: PathBuf,
        #[command(flatten)]
        reporting: Reporting,
    },
    /// Record the qualifying HIL observations of runs as tracked shards.
    HilEvidence {
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// The HIL target whose programs name the run and evidence
        /// directories, instead of one `--manifest`.
        #[arg(long)]
        hil_target: Option<String>,
        /// Record only this run's observations; repeatable.
        #[arg(long = "run", value_name = "RUN_ID")]
        runs: Vec<String>,
        /// Record the checkout's pending runs, which then leave the pending
        /// list.
        #[arg(long)]
        pending: bool,
        #[command(flatten)]
        root: RootOnly,
    },
    /// Capability catalogs without evidence.
    #[command(subcommand)]
    Catalog(CatalogCli),
}

#[derive(clap::Subcommand)]
enum CatalogCli {
    /// Validate selected catalogs, or a program's selection.
    Check {
        #[command(flatten)]
        selection: Selection,
        #[command(flatten)]
        root: RootOnly,
    },
    /// Validate and render selected catalogs, or a program's view.
    Render {
        #[command(flatten)]
        selection: Selection,
        #[arg(long, value_name = "DIRECTORY")]
        out: PathBuf,
        #[command(flatten)]
        root: RootOnly,
    },
    /// Check `// CAPABILITY: <id>` anchors against every selected entry.
    Anchors {
        #[arg(long = "catalog", required = true)]
        catalogs: Vec<PathBuf>,
        /// A repository-relative file an edit touched; repeatable.
        #[arg(long = "changed", value_name = "FILE")]
        changed: Vec<PathBuf>,
        #[command(flatten)]
        root: RootOnly,
    },
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
    /// `hil-evidence --pending`: record the checkout's pending runs.
    pending: bool,
    /// `catalog anchors --changed`: repository-relative files an edit touched.
    changed: Vec<PathBuf>,
}

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<Arguments> {
    use clap::Parser as _;
    let cli =
        Cli::try_parse_from(std::iter::once(String::from("cargo qualification")).chain(arguments))?;
    let none = |command| Arguments {
        command,
        manifest: None,
        catalogs: Vec::new(),
        root: PathBuf::new(),
        json_report: None,
        output_directory: None,
        capability: None,
        details: false,
        hil_target: None,
        runs: Vec::new(),
        pending: false,
        changed: Vec::new(),
    };
    let (arguments, root) = match cli.command {
        CommandCli::Status {
            selection,
            capability,
            details,
            reporting,
        } => (
            Arguments {
                manifest: selection.manifest,
                catalogs: selection.catalogs,
                capability,
                details,
                json_report: reporting.json_report,
                ..none(Command::Status)
            },
            reporting.root,
        ),
        CommandCli::Next {
            selection,
            capability,
            reporting,
        } => (
            Arguments {
                manifest: selection.manifest,
                catalogs: selection.catalogs,
                capability,
                json_report: reporting.json_report,
                ..none(Command::Next)
            },
            reporting.root,
        ),
        CommandCli::Plan {
            manifest,
            capability,
            reporting,
        } => (
            Arguments {
                manifest: Some(manifest),
                capability,
                json_report: reporting.json_report,
                ..none(Command::Plan)
            },
            reporting.root,
        ),
        CommandCli::Validate {
            manifest,
            reporting,
        } => (
            Arguments {
                manifest: Some(manifest),
                json_report: reporting.json_report,
                ..none(Command::Validate)
            },
            reporting.root,
        ),
        CommandCli::Evaluate {
            manifest,
            reporting,
        } => (
            Arguments {
                manifest: Some(manifest),
                json_report: reporting.json_report,
                ..none(Command::Evaluate)
            },
            reporting.root,
        ),
        CommandCli::HilEvidence {
            manifest,
            hil_target,
            runs,
            pending,
            root,
        } => (
            Arguments {
                manifest,
                hil_target,
                runs,
                pending,
                ..none(Command::HilEvidence)
            },
            root.root,
        ),
        CommandCli::Catalog(CatalogCli::Check { selection, root }) => (
            Arguments {
                manifest: selection.manifest,
                catalogs: selection.catalogs,
                ..none(Command::CatalogCheck)
            },
            root.root,
        ),
        CommandCli::Catalog(CatalogCli::Render {
            selection,
            out,
            root,
        }) => (
            Arguments {
                manifest: selection.manifest,
                catalogs: selection.catalogs,
                output_directory: Some(out),
                ..none(Command::CatalogRender)
            },
            root.root,
        ),
        CommandCli::Catalog(CatalogCli::Anchors {
            catalogs,
            changed,
            root,
        }) => (
            Arguments {
                catalogs,
                changed,
                ..none(Command::CatalogAnchors)
            },
            root.root,
        ),
    };
    let Arguments {
        command,
        ref manifest,
        ref catalogs,
        ref hil_target,
        ref runs,
        pending,
        ..
    } = arguments;
    match command {
        Command::Status | Command::Next | Command::CatalogCheck | Command::CatalogRender
            if manifest.is_some() == !catalogs.is_empty() =>
        {
            return Err("command requires exactly one of --manifest or --catalog".into());
        }
        Command::HilEvidence if manifest.is_some() == hil_target.is_some() => {
            return Err("hil-evidence requires exactly one of --manifest or --hil-target".into());
        }
        Command::HilEvidence if pending && !runs.is_empty() => {
            return Err("hil-evidence takes --run or --pending, not both".into());
        }
        _ => {}
    }
    Ok(Arguments {
        root: match root {
            Some(root) => root,
            None => env::current_dir()?,
        },
        ..arguments
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
            arguments.pending,
        );
    }
    let manifest = arguments.manifest.as_ref().ok_or("missing --manifest")?;
    let manifest_path = if manifest.is_absolute() {
        manifest.clone()
    } else {
        arguments.root.join(manifest)
    };
    if arguments.command == Command::HilEvidence {
        return record_hil_evidence(
            &manifest_path,
            &arguments.root,
            &arguments.runs,
            arguments.pending,
        );
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
    if arguments.command == Command::Validate {
        println!(
            "VALID\ttarget={}\tschema={QUALIFICATION_SCHEMA}",
            qualification.target
        );
    }
    Ok(())
}

/// Record the qualifying observations of the program's runs as tracked
/// shards: of `runs`, of the checkout's pending runs with `pending`, or else
/// of every run. The evaluator reads the runs itself; a pending run it
/// observed leaves the pending list, a pending run of another chip stays.
fn record_hil_evidence(
    manifest: &std::path::Path,
    root: &std::path::Path,
    runs: &[String],
    pending: bool,
) -> Result<()> {
    use oer_hil_run_bundle::store::pending as pending_list;
    let runs = if pending {
        let listed = pending_list::load(root)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|entry| entry.run)
            .collect::<std::collections::BTreeSet<_>>();
        if listed.is_empty() {
            println!("HIL-EVIDENCE\tshards=0\tpending=0");
            return Ok(());
        }
        Some(listed)
    } else {
        (!runs.is_empty()).then(|| {
            runs.iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        })
    };
    let evidence = Qualification::record_hil_evidence(manifest, root, runs.as_ref())?;
    println!("HIL-EVIDENCE\tshards={}", evidence.recorded.len());
    for scenario in &evidence.recorded {
        println!("HIL-SHARD\t{scenario}");
    }
    if pending {
        let observed = evidence
            .verdicts
            .iter()
            .filter(|(_, _, verdict)| verdict.id() != "not-found")
            .map(|(run, _, _)| run.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        pending_list::forget(root, &observed).map_err(|error| error.to_string())?;
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

/// The command tree of `cargo qualification`, walked from its parser.
fn command_tree() -> Vec<oer_command_tree::CommandNode> {
    use clap::CommandFactory as _;
    oer_command_tree::command_tree(&Cli::command(), &[String::from("qualification")])
}

fn main() -> ExitCode {
    let raw_arguments = env::args().skip(1).collect::<Vec<_>>();
    if oer_command_tree::requested() {
        println!("{}", oer_command_tree::json(&command_tree()));
        return ExitCode::SUCCESS;
    }
    // Recorded shards enter the repository; hash every file they bind: the
    // unauthenticated cache must not vouch for them.
    if raw_arguments
        .first()
        .is_some_and(|command| command == "hil-evidence")
    {
        digests().disable();
    }
    let arguments = match parse_arguments(raw_arguments) {
        Ok(arguments) => arguments,
        // Help, and a usage error with its usage, as clap prints them.
        Err(error) => match error.downcast::<clap::Error>() {
            Ok(error) => error.exit(),
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    let result = execute(arguments);
    digests().save();
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
