//! Command-line adapter for Blobray operations that run in this process.

use blobray_application as app;
use blobray_domain::CheckVerdict;
use clap::{Parser, Subcommand, ValueEnum};
use oer_riscv_model::{
    CallAbi, DEFAULT_WORKING_BYTES, Error, ErrorCode, FunctionDecoder, FunctionSemantics, Result,
};
use std::io::Write;
use std::{ffi::OsString, path::PathBuf, process::ExitCode};
mod command_tree;

#[derive(Clone, Copy, Default, ValueEnum)]
enum Format {
    #[default]
    Human,
    Json,
}

/// The calling convention a library analysis may assume.
#[derive(Clone, Copy, ValueEnum)]
enum AbiArg {
    /// The RISC-V integer calling convention: a call keeps sp, gp, tp and
    /// s0-s11.
    RiscvInteger,
}

/// Explicit assumptions of a library analysis; none by default.
#[derive(clap::Args)]
struct Assumptions {
    /// Assume this calling convention at every call the analysis cannot
    /// follow; without it, every register is unknown after such a call and
    /// an access through a preserved register loses its address.
    #[arg(long, value_enum)]
    abi: Option<AbiArg>,
}

impl Assumptions {
    fn abi(&self) -> Option<CallAbi> {
        self.abi.map(|AbiArg::RiscvInteger| CallAbi::RiscvInteger)
    }
}

#[derive(Parser)]
#[command(
    name = "blobray",
    about = "Captured binary research inside this process: final-image target audits, library register accesses and function records"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value = "human")]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Audit all executable ELF sections for statically resolved forbidden transfers.
    AuditTargets {
        #[arg(long)]
        artifact: PathBuf,
        /// NAME=START..END (half-open RV32 address range).
        #[arg(long, required=true, value_parser=parse_forbidden)]
        forbid: Vec<blobray_domain::ForbiddenTargetRange>,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// the memory addresses they access, with blocked functions and gaps.
    RegisterAccesses {
        /// Repeat ROLE=PATH; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// START:LENGTH candidate interval; without one, every numeric address.
        #[arg(long = "range", value_parser = parse_range)]
        ranges: Vec<blobray_domain::ImageRegion>,
        /// Keep only observations in this 32-bit word; repeat for several.
        /// Blocked functions, gaps and the summary are never filtered.
        #[arg(long = "address", value_parser = parse_number, value_name = "ADDRESS")]
        addresses: Vec<u64>,
        /// Keep only observations of this function; repeat for several.
        #[arg(long = "function", value_name = "NAME")]
        functions: Vec<String>,
        /// Group the selected observations by word or by function. The human
        /// format groups by address when a filter is given without it.
        #[arg(long = "group-by", value_enum)]
        group_by: Option<GroupKey>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// the complete records of the named functions: every fact, expression and
    /// value the analysis derived, with coverage and semantics completeness.
    FunctionRecords {
        /// Repeat ROLE=PATH; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A function symbol name to report; repeat for several.
        #[arg(long = "function", required = true, value_name = "NAME")]
        functions: Vec<String>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// every memory access that lands on one field offset, as a root symbol,
    /// register or stack and the path of loaded pointers to the field.
    FieldAccesses {
        /// Repeat ROLE=PATH; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// The field's byte displacement from its pointer or root.
        #[arg(long, allow_negative_numbers = true)]
        offset: i64,
        /// Only accesses of this many bytes.
        #[arg(long)]
        width: Option<u8>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// every reference to the named symbols: calls, jumps and addresses taken,
    /// each a relocation in a relocatable object.
    Callers {
        /// Repeat ROLE=PATH; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A symbol name whose references to report; repeat for several.
        #[arg(long = "symbol", required = true, value_name = "NAME")]
        symbols: Vec<String>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
}

/// The `--group-by` key of `register-accesses`.
#[derive(Clone, Copy, ValueEnum)]
enum GroupKey {
    Address,
    Function,
}

impl GroupKey {
    fn by(self) -> blobray_cli::access_groups::GroupBy {
        match self {
            Self::Address => blobray_cli::access_groups::GroupBy::Address,
            Self::Function => blobray_cli::access_groups::GroupBy::Function,
        }
    }
}

const MIB: u64 = 1024 * 1024;

/// Cooperative limits of an operation that runs in this process.
#[derive(clap::Args)]
struct InProcessOptions {
    #[arg(long, default_value_t = DEFAULT_WORKING_BYTES / MIB)]
    working_memory_mib: u64,
    #[arg(long, default_value_t = oer_riscv_model::DEFAULT_TIMEOUT_MS / 1000)]
    timeout_secs: u64,
    #[arg(long, default_value_t = oer_riscv_model::DEFAULT_WORK_UNITS)]
    max_work_units: u64,
}

impl InProcessOptions {
    fn memory(&self) -> Result<oer_riscv_model::WorkingMemory> {
        oer_riscv_model::WorkingMemory::new(
            self.working_memory_mib
                .checked_mul(MIB)
                .ok_or_else(|| invalid("working memory limit overflow"))?,
        )
    }
    fn control(&self) -> app::in_process::Limits {
        app::in_process::Limits::new(
            self.max_work_units,
            std::time::Duration::from_secs(self.timeout_secs),
        )
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args[1] == command_tree::REQUEST {
        print!("{}", command_tree::json());
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
        Command::AuditTargets {
            artifact,
            forbid,
            limits,
        } => audit_targets(artifact, forbid, limits, format),
        Command::RegisterAccesses {
            inputs,
            ranges,
            addresses,
            functions,
            group_by,
            assumptions,
            limits,
        } => {
            let words = addresses
                .into_iter()
                .map(|address| {
                    u32::try_from(address)
                        .map(|address| address & !3)
                        .map_err(|_| invalid("--address exceeds 32 bits"))
                })
                .collect::<Result<Vec<_>>>()?;
            let filter = blobray_cli::access_groups::AccessFilter { words, functions };
            register_accesses(
                inputs,
                ranges,
                filter,
                group_by.map(GroupKey::by),
                assumptions.abi(),
                limits,
                format,
            )
        }
        Command::FunctionRecords {
            inputs,
            functions,
            assumptions,
            limits,
        } => function_records(inputs, functions, assumptions.abi(), limits, format),
        Command::FieldAccesses {
            inputs,
            offset,
            width,
            assumptions,
            limits,
        } => field_accesses(inputs, offset, width, assumptions.abi(), limits, format),
        Command::Callers {
            inputs,
            symbols,
            assumptions,
            limits,
        } => callers(inputs, symbols, assumptions.abi(), limits, format),
    }
}

/// Analyze the libraries `inputs` name in this process and print every
/// access of the field at `offset` (of `width` bytes, when given).
fn field_accesses(
    inputs: Vec<OsString>,
    offset: i64,
    width: Option<u8>,
    abi: Option<CallAbi>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut functions = Vec::new();
    let mut blocked = Vec::new();
    let mut gaps = 0_u64;
    let mut unknown_addresses = 0_u64;
    let mut partial = 0_u64;
    app::library::analyze_library(
        &executables,
        abi,
        &oer_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |outcome, _| {
            match outcome {
                app::library::LibraryOutcome::Analyzed(analyzed) => {
                    partial += u64::from(!analyzed.complete());
                    unknown_addresses += blobray_cli::field::unknown_addresses(analyzed.records);
                    let accesses =
                        blobray_cli::field::field_accesses(analyzed.records, offset, width);
                    if !accesses.is_empty() {
                        functions.push(blobray_cli::wire::FieldAccessFunction {
                            function: analyzed.function.clone(),
                            accesses,
                        });
                    }
                }
                app::library::LibraryOutcome::Blocked { function, .. } => {
                    blocked.push(function.clone());
                }
                app::library::LibraryOutcome::Gap { .. } => gaps += 1,
            }
            Ok(())
        },
    )?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = blobray_cli::wire::FieldAccessesDocument {
                schema: blobray_cli::wire::FIELD_ACCESSES_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| blobray_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi,
                offset,
                width,
                functions,
                blocked,
                partial,
                gaps,
                unknown_addresses,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for function in &functions {
                let name = function
                    .function
                    .name
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                for access in &function.accesses {
                    let root = match &access.root {
                        blobray_cli::field::FieldRoot::Symbol {
                            name: Some(name), ..
                        } => name.clone(),
                        other => format!("{other:?}"),
                    };
                    writeln!(
                        out,
                        "{name} (input {}) +{}: {:?} width {} {root} {:?}",
                        function.function.input,
                        access.offset,
                        access.access,
                        access.width,
                        access.path,
                    )
                    .map_err(io_error)?;
                }
            }
            writeln!(
                out,
                "{} functions access the field; {} functions blocked; {partial} functions analyzed incompletely; {gaps} gaps; {unknown_addresses} accesses at unknown addresses",
                functions.len(),
                blocked.len()
            )
            .map_err(io_error)?;
            if abi.is_none() && unknown_addresses > 0 {
                writeln!(
                    out,
                    "without --abi riscv-integer every register is unknown after a call the analysis cannot follow; accesses through a preserved register then have no address"
                )
                .map_err(io_error)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Analyze the libraries `inputs` name in this process and print the complete
/// records of the functions `names` names.
fn function_records(
    inputs: Vec<OsString>,
    names: Vec<String>,
    abi: Option<CallAbi>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let wanted = |function: &blobray_domain::LibraryFunction| {
        function
            .name
            .as_deref()
            .is_some_and(|name| names.iter().any(|wanted| wanted.as_bytes() == name))
    };
    let mut functions = Vec::new();
    app::library::analyze_library(
        &executables,
        abi,
        &oer_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |outcome, _| {
            match outcome {
                app::library::LibraryOutcome::Analyzed(analyzed) if wanted(analyzed.function) => {
                    functions.push(blobray_cli::wire::NamedFunction::Analyzed {
                        function: analyzed.function.clone(),
                        complete: analyzed.complete(),
                        coverage: analyzed.coverage,
                        semantics: analyzed.semantics,
                        records: analyzed.records.to_vec(),
                    });
                }
                app::library::LibraryOutcome::Blocked { function, error } if wanted(function) => {
                    functions.push(blobray_cli::wire::NamedFunction::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    });
                }
                _ => {}
            }
            Ok(())
        },
    )?;
    let found = |name: &str| {
        functions.iter().any(|function| {
            let (blobray_cli::wire::NamedFunction::Analyzed { function, .. }
            | blobray_cli::wire::NamedFunction::Blocked { function, .. }) = function;
            function.name.as_deref() == Some(name.as_bytes())
        })
    };
    let missing: Vec<String> = names.iter().filter(|name| !found(name)).cloned().collect();
    let status = if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    };
    let images = match format {
        Format::Human => executables
            .iter()
            .map(|executable| image_symbols(executable, &memory, &mut control))
            .collect::<Result<Vec<_>>>()?,
        Format::Json => Vec::new(),
    };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = blobray_cli::wire::FunctionRecordsDocument {
                schema: blobray_cli::wire::FUNCTION_RECORDS_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| blobray_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi,
                functions,
                missing,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for function in &functions {
                match function {
                    blobray_cli::wire::NamedFunction::Analyzed {
                        function,
                        complete,
                        records,
                        ..
                    } => writeln!(
                        out,
                        "{} (input {}): {} records, {}",
                        String::from_utf8_lossy(function.name.as_deref().unwrap_or_default()),
                        function.input,
                        records.len(),
                        if *complete { "complete" } else { "incomplete" }
                    )
                    .and_then(|()| {
                        blobray_cli::listing::listing(
                            records,
                            images.get(function.input as usize).and_then(Option::as_ref),
                        )
                        .iter()
                        .try_for_each(|line| writeln!(out, "{line}"))
                    }),
                    blobray_cli::wire::NamedFunction::Blocked { function, error } => writeln!(
                        out,
                        "{} (input {}): blocked: {error}",
                        String::from_utf8_lossy(function.name.as_deref().unwrap_or_default()),
                        function.input
                    ),
                }
                .map_err(io_error)?;
            }
            for name in &missing {
                writeln!(out, "{name}: no input defines it").map_err(io_error)?;
            }
        }
    }
    out.flush().map_err(io_error)?;
    Ok(status)
}

/// The sized function symbols of `executable` when it is one little-endian
/// executable image (ET_EXEC), whose transfers carry no relocations to name
/// them. Other inputs are recognized from their first bytes and inventoried
/// no further.
fn image_symbols(
    executable: &app::in_process::Executable,
    memory: &oer_riscv_model::WorkingMemory,
    control: &mut app::in_process::Limits,
) -> Result<Option<blobray_cli::listing::ImageSymbols>> {
    const ET_EXEC: u16 = 2;
    const STT_FUNC: u8 = 2;
    // Decide from the ELF header alone, so an archive or relocatable object
    // costs no second inventory pass against the analysis' shared budget.
    if !blobray_cli::listing::is_executable_image(executable.bytes()) {
        return Ok(None);
    }
    let inventory = app::captured::inventory(executable, memory, control)?;
    let [object] = inventory.objects.as_slice() else {
        return Ok(None);
    };
    let Some(elf) = object.elf.as_ref().filter(|elf| elf.object_type == ET_EXEC) else {
        return Ok(None);
    };
    Ok(Some(blobray_cli::listing::ImageSymbols::new(
        elf.symbols
            .iter()
            .filter(|symbol| symbol.symbol_type & 0xf == STT_FUNC)
            .filter_map(|symbol| {
                let name = symbol.name.as_deref()?;
                Some((
                    symbol.value,
                    symbol.size,
                    String::from_utf8_lossy(name).into_owned(),
                ))
            }),
    )))
}

fn io_error(e: std::io::Error) -> Error {
    oer_riscv_model::storage_io(e)
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// One `ROLE=PATH` input binding.
struct Input {
    role: String,
    path: PathBuf,
}

fn parse_input(value: OsString) -> Result<Input> {
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
    Ok(Input {
        role,
        path: OsString::from_vec(bytes[split + 1..].to_vec()).into(),
    })
}

fn parse_number(v: &str) -> std::result::Result<u64, String> {
    if let Some(hex) = v.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        v.parse::<u64>()
    }
    .map_err(|e| e.to_string())
}

fn parse_forbidden(
    value: &str,
) -> std::result::Result<blobray_domain::ForbiddenTargetRange, String> {
    let (name, bounds) = value.split_once('=').ok_or("expected NAME=START..END")?;
    let (start, end) = bounds.split_once("..").ok_or("expected START..END")?;
    let range = blobray_domain::ForbiddenTargetRange {
        name: name.into(),
        start: u32::try_from(parse_number(start)?).map_err(|e| e.to_string())?,
        end: parse_number(end)?,
    };
    range.validate().map_err(|e| e.message)?;
    Ok(range)
}

fn parse_range(text: &str) -> std::result::Result<blobray_domain::ImageRegion, String> {
    let (start, length) = text.split_once(':').ok_or("expected START:LENGTH")?;
    Ok(blobray_domain::ImageRegion {
        start: u32::try_from(parse_number(start)?).map_err(|e| e.to_string())?,
        length: parse_number(length)?,
    })
}

fn json_error(e: serde_json::Error) -> Error {
    invalid(&e.to_string())
}

/// Audit the final image at `artifact` in this process. Only a clean audit,
/// with no forbidden target and no coverage gap, succeeds.
fn audit_targets(
    artifact: PathBuf,
    ranges: Vec<blobray_domain::ForbiddenTargetRange>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let executable = app::in_process::Executable::new(std::fs::read(&artifact).map_err(io_error)?);
    let memory = limits.memory()?;
    let mut control = limits.control();
    let decoder = oer_riscv_lift::RiscvDecoder;
    let mut records = Vec::new();
    let summary = app::audit::audit_targets(
        &executable,
        &ranges,
        &decoder,
        &memory,
        &mut control,
        &mut |record, _| {
            records.push(record.clone());
            Ok(())
        },
    )?;
    let verdict = if summary.forbidden_targets != 0 {
        CheckVerdict::Fail
    } else if summary.coverage_gaps != 0 {
        CheckVerdict::Inconclusive
    } else {
        CheckVerdict::Pass
    };
    let mut out = std::io::stdout().lock();
    match format {
        Format::Json => {
            serde_json::to_writer(
                &mut out,
                &blobray_cli::wire::TargetAuditDocument {
                    schema: blobray_cli::wire::TARGET_AUDIT_SCHEMA,
                    artifact: executable.id().clone(),
                    decoder: decoder.identity().into(),
                    semantics: decoder.semantic_identity().into(),
                    ranges,
                    records,
                    summary,
                    verdict,
                },
            )
            .map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            writeln!(
                out,
                "Target audit of {}: {verdict:?}\n  {} sections, {} instructions, {} forbidden targets, {} coverage gaps, {} unresolved indirect transfers",
                executable.id(),
                summary.sections,
                summary.instructions,
                summary.forbidden_targets,
                summary.coverage_gaps,
                summary.unresolved_indirect
            )
            .map_err(io_error)?;
            for record in &records {
                writeln!(out, "  {record:?}").map_err(io_error)?;
            }
        }
    }
    Ok(if verdict == CheckVerdict::Pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Analyze the libraries `inputs` name in this process and print their
/// register accesses, streaming the JSON records as they are found.
fn register_accesses(
    inputs: Vec<OsString>,
    ranges: Vec<blobray_domain::ImageRegion>,
    filter: blobray_cli::access_groups::AccessFilter,
    group_by: Option<blobray_cli::access_groups::GroupBy>,
    abi: Option<CallAbi>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let json = matches!(format, Format::Json);
    let filtered = !filter.words.is_empty() || !filter.functions.is_empty();
    let group_by = match (group_by, json, filtered) {
        (None, false, true) => Some(blobray_cli::access_groups::GroupBy::Address),
        (group_by, _, _) => group_by,
    };
    let mut groups = group_by.map(blobray_cli::access_groups::AccessGroups::new);
    if json {
        let described: Vec<_> = inputs
            .iter()
            .zip(&executables)
            .map(
                |(input, executable)| blobray_cli::wire::RegisterAccessInput {
                    role: input.role.clone(),
                    sha256: executable.id().clone(),
                },
            )
            .collect();
        write!(
            out,
            "{{\"schema\":{},\"inputs\":{},\"abi\":{},\"records\":[",
            blobray_cli::wire::REGISTER_ACCESSES_SCHEMA,
            serde_json::to_string(&described).map_err(json_error)?,
            serde_json::to_string(&abi).map_err(json_error)?
        )
        .map_err(io_error)?;
    }
    let mut first = true;
    let summary = app::library::register_accesses(
        &executables,
        abi,
        &ranges,
        &oer_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            if !filter.keeps(record) {
                return Ok(());
            }
            if let Some(groups) = groups.as_mut() {
                groups.add(record);
            }
            if json {
                if !first {
                    out.write_all(b",").map_err(io_error)?;
                }
                first = false;
                serde_json::to_writer(&mut out, record).map_err(json_error)?;
            }
            Ok(())
        },
    )?;
    if json {
        out.write_all(b"]").map_err(io_error)?;
        if let Some(groups) = &groups {
            write!(
                out,
                ",\"groups\":{}",
                serde_json::to_string(&groups.groups()).map_err(json_error)?
            )
            .map_err(io_error)?;
        }
        write!(
            out,
            ",\"summary\":{}}}",
            serde_json::to_string(&summary).map_err(json_error)?
        )
        .map_err(io_error)?;
        writeln!(out).map_err(io_error)?;
    } else {
        if let Some(groups) = &groups {
            for line in groups.human() {
                writeln!(out, "{line}").map_err(io_error)?;
            }
        }
        writeln!(
            out,
            "{} functions ({} partial, {} blocked), {} gaps, {} observations ({} unresolved)",
            summary.functions,
            summary.partial_functions,
            summary.blocked_functions,
            summary.gaps,
            summary.observations,
            summary.unresolved_addresses
        )
        .map_err(io_error)?;
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Analyze the libraries `inputs` name in this process and print every
/// reference their functions make to a symbol `symbols` names.
fn callers(
    inputs: Vec<OsString>,
    symbols: Vec<String>,
    abi: Option<CallAbi>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut callers = Vec::new();
    let mut blocked = Vec::new();
    let mut gaps = 0_u64;
    app::library::analyze_library(
        &executables,
        abi,
        &oer_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |outcome, _| {
            match outcome {
                app::library::LibraryOutcome::Analyzed(analyzed) => {
                    let references =
                        blobray_cli::listing::references_to(analyzed.records, &symbols);
                    if !references.is_empty() {
                        callers.push(blobray_cli::wire::Caller {
                            function: analyzed.function.clone(),
                            references,
                        });
                    }
                }
                app::library::LibraryOutcome::Blocked { function, .. } => {
                    blocked.push(function.clone());
                }
                app::library::LibraryOutcome::Gap { .. } => gaps += 1,
            }
            Ok(())
        },
    )?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = blobray_cli::wire::CallersDocument {
                schema: blobray_cli::wire::CALLERS_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| blobray_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi,
                symbols,
                callers,
                blocked,
                gaps,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for caller in &callers {
                let name = caller
                    .function
                    .name
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                for reference in &caller.references {
                    writeln!(
                        out,
                        "{name} (input {}) +{:x}: {:?} {}",
                        caller.function.input,
                        reference.offset,
                        reference.kind,
                        String::from_utf8_lossy(&reference.target),
                    )
                    .map_err(io_error)?;
                }
            }
            writeln!(
                out,
                "{} functions reference the symbols; {} functions blocked; {gaps} gaps",
                callers.len(),
                blocked.len()
            )
            .map_err(io_error)?;
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}
