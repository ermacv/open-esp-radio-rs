mod hil;
mod model;
mod report;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use model::{QUALIFICATION_SCHEMA, Qualification};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage: cargo qualification <validate|evaluate|gate> --manifest PATH [--root PATH] [--json-report PATH]";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Validate,
    Evaluate,
    Gate,
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
    manifest: PathBuf,
    root: PathBuf,
    json_report: Option<PathBuf>,
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
    let command = Command::parse(command_name)?;
    let mut manifest = None;
    let mut root = None;
    let mut json_report = None;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => {
                let value = take_value(&arguments, &mut index, "--manifest")?;
                if manifest.replace(value).is_some() {
                    return Err("duplicate --manifest".into());
                }
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
            option => return Err(format!("unknown option {option:?}").into()),
        }
    }
    Ok(Arguments {
        command,
        manifest: manifest.ok_or("missing --manifest")?,
        root: root.unwrap_or(env::current_dir()?),
        json_report,
    })
}

fn execute(arguments: Arguments) -> Result<()> {
    let manifest_path = if arguments.manifest.is_absolute() {
        arguments.manifest.clone()
    } else {
        arguments.root.join(&arguments.manifest)
    };
    let qualification = Qualification::load_and_evaluate(&manifest_path, &arguments.root)?;
    report::print(&qualification);
    if let Some(path) = arguments.json_report.as_deref() {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            arguments.root.join(path)
        };
        report::write_json(&qualification, &path)?;
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

fn main() -> ExitCode {
    let arguments = match parse_arguments(env::args().skip(1)) {
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
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn command_requires_one_explicit_manifest() {
        let parsed = parse_arguments([
            "evaluate".to_owned(),
            "--manifest".to_owned(),
            "qualification/test.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(parsed.command, Command::Evaluate);
        assert_eq!(parsed.manifest, Path::new("qualification/test.toml"));
        assert!(parsed.json_report.is_none());
    }

    #[test]
    fn removed_check_command_is_rejected() {
        let error = parse_arguments([
            "check".to_owned(),
            "--manifest".to_owned(),
            "qualification/test.toml".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("unknown qualification command"));
    }

    #[test]
    fn command_rejects_silent_extra_options() {
        let error = parse_arguments([
            "gate".to_owned(),
            "--manifest".to_owned(),
            "test.toml".to_owned(),
            "--best-effort".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("unknown option"));
    }
}
