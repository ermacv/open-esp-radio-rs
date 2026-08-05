//! Typed top-level command-line parsing.

use std::path::PathBuf;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
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
            ("image" | "inspect" | "reference" | "driver" | "mmio" | "ir", Some(command)) => {
                return Err(format!("unknown {value} command: {command}").into());
            }
            ("image" | "inspect" | "reference" | "driver" | "mmio" | "ir", None) => {
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Invocation {
    pub(crate) command: Command,
    pub(crate) target_spec: PathBuf,
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
        let mut target_spec = None;
        let mut run_spec = None;
        let mut svd_paths = Vec::new();
        let mut filtered = Vec::new();
        let mut index = 0;
        while index < remaining.len() {
            if remaining[index] == "--target-spec" {
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
        let target_spec = target_spec.ok_or("missing --target-spec")?;
        Ok(Self {
            command,
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
        assert_eq!(invocation.target_spec, PathBuf::from("target.spec"));
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
