//! Process output boundary: command results on stdout, diagnostics elsewhere.

use std::{
    fmt,
    io::{self, Write as _},
    path::Path,
    sync::{Mutex, OnceLock},
};

use serde::Serialize;
use serde_json::value::RawValue;

use super::args::{OutputFormat, UiArgs};
use crate::Result;

static CONTEXT: OnceLock<OutputContext> = OnceLock::new();
static STATE: Mutex<OutputState> = Mutex::new(OutputState {
    claimed: false,
    report: None,
});

thread_local! {
    static PROGRESS_SUSPENSION_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct OutputState {
    claimed: bool,
    report: Option<Box<RawValue>>,
}

#[derive(Clone, Copy, Debug)]
struct OutputContext {
    format: OutputFormat,
    human_ansi: bool,
    human_width: usize,
    details: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct Publication {
    pub(super) path: String,
    pub(super) status: &'static str,
}

impl Publication {
    pub(super) fn new(path: &Path, status: &'static str) -> Self {
        Self {
            path: path.display().to_string(),
            status,
        }
    }
}

pub(super) fn init(arguments: &UiArgs) {
    let stdout_is_terminal = super::terminal::stdout_is_terminal();
    let human_ansi = matches!(arguments.format, OutputFormat::Human)
        && super::terminal::color_enabled(arguments.color, stdout_is_terminal);
    let human_width = super::terminal::stdout_width(stdout_is_terminal);
    CONTEXT
        .set(OutputContext {
            format: arguments.format,
            human_ansi,
            human_width,
            details: arguments.details,
        })
        .expect("the command output boundary must be initialized once");
}

pub(super) fn format() -> OutputFormat {
    context().format
}

pub(super) fn details() -> bool {
    context().details
}

pub(super) fn human_width() -> usize {
    context().human_width
}

pub(super) fn heading(value: impl AsRef<str>) -> String {
    styled("1;36", value.as_ref())
}

pub(super) fn success(value: impl AsRef<str>) -> String {
    styled("1;32", value.as_ref())
}

pub(super) fn warning(value: impl AsRef<str>) -> String {
    styled("1;33", value.as_ref())
}

pub(super) fn failure(value: impl AsRef<str>) -> String {
    styled("1;31", value.as_ref())
}

pub(super) fn line(arguments: fmt::Arguments<'_>) {
    emit_text(arguments.to_string(), true);
}

pub(super) fn text(value: impl Into<String>) {
    emit_text(value.into(), false);
}

pub(super) fn structured(value: &impl Serialize) -> bool {
    if matches!(format(), OutputFormat::Human) {
        return false;
    }
    let data = serde_json::value::to_raw_value(value).expect("serializing typed command report");
    emit_report(data);
    true
}

/// Emits one typed command result or selects its human presentation.
pub(super) fn render_report(value: &impl Serialize, human: impl FnOnce()) {
    if structured(value) {
        return;
    }
    match format() {
        OutputFormat::Human => with_progress_suspended(human),
        OutputFormat::Json => {
            unreachable!("structured command output was already emitted")
        }
    }
}

fn emit_text(text: String, newline: bool) {
    match format() {
        OutputFormat::Human => {
            with_progress_suspended(|| {
                let mut stdout = io::stdout().lock();
                if newline {
                    writeln!(stdout, "{text}").expect("writing command output to stdout");
                } else {
                    write!(stdout, "{text}").expect("writing command output to stdout");
                }
            });
        }
        OutputFormat::Json => {
            panic!("machine output requires one typed command report")
        }
    }
}

fn emit_report(report: Box<RawValue>) {
    let mut state = STATE.lock().expect("command output state lock");
    assert!(
        !state.claimed,
        "a command emitted more than one typed report"
    );
    state.claimed = true;
    match format() {
        OutputFormat::Json => state.report = Some(report),
        OutputFormat::Human => {
            unreachable!("typed reports are emitted only for machine output")
        }
    }
}

pub(super) fn finish() -> Result<()> {
    if format() != OutputFormat::Json {
        return Ok(());
    }
    let state = STATE.lock().expect("command output state lock");
    let report = state
        .report
        .as_ref()
        .expect("a successful JSON command must emit one typed report");
    with_progress_suspended(|| -> Result<()> {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, report)?;
        writeln!(stdout)?;
        Ok(())
    })?;
    Ok(())
}

fn context() -> OutputContext {
    CONTEXT.get().copied().unwrap_or(OutputContext {
        format: OutputFormat::Human,
        human_ansi: false,
        human_width: 100,
        details: false,
    })
}

fn styled(code: &str, value: &str) -> String {
    if context().human_ansi {
        format!("\u{1b}[{code}m{value}\u{1b}[0m")
    } else {
        value.to_owned()
    }
}

fn with_progress_suspended<T>(action: impl FnOnce() -> T) -> T {
    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            PROGRESS_SUSPENSION_DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }

    if PROGRESS_SUSPENSION_DEPTH.with(|depth| depth.get() != 0) {
        return action();
    }
    PROGRESS_SUSPENSION_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _guard = Guard;
    tracing_indicatif::suspend_tracing_indicatif(action)
}
