//! Resolution of caller-owned run bindings into typed command arguments.

use std::path::PathBuf;

use super::{Command, CommandArguments, SourcePath, arguments};
use crate::run_spec::{InputRole, RunSpec};

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
    for input in run_spec.inputs() {
        let role = &input.role;
        let path = &input.path;
        if !command.accepts_run_input_role(role) {
            continue;
        }
        match arguments {
            CommandArguments::ImageAudit(args) if role == &InputRole::Artifact => {
                args.artifact.get_or_insert_with(|| path.clone());
            }
            CommandArguments::MmioDiscover(args) => {
                if let InputRole::SourceArtifact(source) = role {
                    push_source_path(&mut args.artifact, source, path);
                }
            }
            CommandArguments::IrExport(args) => {
                if let InputRole::SourceArtifact(source) = role {
                    push_source_path(&mut args.artifact, source, path);
                } else if role == &InputRole::Companion && use_default_companions {
                    args.companion.push(path.clone());
                }
            }
            CommandArguments::TraceInput(args) => apply_trace_input(args, role, path),
            CommandArguments::InspectCompare(args) => {
                if role == &InputRole::Artifact && args.artifact.is_none() {
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
                if role == &InputRole::Artifact && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                } else if role == &InputRole::Companion && args.companion.is_none() {
                    args.companion = Some(path.clone());
                }
            }
            CommandArguments::ExecuteCompare(args) => match role {
                InputRole::VendorArtifact if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                InputRole::VendorCompanion if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                InputRole::RustArtifact if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                InputRole::RustCompanion if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifyProfiles(args) => match role {
                InputRole::VendorArtifact if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                InputRole::VendorCompanion if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                InputRole::RustArtifact if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                InputRole::RustCompanion if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifySource(args) => match role {
                InputRole::VendorArtifact if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                InputRole::VendorInventory if args.vendor_inventory.is_none() => {
                    args.vendor_inventory = Some(path.clone())
                }
                InputRole::VendorCompanion if args.vendor_companion.is_none() => {
                    args.vendor_companion = Some(path.clone())
                }
                InputRole::RustArtifact if args.rust_artifact.is_none() => {
                    args.rust_artifact = Some(path.clone())
                }
                InputRole::RustCompanion if args.rust_companion.is_none() => {
                    args.rust_companion = Some(path.clone())
                }
                _ => {}
            },
            CommandArguments::VerifyInventory(args) => {
                if let InputRole::SourceArtifact(source) = role {
                    push_source_path(&mut args.source_artifact, source, path);
                } else if let InputRole::SourceInventory(source) = role {
                    push_source_path(&mut args.source_inventory, source, path);
                } else if let InputRole::SourceCompanion(source) = role {
                    push_source_path(&mut args.source_companion, source, path);
                } else if role == &InputRole::RustArtifact && args.rust_artifact.is_none() {
                    args.rust_artifact = Some(path.clone());
                } else if role == &InputRole::RustCompanion && args.rust_companion.is_none() {
                    args.rust_companion = Some(path.clone());
                }
            }
            CommandArguments::VerifyContract(args) => match role {
                InputRole::VendorArtifact if args.vendor_artifact.is_none() => {
                    args.vendor_artifact = Some(path.clone())
                }
                InputRole::VendorCompanion if args.vendor_companion.is_none() => {
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
    role: &InputRole,
    path: &std::path::Path,
) {
    if role == &InputRole::Artifact && arguments.artifact.is_none() {
        arguments.artifact = Some(path.to_owned());
    }
}

fn set_path_role(
    artifact: &mut Option<PathBuf>,
    companions: &mut Vec<PathBuf>,
    role: &InputRole,
    path: &std::path::Path,
    use_default_companions: bool,
) {
    if role == &InputRole::Artifact && artifact.is_none() {
        *artifact = Some(path.to_owned());
    } else if role == &InputRole::Companion && use_default_companions {
        companions.push(path.to_owned());
    }
}

fn push_source_path(
    values: &mut Vec<SourcePath>,
    source: &crate::source_id::SourceId,
    path: &std::path::Path,
) {
    if values.iter().any(|value| value.source == *source) {
        return;
    }
    values.push(SourcePath {
        source: source.clone(),
        path: path.to_owned(),
    });
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
            artifact: vec!["rom=explicit-rom.elf".parse().unwrap()],
            ..Default::default()
        });
        apply_run_spec_defaults(Command::ExportIr, &mut arguments, &run_spec);
        std::fs::remove_file(path).unwrap();
        let CommandArguments::IrExport(arguments) = arguments else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.artifact[0].source.as_str(), "rom");
        assert_eq!(
            arguments.artifact[0].path,
            PathBuf::from("explicit-rom.elf")
        );
        assert_eq!(arguments.artifact[1].source.as_str(), "archive");
        assert!(arguments.artifact[1].path.ends_with("archive.elf"));
    }
}
