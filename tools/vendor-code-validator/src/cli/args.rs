//! Typed top-level command-line parsing.

use std::path::PathBuf;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    ProjectDoctor,
    RegisterInitOverlay,
    RegisterInitModel,
    RegisterImportSvd,
    RegisterValidate,
    RegisterReview,
    RegisterExportSvd,
    RegisterGeneratePac,
    SymbolInventory,
    InterfaceDiscover,
    InterfaceInitPack,
    InterfaceValidate,
    AuditDirectTargets,
    DiscoverMmio,
    ExportIr,
    QualifyContractChannel,
    QualifyContractRfInit,
    Execute,
    ExecuteCompare,
    VerifyProfiles,
    GenerateReference,
    GenerateReferenceBatch,
    GenerateDriver,
    Analyze,
    VerifyAll,
    Verify,
    Extract,
    Compare,
}

impl Command {
    fn parse(value: &str, remaining: &mut Vec<String>) -> Result<Self> {
        Ok(match (value, remaining.first().map(String::as_str)) {
            ("project", Some("doctor")) => {
                remaining.remove(0);
                Self::ProjectDoctor
            }
            ("registers", Some("init-overlay")) => {
                remaining.remove(0);
                Self::RegisterInitOverlay
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
            ("image", Some("audit-targets")) => {
                remaining.remove(0);
                Self::AuditDirectTargets
            }
            ("inspect", Some("analyze")) => {
                remaining.remove(0);
                Self::Analyze
            }
            ("inspect", Some("trace")) => {
                remaining.remove(0);
                Self::Extract
            }
            ("inspect", Some("compare")) => {
                remaining.remove(0);
                Self::Compare
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
                Self::Execute
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
                Self::Verify
            }
            ("verify", Some("inventory")) => {
                remaining.remove(0);
                Self::VerifyAll
            }
            ("verify", Some("contract")) => {
                remaining.remove(0);
                match remaining.first().map(String::as_str) {
                    Some("channel") => {
                        remaining.remove(0);
                        Self::QualifyContractChannel
                    }
                    Some("rf-init") => {
                        remaining.remove(0);
                        Self::QualifyContractRfInit
                    }
                    Some(contract) => {
                        return Err(format!("unknown verification contract: {contract}").into());
                    }
                    None => return Err("verify contract requires a contract name".into()),
                }
            }
            (
                "project" | "registers" | "symbols" | "interfaces" | "image" | "inspect"
                | "reference" | "driver" | "mmio" | "ir",
                Some(command),
            ) => {
                return Err(format!("unknown {value} command: {command}").into());
            }
            (
                "project" | "registers" | "symbols" | "interfaces" | "image" | "inspect"
                | "reference" | "driver" | "mmio" | "ir",
                None,
            ) => {
                return Err(format!("{value} requires a command").into());
            }
            ("audit-direct-targets", _) => Self::AuditDirectTargets,
            ("qualify-esp32s31-channel", _) => Self::QualifyContractChannel,
            ("qualify-esp32s31-rf-init", _) => Self::QualifyContractRfInit,
            ("execute", _) => Self::Execute,
            ("execute-compare", _) => Self::ExecuteCompare,
            ("verify-profiles", _) => Self::VerifyProfiles,
            ("generate-reference", _) => Self::GenerateReference,
            ("generate-reference-batch", _) => Self::GenerateReferenceBatch,
            ("analyze", _) => Self::Analyze,
            ("verify-all", _) => Self::VerifyAll,
            ("verify", _) => Self::Verify,
            ("extract", _) => Self::Extract,
            ("compare", _) => Self::Compare,
            _ => return Err(format!("unknown command: {value}").into()),
        })
    }

    pub(crate) const fn requires_harness(self) -> bool {
        matches!(
            self,
            Self::QualifyContractChannel
                | Self::QualifyContractRfInit
                | Self::GenerateReference
                | Self::GenerateReferenceBatch
                | Self::GenerateDriver
                | Self::Analyze
                | Self::VerifyAll
                | Self::Verify
        )
    }

    pub(crate) const fn requires_backend(self) -> bool {
        !matches!(
            self,
            Self::ProjectDoctor
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitOverlay
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
        )
    }

    pub(crate) const fn requires_mmio_map(self) -> bool {
        !matches!(
            self,
            Self::ProjectDoctor
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitOverlay
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditDirectTargets
                | Self::DiscoverMmio
                | Self::ExportIr
        )
    }

    pub(crate) const fn uses_memory_map(self) -> bool {
        !matches!(
            self,
            Self::RegisterInitOverlay
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditDirectTargets
        )
    }

    pub(crate) const fn uses_register_catalog(self) -> bool {
        !matches!(
            self,
            Self::RegisterInitOverlay
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
                | Self::SymbolInventory
                | Self::InterfaceDiscover
                | Self::AuditDirectTargets
        )
    }

    pub(crate) const fn uses_run_spec(self) -> bool {
        !matches!(
            self,
            Self::InterfaceInitPack
                | Self::InterfaceValidate
                | Self::RegisterInitOverlay
                | Self::RegisterInitModel
                | Self::RegisterImportSvd
                | Self::RegisterValidate
                | Self::RegisterReview
                | Self::RegisterExportSvd
                | Self::RegisterGeneratePac
        )
    }

    pub(crate) fn accepts_run_input_role(self, role: &str) -> bool {
        match self {
            Self::ProjectDoctor
            | Self::InterfaceInitPack
            | Self::InterfaceValidate
            | Self::RegisterInitOverlay
            | Self::RegisterInitModel
            | Self::RegisterImportSvd
            | Self::RegisterValidate
            | Self::RegisterReview
            | Self::RegisterExportSvd
            | Self::RegisterGeneratePac
            | Self::SymbolInventory
            | Self::InterfaceDiscover => false,
            Self::DiscoverMmio => role
                .strip_prefix("source-artifact:")
                .is_some_and(|source| !source.is_empty()),
            Self::ExportIr => {
                role == "companion"
                    || role
                        .strip_prefix("source-artifact:")
                        .is_some_and(|source| !source.is_empty())
            }
            Self::AuditDirectTargets => role == "artifact",
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
            "extract".to_owned(),
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
        assert_eq!(invocation.command, Command::Extract);
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
    fn direct_target_audit_does_not_require_an_unrelated_svd() {
        let invocation = Invocation::parse([
            "audit-direct-targets".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--artifact".to_owned(),
            "runtime.elf".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::AuditDirectTargets);
        assert!(invocation.svd_paths.is_empty());
    }

    #[test]
    fn accepts_a_project_as_the_composed_configuration_root() {
        let invocation = Invocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--project".to_owned(),
            "vendor-validator.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(
            invocation.project,
            Some(PathBuf::from("vendor-validator.toml"))
        );
        assert_eq!(invocation.target_spec, None);
    }

    #[test]
    fn parses_project_doctor_without_command_arguments() {
        let invocation = Invocation::parse([
            "project".to_owned(),
            "doctor".to_owned(),
            "--project".to_owned(),
            "vendor-validator.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectDoctor);
        assert!(invocation.arguments.is_empty());
    }

    #[test]
    fn parses_project_symbol_inventory() {
        let invocation = Invocation::parse([
            "symbols".to_owned(),
            "inventory".to_owned(),
            "--project".to_owned(),
            "vendor-validator.toml".to_owned(),
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
            "vendor-validator.toml".to_owned(),
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
            "vendor-validator.toml".to_owned(),
        ])
        .unwrap();
        assert_eq!(init.command, Command::InterfaceInitPack);
        assert!(!init.command.requires_backend());
        assert!(!init.command.uses_run_spec());

        let validate = Invocation::parse([
            "interfaces".to_owned(),
            "validate".to_owned(),
            "--project".to_owned(),
            "vendor-validator.toml".to_owned(),
            "--deny-unreviewed".to_owned(),
        ])
        .unwrap();
        assert_eq!(validate.command, Command::InterfaceValidate);
        assert_eq!(validate.arguments, ["--deny-unreviewed"]);
    }

    #[test]
    fn parses_register_workspace_commands() {
        for (name, command) in [
            ("init-overlay", Command::RegisterInitOverlay),
            ("init-model", Command::RegisterInitModel),
            ("import-svd", Command::RegisterImportSvd),
            ("validate", Command::RegisterValidate),
            ("review", Command::RegisterReview),
            ("export-svd", Command::RegisterExportSvd),
            ("generate-pac", Command::RegisterGeneratePac),
        ] {
            let invocation = Invocation::parse([
                "registers".to_owned(),
                name.to_owned(),
                "--project".to_owned(),
                "vendor-validator.toml".to_owned(),
            ])
            .unwrap();
            assert_eq!(invocation.command, command);
        }
    }

    #[test]
    fn rejects_ambiguous_project_and_target_roots() {
        let error = Invocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--project".to_owned(),
            "vendor-validator.toml".to_owned(),
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
            "vendor.a".to_owned(),
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
                "vendor.a",
                "--include-reachable",
                "--pseudo-rust",
                "vendor.pseudo.rs"
            ]
        );
    }
}
