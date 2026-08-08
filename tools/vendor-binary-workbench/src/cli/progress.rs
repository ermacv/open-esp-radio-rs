//! Progress policy and span construction for long-running CLI workflows.

use tracing::Span;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use super::args::{Command, OutputFormat, ProgressMode, UiArgs};

pub(super) fn command_span(command: Command) -> Option<Span> {
    let message = match command {
        Command::GenerateCompletions
        | Command::GenerateManpage
        | Command::ProjectInit
        | Command::ProjectConfigure
        | Command::ProjectDoctor
        | Command::ProjectStatus
        | Command::FunctionInitPack
        | Command::InterfaceInitPack
        | Command::RegisterInitModel
        | Command::VerifyEvidence => return None,
        Command::ProjectAnalyze => "Project analysis",
        Command::ProjectPublish => "Project publication",
        Command::FunctionValidate => "Function validation",
        Command::FunctionReview => "Function review",
        Command::RegisterImportSvd => "SVD import",
        Command::RegisterValidate => "Register validation",
        Command::RegisterReview => "Register review",
        Command::RegisterExportSvd => "SVD export",
        Command::RegisterGeneratePac => "PAC generation",
        Command::RegisterGenerateBindings => "PAC binding generation",
        Command::SymbolInventory => "Symbol inventory",
        Command::InterfaceDiscover => "Interface discovery",
        Command::InterfaceValidate => "Interface validation",
        Command::AuditImageTargets => "Linked-image audit",
        Command::DiscoverMmio => "MMIO discovery",
        Command::ExportIr => "IR export",
        Command::BuildIr => "Linked IR build",
        Command::VerifyContractChannel => "Channel contract verification",
        Command::VerifyContractRfInit => "RF-init contract verification",
        Command::ExecuteRun => "Vendor function execution",
        Command::ExecuteCompare => "Function comparison",
        Command::VerifyProfiles => "Profile verification",
        Command::GenerateReference => "Reference generation",
        Command::GenerateReferenceBatch => "Batch reference generation",
        Command::GenerateDriver => "Driver generation",
        Command::InspectAnalyze => "Artifact analysis",
        Command::VerifyInventory => "Inventory verification",
        Command::VerifySource => "Source verification",
        Command::InspectTrace => "Trace extraction",
        Command::InspectCompare => "Trace comparison",
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

pub(super) fn stage_span(name: &str) -> Span {
    let span = tracing::info_span!(
        "project_stage",
        indicatif.pb_show = tracing::field::Empty,
        stage = name
    );
    span.pb_set_message(&format!("Project stage: {name}"));
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
        assert!(command_span(Command::DiscoverMmio).is_some());
        assert!(command_span(Command::ProjectAnalyze).is_some());
        assert!(command_span(Command::ProjectStatus).is_none());
        assert!(command_span(Command::GenerateCompletions).is_none());
    }
}
