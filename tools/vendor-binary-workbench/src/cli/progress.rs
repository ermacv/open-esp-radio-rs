//! Progress policy and span construction for long-running CLI workflows.

use tracing::Span;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use super::args::{Command, OutputFormat, ProgressMode, UiArgs};

pub(super) fn command_span(command: &Command) -> Option<Span> {
    let message = match command {
        Command::GenerateCompletions(_)
        | Command::GenerateManpage(_)
        | Command::ProjectInit(_)
        | Command::ProjectConfigure(_)
        | Command::ProjectInputsInit(_)
        | Command::ProjectDoctor(_)
        | Command::ProjectStatus(_)
        | Command::ProjectBrowse(_)
        | Command::FunctionInitPack(_)
        | Command::CodeInitPack(_)
        | Command::InterfaceInitPack(_)
        | Command::RegisterInitModel(_)
        | Command::VerifyEvidence(_) => return None,
        Command::ProjectAnalyze(_) => "Project analysis",
        Command::ProjectVerify(_) => "Project verification",
        Command::ProjectCheck(_) => "Project check",
        Command::ProjectPublish(_) => "Project publication",
        Command::FunctionValidate(_) => "Function validation",
        Command::FunctionReview(_) => "Function review",
        Command::CodeValidate(_) => "Code-boundary validation",
        Command::CodeReview(_) => "Code-boundary review",
        Command::RegisterImportSvd(_) => "SVD import",
        Command::RegisterValidate(_) => "Register validation",
        Command::RegisterReview(_) => "Register review",
        Command::RegisterExportSvd(_) => "SVD export",
        Command::RegisterGeneratePac(_) => "PAC generation",
        Command::RegisterGenerateBindings(_) => "PAC binding generation",
        Command::SymbolInventory(_) => "Symbol inventory",
        Command::InterfaceDiscover(_) => "Interface discovery",
        Command::InterfaceValidate(_) => "Interface validation",
        Command::AuditImageTargets(_) => "Linked-image audit",
        Command::DiscoverMmio(_) => "MMIO discovery",
        Command::ExportIr(_) => "IR export",
        Command::BuildIr(_) => "Linked IR build",
        Command::VerifyContractChannel(_) => "Channel contract verification",
        Command::VerifyContractRfInit(_) => "RF-init contract verification",
        Command::VerifyContractBluetoothTxPower(_) => "Bluetooth TX-power contract verification",
        Command::VerifyContractBluetoothTxGainInit(_) => {
            "Bluetooth TX-gain parent contract verification"
        }
        Command::ExecuteRun(_) => "Vendor function execution",
        Command::ExecuteCompare(_) => "Function comparison",
        Command::VerifyProfiles(_) => "Profile verification",
        Command::GenerateReference(_) => "Reference generation",
        Command::GenerateReferenceBatch(_) => "Batch reference generation",
        Command::GenerateDriver(_) => "Driver generation",
        Command::InspectAnalyze(_) => "Artifact analysis",
        Command::VerifyInventory(_) => "Inventory verification",
        Command::VerifySource(_) => "Source verification",
        Command::InspectTrace(_) => "Trace extraction",
        Command::InspectCompare(_) => "Trace comparison",
    };
    Some(operation_span(message))
}

fn operation_span(message: &str) -> Span {
    let span = tracing::info_span!(
        "workbench_operation",
        indicatif.pb_show = tracing::field::Empty,
        operation = message
    );
    span.pb_set_message(message);
    span
}

pub(super) fn enabled_for(arguments: &UiArgs, stderr_is_terminal: bool) -> bool {
    if arguments.quiet {
        return false;
    }
    match arguments.progress {
        ProgressMode::Auto => stderr_is_terminal && matches!(arguments.format, OutputFormat::Human),
        ProgressMode::Always => true,
        ProgressMode::Never => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{CompletionArgs, CompletionShell};

    #[test]
    fn automatic_progress_requires_a_human_terminal() {
        let human = UiArgs::default();
        assert!(enabled_for(&human, true));
        assert!(!enabled_for(&human, false));

        let machine = UiArgs {
            format: OutputFormat::Json,
            ..UiArgs::default()
        };
        assert!(!enabled_for(&machine, true));
    }

    #[test]
    fn explicit_progress_modes_override_terminal_detection() {
        let always = UiArgs {
            format: OutputFormat::Json,
            progress: ProgressMode::Always,
            ..UiArgs::default()
        };
        assert!(enabled_for(&always, false));

        let never = UiArgs {
            progress: ProgressMode::Never,
            ..UiArgs::default()
        };
        assert!(!enabled_for(&never, true));
    }

    #[test]
    fn quiet_suppresses_even_forced_progress() {
        let quiet = UiArgs {
            quiet: true,
            progress: ProgressMode::Always,
            ..UiArgs::default()
        };
        assert!(!enabled_for(&quiet, true));
    }

    #[test]
    fn long_commands_get_root_spans_but_inspection_commands_do_not() {
        assert!(command_span(&Command::DiscoverMmio(Default::default())).is_some());
        assert!(command_span(&Command::ProjectAnalyze(Default::default())).is_some());
        assert!(command_span(&Command::ProjectVerify(Default::default())).is_some());
        assert!(command_span(&Command::ProjectStatus(Default::default())).is_none());
        assert!(
            command_span(&Command::GenerateCompletions(CompletionArgs {
                shell: CompletionShell::Bash,
                output: "completion".into(),
            }))
            .is_none()
        );
    }
}
