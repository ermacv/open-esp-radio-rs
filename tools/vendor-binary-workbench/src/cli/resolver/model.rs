//! Typed invocations produced after project and target resolution.

use std::path::PathBuf;

use crate::{
    MemoryMap, MmioMap, ProjectSpec, TargetSpec,
    application::ProjectSession,
    cli::{
        args::{CompletionArgs, ManpageArgs},
        arguments::*,
    },
    run_spec::RunSpec,
};

/// A fully resolved invocation whose variants encode the context required by
/// each workflow. Dispatch never has to pair a command discriminator with an
/// unrelated argument enum or recover required project state from `Option`.
pub(in crate::cli) enum ResolvedInvocation {
    GenerateCompletions(CompletionArgs),
    GenerateManpage(ManpageArgs),
    ProjectInit(ProjectInitArgs),
    ProjectConfigure {
        arguments: ProjectConfigureArgs,
        project_path: PathBuf,
    },
    ProjectInputsInit {
        arguments: ProjectInputsInitArgs,
        project_path: PathBuf,
    },
    ProjectBrowse {
        project_path: PathBuf,
    },
    ProjectDoctor(Box<ProjectSession>),
    ProjectStatus {
        arguments: ProjectStatusArgs,
        session: Box<ProjectSession>,
    },
    ProjectAnalyze {
        arguments: ProjectAnalyzeArgs,
        session: Box<ProjectSession>,
    },
    ProjectVerify {
        arguments: ProjectVerifyArgs,
        session: Box<ProjectSession>,
    },
    ProjectCheck {
        arguments: ProjectCheckArgs,
        session: Box<ProjectSession>,
    },
    ProjectPublish {
        arguments: CheckArgs,
        session: Box<ProjectSession>,
    },
    FunctionWorkspace {
        command: FunctionWorkspaceCommand,
        project: ProjectSpec,
        target: TargetSpec,
    },
    CodeWorkspace {
        command: CodeWorkspaceCommand,
        project: ProjectSpec,
    },
    RegisterWorkspace {
        command: RegisterWorkspaceCommand,
        project: ProjectSpec,
        memory_map: Option<MemoryMap>,
    },
    InterfaceWorkspace {
        command: InterfaceWorkspaceCommand,
        project: ProjectSpec,
        target: TargetSpec,
    },
    SymbolInventory {
        arguments: SymbolInventoryArgs,
        run_spec: RunSpec,
    },
    InterfaceDiscover {
        arguments: InterfaceDiscoverArgs,
        run_spec: RunSpec,
        project: Option<ProjectSpec>,
    },
    BuildIr {
        arguments: IrBuildArgs,
        project: ProjectSpec,
        run_spec: RunSpec,
        target: TargetSpec,
        svd: MmioMap,
    },
    VerifyEvidence(VerifyEvidenceArgs),
    Target {
        command: TargetCommand,
        target: TargetSpec,
        svd: MmioMap,
        project: Option<Box<ProjectSpec>>,
    },
}

pub(in crate::cli) enum FunctionWorkspaceCommand {
    InitPack(OutputArgs),
    Validate(ValidationArgs),
    Review(ReviewArgs),
}

pub(in crate::cli) enum CodeWorkspaceCommand {
    InitPack(OutputArgs),
    Rebase(CodeRebaseArgs),
    Validate(ValidationArgs),
    Review(ReviewArgs),
}

pub(in crate::cli) enum RegisterWorkspaceCommand {
    InitModel(RegisterModelArgs),
    ImportSvd(RegisterImportArgs),
    Validate(ValidationArgs),
    Review(RegisterReviewArgs),
    ExportSvd(RegisterExportArgs),
    GeneratePacRaw(RegisterPacRawArgs),
    GenerateBindings(RegisterBindingsArgs),
}

pub(in crate::cli) enum InterfaceWorkspaceCommand {
    InitPack(OutputArgs),
    Validate(ValidationArgs),
}

pub(in crate::cli) enum TargetCommand {
    AuditImageTargets(ImageAuditArgs),
    DiscoverMmio(MmioDiscoverArgs),
    ExportIr(IrExportArgs),
    VerifyContractChannel(VerifyContractArgs),
    VerifyContractRfInit(VerifyContractArgs),
    VerifyContractBluetoothTxPower(VerifyContractArgs),
    VerifyContractBluetoothTxGainInit(VerifyContractArgs),
    VerifyContractBasebandInit(VerifyContractArgs),
    VerifyContractRegisterInit(VerifyContractArgs),
    ExecuteRun(ExecuteRunArgs),
    ExecuteCompare(ExecuteCompareArgs),
    VerifyProfiles(VerifyProfilesArgs),
    GenerateReference(ReferenceArgs),
    GenerateReferenceBatch(ReferenceBatchArgs),
    GenerateDriver(DriverGenerateArgs),
    InspectAnalyze(InspectAnalyzeArgs),
    InspectFunction(InspectFunctionArgs),
    InspectObject(InspectObjectArgs),
    InspectScope(InspectScopeArgs),
    VerifyInventory(VerifyInventoryArgs),
    VerifySource(VerifySourceArgs),
    InspectTrace(TraceInputArgs),
    InspectCompare(InspectCompareArgs),
}
