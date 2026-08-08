//! Tabular human-readable linked-IR report rendering.

use super::*;

mod functions;
mod header;
mod interfaces;
mod registers;
mod summary;

pub(super) fn print_report(
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
    include_reachable: bool,
) {
    let mut output = String::new();
    header::render(&mut output, artifacts, report, include_reachable);
    functions::render(&mut output, report);
    registers::render(&mut output, report);
    interfaces::render(&mut output, report);
    summary::render(&mut output, artifacts, report);
    crate::cli::output::text(output);
}
