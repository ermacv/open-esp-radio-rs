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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum ProgressMode {
    #[default]
    Auto,
    Always,
    Never,
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

    /// Control progress rendering on stderr.
    #[arg(long, global = true, value_enum, default_value_t)]
    pub(crate) progress: ProgressMode,
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
    /// Review recovered executable-code boundaries before analysis.
    Code {
        #[command(subcommand)]
        command: CodeCommand,
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
            fn into_command(self) -> Command {
                match self {
                    $(Self::$variant(arguments) => $command(arguments)),+
                }
            }
        }
    };
}

leaf_commands!(ToolingCommand {
    Completions(CompletionArgs) => Command::GenerateCompletions, Completion,
    Manpage(ManpageArgs) => Command::GenerateManpage, Manpage,
});

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Create a new project workspace and neutral target specification.
    #[command(
        after_long_help = "Next: run `vendor-binary-workbench project doctor --project PATH/vendor-project.toml`.\nThen add caller-owned binaries with `project inputs init`."
    )]
    Init(ProjectInitArgs),
    /// Attach or remove a reusable platform pack.
    #[command(
        after_long_help = "Next: run `vendor-binary-workbench project doctor --project PATH` to validate the resolved configuration."
    )]
    Configure(ProjectConfigureArgs),
    /// Manage caller-owned artifact bindings.
    Inputs {
        #[command(subcommand)]
        command: ProjectInputsCommand,
    },
    /// Validate configuration and report detailed diagnostics.
    #[command(
        after_long_help = "This checks validity, not workflow readiness. Use `project status` for readiness and `project analyze` to refresh evidence."
    )]
    Doctor(EmptyArgs),
    /// Summarize project workflow readiness without modifying artifacts.
    #[command(
        after_long_help = "Use `project doctor` for detailed configuration diagnostics, or `project analyze` to refresh generated evidence."
    )]
    Status(ProjectStatusArgs),
    /// Browse the resolved project in a read-only terminal interface.
    Browse(EmptyArgs),
    /// Generate or verify reproducible binary-analysis evidence.
    #[command(
        after_long_help = "Use `--check` in CI to compare generated evidence without writing it. Follow with `project status` to inspect readiness."
    )]
    Analyze(ProjectAnalyzeArgs),
    /// Execute every configured Rust/vendor verification suite.
    #[command(
        after_long_help = "Verification uses project suites and caller-owned run bindings. Use `--check` in CI to reproduce the aggregate report without writing it."
    )]
    Verify(ProjectVerifyArgs),
    /// Reproduce analysis, verification and publication outputs without writing them.
    #[command(
        after_long_help = "This is the authoritative CI entry point. It checks analysis evidence, every verification suite and all publication outputs."
    )]
    Check(ProjectCheckArgs),
    /// Generate or verify reviewed SVD, PAC and binding outputs.
    #[command(
        after_long_help = "Use `--check` in CI to validate publication outputs without writing them."
    )]
    Publish(CheckArgs),
}

impl ProjectCommand {
    fn into_command(self) -> Command {
        match self {
            Self::Init(arguments) => Command::ProjectInit(arguments),
            Self::Configure(arguments) => Command::ProjectConfigure(arguments),
            Self::Inputs { command } => command.into_command(),
            Self::Doctor(arguments) => Command::ProjectDoctor(arguments),
            Self::Status(arguments) => Command::ProjectStatus(arguments),
            Self::Browse(arguments) => Command::ProjectBrowse(arguments),
            Self::Analyze(arguments) => Command::ProjectAnalyze(arguments),
            Self::Verify(arguments) => Command::ProjectVerify(arguments),
            Self::Check(arguments) => Command::ProjectCheck(arguments),
            Self::Publish(arguments) => Command::ProjectPublish(arguments),
        }
    }
}

leaf_commands!(ProjectInputsCommand {
    Init(ProjectInputsInitArgs) => Command::ProjectInputsInit, ProjectInputsInit,
});

leaf_commands!(FunctionCommand {
    InitPack(OutputArgs) => Command::FunctionInitPack, Output,
    Validate(ValidationArgs) => Command::FunctionValidate, Validation,
    Review(ReviewArgs) => Command::FunctionReview, Review,
});

leaf_commands!(CodeCommand {
    InitPack(OutputArgs) => Command::CodeInitPack, Output,
    Validate(ValidationArgs) => Command::CodeValidate, Validation,
    Review(ReviewArgs) => Command::CodeReview, Review,
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
    fn into_command(self) -> Command {
        match self {
            Self::Profiles(arguments) => Command::VerifyProfiles(arguments),
            Self::Source(arguments) => Command::VerifySource(arguments),
            Self::Inventory(arguments) => Command::VerifyInventory(arguments),
            Self::Evidence(arguments) => Command::VerifyEvidence(arguments),
            Self::Contract { command } => command.into_command(),
        }
    }
}

leaf_commands!(VerifyContractCommand {
    Channel(VerifyContractArgs) => Command::VerifyContractChannel, VerifyContract,
    RfInit(VerifyContractArgs) => Command::VerifyContractRfInit, VerifyContract,
    BluetoothTxPower(VerifyContractArgs) => Command::VerifyContractBluetoothTxPower, VerifyContract,
    BluetoothTxGainInit(VerifyContractArgs) => Command::VerifyContractBluetoothTxGainInit, VerifyContract,
    BasebandInit(VerifyContractArgs) => Command::VerifyContractBasebandInit, VerifyContract,
    RegisterInit(VerifyContractArgs) => Command::VerifyContractRegisterInit, VerifyContract,
});

impl Workflow {
    fn into_command(self) -> Command {
        match self {
            Self::Tooling { command } => command.into_command(),
            Self::Project { command } => command.into_command(),
            Self::Functions { command } => command.into_command(),
            Self::Code { command } => command.into_command(),
            Self::Symbols { command } => command.into_command(),
            Self::Interfaces { command } => command.into_command(),
            Self::Registers { command } => command.into_command(),
            Self::Inspect { command } => command.into_command(),
            Self::Mmio { command } => command.into_command(),
            Self::Ir { command } => command.into_command(),
            Self::Reference { command } => command.into_command(),
            Self::Driver { command } => command.into_command(),
            Self::Execute { command } => command.into_command(),
            Self::Verify { command } => command.into_command(),
            Self::Image { command } => command.into_command(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Command {
    GenerateCompletions(CompletionArgs),
    GenerateManpage(ManpageArgs),
    ProjectInit(ProjectInitArgs),
    ProjectConfigure(ProjectConfigureArgs),
    ProjectInputsInit(ProjectInputsInitArgs),
    ProjectDoctor(EmptyArgs),
    ProjectStatus(ProjectStatusArgs),
    ProjectBrowse(EmptyArgs),
    ProjectAnalyze(ProjectAnalyzeArgs),
    ProjectVerify(ProjectVerifyArgs),
    ProjectCheck(ProjectCheckArgs),
    ProjectPublish(CheckArgs),
    FunctionInitPack(OutputArgs),
    FunctionValidate(ValidationArgs),
    FunctionReview(ReviewArgs),
    CodeInitPack(OutputArgs),
    CodeValidate(ValidationArgs),
    CodeReview(ReviewArgs),
    SymbolInventory(SymbolInventoryArgs),
    InterfaceDiscover(InterfaceDiscoverArgs),
    InterfaceInitPack(OutputArgs),
    InterfaceValidate(ValidationArgs),
    RegisterInitModel(RegisterModelArgs),
    RegisterImportSvd(RegisterImportArgs),
    RegisterValidate(ValidationArgs),
    RegisterReview(RegisterReviewArgs),
    RegisterExportSvd(RegisterExportArgs),
    RegisterGeneratePac(RegisterPacArgs),
    RegisterGenerateBindings(RegisterBindingsArgs),
    AuditImageTargets(ImageAuditArgs),
    DiscoverMmio(MmioDiscoverArgs),
    ExportIr(IrExportArgs),
    BuildIr(IrBuildArgs),
    VerifyContractChannel(VerifyContractArgs),
    VerifyContractRfInit(VerifyContractArgs),
    VerifyContractBluetoothTxPower(VerifyContractArgs),
    VerifyContractBluetoothTxGainInit(VerifyContractArgs),
    VerifyContractBasebandInit(VerifyContractArgs),
    VerifyContractRegisterInit(VerifyContractArgs),
    ExecuteRun(ExecuteRunArgs),
    ExecuteCompare(ExecuteCompareArgs),
    VerifyProfiles(VerifyProfilesArgs),
    VerifyEvidence(VerifyEvidenceArgs),
    GenerateReference(ReferenceArgs),
    GenerateReferenceBatch(ReferenceBatchArgs),
    GenerateDriver(DriverGenerateArgs),
    InspectAnalyze(InspectAnalyzeArgs),
    VerifyInventory(VerifyInventoryArgs),
    VerifySource(VerifySourceArgs),
    InspectTrace(TraceInputArgs),
    InspectCompare(InspectCompareArgs),
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedInvocation {
    pub(crate) ui: UiArgs,
    pub(crate) command: Command,
    pub(crate) project: Option<PathBuf>,
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
}

impl ParsedInvocation {
    pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self> {
        let cli = Cli::try_parse_from(
            std::iter::once("vendor-binary-workbench".to_owned()).chain(arguments),
        )?;
        let command = cli.workflow.into_command();
        if matches!(command, Command::ProjectBrowse(_)) && cli.ui.format != OutputFormat::Human {
            return Err(crate::Error::invalid(
                "project browse is an interactive human frontend and does not accept a machine output format",
            ));
        }
        Ok(Self {
            ui: cli.ui,
            command,
            project: cli.project,
            target_spec: cli.target_spec,
            run_spec: cli.run_spec,
            svd_paths: cli.svd,
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
            "target.toml".to_owned(),
            "--include-reachable".to_owned(),
            "--svd".to_owned(),
            "radio.svd".to_owned(),
            "--progress".to_owned(),
            "never".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.target_spec, Some(PathBuf::from("target.toml")));
        assert_eq!(invocation.svd_paths, [PathBuf::from("radio.svd")]);
        assert_eq!(invocation.ui.progress, ProgressMode::Never);
        let Command::ExportIr(arguments) = invocation.command else {
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
    fn project_analysis_and_ci_check_have_typed_interfaces() {
        let invocation = ParsedInvocation::parse([
            "project".to_owned(),
            "analyze".to_owned(),
            "--check".to_owned(),
            "--deny-unreviewed".to_owned(),
            "--jobs".to_owned(),
            "2".to_owned(),
        ])
        .unwrap();
        let Command::ProjectAnalyze(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert!(arguments.check);
        assert!(arguments.deny_unreviewed);
        assert_eq!(arguments.jobs, 2);

        let invocation =
            ParsedInvocation::parse(["project".to_owned(), "check".to_owned()]).unwrap();
        let Command::ProjectCheck(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert!(!arguments.deny_unreviewed);
        assert_eq!(arguments.jobs, 0);

        let error =
            ParsedInvocation::parse(["project".to_owned(), "build".to_owned()]).unwrap_err();
        assert!(error.to_string().contains("unrecognized subcommand"));
    }

    #[test]
    fn linked_ir_commands_accept_explicit_function_workers() {
        let invocation = ParsedInvocation::parse([
            "ir".to_owned(),
            "build".to_owned(),
            "--jobs".to_owned(),
            "3".to_owned(),
        ])
        .unwrap();
        let Command::BuildIr(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.jobs, 3);

        let invocation = ParsedInvocation::parse([
            "ir".to_owned(),
            "export".to_owned(),
            "--artifact".to_owned(),
            "rom=rom.elf".to_owned(),
            "--jobs".to_owned(),
            "2".to_owned(),
        ])
        .unwrap();
        let Command::ExportIr(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.jobs, 2);
    }

    #[test]
    fn project_inputs_exposes_typed_non_overwriting_setup() {
        let invocation = ParsedInvocation::parse([
            "project".to_owned(),
            "inputs".to_owned(),
            "init".to_owned(),
            "--bind".to_owned(),
            "source-artifact:rom=/tmp/rom.elf".to_owned(),
            "--check".to_owned(),
        ])
        .unwrap();
        let Command::ProjectInputsInit(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert!(arguments.check);
        assert!(!arguments.force);
        assert_eq!(arguments.bind[0].role.to_string(), "source-artifact:rom");

        assert!(
            ParsedInvocation::parse([
                "project".to_owned(),
                "inputs".to_owned(),
                "init".to_owned(),
                "--bind".to_owned(),
                "artifact=/tmp/vendor.elf".to_owned(),
                "--check".to_owned(),
                "--force".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn project_browser_is_a_human_only_frontend() {
        let invocation = ParsedInvocation::parse([
            "project".to_owned(),
            "browse".to_owned(),
            "--project".to_owned(),
            "vendor-project.toml".to_owned(),
        ])
        .unwrap();
        assert!(matches!(invocation.command, Command::ProjectBrowse(_)));

        let error = ParsedInvocation::parse([
            "project".to_owned(),
            "browse".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("interactive human frontend"));
    }

    #[test]
    fn mmio_discovery_defaults_to_all_code_symbols_and_can_be_narrowed() {
        let invocation = ParsedInvocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--artifact".to_owned(),
            "vendor=/tmp/vendor.a".to_owned(),
            "--range".to_owned(),
            "radio=0x60000000..0x60001000".to_owned(),
        ])
        .unwrap();
        let Command::DiscoverMmio(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.code_symbols, CodeSymbolSelectionArg::All);

        let invocation = ParsedInvocation::parse([
            "mmio".to_owned(),
            "discover".to_owned(),
            "--code-symbols".to_owned(),
            "exported".to_owned(),
        ])
        .unwrap();
        let Command::DiscoverMmio(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.code_symbols, CodeSymbolSelectionArg::Exported);
    }
}
