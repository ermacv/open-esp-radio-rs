mod hil;
mod inventory;
mod model;
mod report;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use model::{CatalogView, QUALIFICATION_SCHEMA, Qualification};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage: cargo qualification <validate|evaluate|gate> --manifest PATH [--root PATH] [--json-report PATH]\n       cargo qualification catalog check (--manifest PATH | --catalog PATH [--catalog PATH ...]) [--root PATH]\n       cargo qualification catalog render (--manifest PATH | --catalog PATH [--catalog PATH ...]) --out DIRECTORY [--root PATH]\n\n--catalog validates/renders every explicitly loaded catalog without vendor evidence or HIL runs.\n--manifest check also validates program selection, dependency closure, and the exact required set without loading evidence; render additionally emits the evaluator-derived program view.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Validate,
    Evaluate,
    Gate,
    CatalogCheck,
    CatalogRender,
}

impl Command {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "validate" => Ok(Self::Validate),
            "evaluate" => Ok(Self::Evaluate),
            "gate" => Ok(Self::Gate),
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
    while index < arguments.len() {
        match arguments[index].as_str() {
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
    if !matches!(command, Command::CatalogRender) && output_directory.is_some() {
        return Err("--out is only accepted by catalog render".into());
    }
    if command == Command::CatalogRender && output_directory.is_none() {
        return Err("catalog render requires --out".into());
    }
    if matches!(command, Command::CatalogCheck | Command::CatalogRender) {
        if manifest.is_some() == !catalogs.is_empty() {
            return Err("catalog commands require exactly one of --manifest or --catalog".into());
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
    })
}

fn execute(arguments: Arguments) -> Result<()> {
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
    let arguments = match parse_arguments(raw_arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("{USAGE}\nerror: {error}");
            return ExitCode::FAILURE;
        }
    };
    match execute(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
