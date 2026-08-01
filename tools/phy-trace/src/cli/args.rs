//! Typed top-level command-line parsing.

use std::path::PathBuf;

use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    AuditDirectTargets,
    QualifyEsp32s31Channel,
    QualifyEsp32s31RfInit,
    Execute,
    ExecuteCompare,
    VerifyProfiles,
    GenerateReference,
    GenerateReferenceBatch,
    Analyze,
    VerifyAll,
    Verify,
    Extract,
    Compare,
}

impl Command {
    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "audit-direct-targets" => Self::AuditDirectTargets,
            "qualify-esp32s31-channel" => Self::QualifyEsp32s31Channel,
            "qualify-esp32s31-rf-init" => Self::QualifyEsp32s31RfInit,
            "execute" => Self::Execute,
            "execute-compare" => Self::ExecuteCompare,
            "verify-profiles" => Self::VerifyProfiles,
            "generate-reference" => Self::GenerateReference,
            "generate-reference-batch" => Self::GenerateReferenceBatch,
            "analyze" => Self::Analyze,
            "verify-all" => Self::VerifyAll,
            "verify" => Self::Verify,
            "extract" => Self::Extract,
            "compare" => Self::Compare,
            _ => return Err(format!("unknown command: {value}").into()),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Invocation {
    pub(crate) command: Command,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) arguments: Vec<String>,
}

impl Invocation {
    pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut arguments = arguments.into_iter();
        let command = Command::parse(&arguments.next().ok_or("missing command")?)?;
        let remaining = arguments.collect::<Vec<_>>();
        let mut svd_paths = Vec::new();
        let mut filtered = Vec::new();
        let mut index = 0;
        while index < remaining.len() {
            if remaining[index] == "--svd" {
                let path = remaining.get(index + 1).ok_or("--svd requires a value")?;
                svd_paths.push(PathBuf::from(path));
                index += 2;
            } else {
                filtered.push(remaining[index].clone());
                index += 1;
            }
        }
        if svd_paths.is_empty() && command != Command::AuditDirectTargets {
            return Err("missing --svd".into());
        }
        Ok(Self {
            command,
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
            "--artifact".to_owned(),
            "oracle.a".to_owned(),
            "--svd".to_owned(),
            "radio.svd".to_owned(),
            "--symbol".to_owned(),
            "target".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::Extract);
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
            "--artifact".to_owned(),
            "runtime.elf".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::AuditDirectTargets);
        assert!(invocation.svd_paths.is_empty());
    }
}
