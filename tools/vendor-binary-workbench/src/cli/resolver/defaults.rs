//! Precedence-aware defaults applied to typed leaf-command arguments.

use std::path::{Path, PathBuf};

use crate::{
    MemoryMap, ProjectSpec, Result, TargetSpec,
    run_spec::{InputRole, RunSpec},
};

use super::super::{
    NamedAddressRange, SourcePath,
    args::{Command, CommandArguments},
    arguments,
};

pub(super) fn apply_target_defaults(
    command: Command,
    command_arguments: &mut CommandArguments,
    target: &TargetSpec,
) {
    if command == Command::VerifyInventory
        && let CommandArguments::VerifyInventory(arguments) = command_arguments
    {
        if !arguments.no_profiles && arguments.profiles.is_none() {
            arguments.profiles.clone_from(&target.profiles);
        }
        if !arguments.no_dispositions && arguments.dispositions.is_none() {
            arguments.dispositions.clone_from(&target.dispositions);
        }
    }
    if matches!(command, Command::VerifyInventory | Command::VerifyEvidence) {
        match command_arguments {
            CommandArguments::VerifyInventory(arguments)
                if !arguments.no_evidence_baseline && arguments.evidence_baseline.is_none() =>
            {
                arguments
                    .evidence_baseline
                    .clone_from(&target.evidence_baseline);
            }
            CommandArguments::VerifyEvidence(arguments)
                if !arguments.no_evidence_baseline && arguments.evidence_baseline.is_none() =>
            {
                arguments
                    .evidence_baseline
                    .clone_from(&target.evidence_baseline);
            }
            _ => {}
        }
    }
}

pub(super) fn apply_project_defaults(
    command: Command,
    command_arguments: &mut CommandArguments,
    project: Option<&ProjectSpec>,
    memory_map: Option<&MemoryMap>,
) -> Result<()> {
    if let Some(project) = project {
        match command_arguments {
            CommandArguments::VerifySource(arguments) => {
                if arguments.rust_prefix.is_none() {
                    arguments.rust_prefix = project
                        .verification
                        .as_ref()
                        .and_then(|verification| verification.rust_prefix.clone());
                }
                if arguments.vendor_prefix.is_empty() {
                    let prefixes = project
                        .ir_profiles
                        .iter()
                        .map(|profile| profile.symbol_prefix.as_str())
                        .collect::<std::collections::BTreeSet<_>>();
                    if prefixes.len() == 1 {
                        arguments.vendor_prefix = prefixes
                            .into_iter()
                            .next()
                            .expect("one project symbol prefix")
                            .to_owned();
                    }
                }
            }
            CommandArguments::VerifyInventory(arguments) => {
                if arguments.rust_prefix.is_none() {
                    arguments.rust_prefix = project
                        .verification
                        .as_ref()
                        .and_then(|verification| verification.rust_prefix.clone());
                }
                let explicit = arguments
                    .source_prefix
                    .iter()
                    .map(|value| value.source.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let configured = arguments
                    .source_artifact
                    .iter()
                    .chain(&arguments.source_inventory)
                    .chain(&arguments.source_companion)
                    .map(|value| value.source.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let mut prefixes = std::collections::BTreeMap::<String, String>::new();
                let mut conflicting = std::collections::BTreeSet::new();
                for profile in &project.ir_profiles {
                    for source in &profile.sources {
                        match prefixes.get(source) {
                            Some(prefix) if prefix != &profile.symbol_prefix => {
                                conflicting.insert(source.clone());
                            }
                            None => {
                                prefixes.insert(source.clone(), profile.symbol_prefix.clone());
                            }
                            _ => {}
                        }
                    }
                }
                for (source, value) in prefixes {
                    let source = source.parse().map_err(crate::Error::invalid)?;
                    if configured.contains(&source)
                        && !explicit.contains(&source)
                        && !conflicting.contains(source.as_str())
                    {
                        arguments
                            .source_prefix
                            .push(super::super::SourceValue { source, value });
                    }
                }
            }
            _ => {}
        }
    }
    if command == Command::SymbolInventory
        && let CommandArguments::SymbolInventory(arguments) = command_arguments
        && arguments.json_report.is_none()
        && let Some(path) = project
            .and_then(|project| project.symbol_inventory.as_ref())
            .map(|symbols| &symbols.output)
    {
        arguments.json_report = Some(path.clone());
    }
    if command == Command::DiscoverMmio
        && let Some(memory_map) = memory_map
        && let CommandArguments::MmioDiscover(arguments) = command_arguments
        && arguments.range.is_empty()
    {
        for (name, start, end) in memory_map.mmio_ranges()? {
            arguments.range.push(NamedAddressRange { name, start, end });
        }
    }
    if command == Command::DiscoverMmio
        && let CommandArguments::MmioDiscover(arguments) = command_arguments
        && arguments.json_report.is_none()
        && let Some(path) = project
            .and_then(|project| project.registers.as_ref())
            .map(|registers| &registers.facts)
    {
        arguments.json_report = Some(path.clone());
    }
    if command == Command::InterfaceDiscover
        && let CommandArguments::InterfaceDiscover(arguments) = command_arguments
        && arguments.json_report.is_none()
        && let Some(path) = project
            .and_then(|project| project.interfaces.as_ref())
            .map(|interfaces| &interfaces.facts)
    {
        arguments.json_report = Some(path.clone());
    }
    Ok(())
}

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

fn apply_trace_input(arguments: &mut arguments::TraceInputArgs, role: &InputRole, path: &Path) {
    if role == &InputRole::Artifact && arguments.artifact.is_none() {
        arguments.artifact = Some(path.to_owned());
    }
}

fn set_path_role(
    artifact: &mut Option<PathBuf>,
    companions: &mut Vec<PathBuf>,
    role: &InputRole,
    path: &Path,
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
    path: &Path,
) {
    if values.iter().any(|value| value.source == *source) {
        return;
    }
    values.push(SourcePath {
        source: source.clone(),
        path: path.to_owned(),
    });
}
