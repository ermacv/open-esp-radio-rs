//! Declarative leaf-command arguments.

use std::{path::PathBuf, str::FromStr};

use clap::{Args, ValueEnum};

use super::{NamedAddressRange, ProjectInputBinding, SourcePath, SourceValue};
use crate::source_id::SourceId;

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct EmptyArgs {}

#[derive(Clone, Debug, Args)]
pub(crate) struct ProjectInitArgs {
    /// Directory in which the new project workspace is created.
    #[arg(long)]
    pub(crate) directory: PathBuf,
    /// Stable project identifier.
    #[arg(long)]
    pub(crate) id: String,
    /// Named half-open MMIO region; repeat for every region.
    #[arg(long, value_name = "NAME=START..END", required = true)]
    pub(crate) mmio: Vec<NamedAddressRange>,
    /// Stable vendor source identifier; repeat for multiple sources.
    #[arg(long)]
    pub(crate) source: Vec<SourceId>,
    /// Rust compilation target used by generated project artifacts.
    #[arg(long)]
    pub(crate) rust_target: Option<String>,
    /// Crate name assigned to the generated internal raw PAC.
    #[arg(long)]
    pub(crate) pac_raw_crate_name: Option<String>,
    /// Existing SVD to import into the initial register workspace.
    #[arg(long)]
    pub(crate) import_svd: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ProjectConfigureArgs {
    /// Attach a reusable platform pack to the project.
    #[arg(long, conflicts_with = "no_platform_pack")]
    pub(crate) platform_pack: Option<PathBuf>,
    /// Remove the configured platform pack.
    #[arg(long, conflicts_with = "platform_pack")]
    pub(crate) no_platform_pack: bool,
    /// Verify that the manifest already contains the requested configuration.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct ProjectInputsInitArgs {
    /// Bind one run-spec role to a caller-owned artifact path.
    #[arg(long, value_name = "ROLE=PATH", required = true)]
    pub(crate) bind: Vec<ProjectInputBinding>,
    /// Local run-spec to create; defaults to local.toml next to the project manifest.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Verify that the existing local run-spec matches the requested bindings.
    #[arg(long, conflicts_with = "force")]
    pub(crate) check: bool,
    /// Replace an existing local run-spec.
    #[arg(long, conflicts_with = "check")]
    pub(crate) force: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ProjectStatusArgs {
    /// Fail unless the generated JSON report is already current.
    #[arg(long, requires = "output")]
    pub(crate) check: bool,
    /// Write the structured project status report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Return failure when any required project component is incomplete.
    #[arg(long)]
    pub(crate) deny_incomplete: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ProjectAnalyzeArgs {
    /// Reproduce and compare every configured output without changing files.
    #[arg(long)]
    pub(crate) check: bool,
    /// Treat unreviewed generated material as a pipeline failure.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
    /// Worker threads for independent analysis.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8), value_name = "N")]
    pub(crate) jobs: u8,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ProjectVerifyArgs {
    /// Run only the named verification suite; repeat to select multiple suites.
    #[arg(long)]
    pub(crate) suite: Vec<String>,
    /// Reproduce the aggregate report without changing the checked-in file.
    #[arg(long)]
    pub(crate) check: bool,
    /// Write one review-only evidence candidate per selected suite.
    #[arg(long, value_name = "DIRECTORY")]
    pub(crate) candidate_evidence_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ProjectCheckArgs {
    /// Treat unreviewed generated material as a check failure.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
    /// Worker threads for independent analysis.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8), value_name = "N")]
    pub(crate) jobs: u8,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct CheckArgs {
    /// Compare generated output with the checked-in file without changing it.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct OutputArgs {
    /// Output path; project configuration supplies it when omitted.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ValidationArgs {
    /// Reject syntactically valid packs that still contain unreviewed entries.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ReviewArgs {
    /// Write the generated review workspace or candidate.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Verify the existing output without modifying it.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct CodeRebaseArgs {
    /// Fail unless the reviewed pack already matches the generated facts.
    #[arg(long, conflicts_with_all = ["output", "apply"])]
    pub(crate) check: bool,
    /// Write a rebased review candidate without replacing the configured pack.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["check", "apply"])]
    pub(crate) output: Option<PathBuf>,
    /// Atomically update the configured pack when every reviewed boundary remains valid.
    #[arg(long, conflicts_with_all = ["check", "output"])]
    pub(crate) apply: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct SymbolInventoryArgs {
    /// Verify that the configured report is current without changing it.
    #[arg(long)]
    pub(crate) check: bool,
    /// Write the machine-readable symbol inventory.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Restrict symbols to this name prefix.
    #[arg(long)]
    pub(crate) name_prefix: Option<String>,
    /// Report only undefined symbols.
    #[arg(long)]
    pub(crate) undefined_only: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InterfaceDiscoverArgs {
    /// Verify that the configured report is current.
    #[arg(long)]
    pub(crate) check: bool,
    /// Write the machine-readable interface discovery report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Restrict discovered symbols to this prefix.
    #[arg(long, default_value = "")]
    pub(crate) name_prefix: String,
    /// Restrict discovery to a run-spec source identifier.
    #[arg(long)]
    pub(crate) source: Vec<String>,
    /// Report only function-pointer tables.
    #[arg(long)]
    pub(crate) tables_only: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RegisterModelArgs {
    /// Register model to create.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Address-space identifier recorded in the model.
    #[arg(long)]
    pub(crate) address_space: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct RegisterImportArgs {
    /// Source SVD document.
    #[arg(long)]
    pub(crate) input: PathBuf,
    /// Register model to create from the SVD.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Address-space identifier recorded in the model.
    #[arg(long)]
    pub(crate) address_space: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RegisterReviewArgs {
    /// Write the register review workspace.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Linked-IR report used as register-field evidence; repeat as needed.
    #[arg(long, conflicts_with = "no_ir_reports")]
    pub(crate) ir_report: Vec<PathBuf>,
    /// Do not load linked-IR evidence configured by the project.
    #[arg(long, conflicts_with = "ir_report")]
    pub(crate) no_ir_reports: bool,
    /// Verify the existing review output without modifying it.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RegisterExportArgs {
    /// SVD document to generate.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Reject register models containing unreviewed entries.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
    /// Verify the existing SVD without modifying it.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RegisterPacRawArgs {
    /// Directory in which the PAC crate is generated.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Rust target used to select generated PAC details.
    #[arg(long)]
    pub(crate) target: Option<String>,
    /// Rust edition for the generated PAC crate.
    #[arg(long)]
    pub(crate) edition: Option<String>,
    /// Reviewed safe API pack applied above raw register accessors.
    #[arg(long, conflicts_with = "no_api_pack")]
    pub(crate) api_pack: Option<PathBuf>,
    /// Generate the PAC without an API pack.
    #[arg(long, conflicts_with = "api_pack")]
    pub(crate) no_api_pack: bool,
    /// Verify the generated crate without modifying it.
    #[arg(long)]
    pub(crate) check: bool,
    /// Reject register or API models containing unreviewed entries.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct RegisterBindingsArgs {
    /// Rust bindings file to generate.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// PAC crate path used in generated bindings.
    #[arg(long)]
    pub(crate) crate_name: Option<String>,
    /// Verify the existing bindings without modifying them.
    #[arg(long)]
    pub(crate) check: bool,
    /// Reject register models containing unreviewed entries.
    #[arg(long)]
    pub(crate) deny_unreviewed: bool,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct ImageAuditArgs {
    /// Final ELF image to audit.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Named address range that direct control flow must not target.
    #[arg(long, value_name = "NAME=START..END")]
    pub(crate) forbid: Vec<NamedAddressRange>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum CodeSymbolSelectionArg {
    /// Analyze every named, sized function symbol, including local functions.
    #[default]
    All,
    /// Analyze only global and weak function definitions.
    Exported,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct MmioDiscoverArgs {
    /// Named vendor artifact; repeat for multi-source analysis.
    #[arg(long, value_name = "SOURCE=PATH")]
    pub(crate) artifact: Vec<SourcePath>,
    /// Named half-open address range to classify.
    #[arg(long, value_name = "NAME=START..END")]
    pub(crate) range: Vec<NamedAddressRange>,
    /// Restrict analyzed functions to this symbol prefix.
    #[arg(long, default_value = "")]
    pub(crate) symbol_prefix: String,
    /// Select which matching function symbols become MMIO analysis roots.
    #[arg(long, value_enum, default_value_t)]
    pub(crate) code_symbols: CodeSymbolSelectionArg,
    /// Worker threads used for independent function analysis.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8), value_name = "N")]
    pub(crate) jobs: u8,
    /// Write the machine-readable MMIO discovery report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Verify the existing report without modifying it.
    #[arg(long)]
    pub(crate) check: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct IrExportArgs {
    /// Named vendor artifact; repeat for a linked multi-source IR.
    #[arg(long, value_name = "SOURCE=PATH")]
    pub(crate) artifact: Vec<SourcePath>,
    /// Companion image used to resolve symbols for a single primary artifact.
    #[arg(long)]
    pub(crate) companion: Vec<PathBuf>,
    /// Restrict IR roots to this symbol prefix.
    #[arg(long, default_value = "")]
    pub(crate) symbol_prefix: String,
    /// Include functions reachable from the selected roots.
    #[arg(long)]
    pub(crate) include_reachable: bool,
    /// Worker threads for artifact-wide independent functions.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8), value_name = "N")]
    pub(crate) jobs: u8,
    /// Registered entry contract used at the binary boundary.
    #[arg(long, default_value = "none")]
    pub(crate) entry_contract: String,
    /// Write a best-effort pseudo-Rust rendering.
    #[arg(long)]
    pub(crate) pseudo_rust: Option<PathBuf>,
    /// Write the machine-readable linked-IR report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct IrBuildArgs {
    /// Build only the named project IR profile; repeat as needed.
    #[arg(long)]
    pub(crate) profile: Vec<String>,
    /// Verify all selected profile outputs without modifying them.
    #[arg(long)]
    pub(crate) check: bool,
    /// Worker threads for independent function-local analysis.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8), value_name = "N")]
    pub(crate) jobs: u8,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct TraceInputArgs {
    /// Artifact containing the function to inspect.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Archive member containing the function.
    #[arg(long)]
    pub(crate) member: Option<String>,
    /// Function symbol to inspect.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectCompareArgs {
    /// Left-hand artifact.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Left-hand archive member.
    #[arg(long)]
    pub(crate) member: Option<String>,
    /// Left-hand function symbol.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// Right-hand artifact.
    #[arg(long)]
    pub(crate) right_artifact: Option<PathBuf>,
    /// Right-hand archive member.
    #[arg(long)]
    pub(crate) right_member: Option<String>,
    /// Right-hand function symbol.
    #[arg(long)]
    pub(crate) right_symbol: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectAnalyzeArgs {
    /// Primary artifact to analyze.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) companion: Vec<PathBuf>,
    /// Restrict analyzed functions to this symbol prefix.
    #[arg(long)]
    pub(crate) symbol_prefix: String,
    /// Registered entry contract used at the binary boundary.
    #[arg(long, default_value = "none")]
    pub(crate) entry_contract: String,
    /// Write the machine-readable analysis report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectFunctionArgs {
    /// Project source and exact function symbol (`SOURCE:SYMBOL`).
    #[arg(value_name = "SOURCE:SYMBOL")]
    pub(crate) selector: String,
    /// Authoritative linked image for the selected source.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Raw archive used as source inventory and origin evidence.
    #[arg(long)]
    pub(crate) inventory: Option<PathBuf>,
    /// Member containing the runtime function when the primary artifact is an archive.
    #[arg(long)]
    pub(crate) member: Option<String>,
    /// Explicit origin archive member; normally recovered from symbol inventory.
    #[arg(long)]
    pub(crate) origin_member: Option<String>,
    /// Include the complete CFG and lossless instruction listing in human output.
    #[arg(long)]
    pub(crate) full: bool,
    /// Number of inter-function graph hops included in the focused report.
    #[arg(long, default_value_t = 2)]
    pub(crate) depth: usize,
    /// Include incoming callers as well as outgoing callees in the graph slice.
    #[arg(long)]
    pub(crate) callers: bool,
    /// Show one shortest structural CFG path (`FROM:TO`); use `+OFFSET` for function offsets.
    #[arg(long, value_name = "FROM:TO")]
    pub(crate) path: Option<String>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectFlowArgs {
    /// Project source and root function (`SOURCE:SYMBOL`).
    #[arg(value_name = "SOURCE:SYMBOL")]
    pub(crate) selector: String,
    /// Stop at a function identity or symbol.
    #[arg(long, value_name = "[SOURCE::]SYMBOL")]
    pub(crate) to_function: Option<String>,
    /// Stop at the first function accessing this reviewed register name.
    #[arg(long, value_name = "REGISTER")]
    pub(crate) to_register: Option<String>,
    /// Stop at the first function accessing this MMIO address.
    #[arg(long, value_name = "ADDRESS")]
    pub(crate) to_address: Option<String>,
    /// Maximum number of inter-function edges to explore.
    #[arg(long, default_value_t = 12)]
    pub(crate) max_depth: usize,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectObjectArgs {
    /// Project source and exact data-object symbol (`SOURCE:SYMBOL`).
    #[arg(value_name = "SOURCE:SYMBOL")]
    pub(crate) selector: String,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct InspectScopeArgs {
    /// Exact project review-scope ID.
    #[arg(value_name = "SCOPE")]
    pub(crate) scope: String,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ReferenceArgs {
    /// Artifact containing the reference function.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) companion: Vec<PathBuf>,
    /// Archive member containing the function.
    #[arg(long)]
    pub(crate) member: Option<String>,
    /// Function symbol to reconstruct.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// Rust reference source to generate.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Registered entry contract used at the binary boundary.
    #[arg(long, default_value = "none")]
    pub(crate) entry_contract: String,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ReferenceBatchArgs {
    /// Artifact whose eligible functions are reconstructed.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) companion: Vec<PathBuf>,
    /// Restrict generated functions to this symbol prefix.
    #[arg(long)]
    pub(crate) symbol_prefix: String,
    /// Prefix applied to generated Rust probe symbols.
    #[arg(long)]
    pub(crate) probe_prefix: Option<String>,
    /// Stable source identifier embedded in generated metadata.
    #[arg(long)]
    pub(crate) source_name: Option<String>,
    /// Registered entry contract used at the binary boundary.
    #[arg(long, default_value = "none")]
    pub(crate) entry_contract: String,
    /// Directory receiving generated reference sources.
    #[arg(long)]
    pub(crate) output_dir: Option<PathBuf>,
    /// Manifest recording generated and blocked candidates.
    #[arg(long)]
    pub(crate) manifest: Option<PathBuf>,
    /// Replace existing generated candidate files.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct DriverGenerateArgs {
    /// Artifact containing the function to translate.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) companion: Vec<PathBuf>,
    /// Archive member containing the function.
    #[arg(long)]
    pub(crate) member: Option<String>,
    /// Function symbol to translate.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// Stable vendor source identifier used by dispositions.
    #[arg(long)]
    pub(crate) source: Option<String>,
    /// Reviewed disposition manifest controlling replacements.
    #[arg(long)]
    pub(crate) dispositions: Option<PathBuf>,
    /// Generated safe PAC bindings used for MMIO operations.
    #[arg(long)]
    pub(crate) pac_bindings: Option<PathBuf>,
    /// Driver candidate kind selected by the generator.
    #[arg(long)]
    pub(crate) kind: Option<String>,
    /// Rust driver candidate to generate.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Machine-readable generation plan to write.
    #[arg(long)]
    pub(crate) plan_output: Option<PathBuf>,
    /// Registered entry contract used at the binary boundary.
    #[arg(long, default_value = "none")]
    pub(crate) entry_contract: String,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ScenarioArgs {
    /// Entry argument value; repeat in ABI argument order.
    #[arg(long)]
    pub(crate) arg: Vec<String>,
    /// Initial MMIO word as ADDR=VALUE.
    #[arg(long)]
    pub(crate) mmio: Vec<String>,
    /// Scripted MMIO read as ADDR=VALUE; repeat values in read order.
    #[arg(long)]
    pub(crate) read: Vec<String>,
    /// Initial RAM word as ADDR=VALUE.
    #[arg(long)]
    pub(crate) ram: Vec<String>,
    /// RAM observation window as ADDR=LENGTH.
    #[arg(long)]
    pub(crate) observe: Vec<String>,
    /// Maximum number of instructions executed by the scenario.
    #[arg(long)]
    pub(crate) max_steps: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct CaseArgs {
    pub(crate) name: String,
    pub(crate) scenario: ScenarioArgs,
    pub(crate) vendor_ram_symbol: Vec<String>,
    pub(crate) rust_ram_symbol: Vec<String>,
}

impl FromStr for CaseArgs {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut clauses = value.split(';');
        let name = clauses.next().unwrap_or_default();
        if name.is_empty() {
            return Err("case requires NAME[;KEY=VALUE...]".to_owned());
        }
        let mut case = Self {
            name: name.to_owned(),
            scenario: ScenarioArgs::default(),
            vendor_ram_symbol: Vec::new(),
            rust_ram_symbol: Vec::new(),
        };
        for clause in clauses {
            let (key, value) = clause
                .split_once('=')
                .filter(|(key, value)| !key.is_empty() && !value.is_empty())
                .ok_or_else(|| format!("invalid case clause {clause:?}; expected KEY=VALUE"))?;
            match key {
                "arg" => case.scenario.arg.push(value.to_owned()),
                "mmio" => case.scenario.mmio.push(value.to_owned()),
                "read" => case.scenario.read.push(value.to_owned()),
                "ram" => case.scenario.ram.push(value.to_owned()),
                "observe" => case.scenario.observe.push(value.to_owned()),
                "max-steps" if case.scenario.max_steps.is_none() => {
                    case.scenario.max_steps = Some(
                        value
                            .parse()
                            .map_err(|error| format!("invalid max-steps in case: {error}"))?,
                    );
                }
                "max-steps" => return Err("duplicate max-steps in case".to_owned()),
                "vendor-ram-symbol" => case.vendor_ram_symbol.push(value.to_owned()),
                "rust-ram-symbol" => case.rust_ram_symbol.push(value.to_owned()),
                _ => return Err(format!("unknown case key {key:?}")),
            }
        }
        Ok(case)
    }
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ExecuteRunArgs {
    /// Executable artifact containing the entry function.
    #[arg(long)]
    pub(crate) artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) companion: Option<PathBuf>,
    /// Function symbol to execute.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// Skip static coverage inventory and execute only the concrete scenario.
    #[arg(long)]
    pub(crate) concrete_only: bool,
    /// Print the complete execution timeline.
    #[arg(long)]
    pub(crate) timeline: bool,
    /// Byte used to initialize the private execution stack.
    #[arg(long)]
    pub(crate) stack_fill: Option<String>,
    /// Stubbed external return as SYMBOL=VALUE; repeat for successive calls.
    #[arg(long)]
    pub(crate) call: Vec<String>,
    #[command(flatten)]
    pub(crate) scenario: ScenarioArgs,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct ExecuteCompareArgs {
    /// Vendor executable containing the reference function.
    #[arg(long)]
    pub(crate) vendor_artifact: Option<PathBuf>,
    /// Companion image for the vendor executable.
    #[arg(long)]
    pub(crate) vendor_companion: Option<PathBuf>,
    /// Vendor function symbol.
    #[arg(long)]
    pub(crate) vendor_symbol: Option<String>,
    /// Rust executable containing the candidate function.
    #[arg(long)]
    pub(crate) rust_artifact: Option<PathBuf>,
    /// Companion image for the Rust executable.
    #[arg(long)]
    pub(crate) rust_companion: Option<PathBuf>,
    /// Rust function symbol.
    #[arg(long)]
    pub(crate) rust_symbol: Option<String>,
    /// Require the return values to match as well as observable effects.
    #[arg(long)]
    pub(crate) compare_return: bool,
    /// Repeated complete scenario: NAME[;arg=V][;mmio=ADDR=V][;read=ADDR=V]...
    #[arg(long, value_name = "SCENARIO")]
    pub(crate) case: Vec<CaseArgs>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct VerifyProfilesArgs {
    /// Execution profile manifest.
    #[arg(long)]
    pub(crate) profiles: Option<PathBuf>,
    /// Vendor executable tested by every selected profile.
    #[arg(long)]
    pub(crate) vendor_artifact: Option<PathBuf>,
    /// Companion image for the vendor executable.
    #[arg(long)]
    pub(crate) vendor_companion: Option<PathBuf>,
    /// Rust executable tested by every selected profile.
    #[arg(long)]
    pub(crate) rust_artifact: Option<PathBuf>,
    /// Companion image for the Rust executable.
    #[arg(long)]
    pub(crate) rust_companion: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct VerifySourceArgs {
    /// Vendor executable used as linker truth.
    #[arg(long)]
    pub(crate) vendor_artifact: Option<PathBuf>,
    /// Optional vendor archive used for symbol inventory coverage.
    #[arg(long)]
    pub(crate) vendor_inventory: Option<PathBuf>,
    /// Companion image for the vendor executable.
    #[arg(long)]
    pub(crate) vendor_companion: Option<PathBuf>,
    /// Rust executable containing generated candidates.
    #[arg(long)]
    pub(crate) rust_artifact: Option<PathBuf>,
    /// Companion image for the Rust executable.
    #[arg(long)]
    pub(crate) rust_companion: Option<PathBuf>,
    /// Execution profile manifest.
    #[arg(long)]
    pub(crate) profiles: Option<PathBuf>,
    /// Vendor symbol prefix included in source coverage.
    #[arg(long)]
    pub(crate) vendor_prefix: String,
    /// Rust symbol prefix mapped to vendor functions.
    #[arg(long)]
    pub(crate) rust_prefix: Option<String>,
    /// Verification gate defined by the profile manifest.
    #[arg(long, default_value = "completion")]
    pub(crate) gate: String,
    /// Optional minimum matched-function count.
    #[arg(long)]
    pub(crate) match_floor: Option<usize>,
    /// Reviewed evidence baseline for regression gating.
    #[arg(long)]
    pub(crate) evidence_baseline: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct VerifyInventoryArgs {
    /// Vendor executable keyed by stable source identifier.
    #[arg(long, value_name = "SOURCE=PATH")]
    pub(crate) source_artifact: Vec<SourcePath>,
    /// Vendor archive keyed by stable source identifier.
    #[arg(long, value_name = "SOURCE=PATH")]
    pub(crate) source_inventory: Vec<SourcePath>,
    /// Companion image keyed by stable source identifier.
    #[arg(long, value_name = "SOURCE=PATH")]
    pub(crate) source_companion: Vec<SourcePath>,
    /// Per-source symbol prefix override.
    #[arg(long, value_name = "SOURCE=PREFIX")]
    pub(crate) source_prefix: Vec<SourceValue>,
    /// Rust executable containing generated candidates.
    #[arg(long)]
    pub(crate) rust_artifact: Option<PathBuf>,
    /// Companion image for the Rust executable.
    #[arg(long)]
    pub(crate) rust_companion: Option<PathBuf>,
    /// Execution profile manifest.
    #[arg(long)]
    pub(crate) profiles: Vec<PathBuf>,
    /// Reviewed architectural disposition manifest.
    #[arg(long)]
    pub(crate) dispositions: Vec<PathBuf>,
    /// Rust symbol prefix mapped to vendor functions.
    #[arg(long)]
    pub(crate) rust_prefix: Option<String>,
    /// Verification gate defined by the profile manifest.
    #[arg(long, default_value = "completion")]
    pub(crate) gate: String,
    /// Optional minimum matched-function count.
    #[arg(long)]
    pub(crate) match_floor: Option<usize>,
    /// Reviewed evidence baseline for regression gating.
    #[arg(long)]
    pub(crate) evidence_baseline: Vec<PathBuf>,
    /// Write the machine-readable verification report.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct VerifyEvidenceArgs {
    /// Verification report whose evidence is reviewed.
    #[arg(long)]
    pub(crate) report: Option<PathBuf>,
    /// Reviewed evidence baseline used for comparison.
    #[arg(long)]
    pub(crate) evidence_baseline: Option<PathBuf>,
    /// Write a deterministic candidate baseline without promoting it.
    #[arg(long)]
    pub(crate) candidate: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct VerifyContractArgs {
    /// Vendor executable containing the contract entry point.
    #[arg(long)]
    pub(crate) vendor_artifact: Option<PathBuf>,
    /// Companion image used for symbol and call resolution.
    #[arg(long)]
    pub(crate) vendor_companion: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_compare_cases_preserve_repeated_typed_clauses() {
        let case: CaseArgs = "enabled;arg=1;mmio=0x10=0x20;mmio=0x14=0x24;max-steps=42"
            .parse()
            .unwrap();
        assert_eq!(case.name, "enabled");
        assert_eq!(case.scenario.arg, ["1"]);
        assert_eq!(case.scenario.mmio, ["0x10=0x20", "0x14=0x24"]);
        assert_eq!(case.scenario.max_steps, Some(42));
    }

    #[test]
    fn malformed_compare_case_is_rejected_by_clap_value_parsing() {
        assert!("enabled;unknown=value".parse::<CaseArgs>().is_err());
        assert!("enabled;max-steps=one".parse::<CaseArgs>().is_err());
    }
}
