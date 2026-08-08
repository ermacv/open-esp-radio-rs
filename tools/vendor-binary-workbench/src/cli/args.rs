//! Typed top-level command-line parsing.

use std::path::PathBuf;

use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, ValueEnum};

use super::arguments::*;
use crate::Result;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    #[default]
    Human,
    Json,
    Jsonl,
    Tsv,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct UiArgs {
    /// Increase diagnostic verbosity; repeat for debug and trace output.
    #[arg(short = 'v', long, global = true, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Suppress workbench warnings and diagnostic tracing.
    #[arg(long, global = true, conflicts_with = "verbose")]
    pub(crate) quiet: bool,

    /// Control ANSI colors in diagnostics and tracing.
    #[arg(long, global = true, value_enum, default_value_t)]
    pub(crate) color: ColorMode,

    /// Select the command-result representation written to stdout.
    #[arg(long, global = true, value_enum, default_value_t)]
    pub(crate) format: OutputFormat,
}

#[derive(Debug, Parser)]
#[command(
    name = "vendor-binary-workbench",
    version,
    about = "Analyze, reconstruct and verify Rust implementations against compiled vendor binaries",
    long_about = "Project-oriented analysis, reconstruction, publication and Rust conformance verification for compiled vendor binaries.\n\nA project composes a target spec, optional platform pack, local run bindings, a memory map and SVD catalogs. Without an explicit configuration root, the nearest vendor-project.toml is used."
)]
struct Cli {
    #[command(flatten)]
    ui: UiArgs,

    /// Project manifest. Conflicts with an explicit target specification.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        conflicts_with = "target_spec"
    )]
    project: Option<PathBuf>,

    /// Explicit target specification for backend and target-pack development.
    #[arg(long, global = true, value_name = "PATH")]
    target_spec: Option<PathBuf>,

    /// Local run bindings and command defaults.
    #[arg(long, global = true, value_name = "PATH")]
    run_spec: Option<PathBuf>,

    /// Additional SVD register catalog.
    #[arg(long, global = true, value_name = "PATH")]
    svd: Vec<PathBuf>,

    #[command(subcommand)]
    workflow: Workflow,
}

#[derive(Debug, Subcommand)]
enum Workflow {
    /// Generate host-shell and manual-page integration assets.
    Tooling {
        #[command(subcommand)]
        command: ToolingCommand,
    },
    /// Create, inspect and execute project-owned workflows.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Manage function contracts and review packs.
    Functions {
        #[command(subcommand)]
        command: FunctionCommand,
    },
    /// Discover and inspect binary symbols.
    Symbols {
        #[command(subcommand)]
        command: SymbolCommand,
    },
    /// Discover and validate vendor interfaces.
    Interfaces {
        #[command(subcommand)]
        command: InterfaceCommand,
    },
    /// Manage register models, SVDs and generated PACs.
    Registers {
        #[command(subcommand)]
        command: RegisterCommand,
    },
    /// Inspect individual artifacts and executions.
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },
    /// Discover MMIO behavior.
    Mmio {
        #[command(subcommand)]
        command: MmioCommand,
    },
    /// Export and build intermediate representations.
    Ir {
        #[command(subcommand)]
        command: IrCommand,
    },
    /// Generate executable references.
    Reference {
        #[command(subcommand)]
        command: ReferenceCommand,
    },
    /// Generate Rust driver candidates.
    Driver {
        #[command(subcommand)]
        command: DriverCommand,
    },
    /// Execute and compare functions.
    Execute {
        #[command(subcommand)]
        command: ExecuteCommand,
    },
    /// Verify source, profiles, evidence and contracts.
    Verify {
        #[command(subcommand)]
        command: VerifyCommand,
    },
    /// Audit linked images.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    #[value(name = "powershell")]
    PowerShell,
    Zsh,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct CompletionArgs {
    /// Shell whose completion script is generated.
    #[arg(value_enum)]
    pub(crate) shell: CompletionShell,
    /// Destination completion script.
    #[arg(long, value_name = "PATH")]
    pub(crate) output: PathBuf,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct ManpageArgs {
    /// Destination roff manual page.
    #[arg(long, value_name = "PATH")]
    pub(crate) output: PathBuf,
}

macro_rules! leaf_commands {
    ($name:ident { $($variant:ident($arguments:ty) => $command:path, $data:ident),+ $(,)? }) => {
        #[derive(Debug, Subcommand)]
        enum $name {
            $($variant($arguments)),+
        }

        impl $name {
            fn into_parts(self) -> (Command, CommandArguments) {
                match self {
                    $(Self::$variant(arguments) => ($command, CommandArguments::$data(arguments))),+
                }
            }
        }
    };
}

leaf_commands!(ToolingCommand {
    Completions(CompletionArgs) => Command::GenerateCompletions, Completion,
    Manpage(ManpageArgs) => Command::GenerateManpage, Manpage,
});

leaf_commands!(ProjectCommand {
    Init(ProjectInitArgs) => Command::ProjectInit, ProjectInit,
    Configure(ProjectConfigureArgs) => Command::ProjectConfigure, ProjectConfigure,
    Doctor(EmptyArgs) => Command::ProjectDoctor, Empty,
    Status(ProjectStatusArgs) => Command::ProjectStatus, ProjectStatus,
    Analyze(ProjectAnalyzeArgs) => Command::ProjectAnalyze, ProjectAnalyze,
    Publish(CheckArgs) => Command::ProjectPublish, Check,
});

leaf_commands!(FunctionCommand {
    InitPack(OutputArgs) => Command::FunctionInitPack, Output,
    Validate(ValidationArgs) => Command::FunctionValidate, Validation,
    Review(ReviewArgs) => Command::FunctionReview, Review,
});

leaf_commands!(SymbolCommand {
    Inventory(SymbolInventoryArgs) => Command::SymbolInventory, SymbolInventory,
});

leaf_commands!(InterfaceCommand {
    Discover(InterfaceDiscoverArgs) => Command::InterfaceDiscover, InterfaceDiscover,
    InitPack(OutputArgs) => Command::InterfaceInitPack, Output,
    Validate(ValidationArgs) => Command::InterfaceValidate, Validation,
});

leaf_commands!(RegisterCommand {
    InitModel(RegisterModelArgs) => Command::RegisterInitModel, RegisterModel,
    ImportSvd(RegisterImportArgs) => Command::RegisterImportSvd, RegisterImport,
    Validate(ValidationArgs) => Command::RegisterValidate, Validation,
    Review(RegisterReviewArgs) => Command::RegisterReview, RegisterReview,
    ExportSvd(RegisterExportArgs) => Command::RegisterExportSvd, RegisterExport,
    GeneratePac(RegisterPacArgs) => Command::RegisterGeneratePac, RegisterPac,
    GenerateBindings(RegisterBindingsArgs) => Command::RegisterGenerateBindings, RegisterBindings,
});

leaf_commands!(InspectCommand {
    Analyze(InspectAnalyzeArgs) => Command::InspectAnalyze, InspectAnalyze,
    Trace(TraceInputArgs) => Command::InspectTrace, TraceInput,
    Compare(InspectCompareArgs) => Command::InspectCompare, InspectCompare,
});

leaf_commands!(MmioCommand {
    Discover(MmioDiscoverArgs) => Command::DiscoverMmio, MmioDiscover,
});

leaf_commands!(IrCommand {
    Export(IrExportArgs) => Command::ExportIr, IrExport,
    Build(IrBuildArgs) => Command::BuildIr, IrBuild,
});

leaf_commands!(ReferenceCommand {
    Generate(ReferenceArgs) => Command::GenerateReference, Reference,
    GenerateBatch(ReferenceBatchArgs) => Command::GenerateReferenceBatch, ReferenceBatch,
});

leaf_commands!(DriverCommand {
    Generate(DriverGenerateArgs) => Command::GenerateDriver, DriverGenerate,
});

leaf_commands!(ExecuteCommand {
    Run(ExecuteRunArgs) => Command::ExecuteRun, ExecuteRun,
    Compare(ExecuteCompareArgs) => Command::ExecuteCompare, ExecuteCompare,
});

leaf_commands!(ImageCommand {
    AuditTargets(ImageAuditArgs) => Command::AuditImageTargets, ImageAudit,
});

#[derive(Debug, Subcommand)]
enum VerifyCommand {
    Profiles(VerifyProfilesArgs),
    Source(VerifySourceArgs),
    Inventory(VerifyInventoryArgs),
    Evidence(VerifyEvidenceArgs),
    Contract {
        #[command(subcommand)]
        command: VerifyContractCommand,
    },
}

impl VerifyCommand {
    fn into_parts(self) -> (Command, CommandArguments) {
        match self {
            Self::Profiles(arguments) => (
                Command::VerifyProfiles,
                CommandArguments::VerifyProfiles(arguments),
            ),
            Self::Source(arguments) => (
                Command::VerifySource,
                CommandArguments::VerifySource(arguments),
            ),
            Self::Inventory(arguments) => (
                Command::VerifyInventory,
                CommandArguments::VerifyInventory(arguments),
            ),
            Self::Evidence(arguments) => (
                Command::VerifyEvidence,
                CommandArguments::VerifyEvidence(arguments),
            ),
            Self::Contract { command } => command.into_parts(),
        }
    }
}

leaf_commands!(VerifyContractCommand {
    Channel(VerifyContractArgs) => Command::VerifyContractChannel, VerifyContract,
    RfInit(VerifyContractArgs) => Command::VerifyContractRfInit, VerifyContract,
});

impl Workflow {
    fn into_parts(self) -> (Command, CommandArguments) {
        match self {
            Self::Tooling { command } => command.into_parts(),
            Self::Project { command } => command.into_parts(),
            Self::Functions { command } => command.into_parts(),
            Self::Symbols { command } => command.into_parts(),
            Self::Interfaces { command } => command.into_parts(),
            Self::Registers { command } => command.into_parts(),
            Self::Inspect { command } => command.into_parts(),
            Self::Mmio { command } => command.into_parts(),
            Self::Ir { command } => command.into_parts(),
            Self::Reference { command } => command.into_parts(),
            Self::Driver { command } => command.into_parts(),
            Self::Execute { command } => command.into_parts(),
            Self::Verify { command } => command.into_parts(),
            Self::Image { command } => command.into_parts(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CommandArguments {
    Completion(CompletionArgs),
    Manpage(ManpageArgs),
    Empty(EmptyArgs),
    ProjectInit(ProjectInitArgs),
    ProjectConfigure(ProjectConfigureArgs),
    ProjectStatus(ProjectStatusArgs),
    ProjectAnalyze(ProjectAnalyzeArgs),
    Check(CheckArgs),
    Output(OutputArgs),
    Validation(ValidationArgs),
    Review(ReviewArgs),
    SymbolInventory(SymbolInventoryArgs),
    InterfaceDiscover(InterfaceDiscoverArgs),
    RegisterModel(RegisterModelArgs),
    RegisterImport(RegisterImportArgs),
    RegisterReview(RegisterReviewArgs),
    RegisterExport(RegisterExportArgs),
    RegisterPac(RegisterPacArgs),
    RegisterBindings(RegisterBindingsArgs),
    ImageAudit(ImageAuditArgs),
    MmioDiscover(MmioDiscoverArgs),
    IrExport(IrExportArgs),
    IrBuild(IrBuildArgs),
    TraceInput(TraceInputArgs),
    InspectCompare(InspectCompareArgs),
    InspectAnalyze(InspectAnalyzeArgs),
    Reference(ReferenceArgs),
    ReferenceBatch(ReferenceBatchArgs),
    DriverGenerate(DriverGenerateArgs),
    ExecuteRun(ExecuteRunArgs),
    ExecuteCompare(ExecuteCompareArgs),
    VerifyProfiles(VerifyProfilesArgs),
    VerifySource(VerifySourceArgs),
    VerifyInventory(VerifyInventoryArgs),
    VerifyEvidence(VerifyEvidenceArgs),
    VerifyContract(VerifyContractArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    GenerateCompletions,
    GenerateManpage,
    ProjectInit,
    ProjectConfigure,
    ProjectDoctor,
    ProjectStatus,
    ProjectAnalyze,
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
    pub(crate) const fn is_tooling(self) -> bool {
        matches!(self, Self::GenerateCompletions | Self::GenerateManpage)
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
        !self.is_tooling()
            && !matches!(
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
        !self.is_tooling()
            && !matches!(
                self,
                Self::ProjectInit
                    | Self::ProjectConfigure
                    | Self::ProjectDoctor
                    | Self::ProjectStatus
                    | Self::ProjectAnalyze
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
        !self.is_tooling()
            && !matches!(
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
        !self.is_tooling()
            && !matches!(
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
        !self.is_tooling()
            && !matches!(
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

    pub(crate) fn accepts_run_input_role(self, role: &crate::run_spec::InputRole) -> bool {
        use crate::run_spec::InputRole;

        match self {
            Self::GenerateCompletions
            | Self::GenerateManpage
            | Self::ProjectInit
            | Self::ProjectConfigure
            | Self::ProjectDoctor
            | Self::ProjectStatus
            | Self::ProjectAnalyze
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
            Self::DiscoverMmio => matches!(role, InputRole::SourceArtifact(_)),
            Self::ExportIr => matches!(role, InputRole::Companion | InputRole::SourceArtifact(_)),
            Self::AuditImageTargets => role == &InputRole::Artifact,
            _ => true,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedInvocation {
    pub(crate) ui: UiArgs,
    pub(crate) command: Command,
    pub(crate) project: Option<PathBuf>,
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) arguments: CommandArguments,
}

impl ParsedInvocation {
    pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let cli = Cli::try_parse_from(
            std::iter::once("vendor-binary-workbench".to_owned()).chain(arguments),
        )?;
        let (command, arguments) = cli.workflow.into_parts();
        Ok(Self {
            ui: cli.ui,
            command,
            project: cli.project,
            target_spec: cli.target_spec,
            run_spec: cli.run_spec,
            svd_paths: cli.svd,
            arguments,
        })
    }
}

pub(super) fn command_definition() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typed_leaf_arguments_and_globals_in_any_position() {
        let invocation = ParsedInvocation::parse([
            "ir".to_owned(),
            "export".to_owned(),
            "--artifact".to_owned(),
            "rom=rom.elf".to_owned(),
            "--target-spec".to_owned(),
            "target.spec".to_owned(),
            "--include-reachable".to_owned(),
            "--svd".to_owned(),
            "radio.svd".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ExportIr);
        assert_eq!(invocation.target_spec, Some(PathBuf::from("target.spec")));
        assert_eq!(invocation.svd_paths, [PathBuf::from("radio.svd")]);
        let CommandArguments::IrExport(arguments) = invocation.arguments else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.artifact[0].source.as_str(), "rom");
        assert_eq!(arguments.artifact[0].path, PathBuf::from("rom.elf"));
        assert!(arguments.include_reachable);
    }

    #[test]
    fn rejects_unknown_leaf_options() {
        let error = ParsedInvocation::parse([
            "project".to_owned(),
            "status".to_owned(),
            "--unknown".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("unexpected argument"));
    }

    #[test]
    fn enforces_declarative_conflicts_and_requirements() {
        assert!(
            ParsedInvocation::parse([
                "project".to_owned(),
                "status".to_owned(),
                "--check".to_owned(),
            ])
            .is_err()
        );
        assert!(
            ParsedInvocation::parse([
                "project".to_owned(),
                "configure".to_owned(),
                "--platform-pack".to_owned(),
                "platform.toml".to_owned(),
                "--no-platform-pack".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn exposes_nested_help_from_the_same_grammar() {
        let error = ParsedInvocation::parse([
            "registers".to_owned(),
            "generate-pac".to_owned(),
            "--help".to_owned(),
        ])
        .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("--api-pack"));
        assert!(help.contains("--deny-unreviewed"));
    }

    #[test]
    fn project_analysis_has_one_write_or_check_interface() {
        let invocation = ParsedInvocation::parse([
            "project".to_owned(),
            "analyze".to_owned(),
            "--check".to_owned(),
            "--deny-unreviewed".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.command, Command::ProjectAnalyze);
        let CommandArguments::ProjectAnalyze(arguments) = invocation.arguments else {
            panic!("unexpected argument type")
        };
        assert!(arguments.check);
        assert!(arguments.deny_unreviewed);

        for removed in ["build", "check"] {
            let error =
                ParsedInvocation::parse(["project".to_owned(), removed.to_owned()]).unwrap_err();
            assert!(error.to_string().contains("unrecognized subcommand"));
        }
    }
}
