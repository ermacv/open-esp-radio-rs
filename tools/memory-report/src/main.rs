#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand, ValueEnum};
use oer_memory_report::{
    MemoryPolicy, Result, StackBudget, analyze, analyze_code, analyze_mono, analyze_stack, audit,
    audit_stack, diff, diff_code, render_audit, render_code_diff, render_code_report, render_diff,
    render_mono_report, render_report, render_stack_report,
};

#[derive(Debug, Parser)]
#[command(about = "Analyze ELF memory ownership and placement policy")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compare exact linked text totals without rebuilding either image.
    CodeDiff {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
    /// Read rustc JSON mono estimates (not linked bytes); never rebuild firmware.
    Mono {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
    /// Inspect surviving text ranges in an exact linked image, without rebuilding.
    Code {
        #[arg(long)]
        elf: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
    /// Report regions, consumers, reservations and unclassified allocations.
    Report(OneElf),
    /// Fail when a required symbol or placement contract is violated.
    Audit(OneElf),
    /// Compare regions and policy-attributed consumers between two ELFs.
    Diff {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
    /// Report and enforce compiler-measured firmware stack-frame sizes.
    Stack {
        #[arg(long)]
        elf: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: OutputFormat,
    },
}

#[derive(Debug, clap::Args)]
struct OneElf {
    #[arg(long)]
    elf: PathBuf,
    #[arg(long)]
    policy: PathBuf,
    #[arg(long, value_enum, default_value_t)]
    format: OutputFormat,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Human,
    Json,
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::CodeDiff {
            before,
            after,
            format,
        } => {
            let report = diff_code(&analyze_code(&before)?, &analyze_code(&after)?);
            print_value(format, &report, || render_code_diff(&report))?;
        }
        Command::Mono { input, format } => {
            let report = analyze_mono(&input)?;
            print_value(format, &report, || render_mono_report(&report))?;
        }
        Command::Code { elf, format } => {
            let report = analyze_code(&elf)?;
            print_value(format, &report, || render_code_report(&report))?;
        }
        Command::Report(arguments) => {
            let policy = MemoryPolicy::load(&arguments.policy)?;
            let report = analyze(&arguments.elf, &policy)?;
            print_value(arguments.format, &report, || render_report(&report))?;
        }
        Command::Audit(arguments) => {
            let policy = MemoryPolicy::load(&arguments.policy)?;
            let report = analyze(&arguments.elf, &policy)?;
            match arguments.format {
                OutputFormat::Human => print!("{}", render_audit(&report.audit)),
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report.audit)?),
            }
            audit(&report)?;
        }
        Command::Diff {
            before,
            after,
            policy,
            format,
        } => {
            let policy = MemoryPolicy::load(&policy)?;
            let before = analyze(&before, &policy)?;
            let after = analyze(&after, &policy)?;
            let report = diff(&before, &after);
            print_value(format, &report, || render_diff(&report))?;
        }
        Command::Stack {
            elf,
            policy,
            format,
        } => {
            let budget = StackBudget::load(&policy)?;
            let report = analyze_stack(&elf, &budget)?;
            print_value(format, &report, || render_stack_report(&report))?;
            audit_stack(&report)?;
        }
    }
    Ok(())
}

fn print_value<T: serde::Serialize>(
    format: OutputFormat,
    value: &T,
    human: impl FnOnce() -> String,
) -> Result<()> {
    match format {
        OutputFormat::Human => print!("{}", human()),
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(value)?),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
