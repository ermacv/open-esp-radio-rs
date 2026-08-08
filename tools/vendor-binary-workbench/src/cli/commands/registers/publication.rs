//! Derived SVD, PAC and PAC-binding publication commands.

use std::path::PathBuf;

use super::*;

mod report;

use report::*;

pub(crate) struct PreparedPublication {
    output: PathBuf,
    contents: String,
    kind: &'static str,
    report: PublicationReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationReadiness {
    Current,
    Missing,
    Stale,
}

impl PublicationReadiness {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Current => "ready",
            Self::Missing => "missing",
            Self::Stale => "stale",
        }
    }
}

enum PublicationReport {
    Svd {
        summary: SvdExportSummary,
    },
    Pac {
        target: PacTarget,
        edition: PacEdition,
        summary: SvdExportSummary,
        api_pack: Option<PathBuf>,
    },
    Bindings {
        crate_name: String,
        summary: SvdExportSummary,
    },
}

impl PreparedPublication {
    pub(crate) fn readiness(&self) -> Result<PublicationReadiness> {
        match std::fs::read_to_string(&self.output) {
            Ok(existing) if existing == self.contents => Ok(PublicationReadiness::Current),
            Ok(_) => Ok(PublicationReadiness::Stale),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(PublicationReadiness::Missing)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn output(&self) -> &std::path::Path {
        &self.output
    }

    pub(crate) fn write_or_check(&self, check: bool) -> Result<bool> {
        super::super::super::generated_output::write_or_check(
            &self.output,
            &self.contents,
            check,
            self.kind,
        )?;
        let status = if check { "verified" } else { "written" };
        match &self.report {
            PublicationReport::Svd { summary } => emit_svd(status, summary, &self.output),
            PublicationReport::Pac {
                target,
                edition,
                summary,
                api_pack,
            } => emit_pac(
                status,
                *target,
                *edition,
                summary,
                api_pack.as_deref(),
                &self.output,
            ),
            PublicationReport::Bindings {
                crate_name,
                summary,
            } => emit_bindings(status, crate_name, summary, &self.output),
        }
        Ok(true)
    }
}

#[tracing::instrument(name = "prepare_project_svd", skip_all)]
pub(crate) fn prepare_project_svd(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let output = paths
        .svd_output
        .clone()
        .ok_or("project SVD publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_release_workspace(paths, "release SVD")?;
    let (contents, summary) = workspace.render_svd()?;
    Ok(PreparedPublication {
        output,
        contents,
        kind: "SVD",
        report: PublicationReport::Svd { summary },
    })
}

#[tracing::instrument(name = "prepare_project_pac", skip_all)]
pub(crate) fn prepare_project_pac(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let configured = paths
        .pac
        .as_ref()
        .ok_or("project PAC publication is not configured")
        .map_err(crate::Error::invalid)?;
    let target = PacTarget::parse(&configured.target)?;
    let edition = PacEdition::parse(&configured.edition)?;
    let api_pack = paths
        .api_pack
        .as_deref()
        .map(PacApiPack::load)
        .transpose()?;
    let workspace = load_release_workspace(paths, "PAC generation")?;
    let (svd, summary) = workspace.render_svd()?;
    let contents = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC",
        report: PublicationReport::Pac {
            target,
            edition,
            summary,
            api_pack: paths.api_pack.clone(),
        },
    })
}

pub(crate) fn prepare_project_bindings(
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<PreparedPublication> {
    let configured = paths
        .bindings
        .as_ref()
        .ok_or("project PAC binding publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_release_workspace(paths, "PAC binding generation")?;
    let (svd, summary) = workspace.render_svd()?;
    let contents =
        open_esp_radio_register_model::generate_pac_binding_index(&svd, &configured.crate_name)?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC binding index",
        report: PublicationReport::Bindings {
            crate_name: configured.crate_name.clone(),
            summary,
        },
    })
}

fn load_release_workspace(
    paths: &crate::project::RegisterWorkspacePaths,
    operation: &str,
) -> Result<ProjectRegisterWorkspace> {
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let summary = workspace.summary()?;
    if summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "{operation} denied {} unreviewed MMIO observations",
            summary.unreviewed
        )));
    }
    Ok(workspace)
}

#[tracing::instrument(name = "export_svd", skip_all)]
pub(super) fn export_svd(
    arguments: RegisterExportArgs,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let output = arguments
        .output
        .as_deref()
        .or(paths.svd_output.as_deref())
        .ok_or("registers export-svd requires --output PATH or [registers.svd] output")
        .map_err(crate::Error::invalid)?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "release SVD denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (contents, summary) = workspace.render_svd()?;
    super::super::super::generated_output::write_or_check(
        output,
        &contents,
        arguments.check,
        "SVD",
    )?;
    emit_svd(
        if arguments.check {
            "verified"
        } else {
            "written"
        },
        &summary,
        output,
    );
    Ok(true)
}

#[tracing::instrument(name = "generate_pac", skip_all)]
pub(super) fn generate_pac_source(
    arguments: RegisterPacArgs,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let configured = paths.pac.as_ref();
    let output = arguments
        .output
        .as_deref()
        .or_else(|| configured.map(|pac| pac.output.as_path()))
        .ok_or("registers generate-pac requires --output PATH or [registers.pac] output")
        .map_err(crate::Error::invalid)?;
    let target = arguments
        .target
        .as_deref()
        .map(PacTarget::parse)
        .unwrap_or_else(|| PacTarget::parse(configured.map_or("none", |pac| &pac.target)))?;
    let edition = arguments
        .edition
        .as_deref()
        .map(PacEdition::parse)
        .unwrap_or_else(|| PacEdition::parse(configured.map_or("2024", |pac| &pac.edition)))?;
    let api_pack_path = if arguments.no_api_pack {
        None
    } else {
        arguments.api_pack.as_deref().or(paths.api_pack.as_deref())
    };
    let api_pack = api_pack_path.map(PacApiPack::load).transpose()?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "PAC generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let source = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    super::super::super::generated_output::write_or_check(output, &source, arguments.check, "PAC")?;
    emit_pac(
        if arguments.check {
            "verified"
        } else {
            "written"
        },
        target,
        edition,
        &svd_summary,
        api_pack_path,
        output,
    );
    Ok(true)
}

pub(super) fn generate_bindings(
    arguments: RegisterBindingsArgs,
    paths: &crate::project::RegisterWorkspacePaths,
) -> Result<bool> {
    let configured = paths.bindings.as_ref();
    let output = arguments
        .output
        .as_deref()
        .or_else(|| configured.map(|bindings| bindings.output.as_path()))
        .ok_or("registers generate-bindings requires --output PATH or [registers.bindings] output")
        .map_err(crate::Error::invalid)?;
    let crate_name = arguments.crate_name
        .as_deref()
        .or_else(|| configured.map(|bindings| bindings.crate_name.as_str()))
        .ok_or(
            "registers generate-bindings requires --crate-name NAME or [registers.bindings] crate-name",
        ).map_err(crate::Error::invalid)?;
    let workspace = ProjectRegisterWorkspace::load(&paths.facts, &paths.model)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "PAC binding generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let contents = open_esp_radio_register_model::generate_pac_binding_index(&svd, crate_name)?;
    super::super::super::generated_output::write_or_check(
        output,
        &contents,
        arguments.check,
        "PAC binding index",
    )?;
    emit_bindings(
        if arguments.check {
            "verified"
        } else {
            "written"
        },
        crate_name,
        &svd_summary,
        output,
    );
    Ok(true)
}
