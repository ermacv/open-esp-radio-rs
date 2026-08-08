//! Process output boundary: command results on stdout, diagnostics elsewhere.

use std::{
    cell::Cell,
    fmt,
    io::{self, Write as _},
    path::Path,
    sync::{Mutex, OnceLock},
};

use serde::Serialize;

use super::args::OutputFormat;
use crate::Result;

static FORMAT: OnceLock<OutputFormat> = OnceLock::new();
static RECORDS: Mutex<Vec<OutputRecord>> = Mutex::new(Vec::new());

thread_local! {
    static SUPPRESSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[derive(Serialize)]
struct OutputRecord {
    kind: &'static str,
    data: serde_json::Value,
}

#[derive(Serialize)]
struct TextRecord {
    text: String,
}

#[derive(Serialize)]
struct FileRecord {
    path: String,
    status: &'static str,
}

#[derive(Serialize)]
struct OutputDocument<'a> {
    schema: u32,
    records: &'a [OutputRecord],
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
    emit_text("line", arguments.to_string(), true);
}

pub(super) fn text(value: impl Into<String>) {
    emit_text("text", value.into(), false);
}

pub(super) fn structured(kind: &'static str, value: &impl Serialize) -> bool {
    if suppressed() {
        return true;
    }
    if matches!(
        FORMAT.get().copied().unwrap_or_default(),
        OutputFormat::Human | OutputFormat::Tsv
    ) {
        return false;
    }
    let data = serde_json::to_value(value).expect("serializing typed command report");
    emit_record(OutputRecord { kind, data });
    true
}

/// Emits one typed command result and selects exactly one presentation
/// renderer when stdout is intended for humans or TSV automation.
pub(super) fn render_report(
    kind: &'static str,
    value: &impl Serialize,
    human: impl FnOnce(),
    tsv: impl FnOnce(),
) {
    if structured(kind, value) {
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

pub(super) fn file(kind: &'static str, path: &Path, status: &'static str) -> bool {
    structured(
        kind,
        &FileRecord {
            path: path.display().to_string(),
            status,
        },
    )
}

fn emit_text(kind: &'static str, text: String, newline: bool) {
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
        OutputFormat::Json => emit_record(OutputRecord {
            kind,
            data: serde_json::to_value(TextRecord { text })
                .expect("serializing command output text"),
        }),
        OutputFormat::Jsonl => {
            emit_record(OutputRecord {
                kind,
                data: serde_json::to_value(TextRecord { text })
                    .expect("serializing command output text"),
            });
        }
    }
}

fn suppressed() -> bool {
    SUPPRESSION_DEPTH.with(|depth| depth.get() != 0)
}

fn emit_record(record: OutputRecord) {
    match FORMAT.get().copied().unwrap_or_default() {
        OutputFormat::Json => RECORDS
            .lock()
            .expect("command output buffer lock")
            .push(record),
        OutputFormat::Jsonl => {
            let mut stdout = io::stdout().lock();
            serde_json::to_writer(&mut stdout, &record).expect("serializing command output record");
            writeln!(stdout).expect("writing command output to stdout");
        }
        OutputFormat::Human | OutputFormat::Tsv => {
            unreachable!("structured records are emitted only for machine output")
        }
    }
}

pub(super) fn finish() -> Result<()> {
    if FORMAT.get().copied().unwrap_or_default() != OutputFormat::Json {
        return Ok(());
    }
    let records = RECORDS.lock().expect("command output buffer lock");
    let document = OutputDocument {
        schema: 1,
        records: &records,
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &document)?;
    writeln!(stdout)?;
    Ok(())
}
