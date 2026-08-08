//! Typed register publication reports and presentation renderers.

use std::path::Path;

use serde::Serialize;

use super::{PacEdition, PacTarget, SvdExportSummary};

#[derive(Serialize)]
struct SvdPublicationDocument<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    peripherals: usize,
    registers: usize,
    fields: usize,
    path: &'a Path,
}

#[derive(Serialize)]
struct PacPublicationDocument<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    target: &'static str,
    edition: &'static str,
    peripherals: usize,
    registers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_pack: Option<&'a Path>,
    path: &'a Path,
}

#[derive(Serialize)]
struct BindingPublicationDocument<'a> {
    schema: u32,
    command: &'static str,
    status: &'static str,
    crate_name: &'a str,
    peripherals: usize,
    registers: usize,
    path: &'a Path,
}

pub(super) fn emit_svd(status: &'static str, summary: &SvdExportSummary, path: &Path) {
    let report = SvdPublicationDocument {
        schema: 1,
        command: "registers export-svd",
        status,
        peripherals: summary.peripherals,
        registers: summary.registers,
        fields: summary.fields,
        path,
    };
    crate::cli::output::render_report(&report, || {
        outputln!("SVD: {} — {}", report.status, report.path.display());
        outputln!(
            "  peripherals={} registers={} fields={}",
            report.peripherals,
            report.registers,
            report.fields
        );
    });
}

pub(super) fn emit_pac(
    status: &'static str,
    target: PacTarget,
    edition: PacEdition,
    summary: &SvdExportSummary,
    api_pack: Option<&Path>,
    path: &Path,
) {
    let report = PacPublicationDocument {
        schema: 1,
        command: "registers generate-pac",
        status,
        target: target.label(),
        edition: edition.label(),
        peripherals: summary.peripherals,
        registers: summary.registers,
        api_pack,
        path,
    };
    crate::cli::output::render_report(&report, || {
        outputln!("PAC: {} — {}", report.status, report.path.display());
        outputln!(
            "  target={} edition={} peripherals={} registers={} api-pack={}",
            report.target,
            report.edition,
            report.peripherals,
            report.registers,
            report
                .api_pack
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string())
        );
    });
}

pub(super) fn emit_bindings(
    status: &'static str,
    crate_name: &str,
    summary: &SvdExportSummary,
    path: &Path,
) {
    let report = BindingPublicationDocument {
        schema: 1,
        command: "registers generate-bindings",
        status,
        crate_name,
        peripherals: summary.peripherals,
        registers: summary.registers,
        path,
    };
    crate::cli::output::render_report(&report, || {
        outputln!(
            "PAC bindings: {} — {}",
            report.status,
            report.path.display()
        );
        outputln!(
            "  crate={} peripherals={} registers={}",
            report.crate_name,
            report.peripherals,
            report.registers
        );
    });
}
