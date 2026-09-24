//! Command-line adapter for the shared Blobray application API.

use blobray_application::{ImportWork, OperationHost, QueryWork, ReadQuery, WorkerReport};

use blobray_application::{self as app, ImportInput};
use blobray_domain::{
    ArtifactId, DEFAULT_WORKING_BYTES, Error, ErrorCode, LimitMode, ResourceBudget, Result,
    RevisionId, RunState, Target,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::{ffi::OsString, path::PathBuf, process::ExitCode};
mod function_display;
mod queries;

#[derive(Clone, Copy, Default, ValueEnum)]
enum Format {
    #[default]
    Human,
    Json,
}

#[derive(Parser)]
#[command(
    name = "blobray",
    about = "Captured binary investigations, synthetic images and retained function analysis"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value = "human")]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum KnowledgeCommand {
    /// Propose an integer or pointer layout for exact captured bytes.
    ProposeData {
        #[arg(long)]
        request: PathBuf,
    },
    /// Propose a constant supported by an exact retained analysis record.
    ProposeConstant {
        #[arg(long)]
        request: PathBuf,
    },
    /// Propose a reviewed register interpretation supported by a retained analysis.
    ProposeRegister {
        #[arg(long)]
        analysis: blobray_domain::FunctionAnalysisId,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_parser=parse_address)]
        address: u32,
        #[arg(long, default_value_t = 4)]
        width: u8,
        /// NAME:LSB:WIDTH, repeat for each nonoverlapping field.
        #[arg(long)]
        field: Vec<String>,
        #[arg(long)]
        base: Option<blobray_domain::KnowledgeRevisionId>,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: String,
    },
    /// Accept a specific proposal against the selected knowledge revision.
    Accept {
        #[arg(long)]
        assertion: blobray_domain::AssertionId,
        #[arg(long)]
        base: blobray_domain::KnowledgeRevisionId,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: String,
    },
    /// Validate a proposed change against the current base without publication.
    Validate {
        #[arg(long)]
        change: PathBuf,
    },
    /// Apply a KnowledgeChange JSON document (propose, accept/reject, or supersede).
    Apply {
        #[arg(long)]
        change: PathBuf,
    },
    /// List assertion states at an explicit revision or the current head.
    Show {
        #[arg(long)]
        revision: Option<blobray_domain::KnowledgeRevisionId>,
    },
    /// Stream immutable proposals and review decisions.
    History {
        #[arg(long)]
        revision: Option<blobray_domain::KnowledgeRevisionId>,
    },
    /// Export review events and evidence references; excludes private binary payloads.
    Export {
        #[arg(long)]
        revision: Option<blobray_domain::KnowledgeRevisionId>,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Inspect exact saved bindings and temporal obligations of a conditional event route.
    EventRoute {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Inspect local write definitions immediately before an exact saved publication site.
    MemorySlice {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Inspect structural paths and effects in an explicitly selected saved graph.
    Flow {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Navigate explicitly selected saved functions, calls, object accesses and context fields.
    Navigate {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Discover captured pointer slots or saved indirect-call paths with explicit review selection.
    Interfaces {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        /// Atomically export the completed JSON observation stream to a new file.
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Report executable intervals inside and outside selected function extents.
    Coverage {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Observe project logical file sizes without recovery or pruning.
    StorageUsage {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Inspect exact data ranges and supporting analyses; optionally export captured observations.
    Data {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Export one accepted table or constant at an explicit knowledge revision.
    ExportData {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        revision: blobray_domain::KnowledgeRevisionId,
        #[arg(long)]
        assertion: blobray_domain::AssertionId,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Audit all executable ELF sections for statically resolved forbidden transfers.
    AuditTargets {
        #[arg(long)]
        artifact: PathBuf,
        /// NAME=START..END (half-open RV32 address range).
        #[arg(long, required=true, value_parser=parse_forbidden)]
        forbid: Vec<blobray_domain::ForbiddenTargetRange>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Execute a captured compiled entry with an explicit scenario.
    Execute {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Compare concrete vendor and production observations.
    Compare {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Repeat an exact retained execution recipe with the same implementations.
    Replay {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: ArtifactId,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Read retained concrete observations without executing.
    Execution {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: ArtifactId,
        #[command(flatten)]
        limits: ResourceOptions,
    },

    /// Research one exact function selected from a saved publication.
    Research {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[arg(long, required_unless_present = "address", conflicts_with = "address")]
        name: Option<String>,
        #[arg(long, value_parser=parse_address)]
        address: Option<u32>,
        /// Explicit integer calling-convention assumption, independent of ELF flags.
        #[arg(long, value_parser=["riscv-integer"])]
        abi_contract: Option<String>,
        #[arg(long)]
        knowledge: Option<blobray_domain::KnowledgeRevisionId>,
        #[arg(long)]
        companion_publication: Vec<blobray_domain::PublicationId>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Export exact preserved bytes using a digest from the legacy/evidence catalog.
    ExportPayload {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: ArtifactId,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Preserve a legacy project and explicitly selected private roots into a new Next project.
    ImportLegacy {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Read the lossless legacy capture catalog and conversion outcomes.
    Legacy {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Save a consistent private project snapshot, including evidence bytes.
    Backup {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Verify a snapshot and restore it into a new project directory.
    Restore {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Propose or review retained knowledge with an explicit expected base.
    Knowledge {
        #[arg(long)]
        project: PathBuf,
        #[command(subcommand)]
        command: KnowledgeCommand,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Freeze all selected functions and coverage gaps into a portable plan.
    PlanInvestigation {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: Option<PathBuf>,
        /// Analyze this retained image instead of the revision's object inputs.
        #[arg(long, conflicts_with = "request")]
        image: Option<blobray_domain::PreparedImageId>,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Execute a frozen library plan and atomically publish its results.
    AnalyzeProject {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        plan: Option<PathBuf>,
        /// Without a saved plan, select this prepared image; otherwise analyze current inputs.
        #[arg(long, conflicts_with = "plan")]
        image: Option<blobray_domain::PreparedImageId>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List immutable library publications.
    Investigations {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List functions in a publication. Duplicate names remain separate candidates.
    Functions {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser=parse_address)]
        address: Option<u32>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List retained calls and out-of-function jumps, including unresolved targets.
    Calls {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        /// Function entry virtual address (callees of this function).
        #[arg(long, value_parser=parse_address)]
        caller: Option<u32>,
        /// Target virtual address (callers of this address).
        #[arg(long, value_parser=parse_address, conflicts_with="unresolved_only")]
        callee: Option<u32>,
        #[arg(long)]
        unresolved_only: bool,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Read function outcomes and gaps in a saved publication.
    Investigation {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Search saved memory accesses, preserving function/instruction provenance.
    FindAccesses {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[arg(long, value_parser=parse_address, conflicts_with_all=["symbol", "unknown_only"])]
        address: Option<u32>,
        /// JSON file containing an exact SymbolId, including its object occurrence.
        #[arg(long, conflicts_with = "unknown_only")]
        symbol: Option<PathBuf>,
        #[arg(long)]
        unknown_only: bool,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Search saved relocations, or image-address uses and memory accesses.
    FindReferences {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PublicationId,
        #[arg(long)]
        symbol: Option<PathBuf>,
        #[arg(long, value_parser=parse_address, conflicts_with="symbol")]
        address: Option<u32>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Show publication coverage and whether it describes the current revision.
    Status {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Analyze a selected captured function and retain its local graph.
    AnalyzeFunction {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List retained function analyses.
    Analyses {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Read saved function instructions, graph, references and coverage.
    Analysis {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::FunctionAnalysisId,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Export saved function records and manifest into a new directory.
    ExportAnalysis {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::FunctionAnalysisId,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },

    /// Plan a synthetic RV32 image from captured archive/object occurrences.
    LinkPlan {
        /// Exact ROM input and symbol name, for example 1:ets_delay_us.
        #[arg(long)]
        companion: Vec<String>,
        #[arg(long)]
        project: PathBuf,
        #[arg(long, conflicts_with_all=["entry", "inputs", "entry_input", "code_start", "data_start"])]
        request: Option<PathBuf>,
        #[arg(long, required_unless_present = "request")]
        entry: Option<String>,
        /// Captured input ordinals in link order.
        #[arg(long, value_delimiter = ',', required_unless_present = "request")]
        inputs: Vec<u64>,
        #[arg(long, required_unless_present = "request")]
        entry_input: Option<u64>,
        /// Synthetic placement, not an assertion about original firmware addresses.
        #[arg(long, value_parser=parse_address, required_unless_present="request")]
        code_start: Option<u32>,
        #[arg(long, value_parser=parse_address, required_unless_present="request")]
        data_start: Option<u32>,
        #[arg(long, default_value_t = 16777216)]
        region_bytes: u64,
        #[arg(long)]
        linker: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Link, validate and durably publish an image without changing current revision.
    PrepareImage {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        linker: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List retained prepared images.
    Images {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Verify and inspect an existing image, without requiring a linker.
    Image {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PreparedImageId,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Export verified ELF, recipe and provenance into a new directory.
    ExportImage {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        id: blobray_domain::PreparedImageId,
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Enumerate exact-name candidates without choosing an occurrence.
    Select {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        revision: Option<RevisionId>,
        #[arg(long, value_enum)]
        kind: SelectKind,
        #[arg(
            long,
            required_unless_present = "name_hex",
            conflicts_with = "name_hex"
        )]
        name: Option<String>,
        #[arg(long)]
        name_hex: Option<String>,
        #[arg(long)]
        input: Option<u64>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Create an immutable inspection recipe. --output saves its portable JSON.
    Plan {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Validate/reopen a saved plan, then execute its recorded inspection.
    Run {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        /// Reopen/validation limits; execution uses the saved plan budget.
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Create a project; never reset existing storage.
    Init {
        #[arg(long)]
        project: PathBuf,
    },
    /// Capture ordered input occurrences and publish their inventory.
    Import {
        #[arg(long)]
        project: PathBuf,
        /// Repeat ROLE=PATH in the required input order. Duplicate roles are allowed.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// Expected digest for a zero-based input occurrence: INDEX=SHA256.
        #[arg(long = "expect", value_name = "INDEX=SHA256")]
        expected: Vec<String>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Verify retained revisions without writes or repairs.
    Doctor {
        #[arg(long)]
        project: PathBuf,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// Recover abandoned import metadata and temporary data.
    Recover {
        #[arg(long)]
        project: PathBuf,
    },
    /// List durable import outcomes, including interrupted attempts.
    Runs {
        #[arg(long)]
        project: PathBuf,
    },
    /// Read persisted inventory and verify all retained payloads.
    Inventory {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        revision: Option<RevisionId>,
        #[command(flatten)]
        limits: ResourceOptions,
    },
    /// List committed revisions in publication order.
    Revisions {
        #[arg(long)]
        project: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SelectKind {
    Object,
    Symbol,
}

#[derive(clap::Args)]
struct ResourceOptions {
    /// Private runtime directory; defaults to a host-selected per-user directory.
    #[arg(long)]
    temporary_root: Option<PathBuf>,
    /// Per-operation logical temporary storage, including a 1 MiB control reserve.
    #[arg(long, default_value_t = 8192)]
    temporary_mib: u64,
    /// Aggregate reservations in this Application, including retained query/Plan results.
    #[arg(long, default_value_t = 32768)]
    temporary_total_mib: u64,
    #[arg(long, value_parser = ["kernel", "watchdog"], default_value = "kernel")]
    limit_mode: String,
    #[arg(long, default_value_t = 4096)]
    memory_mib: u64,
    #[arg(long, default_value_t = DEFAULT_WORKING_BYTES / (1024 * 1024))]
    working_memory_mib: u64,
    #[arg(long, default_value_t = 900)]
    timeout_secs: u64,
    #[arg(long, default_value_t = blobray_domain::DEFAULT_WORK_UNITS)]
    max_work_units: u64,
    /// Delegated cgroup v2 parent; defaults to the current cgroup.
    #[arg(long)]
    cgroup_root: Option<PathBuf>,
}

impl ResourceOptions {
    fn application(&self) -> Result<app::Application> {
        app::Application::with_temporary_storage(
            host(self.cgroup_root.clone())?,
            app::ApplicationLimits::default(),
            app::TemporaryStoragePolicy {
                root: self.temporary_root.clone(),
                operation_bytes: self
                    .temporary_mib
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| invalid("temporary limit overflow"))?,
                total_bytes: self
                    .temporary_total_mib
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| invalid("temporary total limit overflow"))?,
            },
        )
    }
    fn budget(&self) -> Result<ResourceBudget> {
        Ok(ResourceBudget {
            mode: if self.limit_mode == "kernel" {
                LimitMode::Kernel
            } else {
                LimitMode::Watchdog
            },
            working_memory_bytes: Some(
                self.working_memory_mib
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| invalid("working memory limit overflow"))?,
            ),
            memory_bytes: self
                .memory_mib
                .checked_mul(1024 * 1024)
                .ok_or_else(|| invalid("memory limit overflow"))?,
            timeout_ms: self
                .timeout_secs
                .checked_mul(1000)
                .ok_or_else(|| invalid("timeout overflow"))?,
            max_work_units: Some(self.max_work_units),
            ..Default::default()
        })
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|s| s == "__guard" || s == "__worker")
    {
        let result = internal(&args);
        if let Err(error) = result {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            let json = args.iter().any(|s| s == "--format=json")
                || args
                    .windows(2)
                    .any(|pair| pair[0] == "--format" && pair[1] == "json");
            if error.use_stderr() && json {
                eprintln!(
                    "{}",
                    serde_json::json!({"schema": 1, "error": invalid(&error.to_string())})
                );
            } else {
                let _ = error.print();
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(cli.command, cli.format) {
        Ok(status) => status,
        Err(error) => {
            match cli.format {
                Format::Human => eprintln!("blobray: {error}"),
                Format::Json => eprintln!("{}", serde_json::json!({"schema": 1, "error": error})),
            }
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command, format: Format) -> Result<ExitCode> {
    match command {
        Command::Data {
            project,
            request,
            output,
            limits,
        } => {
            let query = ReadQuery::Data {
                request: read_json_file(&request)?,
            };
            if let Some(output) = output {
                return export_data_query(project, query, output, limits, format);
            }
            return read_query(project, query, limits, format);
        }
        Command::ExportData {
            project,
            revision,
            assertion,
            output,
            limits,
        } => {
            return export_data_query(
                project,
                ReadQuery::ReviewedData {
                    revision,
                    assertion,
                },
                output,
                limits,
                format,
            );
        }
        Command::AuditTargets {
            artifact,
            forbid,
            limits,
        } => {
            let artifact = std::path::absolute(artifact).map_err(io_error)?;
            return read_query(
                PathBuf::from("."),
                ReadQuery::AuditTargets {
                    artifact: blobray_domain::OriginPath::from_path(&artifact),
                    ranges: forbid,
                },
                limits,
                format,
            );
        }
        Command::ExportPayload {
            project,
            id,
            output,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(
                &project,
                ReadQuery::RetainedPayload { id },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            handle
                .take_output()?
                .export_payload(&output, &|| signals.cancelled())?;
        }
        Command::Legacy { project, limits } => {
            return read_query(project, ReadQuery::Legacy, limits, format);
        }
        Command::ImportLegacy {
            request,
            project,
            limits,
        } => {
            let request = read_json_file(&request)?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(
                &project,
                ReadQuery::ImportLegacy { request },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let mut output = handle.take_output()?;
            output.publish_restore(&project, &|| signals.cancelled())?;
            match format {
                Format::Json => println!(
                    "{}",
                    serde_json::to_string(output.summary()).map_err(|e| invalid(&e.to_string()))?
                ),
                Format::Human => println!("Project preserved at {}", project.display()),
            }
        }
        Command::Backup {
            project,
            output,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(&project, ReadQuery::Backup, limits.budget()?)?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            handle
                .take_output()?
                .export_backup(&output, &|| signals.cancelled())?;
        }
        Command::Restore {
            backup,
            project,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let backup = std::path::absolute(backup).map_err(io_error)?;
            let handle = application.start_query(
                &project,
                ReadQuery::Restore {
                    bundle: blobray_domain::OriginPath::from_path(&backup),
                },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let mut output = handle.take_output()?;
            output.publish_restore(&project, &|| signals.cancelled())?;
            match format {
                Format::Json => println!(
                    "{}",
                    serde_json::to_string(output.summary()).map_err(|e| invalid(&e.to_string()))?
                ),
                Format::Human => println!("Project preserved at {}", project.display()),
            }
        }
        Command::EventRoute {
            project,
            request,
            output,
            limits,
        } => {
            return read_query_output(
                project,
                ReadQuery::EventRoute {
                    request: read_json_file(&request)?,
                },
                limits,
                format,
                output,
            );
        }
        Command::MemorySlice {
            project,
            request,
            output,
            limits,
        } => {
            return read_query_output(
                project,
                ReadQuery::MemorySlice {
                    request: read_json_file(&request)?,
                },
                limits,
                format,
                output,
            );
        }
        Command::Flow {
            project,
            request,
            output,
            limits,
        } => {
            return read_query_output(
                project,
                ReadQuery::Flow {
                    request: read_json_file(&request)?,
                },
                limits,
                format,
                output,
            );
        }
        Command::Navigate {
            project,
            request,
            output,
            limits,
        } => {
            return read_query_output(
                project,
                ReadQuery::Navigate {
                    request: read_json_file(&request)?,
                },
                limits,
                format,
                output,
            );
        }
        Command::Interfaces {
            project,
            request,
            output,
            limits,
        } => {
            return read_query_output(
                project,
                ReadQuery::Interfaces {
                    request: read_json_file(&request)?,
                },
                limits,
                format,
                output,
            );
        }
        Command::Knowledge {
            project,
            command,
            limits,
        } => match command {
            KnowledgeCommand::ProposeData { request } => {
                return propose_data_command(project, request, limits, format, false);
            }
            KnowledgeCommand::ProposeConstant { request } => {
                return propose_data_command(project, request, limits, format, true);
            }
            KnowledgeCommand::Accept {
                assertion,
                base,
                actor,
                reason,
            } => {
                let change = blobray_domain::KnowledgeChange {
                    expected_base: Some(base),
                    actor,
                    reason,
                    action: blobray_domain::KnowledgeAction::Review {
                        assertion,
                        decision: blobray_domain::ReviewDecision::Accept,
                        supersedes: None,
                    },
                };
                return apply_native_knowledge(
                    &project,
                    &change,
                    &limits,
                    format,
                    limits.budget()?,
                );
            }
            KnowledgeCommand::ProposeRegister {
                analysis,
                subject,
                name,
                address,
                width,
                field,
                base,
                actor,
                reason,
            } => {
                let fields = field
                    .iter()
                    .map(|s| {
                        let parts: Vec<_> = s.split(':').collect();
                        if parts.len() != 3 {
                            return Err(invalid("field must be NAME:LSB:WIDTH"));
                        }
                        Ok(blobray_domain::MmioField {
                            name: parts[0].into(),
                            lsb: parts[1].parse().map_err(|_| invalid("invalid field lsb"))?,
                            width: parts[2]
                                .parse()
                                .map_err(|_| invalid("invalid field width"))?,
                        })
                    })
                    .collect::<Result<_>>()?;
                let application = limits.application()?;
                let _diagnostics = TemporaryDiagnostics(&application, format);
                let signals = Signals::new()?;
                let handle = application.start_propose_register(
                    &project,
                    blobray_domain::RegisterProposalRequest {
                        analysis,
                        subject: subject.try_into()?,
                        register: blobray_domain::MmioRegister {
                            name,
                            address,
                            width,
                            fields,
                        },
                        expected_base: base,
                        actor,
                        reason,
                    },
                    limits.budget()?,
                )?;
                if !wait_handle(&handle, &signals, format) {
                    return Ok(ExitCode::FAILURE);
                }
                let run = handle.wait();
                match format {
                    Format::Json => println!("{}", serde_json::json!({"schema":6,"run":run})),
                    Format::Human => println!("Knowledge revision {}", run.knowledge.unwrap()),
                }
                return Ok(ExitCode::SUCCESS);
            }
            KnowledgeCommand::Validate { change } => {
                return read_query(
                    project,
                    ReadQuery::ValidateKnowledge {
                        change: read_json_file(&change)?,
                    },
                    limits,
                    format,
                );
            }
            KnowledgeCommand::Apply { change } => {
                let change = read_json_file(&change)?;
                let application = limits.application()?;
                let _diagnostics = TemporaryDiagnostics(&application, format);
                let signals = Signals::new()?;
                let handle = application.start_knowledge(&project, &change, limits.budget()?)?;
                if !wait_handle(&handle, &signals, format) {
                    return Ok(ExitCode::FAILURE);
                }
                let record = handle.wait();
                match format {
                    Format::Json => println!("{}", serde_json::json!({"schema":6,"run":record})),
                    Format::Human => println!("Knowledge revision {}", record.knowledge.unwrap()),
                }
            }
            KnowledgeCommand::Show { revision } => {
                return read_query(
                    project,
                    ReadQuery::Knowledge {
                        revision,
                        history: false,
                    },
                    limits,
                    format,
                );
            }
            KnowledgeCommand::History { revision } => {
                return read_query(
                    project,
                    ReadQuery::Knowledge {
                        revision,
                        history: true,
                    },
                    limits,
                    format,
                );
            }
            KnowledgeCommand::Export { revision, output } => {
                let application = limits.application()?;
                let _diagnostics = TemporaryDiagnostics(&application, format);
                let signals = Signals::new()?;
                let handle = application.start_query(
                    &project,
                    ReadQuery::Knowledge {
                        revision,
                        history: true,
                    },
                    limits.budget()?,
                )?;
                if !wait_handle(&handle, &signals, format) {
                    return Ok(ExitCode::FAILURE);
                }
                let parent = output
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
                queries::render(
                    &mut handle.take_output()?,
                    Format::Json,
                    file.as_file_mut(),
                    &|| signals.cancelled(),
                )?;
                file.as_file().sync_all().map_err(io_error)?;
                file.persist_noclobber(output)
                    .map_err(|e| io_error(e.error))?;
            }
        },
        Command::PlanInvestigation {
            project,
            request,
            image,
            output,
            limits,
        } => {
            use blobray_domain::{FunctionDecoder, FunctionSemantics};
            let mut request: blobray_domain::InvestigationRequest = request
                .as_ref()
                .map(|p| read_json_file(p))
                .transpose()?
                .unwrap_or_default();
            if image.is_some() {
                request.image = image;
            }
            let decoder = blobray_backend_riscv::RiscvDecoder;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(
                &project,
                ReadQuery::PlanInvestigation {
                    request,
                    producer: blobray_domain::FunctionProducer {
                        decoder: decoder.identity().into(),
                        semantics: decoder.semantic_identity().into(),
                    },
                },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let mut result = handle.take_output()?;
            let app::QuerySummary::InvestigationPlan { plan } = result.summary() else {
                return Err(invalid("unexpected plan summary"));
            };
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(std::path::Path::new("."));
            let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            app::write_control_message(file.as_file_mut(), plan)?;
            file.as_file().sync_all().map_err(io_error)?;
            file.persist_noclobber(&output)
                .map_err(|e| io_error(e.error))?;
            queries::render(&mut result, format, &mut std::io::stdout().lock(), &|| {
                signals.cancelled()
            })?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::AnalyzeProject {
            project,
            plan,
            image,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let input = if let Some(plan) = plan {
                blobray_domain::InvestigationInput::Plan {
                    plan: read_json_file(&plan)?,
                }
            } else {
                use blobray_domain::{FunctionDecoder, FunctionSemantics};
                let decoder = blobray_backend_riscv::RiscvDecoder;
                blobray_domain::InvestigationInput::Automatic {
                    request: blobray_domain::InvestigationRequest {
                        image,
                        ..Default::default()
                    },
                    producer: blobray_domain::FunctionProducer {
                        decoder: decoder.identity().into(),
                        semantics: decoder.semantic_identity().into(),
                    },
                }
            };
            let handle = application.start_analyze_project(&project, input, limits.budget()?)?;
            while !handle.status().state.terminal() {
                if signals.cancelled() {
                    handle.cancel();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let record = handle.wait();
            let success = record.state == RunState::Completed;
            let text = match format {
                Format::Json => serde_json::json!({"schema":5,"run":record}).to_string(),
                Format::Human => format!(
                    "{}\nPublication: {} ({})",
                    render_run(&record),
                    record.publication.as_ref().map_or("none", |p| p.as_str()),
                    if record.assessment.as_ref().is_some_and(|a| a.is_complete()) {
                        "complete"
                    } else {
                        "partial"
                    }
                ),
            };
            if success {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
            return Ok(if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::Investigations { project, limits } => {
            return read_query(project, ReadQuery::Publications, limits, format);
        }
        Command::Functions {
            project,
            id,
            name,
            address,
            limits,
        } => {
            return read_query(
                project,
                ReadQuery::Publication {
                    id,
                    filter: blobray_domain::InvestigationFilter::Functions { name, address },
                },
                limits,
                format,
            );
        }
        Command::Calls {
            project,
            id,
            caller,
            callee,
            unresolved_only,
            limits,
        } => {
            return read_query(
                project,
                ReadQuery::Publication {
                    id,
                    filter: blobray_domain::InvestigationFilter::Calls {
                        caller,
                        callee,
                        unresolved_only,
                    },
                },
                limits,
                format,
            );
        }
        Command::Investigation {
            project,
            id,
            limits,
        } => {
            return read_query(
                project,
                ReadQuery::Publication {
                    id,
                    filter: blobray_domain::InvestigationFilter::Members,
                },
                limits,
                format,
            );
        }
        Command::FindAccesses {
            project,
            id,
            address,
            symbol,
            unknown_only,
            limits,
        } => {
            let symbol = symbol.as_ref().map(|p| read_json_file(p)).transpose()?;
            return read_query(
                project,
                ReadQuery::Publication {
                    id,
                    filter: blobray_domain::InvestigationFilter::Accesses {
                        address,
                        symbol,
                        unknown_only,
                    },
                },
                limits,
                format,
            );
        }
        Command::FindReferences {
            project,
            id,
            symbol,
            address,
            limits,
        } => {
            let symbol = symbol.as_ref().map(|p| read_json_file(p)).transpose()?;
            return read_query(
                project,
                ReadQuery::Publication {
                    id,
                    filter: blobray_domain::InvestigationFilter::References { symbol, address },
                },
                limits,
                format,
            );
        }
        Command::Status { project, limits } => {
            return read_query(project, ReadQuery::InvestigationStatus, limits, format);
        }
        Command::Research {
            project,
            id,
            name,
            address,
            abi_contract,
            knowledge,
            companion_publication,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let request = blobray_domain::ResearchRequest {
                publication: id.clone(),
                name,
                address,
                options: blobray_domain::ResearchOptions {
                    companions: companion_publication,
                    publication: id,
                    abi: abi_contract.map(|_| blobray_domain::CallAbi::RiscvInteger),
                    knowledge,
                },
            };
            let handle = application.start_research(&project, request, limits.budget()?)?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let run = handle.wait();
            match format {
                Format::Json => println!("{}", serde_json::json!({"schema":4,"run":run})),
                Format::Human => println!(
                    "Research saved: {} ({})",
                    run.analysis.as_ref().map_or("", |id| id.as_str()),
                    if run.assessment.as_ref().is_some_and(|a| a.is_complete()) {
                        "complete in selected static profile"
                    } else {
                        "partial; inspect retained gaps"
                    }
                ),
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::Execute {
            project,
            request,
            limits,
        } => {
            let request: blobray_domain::ExecutionRequest = read_json_file(&request)?;
            if request.replacement.is_some() {
                return Err(invalid("execute accepts one implementation; use compare"));
            }
            return execute_request(&project, request, &limits, format);
        }
        Command::Compare {
            project,
            request,
            limits,
        } => {
            let request: blobray_domain::ExecutionRequest = read_json_file(&request)?;
            if request.replacement.is_none() {
                return Err(invalid("compare requires a replacement and binding"));
            }
            return execute_request(&project, request, &limits, format);
        }
        Command::Execution {
            project,
            id,
            limits,
        } => return read_query(project, ReadQuery::Execution { id }, limits, format),
        Command::Replay {
            project,
            id,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_replay(
                &project,
                id,
                &blobray_backend_riscv::RiscvExecutor,
                limits.budget()?,
            )?;
            return Ok(finish_execution(&handle, &signals, format));
        }
        Command::AnalyzeFunction {
            project,
            request,
            limits,
        } => {
            let request = read_json_file(&request)?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_analyze_function(&project, request, limits.budget()?)?;
            while !handle.status().state.terminal() {
                if signals.cancelled() {
                    handle.cancel();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let record = handle.wait();
            let success = record.state == RunState::Completed;
            let text = match format {
                Format::Json => serde_json::json!({"schema":4,"run":record}).to_string(),
                Format::Human => {
                    let mut text = format!("Function analysis {:?}", record.state);
                    if let Some(id) = &record.analysis {
                        let coverage =
                            if record.assessment.as_ref().is_some_and(|a| a.is_complete()) {
                                "complete"
                            } else {
                                "partial"
                            };
                        text.push_str(&format!(": {} ({coverage})", id.as_str()));
                    }
                    if let Some(error) = &record.error {
                        text.push_str(&format!(": {:?}: {}", error.code, error.message));
                    }
                    text
                }
            };
            if success {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
            return Ok(if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::Analyses { project, limits } => {
            return read_query(project, ReadQuery::Analyses, limits, format);
        }
        Command::Analysis {
            project,
            id,
            limits,
        } => {
            return read_query(
                project,
                ReadQuery::Analysis { id, export: false },
                limits,
                format,
            );
        }
        Command::ExportAnalysis {
            project,
            id,
            output,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(
                &project,
                ReadQuery::Analysis { id, export: true },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            handle
                .take_output()?
                .export_analysis(&output, &|| signals.cancelled())?;
        }

        Command::LinkPlan {
            companion,
            project,
            request,
            entry,
            inputs,
            entry_input,
            code_start,
            data_start,
            region_bytes,
            linker,
            output,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let query = if let Some(path) = request {
                ReadQuery::LinkPlan {
                    request: read_json_file(&path)?,
                    linker: blobray_domain::OriginPath::from_path(&linker),
                }
            } else {
                ReadQuery::NamedLinkPlan {
                    request: blobray_domain::NamedLinkRequest {
                        companions: companion
                            .iter()
                            .map(|v| {
                                let (input, name) = v
                                    .split_once(':')
                                    .ok_or_else(|| invalid("companion must be INPUT:NAME"))?;
                                Ok(blobray_domain::NamedCompanion {
                                    input: input
                                        .parse()
                                        .map_err(|_| invalid("invalid companion input"))?,
                                    name: name.into(),
                                })
                            })
                            .collect::<Result<_>>()?,
                        revision: None,
                        inputs,
                        entry_input: entry_input.ok_or_else(|| invalid("entry input required"))?,
                        entry_name: entry.ok_or_else(|| invalid("entry required"))?.into_bytes(),
                        layout: blobray_domain::ImageLayout {
                            code: blobray_domain::ImageRegion {
                                start: code_start.ok_or_else(|| invalid("code start required"))?,
                                length: region_bytes,
                            },
                            data: blobray_domain::ImageRegion {
                                start: data_start.ok_or_else(|| invalid("data start required"))?,
                                length: region_bytes,
                            },
                        },
                    },
                    linker: blobray_domain::OriginPath::from_path(&linker),
                }
            };
            let handle = application.start_query(&project, query, limits.budget()?)?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let mut result = handle.take_output()?;
            if matches!(result.summary(), app::QuerySummary::Selection { .. }) {
                queries::render(&mut result, format, &mut std::io::stdout().lock(), &|| {
                    signals.cancelled()
                })?;
                return Ok(ExitCode::FAILURE);
            }
            let plan = app::LinkPlan::from_output(result)?;
            if let Some(path) = output {
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
                plan.write(file.as_file_mut(), &|| signals.cancelled())?;
                file.as_file().sync_all().map_err(io_error)?;
                file.persist_noclobber(&path)
                    .map_err(|e| io_error(e.error))?;
            } else if matches!(format, Format::Json) {
                plan.write(&mut std::io::stdout().lock(), &|| signals.cancelled())?;
            } else {
                println!(
                    "LinkPlan {}: {}",
                    plan.description().id,
                    if plan.description().ready() {
                        "ready"
                    } else {
                        "blocked"
                    }
                );
                for blocker in &plan.description().blockers {
                    println!("Input {:?}: {}", blocker.input, blocker.message);
                }
            }
            return Ok(if plan.description().ready() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::PrepareImage {
            project,
            plan,
            linker,
            limits,
        } => {
            let description = app::read_link_plan(std::fs::File::open(plan).map_err(io_error)?)?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_prepare_image(
                &project,
                &description,
                &linker,
                limits.budget()?,
            )?;
            while !handle.status().state.terminal() {
                if signals.cancelled() {
                    handle.cancel();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let record = handle.wait();
            let success = record.state == RunState::Completed;
            let text = match format {
                Format::Json => serde_json::json!({"schema":3,"run":record}).to_string(),
                Format::Human => format!(
                    "{}\nImage: {}",
                    render_run(&record),
                    record
                        .image
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "none".into())
                ),
            };
            if success {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
            return Ok(if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::Images { project, limits } => {
            return read_query(project, ReadQuery::Images, limits, format);
        }
        Command::Image {
            project,
            id,
            limits,
        } => {
            return read_query(
                project,
                ReadQuery::Image { id, export: false },
                limits,
                format,
            );
        }
        Command::ExportImage {
            project,
            id,
            output,
            limits,
        } => {
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_query(
                &project,
                ReadQuery::Image { id, export: true },
                limits.budget()?,
            )?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            handle
                .take_output()?
                .export_image(&output, &|| signals.cancelled())?;
        }
        Command::Select {
            project,
            revision,
            kind,
            name,
            name_hex,
            input,
            limits,
        } => {
            let name = if let Some(hex) = name_hex {
                if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(invalid("name-hex requires pairs of hexadecimal digits"));
                }
                hex.as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect()
            } else {
                name.unwrap().into_bytes()
            };
            let kind = match kind {
                SelectKind::Object => blobray_domain::SelectionKind::Object,
                SelectKind::Symbol => blobray_domain::SelectionKind::Symbol,
            };
            return read_query(
                project,
                ReadQuery::Select {
                    revision,
                    request: blobray_domain::SelectionRequest { kind, name, input },
                },
                limits,
                format,
            );
        }
        Command::Plan {
            project,
            request,
            output,
            limits,
        } => {
            let request: app::PlanRequest = read_json_file(&request)?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle = application.start_plan(&project, request, limits.budget()?)?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let plan = handle.take_plan()?;
            if let Some(path) = output {
                // Stage and publish without overwriting an existing user file.
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
                plan.write(temporary.as_file_mut(), &|| signals.cancelled())?;
                temporary.as_file().sync_all().map_err(io_error)?;
                temporary
                    .persist_noclobber(&path)
                    .map_err(|e| io_error(e.error))?;
                eprintln!("Saved plan {} to {}", plan.description().id, path.display());
            } else if matches!(format, Format::Json) {
                plan.write(&mut std::io::stdout().lock(), &|| signals.cancelled())?;
            } else {
                let d = plan.description();
                println!(
                    "Plan {}\nRevision {}\nScope {:?}\nInventory {}\nUse --format json or --output to save this plan.",
                    d.id,
                    d.recipe.revision,
                    d.recipe.scope,
                    if d.recipe.revision_complete {
                        "complete"
                    } else {
                        "incomplete; inspection retains coverage gaps"
                    }
                );
            }
        }
        Command::Run {
            project,
            plan,
            limits,
        } => {
            let description =
                app::PlanDescription::read(std::fs::File::open(plan).map_err(io_error)?)?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let reopened =
                application.start_reopen_plan(&project, description, limits.budget()?)?;
            if !wait_handle(&reopened, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let plan = reopened.take_plan()?;
            drop(reopened);
            let handle = application.start_run(&plan)?;
            if !wait_handle(&handle, &signals, format) {
                return Ok(ExitCode::FAILURE);
            }
            let mut result = handle.take_output()?;
            queries::render(&mut result, format, &mut std::io::stdout().lock(), &|| {
                signals.cancelled()
            })?;
        }
        Command::Init { project } => {
            let id = app::create_project(&project)?;
            match format {
                Format::Human => println!("Project {id}"),
                Format::Json => println!("{}", serde_json::json!({"schema": 1, "project": id})),
            }
        }
        Command::Import {
            project,
            inputs,
            expected,
            limits,
        } => {
            let mut inputs = inputs
                .into_iter()
                .map(parse_input)
                .collect::<Result<Vec<_>>>()?;
            for binding in expected {
                let (index, digest) = binding
                    .split_once('=')
                    .ok_or_else(|| invalid("expected INDEX=SHA256"))?;
                let index = index
                    .parse::<usize>()
                    .map_err(|_| invalid("expected input index must be a nonnegative integer"))?;
                let input = inputs
                    .get_mut(index)
                    .ok_or_else(|| invalid("expected digest references an absent input index"))?;
                if input.expected.is_some() {
                    return Err(invalid("duplicate expected digest for an input index"));
                }
                input.expected = Some(digest.parse::<ArtifactId>()?);
            }
            let budget = limits.budget()?;
            let application = limits.application()?;
            let _diagnostics = TemporaryDiagnostics(&application, format);
            let signals = Signals::new()?;
            let handle =
                application.start_import(&project, inputs, Target::Riscv32Ilp32, budget)?;
            while !handle.status().state.terminal() {
                if signals.cancelled() {
                    handle.cancel();
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let record = handle.wait();
            let success = record.state == RunState::Completed;
            let text = match format {
                Format::Json => serde_json::json!({"schema": 3, "run": record}).to_string(),
                Format::Human => render_run(&record),
            };
            if success {
                println!("{text}");
            } else {
                eprintln!("{text}");
            }
            return Ok(if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::Inventory {
            project,
            revision,
            limits,
        } => {
            return read_query(project, ReadQuery::Inventory { revision }, limits, format);
        }
        Command::Coverage {
            project,
            id,
            limits,
        } => {
            return read_query(project, ReadQuery::Coverage { id }, limits, format);
        }
        Command::StorageUsage { project, limits } => {
            return read_query(project, ReadQuery::StorageUsage, limits, format);
        }
        Command::Doctor { project, limits } => {
            return read_query(project, ReadQuery::Doctor, limits, format);
        }
        Command::Recover { project } => {
            let recovered = application(None)?.recover(&project)?;
            match format {
                Format::Json => println!(
                    "{}",
                    serde_json::json!({"schema": 2, "recovered": recovered})
                ),
                Format::Human => {
                    println!("Recovery complete; abandoned runs: {}", recovered.len());
                    for run in recovered {
                        println!("{}", render_run(&run));
                    }
                }
            }
        }
        Command::Runs { project } => {
            let runs = app::runs(&project)?;
            match format {
                Format::Json => println!("{}", serde_json::json!({"schema": 2, "runs": runs})),
                Format::Human => {
                    for run in runs {
                        println!("{}", render_run(&run));
                    }
                }
            }
        }
        Command::Revisions { project } => {
            let revisions = app::revisions(&project)?;
            match format {
                Format::Human => {
                    for revision in revisions {
                        println!("{revision}");
                    }
                }
                Format::Json => println!(
                    "{}",
                    serde_json::json!({"schema": 1, "revisions": revisions})
                ),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn io_error(e: std::io::Error) -> Error {
    blobray_domain::storage_io(e)
}
fn execute_request(
    project: &std::path::Path,
    request: blobray_domain::ExecutionRequest,
    limits: &ResourceOptions,
    format: Format,
) -> Result<ExitCode> {
    let application = limits.application()?;
    let _diagnostics = TemporaryDiagnostics(&application, format);
    let signals = Signals::new()?;
    let handle = application.start_execution(
        project,
        request,
        &blobray_backend_riscv::RiscvExecutor,
        limits.budget()?,
    )?;
    Ok(finish_execution(&handle, &signals, format))
}
fn finish_execution(handle: &app::RunHandle, signals: &Signals, format: Format) -> ExitCode {
    let success = wait_handle(handle, signals, format);
    let record = handle.wait();
    match format {
        Format::Json => println!("{}", serde_json::json!({"schema":1,"run":record})),
        Format::Human => println!(
            "Execution {:?}{}: {}",
            record.state,
            record
                .assessment
                .as_ref()
                .and_then(|a| a.comparison)
                .map(|v| format!(" / {v:?}"))
                .unwrap_or_default(),
            record
                .execution
                .as_ref()
                .map_or("no publication", ArtifactId::as_str)
        ),
    }
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
fn read_json_file<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io_error)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 65536 {
        return Err(invalid("request exceeds 64 KiB"));
    }
    serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))
}
struct TemporaryDiagnostics<'a>(&'a app::Application, Format);
impl Drop for TemporaryDiagnostics<'_> {
    fn drop(&mut self) {
        self.0.shutdown();
        temporary_warnings(self.0, self.1);
    }
}
fn temporary_warnings(application: &app::Application, format: Format) {
    let temporary = application.temporary_storage_status();
    for error in &temporary.diagnostics {
        match format {
            Format::Json => eprintln!(
                "{}",
                serde_json::json!({"schema":1,"temporary_storage_warning":error})
            ),
            Format::Human => eprintln!("Temporary storage: {}", error.message),
        }
    }
    if temporary.diagnostics_truncated {
        match format {
            Format::Json => eprintln!(
                "{}",
                serde_json::json!({"schema":1,"temporary_storage_warnings_truncated":true})
            ),
            Format::Human => {
                eprintln!("Temporary storage: additional residue diagnostics omitted (limit 4)")
            }
        }
    }
}
fn wait_handle(handle: &app::RunHandle, signals: &Signals, format: Format) -> bool {
    loop {
        if signals.cancelled() {
            handle.cancel();
        }
        if handle.status().state.terminal() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let record = handle.wait();
    if record.state == RunState::Completed {
        return true;
    }
    let report = handle.report();
    match format {
        Format::Json => eprintln!("{}", serde_json::json!({"schema":1,"query":report})),
        Format::Human => {
            eprintln!(
                "Query {:?}: {}",
                report.state,
                report
                    .error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("worker failure")
            );
            let mut diagnostics = String::new();
            render_diagnostics(
                &mut diagnostics,
                &report.diagnostics,
                record.budget.max_work_units,
            );
            eprintln!("{diagnostics}");
        }
    }
    false
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

fn parse_input(value: OsString) -> Result<ImportInput> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let bytes = value.as_os_str().as_bytes();
        let split = bytes
            .iter()
            .position(|b| *b == b'=')
            .ok_or_else(|| invalid("expected ROLE=PATH"))?;
        let role = std::str::from_utf8(&bytes[..split])
            .map_err(|_| invalid("role must be UTF-8"))?
            .to_owned();
        if split + 1 == bytes.len() {
            return Err(invalid("input path is empty"));
        }
        Ok(ImportInput {
            role,
            path: OsString::from_vec(bytes[split + 1..].to_vec()).into(),
            expected: None,
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units: Vec<u16> = value.encode_wide().collect();
        let split = units
            .iter()
            .position(|c| *c == u16::from(b'='))
            .ok_or_else(|| invalid("expected ROLE=PATH"))?;
        let role =
            String::from_utf16(&units[..split]).map_err(|_| invalid("role must be Unicode"))?;
        if split + 1 == units.len() {
            return Err(invalid("input path is empty"));
        }
        Ok(ImportInput {
            role,
            path: OsString::from_wide(&units[split + 1..]).into(),
            expected: None,
        })
    }
}

fn host(cgroup: Option<PathBuf>) -> Result<std::sync::Arc<dyn OperationHost>> {
    #[cfg(target_os = "linux")]
    {
        Ok(std::sync::Arc::new(
            blobray_next_host::linux::LinuxHost::new(
                std::env::current_exe().map_err(blobray_domain::storage_io)?,
                cgroup,
            ),
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cgroup;
        Err(Error::new(
            ErrorCode::Unavailable,
            "supervised operations require Linux",
        ))
    }
}
fn application(cgroup: Option<PathBuf>) -> Result<app::Application> {
    Ok(app::Application::new(host(cgroup)?))
}
struct Signals {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    fn new() -> Result<Self> {
        let mut result = Self {
            flag: Default::default(),
            ids: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            result.ids.push(
                signal_hook::flag::register(signal, result.flag.clone())
                    .map_err(blobray_domain::storage_io)?,
            );
        }
        Ok(result)
    }
    fn cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::Relaxed)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
fn parse_forbidden(
    value: &str,
) -> std::result::Result<blobray_domain::ForbiddenTargetRange, String> {
    let (name, bounds) = value.split_once('=').ok_or("expected NAME=START..END")?;
    let (start, end) = bounds.split_once("..").ok_or("expected START..END")?;
    let parse = |v: &str| {
        if let Some(hex) = v.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else {
            v.parse::<u64>()
        }
        .map_err(|e| e.to_string())
    };
    let range = blobray_domain::ForbiddenTargetRange {
        name: name.into(),
        start: u32::try_from(parse(start)?).map_err(|e| e.to_string())?,
        end: parse(end)?,
    };
    range.validate().map_err(|e| e.message)?;
    Ok(range)
}

fn read_query(
    project: PathBuf,
    query: ReadQuery,
    limits: ResourceOptions,
    format: Format,
) -> Result<ExitCode> {
    read_query_output(project, query, limits, format, None)
}
fn read_query_output(
    project: PathBuf,
    query: ReadQuery,
    limits: ResourceOptions,
    format: Format,
    output: Option<PathBuf>,
) -> Result<ExitCode> {
    let budget = limits.budget()?;
    let signals = Signals::new()?;
    let application = limits.application()?;
    let _diagnostics = TemporaryDiagnostics(&application, format);
    let handle = application.start_query(&project, query, budget)?;
    if !wait_handle(&handle, &signals, format) {
        return Ok(ExitCode::FAILURE);
    }
    let mut result = handle.take_output()?;
    if let Some(path) = output {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
        queries::render(&mut result, Format::Json, file.as_file_mut(), &|| {
            signals.cancelled()
        })?;
        file.as_file().sync_all().map_err(io_error)?;
        file.persist_noclobber(path)
            .map_err(|e| io_error(e.error))?;
    } else {
        queries::render(&mut result, format, &mut std::io::stdout().lock(), &|| {
            signals.cancelled()
        })?;
    }
    Ok(if result.assessment().check_passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn render_run(run: &blobray_application::RunRecord) -> String {
    use std::fmt::Write;
    let mut output = format!(
        "Run {}: {:?}; mode={:?}; revision={}",
        run.id,
        run.state,
        run.budget.mode,
        run.revision
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "none".into())
    );
    if let Some(error) = &run.error {
        let _ = write!(output, "\n{:?}: {}", error.code, error.message);
    }
    if let Some(diagnostics) = &run.diagnostics {
        render_diagnostics(&mut output, diagnostics, run.budget.max_work_units);
    }
    output
}

fn render_diagnostics(
    output: &mut String,
    diagnostics: &blobray_domain::RunDiagnostics,
    work_limit: Option<u64>,
) {
    use std::fmt::Write;
    if let Some(progress) = diagnostics.progress {
        let _ = write!(
            output,
            "\nLast checkpoint: {:?}; input={:?} member={:?} table={:?} entry={:?}; work={}/{}; elapsed={} ms",
            progress.position.phase,
            progress.position.input,
            progress.position.member,
            progress.position.table,
            progress.position.entry,
            progress.work_used,
            work_limit
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unknown".into()),
            progress.elapsed_ms
        );
        if let Some(memory) = progress.working_memory {
            let _ = write!(
                output,
                "\nWorking memory: peak reserved {} / {} bytes; live {} bytes",
                memory.peak_reserved_bytes, memory.limit_bytes, memory.reserved_bytes
            );
        }
        if let Some(disk) = progress.temporary_storage {
            let _ = write!(
                output,
                "\nTemporary storage: peak {} / {} bytes; current {}; control reserve {}",
                disk.peak_bytes, disk.limit_bytes, disk.current_bytes, disk.control_reserved_bytes
            );
        }
        if let Some(stop) = progress.stop {
            let _ = write!(
                output,
                "; stop={:?}; requested={:?}",
                stop.reason, stop.requested_units
            );
        }
    }
    if let Some(memory) = diagnostics.memory {
        let _ = write!(
            output,
            "\nMemory peak: {} bytes ({:?})",
            memory.peak_bytes, memory.source
        );
    }
    if let Some(exit) = diagnostics.exit {
        let _ = write!(
            output,
            "\nWorker exit: code={:?} signal={:?} cgroup_oom={}",
            exit.code, exit.signal, exit.cgroup_oom
        );
    }
    for error in &diagnostics.secondary {
        let _ = write!(output, "\nSecondary {:?}: {}", error.code, error.message);
    }
    if diagnostics.secondary_truncated {
        output.push_str("\nAdditional secondary diagnostics truncated");
    }
    if !diagnostics.stderr_tail.is_empty() {
        let _ = write!(
            output,
            "\nWorker stderr tail (truncated={}):\n{}",
            diagnostics.stderr_truncated,
            String::from_utf8_lossy(&diagnostics.stderr_tail).escape_debug()
        );
    }
}

fn internal(args: &[OsString]) -> Result<()> {
    let stage = args
        .get(2)
        .map(PathBuf::from)
        .ok_or_else(|| invalid("missing worker staging path"))?;
    if args[1] == "__guard" {
        #[cfg(target_os = "linux")]
        return blobray_next_host::linux::run_guard(&stage);
        #[cfg(not(target_os = "linux"))]
        return Err(Error::new(
            ErrorCode::Unavailable,
            "Linux guard unavailable",
        ));
    }
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("lease.lock"))
        .map_err(blobray_domain::storage_io)?;
    lease.lock_shared().map_err(blobray_domain::storage_io)?;
    fn request<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(blobray_domain::storage_io)?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(blobray_domain::storage_io)?;
        if bytes.len() > 65536 {
            return Err(invalid("worker request exceeds 64 KiB"));
        }
        serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))
    }
    let scenario: Option<app::ScenarioWork> = if stage.join("scenario.json").exists() {
        Some(request(&stage.join("scenario.json"))?)
    } else {
        None
    };
    let execution: Option<app::ExecutionWork> = if stage.join("execution.json").exists() {
        Some(request(&stage.join("execution.json"))?)
    } else {
        None
    };
    let query: Option<QueryWork> = if stage.join("query.json").exists() {
        Some(request(&stage.join("query.json"))?)
    } else {
        None
    };
    let image: Option<app::ImageWork> = if stage.join("image.json").exists() {
        Some(request(&stage.join("image.json"))?)
    } else {
        None
    };
    let function: Option<app::FunctionWork> = if stage.join("function.json").exists() {
        Some(request(&stage.join("function.json"))?)
    } else {
        None
    };
    let investigation: Option<app::InvestigationWork> = if stage.join("investigation.json").exists()
    {
        Some(request(&stage.join("investigation.json"))?)
    } else {
        None
    };
    let knowledge: Option<app::KnowledgeWork> = if stage.join("knowledge.json").exists() {
        Some(request(&stage.join("knowledge.json"))?)
    } else {
        None
    };
    let import: Option<ImportWork> = if query.is_none()
        && image.is_none()
        && function.is_none()
        && investigation.is_none()
        && knowledge.is_none()
        && execution.is_none()
        && scenario.is_none()
    {
        Some(request(&stage.join("request.json"))?)
    } else {
        None
    };
    let (run, started, deadline, budget) = if let Some(w) = &scenario {
        (&w.run, w.started_ms, w.deadline_ms, &w.budget)
    } else if let Some(w) = &execution {
        (&w.run, w.started_ms, w.deadline_ms, &w.budget)
    } else {
        match (
            &query,
            &import,
            &image,
            &function,
            &investigation,
            &knowledge,
        ) {
            (Some(work), _, _, _, _, _) => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            (_, Some(work), _, _, _, _) if work.schema == 2 => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            (_, _, Some(work), _, _, _) if work.schema == 1 => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            (_, _, _, Some(work), _, _) if work.schema == 1 => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            (_, _, _, _, Some(work), _) if work.schema == 1 => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            (_, _, _, _, _, Some(work)) if work.schema == 1 => {
                (&work.run, work.started_ms, work.deadline_ms, &work.budget)
            }
            _ => return Err(invalid("unsupported worker protocol")),
        }
    };
    let environment = blobray_next_host::linux::WorkerEnvironment::new(&stage, run.clone())?;
    let mut context = app::RunContext::new(&environment, started, deadline, budget, None)?;
    let mut linker_diagnostics = None;
    let result = if let Some(work) = scenario {
        app::prepare_scenario_worker(
            &stage,
            &work,
            &blobray_backend_riscv::RiscvDecoder,
            &blobray_backend_riscv::RiscvExecutor,
            &mut context,
        )
        .map(Some)
    } else if let Some(work) = execution {
        app::prepare_execution_worker(
            &stage,
            &work,
            &blobray_backend_riscv::RiscvExecutor,
            &mut context,
        )
        .map(|p| Some(app::PreparedReceipt::Execution(p)))
    } else {
        match (query, import, image, function, investigation, knowledge) {
            (Some(work), _, _, _, _, _) => app::prepare_query_with_tools(
                &stage,
                &work,
                &mut context,
                &blobray_next_host::linux::Lld22,
                Some(&blobray_backend_riscv::RiscvDecoder),
            )
            .map(|_| None),
            (_, Some(work), _, _, _, _) => app::prepare_import(&stage, work, &mut context)
                .map(|p| Some(app::PreparedReceipt::Import(p))),
            (_, _, Some(work), _, _, _) => app::prepare_image_worker(
                &stage,
                &work,
                &blobray_next_host::linux::Lld22,
                &mut context,
                &mut linker_diagnostics,
            )
            .map(|p| Some(app::PreparedReceipt::Image(p))),
            (_, _, _, Some(work), _, _) => app::prepare_function_worker(
                &stage,
                &work,
                &blobray_backend_riscv::RiscvDecoder,
                &mut context,
            )
            .map(|p| Some(app::PreparedReceipt::Function(p))),
            (_, _, _, _, Some(work), _) => app::prepare_investigation_worker(
                &stage,
                &work,
                &blobray_backend_riscv::RiscvDecoder,
                &mut context,
            )
            .map(|p| Some(app::PreparedReceipt::Investigation(p))),
            (_, _, _, _, _, Some(work)) => {
                app::prepare_knowledge_worker(&stage, &work, &mut context)
                    .map(|p| Some(app::PreparedReceipt::Knowledge(p)))
            }
            _ => unreachable!(),
        }
    };
    let mut diagnostics = blobray_domain::RunDiagnostics {
        linker: linker_diagnostics,
        progress: Some(context.snapshot()),
        ..Default::default()
    };
    let observed = context
        .flush()
        .and_then(|()| environment.finish(&context.snapshot()));
    diagnostics.progress = Some(context.snapshot());
    let result = match (result, observed) {
        (Err(error), Err(secondary)) => {
            diagnostics.secondary(secondary);
            Err(error)
        }
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
    };
    let report = match result {
        Ok(prepared) => WorkerReport {
            schema: 6,
            diagnostics,
            state: RunState::Completed,
            prepared,
            error: None,
        },
        Err(mut error) => {
            blobray_domain::truncate_message(&mut error.message);
            let state = match error.code {
                ErrorCode::ResourceLimited => RunState::ResourceLimited,
                ErrorCode::TimedOut => RunState::TimedOut,
                ErrorCode::Cancelled => RunState::Cancelled,
                _ => RunState::Failed,
            };
            WorkerReport {
                schema: 6,
                diagnostics,
                state,
                prepared: None,
                error: Some(error),
            }
        }
    };
    let file = std::fs::File::create(stage.join("worker-report.json"))
        .map_err(blobray_domain::storage_io)?;
    app::write_control_message(file, &report)
}

fn parse_address(text: &str) -> std::result::Result<u32, String> {
    if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|e| e.to_string())
}

fn apply_native_knowledge(
    project: &std::path::Path,
    change: &blobray_domain::KnowledgeChange,
    limits: &ResourceOptions,
    format: Format,
    budget: ResourceBudget,
) -> Result<ExitCode> {
    let application = limits.application()?;
    let _diagnostics = TemporaryDiagnostics(&application, format);
    let signals = Signals::new()?;
    let handle = application.start_knowledge(project, change, budget)?;
    if !wait_handle(&handle, &signals, format) {
        return Ok(ExitCode::FAILURE);
    }
    let run = handle.wait();
    match format {
        Format::Json => println!("{}", serde_json::json!({"schema":6,"run":run})),
        Format::Human => println!("Knowledge revision {}", run.knowledge.unwrap()),
    }
    Ok(ExitCode::SUCCESS)
}

fn export_data_query(
    project: PathBuf,
    query: ReadQuery,
    output: PathBuf,
    limits: ResourceOptions,
    format: Format,
) -> Result<ExitCode> {
    let application = limits.application()?;
    let _diagnostics = TemporaryDiagnostics(&application, format);
    let signals = Signals::new()?;
    let handle = application.start_query(&project, query, limits.budget()?)?;
    if !wait_handle(&handle, &signals, format) {
        return Ok(ExitCode::FAILURE);
    }
    let mut result = handle.take_output()?;
    result.export_data(&output, &|| signals.cancelled())?;
    match format {
        Format::Json => println!(
            "{}",
            serde_json::json!({"schema":1,"summary":result.summary(),"output":output})
        ),
        Format::Human => println!("Data exported to {}", output.display()),
    }
    Ok(ExitCode::SUCCESS)
}

fn propose_data_command(
    project: PathBuf,
    request: PathBuf,
    limits: ResourceOptions,
    format: Format,
    constant: bool,
) -> Result<ExitCode> {
    let application = limits.application()?;
    let _diagnostics = TemporaryDiagnostics(&application, format);
    let signals = Signals::new()?;
    let handle = if constant {
        application.start_propose_constant(&project, read_json_file(&request)?, limits.budget()?)?
    } else {
        application.start_propose_data(&project, read_json_file(&request)?, limits.budget()?)?
    };
    if !wait_handle(&handle, &signals, format) {
        return Ok(ExitCode::FAILURE);
    }
    let run = handle.wait();
    match format {
        Format::Json => println!("{}", serde_json::json!({"schema":6,"run":run})),
        Format::Human => println!("Knowledge revision {}", run.knowledge.unwrap()),
    }
    Ok(ExitCode::SUCCESS)
}
