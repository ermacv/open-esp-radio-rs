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
    #[arg(
        short = 'v',
        long,
        global = true,
        action = ArgAction::Count,
        help_heading = "Output and diagnostics"
    )]
    pub(crate) verbose: u8,

    /// Suppress workbench warnings and diagnostic tracing.
    #[arg(
        long,
        global = true,
        conflicts_with = "verbose",
        help_heading = "Output and diagnostics"
    )]
    pub(crate) quiet: bool,

    /// Control ANSI colors in human output, diagnostics and tracing.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t,
        help_heading = "Output and diagnostics"
    )]
    pub(crate) color: ColorMode,

    /// Select the command-result representation written to stdout.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t,
        help_heading = "Output and diagnostics"
    )]
    pub(crate) format: OutputFormat,

    /// Include expanded evidence and component details in human output.
    #[arg(long, global = true, help_heading = "Output and diagnostics")]
    pub(crate) details: bool,

    /// Control progress rendering on stderr.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t,
        help_heading = "Output and diagnostics"
    )]
    pub(crate) progress: ProgressMode,
}

#[derive(Debug, Parser)]
#[command(
    name = "vendor-binary-workbench",
    version,
    arg_required_else_help = true,
    about = "Analyze, reconstruct and verify Rust implementations against compiled vendor binaries",
    long_about = "Project-oriented analysis, reconstruction, publication and Rust conformance verification for compiled vendor binaries.\n\nA project composes a target spec, optional platform pack, local run bindings, a memory map and SVD catalogs. Without an explicit configuration root, the nearest vendor-project.toml is used.",
    after_help = "START HERE:\n  vendor-binary-workbench project status --project PATH/vendor-project.toml\n\nNEW PROJECT:\n  vendor-binary-workbench project init --help\n\nUse `project` for the normal workflow. Low-level analysis engines are under `advanced`."
)]
struct Cli {
    #[command(flatten)]
    ui: UiArgs,

    /// Project manifest. Conflicts with an explicit target specification.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        conflicts_with = "target_spec",
        help_heading = "Project selection"
    )]
    project: Option<PathBuf>,

    /// Explicit target specification for backend and target-pack development.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Project selection"
    )]
    target_spec: Option<PathBuf>,

    /// Local run bindings and command defaults.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Project selection"
    )]
    run_spec: Option<PathBuf>,

    /// Additional SVD register catalog.
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Project selection"
    )]
    svd: Vec<PathBuf>,

    #[command(subcommand)]
    workflow: Workflow,
}

#[derive(Debug, Subcommand)]
enum Workflow {
    /// Create, inspect and execute project-owned workflows.
    #[command(
        after_long_help = "EXISTING PROJECT:\n  project status  → current readiness and exact next actions\n  project files   → ownership and purpose of every configured file\n  project browse  → read-only TUI over generated evidence\n\nNEW PROJECT:\n  project init → project inputs init → project doctor → project analyze\n  registers review/validate → project publish → project verify/check"
    )]
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
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
    /// Run focused low-level analysis, execution and verification engines.
    Advanced {
        #[command(subcommand)]
        command: AdvancedCommand,
    },
    /// Generate host-shell and manual-page integration assets.
    Tooling {
        #[command(subcommand)]
        command: ToolingCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AdvancedCommand {
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
    ($name:ident { $($(#[$metadata:meta])* $variant:ident($arguments:ty) => $command:path, $data:ident),+ $(,)? }) => {
        #[derive(Debug, Subcommand)]
        enum $name {
            $($(#[$metadata])* $variant($arguments)),+
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
    /// Generate a completion script from the current CLI grammar.
    Completions(CompletionArgs) => Command::GenerateCompletions, Completion,
    /// Generate the complete roff manual from the current CLI grammar.
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
    /// List every project file with its role, owner, producer and status.
    #[command(
        after_long_help = "Use this before editing a project to distinguish local bindings, reviewed knowledge, generated evidence and external artifacts."
    )]
    Files(EmptyArgs),
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
            Self::Files(arguments) => Command::ProjectFiles(arguments),
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
    /// Create or verify caller-owned local artifact bindings.
    Init(ProjectInputsInitArgs) => Command::ProjectInputsInit, ProjectInputsInit,
});

leaf_commands!(FunctionCommand {
    /// Create a reviewable function-contract pack from project evidence.
    InitPack(OutputArgs) => Command::FunctionInitPack, Output,
    /// Validate the configured reviewed function-contract pack.
    Validate(ValidationArgs) => Command::FunctionValidate, Validation,
    /// Generate or check a human-review function workspace.
    Review(ReviewArgs) => Command::FunctionReview, Review,
});

leaf_commands!(CodeCommand {
    /// Create a reviewable executable-code boundary pack.
    InitPack(OutputArgs) => Command::CodeInitPack, Output,
    /// Rebase reviewed code boundaries onto current binary facts.
    Rebase(CodeRebaseArgs) => Command::CodeRebase, CodeRebase,
    /// Validate reviewed executable-code boundaries.
    Validate(ValidationArgs) => Command::CodeValidate, Validation,
    /// Generate or check a code-boundary review workspace.
    Review(ReviewArgs) => Command::CodeReview, Review,
});

leaf_commands!(SymbolCommand {
    /// Inventory definitions, references and cross-input symbol candidates.
    Inventory(SymbolInventoryArgs) => Command::SymbolInventory, SymbolInventory,
});

leaf_commands!(InterfaceCommand {
    /// Discover indirect calls and function-pointer table candidates.
    Discover(InterfaceDiscoverArgs) => Command::InterfaceDiscover, InterfaceDiscover,
    /// Create a reviewed interface-layout pack from discovery facts.
    InitPack(OutputArgs) => Command::InterfaceInitPack, Output,
    /// Validate reviewed interface layouts and semantic bindings.
    Validate(ValidationArgs) => Command::InterfaceValidate, Validation,
});

leaf_commands!(RegisterCommand {
    /// Create an empty reviewed register model for a memory region.
    InitModel(RegisterModelArgs) => Command::RegisterInitModel, RegisterModel,
    /// Import an existing SVD into the reviewed register model.
    ImportSvd(RegisterImportArgs) => Command::RegisterImportSvd, RegisterImport,
    /// Validate register names, fields, evidence and publication policy.
    Validate(ValidationArgs) => Command::RegisterValidate, Validation,
    /// Generate or check the editable register-review workspace.
    Review(RegisterReviewArgs) => Command::RegisterReview, RegisterReview,
    /// Export a clean publication SVD from reviewed register data.
    ExportSvd(RegisterExportArgs) => Command::RegisterExportSvd, RegisterExport,
    /// Generate the internal unsafe raw PAC implementation.
    GeneratePacRaw(RegisterPacRawArgs) => Command::RegisterGeneratePacRaw, RegisterPacRaw,
    /// Generate the restricted public register binding API.
    GenerateBindings(RegisterBindingsArgs) => Command::RegisterGenerateBindings, RegisterBindings,
});

leaf_commands!(InspectCommand {
    /// Investigate one function with lossless code, CFG and semantic evidence.
    Function(InspectFunctionArgs) => Command::InspectFunction, InspectFunction,
    /// Trace argument and constant flow across a focused call-graph slice.
    Flow(InspectFlowArgs) => Command::InspectFlow, InspectFlow,
    /// Inspect accesses and ownership evidence for one memory object.
    Object(InspectObjectArgs) => Command::InspectObject, InspectObject,
    /// Summarize a reviewed analysis scope and its blockers.
    Scope(InspectScopeArgs) => Command::InspectScope, InspectScope,
    /// Analyze one artifact without running the complete project pipeline.
    Analyze(InspectAnalyzeArgs) => Command::InspectAnalyze, InspectAnalyze,
    /// Extract the observable trace for one function execution.
    Trace(TraceInputArgs) => Command::InspectTrace, TraceInput,
    /// Compare two focused function traces.
    Compare(InspectCompareArgs) => Command::InspectCompare, InspectCompare,
});

leaf_commands!(MmioCommand {
    /// Find MMIO accesses and infer register and field candidates.
    Discover(MmioDiscoverArgs) => Command::DiscoverMmio, MmioDiscover,
});

leaf_commands!(IrCommand {
    /// Export linked semantic IR directly from explicit artifacts.
    Export(IrExportArgs) => Command::ExportIr, IrExport,
    /// Build configured project IR profiles.
    Build(IrBuildArgs) => Command::BuildIr, IrBuild,
});

leaf_commands!(ReferenceCommand {
    /// Generate a Rust-side executable reference for one profile.
    Generate(ReferenceArgs) => Command::GenerateReference, Reference,
    /// Generate executable references for a configured profile set.
    GenerateBatch(ReferenceBatchArgs) => Command::GenerateReferenceBatch, ReferenceBatch,
});

leaf_commands!(DriverCommand {
    /// Generate a review candidate for a Rust driver function.
    Generate(DriverGenerateArgs) => Command::GenerateDriver, DriverGenerate,
});

leaf_commands!(ExecuteCommand {
    /// Execute a vendor function under a concrete scenario.
    Run(ExecuteRunArgs) => Command::ExecuteRun, ExecuteRun,
    /// Compare vendor and Rust observable effects under one scenario.
    Compare(ExecuteCompareArgs) => Command::ExecuteCompare, ExecuteCompare,
});

leaf_commands!(ImageCommand {
    /// Audit resolved call targets in a linked image.
    AuditTargets(ImageAuditArgs) => Command::AuditImageTargets, ImageAudit,
});

#[derive(Debug, Subcommand)]
enum VerifyCommand {
    /// Validate executable profile definitions and coverage gates.
    Profiles(VerifyProfilesArgs),
    /// Compare recovered vendor functions with Rust source candidates.
    Source(VerifySourceArgs),
    /// Build the cross-source verification inventory.
    Inventory(VerifyInventoryArgs),
    /// Review or update a verification evidence baseline.
    Evidence(VerifyEvidenceArgs),
    /// Run a focused built-in behavioral contract.
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
    /// Verify the Wi-Fi channel-selection contract.
    Channel(VerifyContractArgs) => Command::VerifyContractChannel, VerifyContract,
    /// Verify the RF initialization contract.
    RfInit(VerifyContractArgs) => Command::VerifyContractRfInit, VerifyContract,
    /// Verify Bluetooth transmit-power behavior.
    BluetoothTxPower(VerifyContractArgs) => Command::VerifyContractBluetoothTxPower, VerifyContract,
    /// Verify Bluetooth transmit-gain initialization behavior.
    BluetoothTxGainInit(VerifyContractArgs) => Command::VerifyContractBluetoothTxGainInit, VerifyContract,
    /// Verify the baseband initialization parent contract.
    BasebandInit(VerifyContractArgs) => Command::VerifyContractBasebandInit, VerifyContract,
    /// Verify the PHY registration initialization contract.
    RegisterInit(VerifyContractArgs) => Command::VerifyContractRegisterInit, VerifyContract,
});

impl Workflow {
    fn into_command(self) -> Command {
        match self {
            Self::Project { command } => command.into_command(),
            Self::Registers { command } => command.into_command(),
            Self::Inspect { command } => command.into_command(),
            Self::Advanced { command } => command.into_command(),
            Self::Tooling { command } => command.into_command(),
        }
    }
}

impl AdvancedCommand {
    fn into_command(self) -> Command {
        match self {
            Self::Functions { command } => command.into_command(),
            Self::Code { command } => command.into_command(),
            Self::Symbols { command } => command.into_command(),
            Self::Interfaces { command } => command.into_command(),
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
    ProjectFiles(EmptyArgs),
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
    CodeRebase(CodeRebaseArgs),
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
    RegisterGeneratePacRaw(RegisterPacRawArgs),
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
    InspectFunction(InspectFunctionArgs),
    InspectFlow(InspectFlowArgs),
    InspectObject(InspectObjectArgs),
    InspectScope(InspectScopeArgs),
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
            "advanced".to_owned(),
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
        assert!(
            ParsedInvocation::parse(["registers".to_owned(), "generate-pac".to_owned(),]).is_err(),
            "the old command must not survive as a compatibility alias"
        );
        let error = ParsedInvocation::parse([
            "registers".to_owned(),
            "generate-pac-raw".to_owned(),
            "--help".to_owned(),
        ])
        .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("--api-pack"));
        assert!(help.contains("--deny-unreviewed"));
    }

    #[test]
    fn root_help_is_project_first_and_low_level_engines_are_nested() {
        let mut command = command_definition();
        let help = command.render_long_help().to_string();
        assert!(help.contains("project"));
        assert!(help.contains("inspect"));
        assert!(help.contains("registers"));
        assert!(help.contains("advanced"));
        assert!(help.contains("START HERE"));
        assert!(!help.contains("  mmio "));
        assert!(!help.contains("  ir "));

        let error = ParsedInvocation::parse([
            "advanced".to_owned(),
            "mmio".to_owned(),
            "discover".to_owned(),
            "--help".to_owned(),
        ])
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Find MMIO accesses and infer register and field candidates")
        );
    }

    #[test]
    fn machine_output_is_one_json_document_and_details_are_human_metadata() {
        let invocation = ParsedInvocation::parse([
            "project".to_owned(),
            "status".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--details".to_owned(),
            "--output".to_owned(),
            "status.json".to_owned(),
        ])
        .unwrap();
        assert_eq!(invocation.ui.format, OutputFormat::Json);
        assert!(invocation.ui.details);
        let Command::ProjectStatus(arguments) = invocation.command else {
            panic!("unexpected argument type")
        };
        assert_eq!(arguments.output, Some(PathBuf::from("status.json")));

        assert!(
            ParsedInvocation::parse([
                "project".to_owned(),
                "status".to_owned(),
                "--format".to_owned(),
                "jsonl".to_owned(),
            ])
            .is_err()
        );
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
        assert_eq!(arguments.jobs, 1);

        let error =
            ParsedInvocation::parse(["project".to_owned(), "build".to_owned()]).unwrap_err();
        assert!(error.to_string().contains("unrecognized subcommand"));
    }

    #[test]
    fn linked_ir_commands_accept_explicit_function_workers() {
        let invocation = ParsedInvocation::parse([
            "advanced".to_owned(),
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
            "advanced".to_owned(),
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
            "advanced".to_owned(),
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
            "advanced".to_owned(),
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
