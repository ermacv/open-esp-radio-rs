//! Command-line adapter for Blobray operations that run in this process.

use blobray_application as app;
use blobray_domain::{
    CheckVerdict, DEFAULT_WORKING_BYTES, Error, ErrorCode, FunctionDecoder, FunctionSemantics,
    Result,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::Write;
use std::{ffi::OsString, path::PathBuf, process::ExitCode};
mod command_tree;

#[derive(Clone, Copy, Default, ValueEnum)]
enum Format {
    #[default]
    Human,
    Json,
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
        limits: InProcessOptions,
    },
}

const MIB: u64 = 1024 * 1024;

/// Cooperative limits of an operation that runs in this process.
#[derive(clap::Args)]
struct InProcessOptions {
    #[arg(long, default_value_t = DEFAULT_WORKING_BYTES / MIB)]
    working_memory_mib: u64,
    #[arg(long, default_value_t = blobray_domain::DEFAULT_TIMEOUT_MS / 1000)]
    timeout_secs: u64,
    #[arg(long, default_value_t = blobray_domain::DEFAULT_WORK_UNITS)]
    max_work_units: u64,
}

impl InProcessOptions {
    fn memory(&self) -> Result<blobray_domain::WorkingMemory> {
        blobray_domain::WorkingMemory::new(
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
            limits,
        } => register_accesses(inputs, ranges, limits, format),
        Command::FunctionRecords {
            inputs,
            functions,
            limits,
        } => function_records(inputs, functions, limits, format),
        Command::FieldAccesses {
            inputs,
            offset,
            width,
            limits,
        } => field_accesses(inputs, offset, width, limits, format),
    }
}

/// Analyze the libraries `inputs` name in this process and print every
/// access of the field at `offset` (of `width` bytes, when given).
fn field_accesses(
    inputs: Vec<OsString>,
    offset: i64,
    width: Option<u8>,
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
    app::library::analyze_library(
        &executables,
        &blobray_backend_riscv::RiscvDecoder,
        &memory,
        &mut control,
        &mut |outcome, _| {
            match outcome {
                app::library::LibraryOutcome::Analyzed(analyzed) => {
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
                offset,
                width,
                functions,
                blocked,
                gaps,
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
                "{} functions access the field; {} functions blocked; {gaps} gaps",
                functions.len(),
                blocked.len()
            )
            .map_err(io_error)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Analyze the libraries `inputs` name in this process and print the complete
/// records of the functions `names` names.
fn function_records(
    inputs: Vec<OsString>,
    names: Vec<String>,
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
        &blobray_backend_riscv::RiscvDecoder,
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
                    ),
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

fn io_error(e: std::io::Error) -> Error {
    blobray_domain::storage_io(e)
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
    let decoder = blobray_backend_riscv::RiscvDecoder;
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
            "{{\"schema\":{},\"inputs\":{},\"records\":[",
            blobray_cli::wire::REGISTER_ACCESSES_SCHEMA,
            serde_json::to_string(&described).map_err(json_error)?
        )
        .map_err(io_error)?;
    }
    let mut first = true;
    let summary = app::library::register_accesses(
        &executables,
        &ranges,
        &blobray_backend_riscv::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
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
        write!(
            out,
            "],\"summary\":{}}}",
            serde_json::to_string(&summary).map_err(json_error)?
        )
        .map_err(io_error)?;
        writeln!(out).map_err(io_error)?;
    } else {
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
