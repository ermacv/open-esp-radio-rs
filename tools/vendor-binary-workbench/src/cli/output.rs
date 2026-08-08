//! Process output boundary: command results on stdout, diagnostics elsewhere.

use std::{
    cell::Cell,
    fmt,
    io::{self, Write as _},
    path::Path,
    sync::{Mutex, OnceLock},
};

use serde::Serialize;
use serde_json::value::RawValue;

use super::args::OutputFormat;
use crate::Result;

static FORMAT: OnceLock<OutputFormat> = OnceLock::new();
static STATE: Mutex<OutputState> = Mutex::new(OutputState {
    claimed: false,
    report: None,
});

thread_local! {
    static SUPPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct OutputState {
    claimed: bool,
    report: Option<Box<RawValue>>,
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

pub(super) fn init(format: OutputFormat) {
    FORMAT
        .set(format)
        .expect("the command output boundary must be initialized once");
}

pub(super) fn format() -> OutputFormat {
    FORMAT.get().copied().unwrap_or_default()
}

/// Runs a nested command without letting its presentation leak into the
/// enclosing command's report. Diagnostics and tracing remain unaffected.
pub(super) fn suppress<T>(action: impl FnOnce() -> T) -> T {
    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            SUPPRESSION_DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }

    SUPPRESSION_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _guard = Guard;
    action()
}

pub(super) fn line(arguments: fmt::Arguments<'_>) {
    emit_text(arguments.to_string(), true);
}

pub(super) fn text(value: impl Into<String>) {
    emit_text(value.into(), false);
}

pub(super) fn structured(value: &impl Serialize) -> bool {
    if suppressed() {
        return true;
    }
    if matches!(
        FORMAT.get().copied().unwrap_or_default(),
        OutputFormat::Human | OutputFormat::Tsv
    ) {
        return false;
    }
    let data = serde_json::value::to_raw_value(value).expect("serializing typed command report");
    emit_report(data);
    true
}

/// Emits one typed command result and selects exactly one presentation
/// renderer when stdout is intended for humans or TSV automation.
pub(super) fn render_report(value: &impl Serialize, human: impl FnOnce(), tsv: impl FnOnce()) {
    if structured(value) {
        return;
    }
    match format() {
        OutputFormat::Human => human(),
        OutputFormat::Tsv => tsv(),
        OutputFormat::Json | OutputFormat::Jsonl => {
            unreachable!("structured command output was already emitted")
        }
    }
}

fn emit_text(text: String, newline: bool) {
    if suppressed() {
        return;
    }
    match FORMAT.get().copied().unwrap_or_default() {
        OutputFormat::Human | OutputFormat::Tsv => {
            let mut stdout = io::stdout().lock();
            if newline {
                writeln!(stdout, "{text}").expect("writing command output to stdout");
            } else {
                write!(stdout, "{text}").expect("writing command output to stdout");
            }
        }
        OutputFormat::Json | OutputFormat::Jsonl => {
            panic!("machine output requires one typed command report")
        }
    }
}

fn suppressed() -> bool {
    SUPPRESSION_DEPTH.with(|depth| depth.get() != 0)
}

fn emit_report(report: Box<RawValue>) {
    let mut state = STATE.lock().expect("command output state lock");
    assert!(
        !state.claimed,
        "a command emitted more than one typed report"
    );
    state.claimed = true;
    match FORMAT.get().copied().unwrap_or_default() {
        OutputFormat::Json => state.report = Some(report),
        OutputFormat::Jsonl => {
            drop(state);
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &report).expect("serializing command report");
            writeln!(stdout).expect("writing command output to stdout");
        }
        OutputFormat::Human | OutputFormat::Tsv => {
            unreachable!("typed reports are emitted only for machine output")
        }
    }
}

pub(super) fn finish() -> Result<()> {
    if FORMAT.get().copied().unwrap_or_default() != OutputFormat::Json {
        return Ok(());
    }
    let state = STATE.lock().expect("command output state lock");
    let report = state
        .report
        .as_ref()
        .expect("a successful JSON command must emit one typed report");
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, report)?;
    writeln!(stdout)?;
    Ok(())
}
