//! Derived SVD, PAC and PAC-binding publication commands.

use super::*;

mod report;

use report::*;

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
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "release SVD denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (contents, summary) = workspace.render_svd()?;
    crate::application::generated_file::write_or_check(output, &contents, arguments.check, "SVD")?;
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
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "PAC generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let source = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    crate::application::generated_file::write_or_check(output, &source, arguments.check, "PAC")?;
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
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let workspace_summary = workspace.summary()?;
    if arguments.deny_unreviewed && workspace_summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "PAC binding generation denied {} unreviewed MMIO observations",
            workspace_summary.unreviewed
        )));
    }
    let (svd, svd_summary) = workspace.render_svd()?;
    let contents = open_esp_radio_register_model::generate_pac_binding_index(&svd, crate_name)?;
    crate::application::generated_file::write_or_check(
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
