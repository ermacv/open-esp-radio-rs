//! Typed top-level command-line parsing.

use std::path::PathBuf;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    ProjectInit,
    ProjectConfigure,
    ProjectDoctor,
    ProjectStatus,
    ProjectBuild,
    ProjectCheck,
    ProjectPublish,
    FunctionInitPack,
    FunctionValidate,
    FunctionReview,
    RegisterInitModel,
    RegisterImportSvd,
    RegisterValidate,
    RegisterReview,
    RegisterExportSvd,
    RegisterGeneratePac,
    RegisterGenerateBindings,
    SymbolInventory,
    InterfaceDiscover,
    InterfaceInitPack,
    InterfaceValidate,
    AuditImageTargets,
    DiscoverMmio,
    ExportIr,
    BuildIr,
    VerifyContractChannel,
    VerifyContractRfInit,
    ExecuteRun,
    ExecuteCompare,
    VerifyProfiles,
    VerifyEvidence,
    GenerateReference,
    GenerateReferenceBatch,
    GenerateDriver,
    InspectAnalyze,
    VerifyInventory,
    VerifySource,
    InspectTrace,
    InspectCompare,
}

impl Command {
    fn parse(value: &str, remaining: &mut Vec<String>) -> Result<Self> {
        Ok(match (value, remaining.first().map(String::as_str)) {
            ("project", Some("init")) => {
                remaining.remove(0);
                Self::ProjectInit
            }
            ("project", Some("configure")) => {
                remaining.remove(0);
                Self::ProjectConfigure
            }
            ("project", Some("doctor")) => {
                remaining.remove(0);
                Self::ProjectDoctor
            }
            ("project", Some("status")) => {
                remaining.remove(0);
                Self::ProjectStatus
            }
            ("project", Some("build")) => {
                remaining.remove(0);
                Self::ProjectBuild
            }
            ("project", Some("check")) => {
                remaining.remove(0);
                Self::ProjectCheck
            }
            ("project", Some("publish")) => {
                remaining.remove(0);
                Self::ProjectPublish
            }
            ("functions", Some("init-pack")) => {
                remaining.remove(0);
                Self::FunctionInitPack
            }
            ("functions", Some("validate")) => {
                remaining.remove(0);
                Self::FunctionValidate
            }
            ("functions", Some("review")) => {
                remaining.remove(0);
                Self::FunctionReview
            }
            ("registers", Some("init-model")) => {
                remaining.remove(0);
                Self::RegisterInitModel
            }
            ("registers", Some("import-svd")) => {
                remaining.remove(0);
                Self::RegisterImportSvd
            }
            ("registers", Some("validate")) => {
                remaining.remove(0);
                Self::RegisterValidate
            }
            ("registers", Some("review")) => {
                remaining.remove(0);
                Self::RegisterReview
            }
            ("registers", Some("export-svd")) => {
                remaining.remove(0);
                Self::RegisterExportSvd
            }
            ("registers", Some("generate-pac")) => {
                remaining.remove(0);
                Self::RegisterGeneratePac
            }
            ("registers", Some("generate-bindings")) => {
                remaining.remove(0);
                Self::RegisterGenerateBindings
            }
            ("symbols", Some("inventory")) => {
                remaining.remove(0);
                Self::SymbolInventory
            }
            ("interfaces", Some("discover")) => {
                remaining.remove(0);
                Self::InterfaceDiscover
            }
            ("interfaces", Some("init-pack")) => {
                remaining.remove(0);
                Self::InterfaceInitPack
            }
            ("interfaces", Some("validate")) => {
                remaining.remove(0);
                Self::InterfaceValidate
            }
            ("mmio", Some("discover")) => {
                remaining.remove(0);
                Self::DiscoverMmio
            }
            ("ir", Some("export")) => {
                remaining.remove(0);
                Self::ExportIr
            }
            ("ir", Some("build")) => {
                remaining.remove(0);
                Self::BuildIr
            }
            ("image", Some("audit-targets")) => {
                remaining.remove(0);
                Self::AuditImageTargets
            }
            ("inspect", Some("analyze")) => {
                remaining.remove(0);
                Self::InspectAnalyze
            }
            ("inspect", Some("trace")) => {
                remaining.remove(0);
                Self::InspectTrace
            }
            ("inspect", Some("compare")) => {
                remaining.remove(0);
                Self::InspectCompare
            }
            ("reference", Some("generate")) => {
                remaining.remove(0);
                Self::GenerateReference
            }
            ("reference", Some("generate-batch")) => {
                remaining.remove(0);
                Self::GenerateReferenceBatch
            }
            ("driver", Some("generate")) => {
                remaining.remove(0);
                Self::GenerateDriver
            }
            ("execute", Some("run")) => {
                remaining.remove(0);
                Self::ExecuteRun
            }
            ("execute", Some("compare")) => {
                remaining.remove(0);
                Self::ExecuteCompare
            }
            ("verify", Some("profiles")) => {
                remaining.remove(0);
                Self::VerifyProfiles
            }
            ("verify", Some("source")) => {
                remaining.remove(0);
                Self::VerifySource
            }
            ("verify", Some("inventory")) => {
                remaining.remove(0);
                Self::VerifyInventory
            }
            ("verify", Some("evidence")) => {
                remaining.remove(0);
                Self::VerifyEvidence
            }
            ("verify", Some("contract")) => {
                remaining.remove(0);
                match remaining.first().map(String::as_str) {
                    Some("channel") => {
                        remaining.remove(0);
                        Self::VerifyContractChannel
                    }
                    Some("rf-init") => {
                        remaining.remove(0);
                        Self::VerifyContractRfInit
                    }
                    Some(contract) => {
                        return Err(format!("unknown verification contract: {contract}").into());
                    }
                    None => return Err("verify contract requires a contract name".into()),
                }
            }
            (
                "project" | "functions" | "registers" | "symbols" | "interfaces" | "image"
                | "inspect" | "reference" | "driver" | "mmio" | "ir" | "execute" | "verify",
                Some(command),
            ) => {
                return Err(format!("unknown {value} command: {command}").into());
            }
            (
                "project" | "functions" | "registers" | "symbols" | "interfaces" | "image"
                | "inspect" | "reference" | "driver" | "mmio" | "ir" | "execute" | "verify",
                None,
            ) => {
                return Err(format!("{value} requires a command").into());
            }
            _ => return Err(format!("unknown command: {value}").into()),
        })
    }

    pub(crate) const fn requires_harness(self) -> bool {
        matches!(
            self,
            Self::VerifyContractChannel
                | Self::VerifyContractRfInit
                | Self::GenerateReference
                | Self::GenerateReferenceBatch
                | Self::GenerateDriver
                | Self::InspectAnalyze
                | Self::VerifyInventory
                | Self::VerifySource
        )
    }

    pub(crate) const fn requires_backend(self) -> bool {
        !matches!(
            self,
            Self::ProjectInit
                | Self::ProjectConfigure
                | Self::ProjectDoctor
                | Self::ProjectStatus
                | Self::ProjectPublish
                | Self::FunctionInitPack
                | Self::FunctionValidate
                | Self::FunctionReview
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::RegisterGenerateBindings
                | Self::VerifyEvidence
        )
    }

    pub(crate) const fn requires_mmio_map(self) -> bool {
        !matches!(
            self,
            Self::ProjectInit
                | Self::ProjectConfigure
                | Self::ProjectDoctor
                | Self::ProjectStatus
                | Self::ProjectBuild
                | Self::ProjectCheck
                | Self::ProjectPublish
                | Self::FunctionInitPack
                | Self::FunctionValidate
                | Self::FunctionReview
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::RegisterGenerateBindings
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditImageTargets
                | Self::DiscoverMmio
                | Self::ExportIr
                | Self::BuildIr
                | Self::VerifyEvidence
        )
    }

    pub(crate) const fn uses_memory_map(self) -> bool {
        !matches!(
            self,
            Self::ProjectInit
                | Self::ProjectConfigure
                | Self::FunctionInitPack
                | Self::FunctionValidate
                | Self::FunctionReview
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::RegisterGenerateBindings
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditImageTargets
                | Self::VerifyEvidence
        )
    }

    pub(crate) const fn uses_register_catalog(self) -> bool {
        !matches!(
            self,
            Self::ProjectInit
                | Self::ProjectConfigure
                | Self::ProjectStatus
                | Self::FunctionInitPack
                | Self::FunctionValidate
                | Self::FunctionReview
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::RegisterGenerateBindings
                | Self::ProjectPublish
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditImageTargets
                | Self::VerifyEvidence
        )
    }

    pub(crate) const fn uses_run_spec(self) -> bool {
        !matches!(
            self,
            Self::ProjectInit
                | Self::ProjectConfigure
                | Self::FunctionInitPack
                | Self::FunctionValidate
                | Self::FunctionReview
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::RegisterGenerateBindings
                | Self::ProjectPublish
                | Self::VerifyEvidence
        )
    }

    pub(crate) fn accepts_run_input_role(self, role: &str) -> bool {
        match self {
            Self::ProjectInit
            | Self::ProjectConfigure
            | Self::ProjectDoctor
            | Self::ProjectStatus
            | Self::ProjectBuild
            | Self::ProjectCheck
            | Self::ProjectPublish
            | Self::FunctionInitPack
            | Self::FunctionValidate
            | Self::FunctionReview
            | Self::InterfaceInitPack
            | Self::InterfaceValidate
            | Self::RegisterInitModel
            | Self::RegisterImportSvd
            | Self::RegisterValidate
            | Self::RegisterReview
            | Self::RegisterExportSvd
            | Self::RegisterGeneratePac
            | Self::RegisterGenerateBindings
            | Self::SymbolInventory
            | Self::InterfaceDiscover
            | Self::BuildIr
            | Self::VerifyEvidence => false,
            Self::DiscoverMmio => role
                .strip_prefix("source-artifact:")
                .is_some_and(|source| !source.is_empty()),
            Self::ExportIr => {
                role == "companion"
                    || role
                        .strip_prefix("source-artifact:")
                        .is_some_and(|source| !source.is_empty())
            }
            Self::AuditImageTargets => role == "artifact",
            _ => true,
        }
    }

    pub(crate) fn input_role_is_overridden(self, role: &str, arguments: &[String]) -> bool {
        let option = format!("--{role}");
        if arguments.iter().any(|argument| argument == &option) {
            return true;
        }
        let Some(source) = role.strip_prefix("source-artifact:") else {
            return false;
        };
        if !matches!(self, Self::DiscoverMmio | Self::ExportIr) {
            return false;
        }
        arguments.windows(2).any(|pair| {
            pair[0] == "--artifact"
                && pair[1]
                    .split_once('=')
                    .is_some_and(|(explicit_source, _)| explicit_source == source)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Invocation {
    pub(crate) command: Command,
    pub(crate) project: Option<PathBuf>,
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) arguments: Vec<String>,
}

impl Invocation {
    pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut arguments = arguments.into_iter();
        let primary = arguments.next().ok_or("missing command")?;
        let mut remaining = arguments.collect::<Vec<_>>();
        let command = Command::parse(&primary, &mut remaining)?;
        let mut project = None;
        let mut target_spec = None;
        let mut run_spec = None;
        let mut svd_paths = Vec::new();
        let mut filtered = Vec::new();
        let mut index = 0;
        while index < remaining.len() {
            if remaining[index] == "--project" {
                let path = remaining
                    .get(index + 1)
                    .ok_or("--project requires a value")?;
                if project.replace(PathBuf::from(path)).is_some() {
                    return Err("duplicate --project".into());
                }
                index += 2;
            } else if remaining[index] == "--target-spec" {
                let path = remaining
                    .get(index + 1)
                    .ok_or("--target-spec requires a value")?;
                if target_spec.replace(PathBuf::from(path)).is_some() {
                    return Err("duplicate --target-spec".into());
                }
                index += 2;
            } else if remaining[index] == "--run-spec" {
                let path = remaining
                    .get(index + 1)
                    .ok_or("--run-spec requires a value")?;
                if run_spec.replace(PathBuf::from(path)).is_some() {
                    return Err("duplicate --run-spec".into());
                }
                index += 2;
            } else if remaining[index] == "--svd" {
                let path = remaining.get(index + 1).ok_or("--svd requires a value")?;
                svd_paths.push(PathBuf::from(path));
                index += 2;
            } else {
                filtered.push(remaining[index].clone());
                index += 1;
            }
        }
        if project.is_some() && target_spec.is_some() {
            return Err("--project and --target-spec are mutually exclusive".into());
        }
        Ok(Self {
            command,
            project,
            target_spec,
            run_spec,
            svd_paths,
            arguments: filtered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shared_svd_arguments_without_reordering_command_arguments() {
        let invocation = Invocation::parse([
            "inspect".to_owned(),
            "trace".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--run-spec".to_owned(),
            "local.run".to_owned(),
            "--artifact".to_owned(),
            "oracle.a".to_owned(),
            "--svd".to_owned(),
            "radio.svd".to_owned(),
            "--symbol".to_owned(),
            "target".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::InspectTrace);
        assert_eq!(invocation.project, None);
        assert_eq!(invocation.target_spec, Some(PathBuf::from("target.spec")));
        assert_eq!(invocation.run_spec, Some(PathBuf::from("local.run")));
        assert_eq!(invocation.svd_paths, [PathBuf::from("radio.svd")]);
        assert_eq!(
            invocation.arguments,
            ["--artifact", "oracle.a", "--symbol", "target"]
        );
    }

    #[test]
    fn rejects_unknown_commands_before_loading_svd() {
        let error = Invocation::parse(["guess".to_owned(), "--svd".to_owned(), "x".to_owned()])
            .unwrap_err();
        assert!(error.to_string().contains("unknown command"));
    }

    #[test]
    fn image_target_audit_does_not_require_an_unrelated_svd() {
        let invocation = Invocation::parse([
            "image".to_owned(),
            "audit-targets".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--artifact".to_owned(),
            "runtime.elf".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::AuditImageTargets);
        assert!(invocation.svd_paths.is_empty());
    }

    #[test]
    fn evidence_review_is_project_scoped_but_needs_no_analysis_inputs() {
        let invocation = Invocation::parse([
            "verify".to_owned(),
            "evidence".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--report".to_owned(),
            "oracle-regression.json".to_owned(),
            "--candidate".to_owned(),
            "oracle-regression.candidate.evidence".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::VerifyEvidence);
        assert!(!invocation.command.requires_backend());
        assert!(!invocation.command.requires_harness());
        assert!(!invocation.command.requires_mmio_map());
        assert!(!invocation.command.uses_memory_map());
        assert!(!invocation.command.uses_register_catalog());
        assert!(!invocation.command.uses_run_spec());
    }

    #[test]
    fn accepts_a_project_as_the_composed_configuration_root() {
        let invocation = Invocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(
            invocation.project,
            Some(PathBuf::from("vendor-project.toml"))
        );
        assert_eq!(invocation.target_spec, None);
    }

    #[test]
    fn parses_project_doctor_without_command_arguments() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "doctor".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectDoctor);
        assert!(invocation.arguments.is_empty());
    }

    #[test]
    fn parses_project_init_without_an_existing_configuration_root() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "init".to_owned(),
            "--directory".to_owned(),
            "new-project".to_owned(),
            "--id".to_owned(),
            "radio".to_owned(),
            "--mmio".to_owned(),
            "radio=0x20000000..0x20010000".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectInit);
        assert_eq!(invocation.project, None);
        assert_eq!(
            invocation.arguments,
            [
                "--directory",
                "new-project",
                "--id",
                "radio",
                "--mmio",
                "radio=0x20000000..0x20010000"
            ]
        );
        assert!(!invocation.command.requires_backend());
        assert!(!invocation.command.uses_memory_map());
        assert!(!invocation.command.uses_run_spec());
    }

    #[test]
    fn parses_project_configure_as_a_manifest_only_command() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "configure".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--platform-pack".to_owned(),
            "platform.toml".to_owned(),
            "--check".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectConfigure);
        assert_eq!(
            invocation.arguments,
            ["--platform-pack", "platform.toml", "--check"]
        );
        assert!(!invocation.command.requires_backend());
        assert!(!invocation.command.uses_memory_map());
        assert!(!invocation.command.uses_run_spec());
    }

    #[test]
    fn parses_project_status_as_read_only_project_inventory() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "status".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--json-report".to_owned(),
            "status.json".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectStatus);
        assert_eq!(invocation.arguments, ["--json-report", "status.json"]);
        assert!(!invocation.command.requires_backend());
        assert!(invocation.command.uses_memory_map());
        assert!(!invocation.command.uses_register_catalog());
        assert!(invocation.command.uses_run_spec());
    }

    #[test]
    fn parses_project_build_and_check_as_project_owned_pipelines() {
        for (name, expected) in [
            ("build", Command::ProjectBuild),
            ("check", Command::ProjectCheck),
        ] {
            let invocation = Invocation::parse([
                "project".to_owned(),
                name.to_owned(),
                "--project".to_owned(),
                "vendor-project.toml".to_owned(),
                "--deny-unreviewed".to_owned(),
            ])
            .unwrap();
            assert_eq!(invocation.command, expected);
            assert_eq!(invocation.arguments, ["--deny-unreviewed"]);
            assert!(invocation.command.uses_memory_map());
            assert!(invocation.command.uses_register_catalog());
            assert!(invocation.command.uses_run_spec());
            assert!(!invocation.command.requires_mmio_map());
        }
    }

    #[test]
    fn parses_project_publish_without_analysis_inputs() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "publish".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--check".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectPublish);
        assert_eq!(invocation.arguments, ["--check"]);
        assert!(invocation.command.uses_memory_map());
        assert!(!invocation.command.uses_register_catalog());
        assert!(!invocation.command.uses_run_spec());
        assert!(!invocation.command.requires_backend());
        assert!(!invocation.command.requires_mmio_map());
    }

    #[test]
    fn parses_project_symbol_inventory() {
        let invocation = Invocation::parse([
            "symbols".to_owned(),
            "inventory".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--undefined-only".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::SymbolInventory);
        assert_eq!(invocation.arguments, ["--undefined-only"]);
    }

    #[test]
    fn parses_generic_interface_discovery_without_semantic_capabilities() {
        let invocation = Invocation::parse([
            "interfaces".to_owned(),
            "discover".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--source".to_owned(),
            "libpp".to_owned(),
            "--tables-only".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::InterfaceDiscover);
        assert_eq!(invocation.arguments, ["--source", "libpp", "--tables-only"]);
        assert!(!invocation.command.requires_harness());
        assert!(!invocation.command.requires_mmio_map());
    }

    #[test]
    fn parses_interface_pack_lifecycle_without_backend_inputs() {
        let init = Invocation::parse([
            "interfaces".to_owned(),
            "init-pack".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(init.command, Command::InterfaceInitPack);
        assert!(!init.command.requires_backend());
        assert!(!init.command.uses_run_spec());

        let validate = Invocation::parse([
            "interfaces".to_owned(),
            "validate".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--deny-unreviewed".to_owned(),
        ])
        .unwrap();
        assert_eq!(validate.command, Command::InterfaceValidate);
        assert_eq!(validate.arguments, ["--deny-unreviewed"]);
    }

    #[test]
    fn parses_function_pack_lifecycle_without_backend_inputs() {
        for (name, command) in [
            ("init-pack", Command::FunctionInitPack),
            ("validate", Command::FunctionValidate),
            ("review", Command::FunctionReview),
        ] {
            let invocation = Invocation::parse([
                "functions".to_owned(),
                name.to_owned(),
                "--project".to_owned(),
                "vendor-project.toml".to_owned(),
            ])
            .unwrap();
            assert_eq!(invocation.command, command);
            assert!(!invocation.command.requires_backend());
            assert!(!invocation.command.uses_run_spec());
            assert!(!invocation.command.uses_memory_map());
            assert!(!invocation.command.uses_register_catalog());
        }
    }

    #[test]
    fn parses_register_workspace_commands() {
        for (name, command, uses_memory_map, uses_register_catalog) in [
            ("init-model", Command::RegisterInitModel, true, false),
            ("import-svd", Command::RegisterImportSvd, true, false),
            ("validate", Command::RegisterValidate, true, true),
            ("review", Command::RegisterReview, false, false),
            ("export-svd", Command::RegisterExportSvd, false, false),
            ("generate-pac", Command::RegisterGeneratePac, false, false),
            (
                "generate-bindings",
                Command::RegisterGenerateBindings,
                false,
                false,
            ),
        ] {
            let invocation = Invocation::parse([
                "registers".to_owned(),
                name.to_owned(),
                "--project".to_owned(),
                "vendor-project.toml".to_owned(),
            ])
            .unwrap();
            assert_eq!(invocation.command, command);
            assert_eq!(invocation.command.uses_memory_map(), uses_memory_map);
            assert_eq!(
                invocation.command.uses_register_catalog(),
                uses_register_catalog
            );
        }
    }

    #[test]
    fn rejects_ambiguous_project_and_target_roots() {
        let error = Invocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn named_artifact_overrides_the_same_project_source() {
        let arguments = ["--artifact".to_owned(), "rom=override.elf".to_owned()];
        assert!(Command::DiscoverMmio.input_role_is_overridden("source-artifact:rom", &arguments));
        assert!(
            !Command::DiscoverMmio.input_role_is_overridden("source-artifact:archive", &arguments)
        );
    }

    #[test]
    fn parses_hierarchical_workflow_commands() {
        let invocation = Invocation::parse([
            "verify".to_owned(),
            "contract".to_owned(),
            "channel".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::VerifyContractChannel);
        assert!(invocation.command.requires_harness());

        let invocation = Invocation::parse([
            "reference".to_owned(),
            "generate".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--artifact".to_owned(),
            "input.elf".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::GenerateReference);
        assert_eq!(invocation.run_spec, None);
        assert_eq!(invocation.arguments, ["--artifact", "input.elf"]);

        let invocation = Invocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--artifact".to_owned(),
            "rom=rom.elf".to_owned(),
            "--range".to_owned(),
            "radio=0x20000000..0x20010000".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::DiscoverMmio);
        assert_eq!(
            invocation.arguments,
            [
                "--artifact",
                "rom=rom.elf",
                "--range",
                "radio=0x20000000..0x20010000"
            ]
        );

        let invocation = Invocation::parse([
            "ir".to_owned(),
            "export".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--artifact".to_owned(),
            "vendor=vendor.a".to_owned(),
            "--include-reachable".to_owned(),
            "--pseudo-rust".to_owned(),
            "vendor.pseudo.rs".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ExportIr);
        assert_eq!(
            invocation.arguments,
            [
                "--artifact",
                "vendor=vendor.a",
                "--include-reachable",
                "--pseudo-rust",
                "vendor.pseudo.rs"
            ]
        );

        let invocation = Invocation::parse([
            "ir".to_owned(),
            "build".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
            "--run-spec".to_owned(),
            "local.run".to_owned(),
            "--profile".to_owned(),
            "phy".to_owned(),
            "--check".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::BuildIr);
        assert_eq!(invocation.arguments, ["--profile", "phy", "--check"]);
        assert!(invocation.command.requires_backend());
        assert!(invocation.command.uses_run_spec());
        assert!(!invocation.command.requires_mmio_map());
    }

    #[test]
    fn rejects_removed_flat_command_aliases() {
        for command in [
            "audit-direct-targets",
            "qualify-esp32s31-channel",
            "qualify-esp32s31-rf-init",
            "execute-compare",
            "verify-profiles",
            "generate-reference",
            "generate-reference-batch",
            "analyze",
            "verify-all",
            "extract",
            "compare",
        ] {
            assert!(
                Invocation::parse([command.to_owned()])
                    .unwrap_err()
                    .to_string()
                    .contains("unknown command"),
                "removed alias {command} was still accepted"
            );
        }
    }
}
