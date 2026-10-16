mod anchors;
mod engineering;
mod hil;
mod inventory;
mod model;
mod planning;
mod readiness;
mod references;
mod report;
mod verdict;

use std::{env, error::Error, path::PathBuf, process::ExitCode, sync::OnceLock};

use oer_durable::digests::DigestCache;

use model::{CatalogView, QUALIFICATION_SCHEMA, Qualification};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// `error` behind `context`, still an I/O failure when it was one (content
/// that is not UTF-8 is not one), so that
/// `check references` and `verdict` tell the environment's failures (exit
/// 3) from a broken rule or bundle (exit 1).
fn with_context(
    error: impl Into<Box<dyn Error>>,
    context: impl std::fmt::Display,
) -> Box<dyn Error> {
    let error = error.into();
    // `InvalidData` is a file read whole whose content is not UTF-8: the
    // file is broken, not its reading.
    match error
        .downcast_ref::<std::io::Error>()
        .filter(|io| io.kind() != std::io::ErrorKind::InvalidData)
    {
        Some(io) => Box::new(std::io::Error::new(
            io.kind(),
            format!("{context}: {error}"),
        )),
        None => format!("{context}: {error}").into(),
    }
}

/// An error of a library whose errors are `Send + Sync`, as this binary's.
fn boxed(error: Box<dyn Error + Send + Sync>) -> Box<dyn Error> {
    error
}

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

const USAGE: &str = "usage: cargo qualification <status|next> (--manifest PATH [--archive DIR] | --catalog PATH [--catalog PATH ...]) [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification plan --manifest PATH [--archive DIR] [--capability ID] [--root PATH] [--json-report PATH]\n       cargo qualification validate --manifest PATH [--root PATH] [--json-report PATH]\n       cargo qualification evaluate --manifest PATH [--archive DIR] [--root PATH] [--json-report PATH]\n       cargo qualification check references [--candidate FILE] [--root PATH]\n       cargo qualification verdict --run PATH [--root PATH]\n       cargo qualification readiness --out DIRECTORY [--archive DIR] [--base DIRECTORY] [--drops PATH] [--root PATH]\n       cargo qualification catalog check (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--root PATH]\n       cargo qualification catalog render (--manifest PATH | --catalog PATH [--catalog PATH ...]) --out DIRECTORY [--root PATH]\n       cargo qualification catalog anchors --catalog PATH [--catalog PATH ...] [--changed FILE ...] [--root PATH]\n\nstatus --details expands scopes, limits, links and observations.\nstatus and next read declarations (--catalog) or saved evidence (--manifest); they never run hardware, tests or vendor analysis. --capability selects a capability and its dependency context, not a rerun plan.\n--catalog validates/renders selected catalogs and their transitive imports without vendor evidence or HIL runs.\n--archive DIR reads HIL runs only from DIR, a copy of the evidence archive version qualification/evidence/archive.toml pins (`cargo hil archive fetch`), verified when opened; without it no HIL run is read. validate never reads runs.\ncheck references checks offline, without the archive, the pin, that every run a tracked record cites is in the pinned version with the same SHA-256 (rule 2), that the version holds no run nothing cites (rule 1), and every program's catalogs; with --candidate, rules 1 and 2 hold for the runs of FILE (a references file or a version's manifest.json) instead of the pinned version, as `cargo hil archive pack` and `publish` check before creating anything; exit status 1 when one is not, 2 for a usage error, 3 when the check itself failed, 130 on Ctrl-C.\nverdict judges one run bundle against the HIL requirements of every program with the same evaluator, for work on the stand; its output is marked not published and not evidence, and status, next, plan, readiness and evaluate never read it; exit status 1 when the run does not meet a requirement it observed, observed none a program requires, is of no program's chip or its bundle is not valid, 2 for a usage error, 3 when the command itself failed, 130 on Ctrl-C.\nreadiness evaluates every program into one JSON report each in --out and, against the reports of an earlier evaluation in --base, lists every capability whose readiness dropped, as a table in --drops.\ncatalog anchors checks the `// CAPABILITY: <id>` comments in code against every selected catalog entry (pass all catalogs: an anchor naming an unselected entry is unknown) and lists the entries anchored in --changed files.\n--manifest check also validates program selection, dependency closure, and the declared required-set policy without loading evidence; render additionally emits the evaluator-derived program view.";

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
    CheckReferences,
    Verdict,
    Readiness,
}

impl Command {
    const ALL: [Self; 11] = [
        Self::Status,
        Self::Next,
        Self::Plan,
        Self::Validate,
        Self::Evaluate,
        Self::CheckReferences,
        Self::Verdict,
        Self::Readiness,
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
            Self::CheckReferences => &["check", "references"],
            Self::Verdict => &["verdict"],
            Self::Readiness => &["readiness"],
            Self::CatalogCheck => &["catalog", "check"],
            Self::CatalogRender => &["catalog", "render"],
            Self::CatalogAnchors => &["catalog", "anchors"],
        }
    }

    /// Every option the command accepts: the parser rejects the others, and
    /// `__command-tree` prints these for the documentation check.
    fn options(self) -> &'static [&'static str] {
        match self {
            Self::Status => &[
                "--manifest",
                "--catalog",
                "--archive",
                "--capability",
                "--details",
                "--root",
                "--json-report",
            ],
            Self::Next => &[
                "--manifest",
                "--catalog",
                "--archive",
                "--capability",
                "--root",
                "--json-report",
            ],
            Self::Plan => &[
                "--manifest",
                "--archive",
                "--capability",
                "--root",
                "--json-report",
            ],
            Self::Validate => &["--manifest", "--root", "--json-report"],
            Self::Evaluate => &["--manifest", "--archive", "--root", "--json-report"],
            Self::CheckReferences => &["--candidate", "--root"],
            Self::Verdict => &["--run", "--root"],
            Self::Readiness => &["--out", "--archive", "--base", "--drops", "--root"],
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
        if first != "catalog" && first != "check" {
            return Err(format!("unknown qualification command {first:?}").into());
        }
        match arguments.get(1) {
            None => Err(format!("missing {first} operation").into()),
            Some(operation) => Err(format!("unknown {first} operation {operation:?}").into()),
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
    /// `--archive`: the verified copy of the pinned evidence archive version
    /// whose runs are the HIL evidence.
    archive: Option<PathBuf>,
    /// `verdict --run`: the run bundle to judge.
    run: Option<PathBuf>,
    /// `check references --candidate`: the runs of a version about to be
    /// packed or published, checked instead of the pinned version.
    candidate: Option<PathBuf>,
    /// `catalog anchors --changed`: repository-relative files an edit touched.
    changed: Vec<PathBuf>,
    /// `readiness --base`: the reports of an earlier evaluation.
    base: Option<PathBuf>,
    /// `readiness --drops`: where the table of dropped readiness goes.
    drops: Option<PathBuf>,
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
    let mut archive = None;
    let mut run = None;
    let mut candidate = None;
    let mut changed = Vec::new();
    let mut base = None;
    let mut drops = None;
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
            "--candidate" => {
                let value = take_value(&arguments, &mut index, "--candidate")?;
                if candidate.replace(value).is_some() {
                    return Err("duplicate --candidate".into());
                }
            }
            "--run" => {
                let value = take_value(&arguments, &mut index, "--run")?;
                if run.replace(value).is_some() {
                    return Err("duplicate --run".into());
                }
            }
            "--archive" => {
                let value = take_value(&arguments, &mut index, "--archive")?;
                if archive.replace(value).is_some() {
                    return Err("duplicate --archive".into());
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
            "--base" => {
                let value = take_value(&arguments, &mut index, "--base")?;
                if base.replace(value).is_some() {
                    return Err("duplicate --base".into());
                }
            }
            "--drops" => {
                let value = take_value(&arguments, &mut index, "--drops")?;
                if drops.replace(value).is_some() {
                    return Err("duplicate --drops".into());
                }
            }
            option => return Err(format!("unknown option {option:?}").into()),
        }
    }
    if command == Command::CatalogRender && output_directory.is_none() {
        return Err("catalog render requires --out".into());
    }
    if command == Command::Readiness && output_directory.is_none() {
        return Err("readiness requires --out".into());
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
        Command::Verdict if run.is_none() => {
            return Err("verdict requires --run".into());
        }
        Command::Status | Command::Next if archive.is_some() && manifest.is_none() => {
            return Err("--archive requires --manifest".into());
        }
        Command::Plan | Command::Validate | Command::Evaluate if manifest.is_none() => {
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
        archive,
        run,
        candidate,
        changed,
        base,
        drops,
    })
}

fn execute(arguments: Arguments) -> Result<()> {
    if matches!(
        arguments.command,
        Command::Status | Command::Next | Command::Plan
    ) {
        let map = if let Some(manifest) = &arguments.manifest {
            let path = arguments.root.join(manifest);
            let archive = archive_copy(&arguments.root, arguments.archive.as_deref())?;
            let input = archive_input(
                archive
                    .as_ref()
                    .map(|copy| copy.manifest().version().to_string()),
            );
            if input_on_stderr(arguments.command) {
                eprintln!("{input}");
            } else {
                println!("{input}");
            }
            let program =
                Qualification::load_and_evaluate(&path, &arguments.root, archive.as_ref())?;
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
    if arguments.command == Command::Readiness {
        let root = &arguments.root;
        let out = arguments
            .output_directory
            .as_deref()
            .ok_or("missing --out")?;
        let base = arguments
            .base
            .as_deref()
            .map(|p| readiness::rooted(root, p));
        let drops = arguments
            .drops
            .as_deref()
            .map(|p| readiness::rooted(root, p));
        let archive = archive_copy(root, arguments.archive.as_deref())?;
        readiness::run(
            root,
            archive.as_ref(),
            &readiness::rooted(root, out),
            base.as_deref(),
            drops.as_deref(),
        )?;
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
    if arguments.command == Command::CheckReferences {
        return references::run(&arguments.root, arguments.candidate.as_deref());
    }
    if arguments.command == Command::Verdict {
        let run = arguments.run.as_deref().ok_or("verdict requires --run")?;
        return verdict::run(&arguments.root, run);
    }
    let manifest = arguments.manifest.as_ref().ok_or("missing --manifest")?;
    let manifest_path = if manifest.is_absolute() {
        manifest.clone()
    } else {
        arguments.root.join(manifest)
    };
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
    let archive = archive_copy(&arguments.root, arguments.archive.as_deref())?;
    let qualification =
        Qualification::load_and_evaluate(&manifest_path, &arguments.root, archive.as_ref())?;
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

/// The line that says which archive version an evaluation read, `none`
/// when it read no HIL run.
fn archive_input(version: Option<String>) -> String {
    format!(
        "INPUT\thil-archive={}",
        version.as_deref().unwrap_or("none")
    )
}

/// Whether the command prints [`archive_input`] on stderr: `plan`, whose
/// stdout is its JSON.
fn input_on_stderr(command: Command) -> bool {
    command == Command::Plan
}

/// The copy at `directory` (relative to `root` unless absolute) of the
/// evidence archive version the repository pins, verified against the pin;
/// none without a directory.
fn archive_copy(
    root: &std::path::Path,
    directory: Option<&std::path::Path>,
) -> Result<Option<oer_hil_evidence_archive::Copy>> {
    let Some(directory) = directory else {
        return Ok(None);
    };
    let pin = root.join(oer_hil_evidence_archive::PIN);
    let pinned = oer_hil_evidence_archive::Pin::load(&pin)
        .map_err(boxed)?
        .ok_or_else(|| format!("{} pins no evidence archive version", pin.display()))?;
    let copy = oer_hil_evidence_archive::Copy::open(&root.join(directory), pinned.manifest())
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    Ok(Some(copy))
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
    // `check references` and `verdict` follow the CLI exit codes of #783,
    // where a usage error is 2.
    let usage_status = ExitCode::from(usage_status(&raw_arguments));
    let arguments = match parse_arguments(raw_arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("{USAGE}\nerror: {error}");
            return usage_status;
        }
    };
    let command = arguments.command;
    // `check references` and `verdict` stop on Ctrl-C at their next step
    // and exit 130 (#783); the other commands keep the default signal
    // handling until `oer-cli`.
    let _signals = match command {
        Command::CheckReferences | Command::Verdict => match oer_process::install_signal_handlers()
        {
            Ok(guard) => Some(guard),
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(3);
            }
        },
        _ => None,
    };
    let result = execute(arguments);
    digests().save();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            // `check references` and `verdict` follow the CLI exit codes of
            // #783: 1 a broken rule, an unmet requirement or nothing to
            // judge, 3 the command itself failed, 130 cancelled.
            if !matches!(command, Command::CheckReferences | Command::Verdict) {
                ExitCode::FAILURE
            } else {
                ExitCode::from(exit_status(&*error, oer_process::cancellation_requested()))
            }
        }
    }
}

/// The exit status of a usage error: 2 for `check` and `verdict`, which
/// follow #783 (an unknown `check` operation included), 1 for the others.
fn usage_status(arguments: &[String]) -> u8 {
    match Command::find(arguments) {
        Ok(Command::CheckReferences | Command::Verdict) => 2,
        Err(_) if matches!(arguments.first().map(String::as_str), Some("check")) => 2,
        _ => 1,
    }
}

/// The exit status of a failed `check references` or `verdict` (#783).
/// After Ctrl-C (`cancelled`) every failure is the cancellation's, even one
/// a tool reported only as text, such as a git query it interrupted.
fn exit_status(error: &(dyn Error + 'static), cancelled: bool) -> u8 {
    if cancelled || oer_process::is_cancelled(error) {
        130
    } else if error.is::<oer_hil_evidence_archive::Refused>()
        || error.is::<verdict::Unmet>()
        || error.is::<verdict::Invalid>()
        || error.is::<verdict::Unjudged>()
    {
        1
    } else {
        3
    }
}

#[cfg(test)]
mod tests;
