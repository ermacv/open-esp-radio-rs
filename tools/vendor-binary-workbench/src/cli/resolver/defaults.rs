//! Precedence-aware defaults applied to typed leaf-command arguments.

use std::path::{Path, PathBuf};

use crate::{
    MemoryMap, ProjectSpec, Result, TargetSpec,
    run_spec::{InputRole, RunSpec},
};

use super::super::{NamedAddressRange, SourcePath, args::Command, arguments};

pub(super) fn apply_target_defaults(_command: &mut Command, _target: &TargetSpec) {}

pub(super) fn apply_project_defaults(
    command: &mut Command,
    project: Option<&ProjectSpec>,
    memory_map: Option<&MemoryMap>,
) -> Result<()> {
    if let Some(project) = project {
        match command {
            Command::VerifySource(arguments) => {
                if arguments.vendor_prefix.is_empty() {
                    let prefixes = project
                        .ir_profiles
                        .iter()
                        .map(|profile| profile.roots.symbol_prefix())
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
            Command::VerifyInventory(arguments) => {
                let explicit = arguments
                    .source_prefix
                    .iter()
                    .chain(&arguments.source_symbol)
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
                        let profile_prefix = profile.roots.symbol_prefix();
                        match prefixes.get(source) {
                            Some(prefix) if prefix != profile_prefix => {
                                conflicting.insert(source.clone());
                            }
                            None => {
                                prefixes.insert(source.clone(), profile_prefix.to_owned());
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
    if let Command::SymbolInventory(arguments) = command
        && arguments.output.is_none()
        && let Some(path) = project
            .and_then(|project| project.symbol_inventory.as_ref())
            .map(|symbols| &symbols.output)
    {
        arguments.output = Some(path.clone());
    }
    if let Command::DiscoverMmio(arguments) = command
        && let Some(memory_map) = memory_map
        && arguments.range.is_empty()
    {
        for (name, start, end) in memory_map.mmio_ranges()? {
            arguments.range.push(NamedAddressRange { name, start, end });
        }
    }
    if let Command::DiscoverMmio(arguments) = command
        && arguments.output.is_none()
        && let Some(path) = project
            .and_then(|project| project.registers.as_ref())
            .map(|registers| &registers.facts)
    {
        arguments.output = Some(path.clone());
    }
    if let Command::InterfaceDiscover(arguments) = command
        && arguments.output.is_none()
        && let Some(path) = project
            .and_then(|project| project.interfaces.as_ref())
            .map(|interfaces| &interfaces.facts)
    {
        arguments.output = Some(path.clone());
    }
    Ok(())
}

pub(super) fn apply_run_spec_defaults(command: &mut Command, run_spec: &RunSpec) {
    let use_default_companions = match command {
        Command::ExportIr(arguments) => arguments.companion.is_empty(),
        Command::InspectAnalyze(arguments) => arguments.companion.is_empty(),
        Command::GenerateReference(arguments) => arguments.companion.is_empty(),
        Command::GenerateReferenceBatch(arguments) => arguments.companion.is_empty(),
        _ => false,
    };
    for input in run_spec.inputs() {
        let role = &input.role;
        let path = &input.path;
        match command {
            Command::AuditImageTargets(args) if role == &InputRole::Artifact => {
                args.artifact.get_or_insert_with(|| path.clone());
            }
            Command::DiscoverMmio(args) => {
                if let InputRole::SourceArtifact(source) = role {
                    push_source_path(&mut args.artifact, source, path);
                }
            }
            Command::ExportIr(args) => {
                if let InputRole::SourceArtifact(source) = role {
                    push_source_path(&mut args.artifact, source, path);
                } else if role == &InputRole::Companion && use_default_companions {
                    args.companion.push(path.clone());
                }
            }
            Command::InspectTrace(args) => apply_trace_input(args, role, path),
            Command::InspectFunction(args) => {
                let source = args.selector.split_once(':').map(|(source, _)| source);
                match role {
                    InputRole::SourceArtifact(candidate)
                        if source == Some(candidate.as_str()) && args.artifact.is_none() =>
                    {
                        args.artifact = Some(path.clone());
                    }
                    InputRole::SourceInventory(candidate)
                        if source == Some(candidate.as_str()) && args.inventory.is_none() =>
                    {
                        args.inventory = Some(path.clone());
                    }
                    _ => {}
                }
            }
            Command::InspectCompare(args) => {
                if role == &InputRole::Artifact && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                }
            }
            Command::InspectAnalyze(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            Command::GenerateReference(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            Command::GenerateReferenceBatch(args) => {
                set_path_role(
                    &mut args.artifact,
                    &mut args.companion,
                    role,
                    path,
                    use_default_companions,
                );
            }
            Command::ExecuteRun(args) => {
                if role == &InputRole::Artifact && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                } else if role == &InputRole::Companion && args.companion.is_none() {
                    args.companion = Some(path.clone());
                }
            }
            Command::ExecuteReplay(args) => {
                let source_matches = args.source.as_deref().is_some_and(|source| {
                    matches!(role, InputRole::SourceArtifact(candidate) if candidate.as_str() == source)
                });
                let companion_matches = args.source.as_deref().is_some_and(|source| {
                    matches!(role, InputRole::SourceCompanion(candidate) if candidate.as_str() == source)
                });
                if (role == &InputRole::Artifact || source_matches) && args.artifact.is_none() {
                    args.artifact = Some(path.clone());
                } else if (role == &InputRole::Companion || companion_matches)
                    && args.companion.is_none()
                {
                    args.companion = Some(path.clone());
                }
            }
            Command::ExecuteCompare(args) => match role {
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
            Command::VerifyProfiles(args) => match role {
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
            Command::VerifySource(args) => match role {
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
            Command::VerifyInventory(args) => {
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
            _ => {}
        }
    }
    if let Command::ExportIr(arguments) = command
        && use_default_companions
        && arguments.artifact.len() == 1
    {
        let source = &arguments.artifact[0].source;
        arguments
            .companion
            .extend(
                run_spec
                    .inputs()
                    .iter()
                    .filter_map(|input| match &input.role {
                        InputRole::SourceCompanion(companion_source)
                            if companion_source == source =>
                        {
                            Some(input.path.clone())
                        }
                        _ => None,
                    }),
            );
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
