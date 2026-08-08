//! Resolution of caller-owned run bindings into typed command arguments.

use std::path::PathBuf;

use super::{Command, CommandArguments, arguments};
use crate::run_spec::RunSpec;

pub(super) fn apply_run_spec_defaults(
    command: Command,
    arguments: &mut CommandArguments,
    run_spec: &RunSpec,
) {
    let use_default_companions = match arguments {
        CommandArguments::IrExport(arguments) => arguments.companion.is_empty(),
        CommandArguments::InspectAnalyze(arguments) => arguments.companion.is_empty(),
        CommandArguments::Reference(arguments) => arguments.companion.is_empty(),
        CommandArguments::ReferenceBatch(arguments) => arguments.companion.is_empty(),
        CommandArguments::DriverGenerate(arguments) => arguments.companion.is_empty(),
        _ => false,
    };
    for (role, path) in run_spec.inputs() {
        if !command.accepts_run_input_role(role) {
            continue;
        }
        match arguments {
            CommandArguments::ImageAudit(args) if role == "artifact" => {
                args.artifact.get_or_insert_with(|| path.clone());
            }
            CommandArguments::MmioDiscover(args) => {
                if let Some(source) = role.strip_prefix("source-artifact:") {
                    push_named_path(&mut args.artifact, source, path);
                }
            }
            CommandArguments::IrExport(args) => {
                if let Some(source) = role.strip_prefix("source-artifact:") {
                    push_named_path(&mut args.artifact, source, path);
                } else if role == "companion" && use_default_companions {
                    args.companion.push(path.clone());
                }
            }
            CommandArguments::TraceInput(args) => apply_trace_input(args, role, path),
            CommandArguments::InspectCompare(args) => {
                if role == "artifact" && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                }
            }
            CommandArguments::InspectAnalyze(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            CommandArguments::Reference(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            CommandArguments::ReferenceBatch(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            CommandArguments::DriverGenerate(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            CommandArguments::ExecuteRun(args) => {
                if role == "artifact" && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                } else if role == "companion" && args.companion.is_none() {
                    args.companion = Some(path.clone());
                }
            }
            CommandArguments::ExecuteCompare(args) => match role.as_str() {
                "vendor-artifact" if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                "vendor-companion" if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                "rust-artifact" if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                "rust-companion" if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifyProfiles(args) => match role.as_str() {
                "vendor-artifact" if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                "vendor-companion" if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                "rust-artifact" if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                "rust-companion" if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifySource(args) => match role.as_str() {
                "vendor-artifact" if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                "vendor-inventory" if args.vendor_inventory.is_none() => {
                    args.vendor_inventory = Some(path.clone())
                }
                "vendor-companion" if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                "rust-artifact" if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                "rust-companion" if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifyInventory(args) => {
                if let Some(source) = role.strip_prefix("source-artifact:") {
                    push_named_path(&mut args.source_artifact, source, path);
                } else if let Some(source) = role.strip_prefix("source-inventory:") {
                    push_named_path(&mut args.source_inventory, source, path);
                } else if let Some(source) = role.strip_prefix("source-companion:") {
                    push_named_path(&mut args.source_companion, source, path);
                } else if role == "rust-artifact" && args.rust_artifact.is_none() {
                    args.rust_artifact = Some(path.clone());
                } else if role == "rust-companion" && args.rust_companion.is_none() {
                    args.rust_companion = Some(path.clone());
                }
            }
            CommandArguments::VerifyContract(args) => match role.as_str() {
                "vendor-artifact" if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                "vendor-companion" if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn apply_trace_input(
    arguments: &mut arguments::TraceInputArgs,
    role: &str,
    path: &std::path::Path,
) {
    if role == "artifact" && arguments.artifact.is_none() {
        arguments.artifact = Some(path.to_owned());
    }
}

fn set_path_role(
    artifact: &mut Option<PathBuf>,
    companions: &mut Vec<PathBuf>,
    role: &str,
    path: &std::path::Path,
    use_default_companions: bool,
) {
    if role == "artifact" && artifact.is_none() {
        *artifact = Some(path.to_owned());
    } else if role == "companion" && use_default_companions {
        companions.push(path.to_owned());
    }
}

fn push_named_path(values: &mut Vec<String>, name: &str, path: &std::path::Path) {
    if values.iter().any(|value| {
        value
            .split_once('=')
            .is_some_and(|(current, _)| current == name)
    }) {
        return;
    }
    values.push(format!("{name}={}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{IrExportArgs, ReferenceArgs};

    fn run_spec(name: &str, contents: &str) -> (std::path::PathBuf, RunSpec) {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-resolver-{}-{}.run",
            std::process::id(),
            name
        ));
        std::fs::write(&path, format!("schema 1\n{contents}")).unwrap();
        let run_spec = RunSpec::load(&path).unwrap();
        (path, run_spec)
    }

    #[test]
    fn explicit_cli_paths_win_over_all_run_spec_defaults() {
        let (path, run_spec) = run_spec(
            "explicit",
            "input artifact default.elf\ninput companion first.a\ninput companion second.a\n",
        );
        let mut arguments = CommandArguments::Reference(ReferenceArgs {
            artifact: Some("explicit.elf".into()),
            companion: vec!["explicit.a".into()],
            ..Default::default()
        });
        apply_run_spec_defaults(Command::GenerateReference, &mut arguments, &run_spec);
        std::fs::remove_file(path).unwrap();
        let CommandArguments::Reference(arguments) = arguments else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.artifact, Some("explicit.elf".into()));
        assert_eq!(arguments.companion, [PathBuf::from("explicit.a")]);
    }

    #[test]
    fn all_default_companions_are_preserved_when_cli_has_none() {
        let (path, run_spec) = run_spec(
            "companions",
            "input artifact default.elf\ninput companion first.a\ninput companion second.a\n",
        );
        let mut arguments = CommandArguments::Reference(ReferenceArgs::default());
        apply_run_spec_defaults(Command::GenerateReference, &mut arguments, &run_spec);
        std::fs::remove_file(path).unwrap();
        let CommandArguments::Reference(arguments) = arguments else {
            panic!("unexpected argument type")
        };
        assert!(arguments.artifact.unwrap().ends_with("default.elf"));
        assert_eq!(arguments.companion.len(), 2);
        assert!(arguments.companion[0].ends_with("first.a"));
        assert!(arguments.companion[1].ends_with("second.a"));
    }

    #[test]
    fn source_qualified_cli_artifacts_override_only_the_same_source() {
        let (path, run_spec) = run_spec(
            "sources",
            "input source-artifact:rom default-rom.elf\ninput source-artifact:archive archive.elf\n",
        );
        let mut arguments = CommandArguments::IrExport(IrExportArgs {
            artifact: vec!["rom=explicit-rom.elf".to_owned()],
            ..Default::default()
        });
        apply_run_spec_defaults(Command::ExportIr, &mut arguments, &run_spec);
        std::fs::remove_file(path).unwrap();
        let CommandArguments::IrExport(arguments) = arguments else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.artifact[0], "rom=explicit-rom.elf");
        assert!(
            arguments.artifact[1]
                .strip_prefix("archive=")
                .unwrap()
                .ends_with("archive.elf")
        );
    }
}
