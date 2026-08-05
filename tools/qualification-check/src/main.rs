mod model;
mod report;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use model::Ledger;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str =
    "usage: cargo qualification check --manifest PATH [--root PATH] [--json-report PATH]";

#[derive(Debug)]
struct CheckArguments {
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

fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Result<CheckArguments> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.first().map(String::as_str) != Some("check") {
        return Err("the only supported command is `check`".into());
    }
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
    Ok(CheckArguments {
        manifest: manifest.ok_or("missing --manifest")?,
        root: root.unwrap_or(env::current_dir()?),
        json_report,
    })
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<()> {
    let arguments = parse_arguments(arguments)?;
    let manifest_path = if arguments.manifest.is_absolute() {
        arguments.manifest.clone()
    } else {
        arguments.root.join(&arguments.manifest)
    };
    let ledger = Ledger::load(&manifest_path)?;
    ledger.validate(&arguments.root)?;
    report::print(&ledger);
    if let Some(path) = arguments.json_report.as_deref() {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            arguments.root.join(path)
        };
        report::write_json(&ledger, &path)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{USAGE}\nerror: {error}");
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
            "check".to_owned(),
            "--manifest".to_owned(),
            "qualification/test.ledger".to_owned(),
        ])
        .unwrap();
        assert_eq!(parsed.manifest, Path::new("qualification/test.ledger"));
        assert!(parsed.json_report.is_none());
    }

    #[test]
    fn command_rejects_silent_extra_options() {
        let error = parse_arguments([
            "check".to_owned(),
            "--manifest".to_owned(),
            "test.ledger".to_owned(),
            "--best-effort".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("unknown option"));
    }
}
