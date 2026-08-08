//! Shell-completion and manual-page generation from the canonical clap grammar.

use std::fs;

use clap_complete::{Shell, generate};
use serde::Serialize;

use super::super::{Command, CommandArguments, args::CompletionShell};
use crate::Result;

#[derive(Serialize)]
struct ToolingAssetReport {
    schema: u32,
    command: &'static str,
    kind: &'static str,
    variant: Option<&'static str>,
    status: &'static str,
    path: std::path::PathBuf,
}

pub(super) fn run(command: Command, arguments: CommandArguments) -> Result<bool> {
    let (contents, report) = match (command, arguments) {
        (Command::GenerateCompletions, CommandArguments::Completion(arguments)) => {
            let mut command = super::super::args::command_definition();
            let shell = completion_shell(arguments.shell);
            let mut contents = Vec::new();
            generate(
                shell,
                &mut command,
                "vendor-binary-workbench",
                &mut contents,
            );
            let report = ToolingAssetReport {
                schema: 1,
                command: "tooling completions",
                kind: "shell-completion",
                variant: Some(shell_label(arguments.shell)),
                status: "written",
                path: arguments.output.clone(),
            };
            fs::write(&arguments.output, &contents)?;
            (contents, report)
        }
        (Command::GenerateManpage, CommandArguments::Manpage(arguments)) => {
            let mut contents = Vec::new();
            clap_mangen::Man::new(super::super::args::command_definition())
                .render(&mut contents)?;
            let report = ToolingAssetReport {
                schema: 1,
                command: "tooling manpage",
                kind: "manpage",
                variant: None,
                status: "written",
                path: arguments.output.clone(),
            };
            fs::write(&arguments.output, &contents)?;
            (contents, report)
        }
        _ => unreachable!("tooling dispatcher received another command"),
    };
    debug_assert!(!contents.is_empty());
    crate::cli::output::render_report(
        "tooling-asset",
        &report,
        || {
            outputln!(
                "Generated {}{}: {}",
                report.kind,
                report
                    .variant
                    .map_or_else(String::new, |variant| format!(" ({variant})")),
                report.path.display()
            );
        },
        || {
            outputln!(
                "TOOLING-ASSET\tkind={}\tvariant={}\tstatus={}\tpath={}",
                report.kind,
                report.variant.unwrap_or("-"),
                report.status,
                report.path.display()
            );
        },
    );
    Ok(true)
}

const fn completion_shell(shell: CompletionShell) -> Shell {
    match shell {
        CompletionShell::Bash => Shell::Bash,
        CompletionShell::Elvish => Shell::Elvish,
        CompletionShell::Fish => Shell::Fish,
        CompletionShell::PowerShell => Shell::PowerShell,
        CompletionShell::Zsh => Shell::Zsh,
    }
}

const fn shell_label(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => "bash",
        CompletionShell::Elvish => "elvish",
        CompletionShell::Fish => "fish",
        CompletionShell::PowerShell => "powershell",
        CompletionShell::Zsh => "zsh",
    }
}
