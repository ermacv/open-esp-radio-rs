//! Prepared register publications and read-only freshness checks.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::*;

pub(crate) struct PreparedPublication {
    output: PathBuf,
    contents: String,
    kind: &'static str,
}

impl PreparedPublication {
    pub(crate) fn output(&self) -> &Path {
        &self.output
    }

    pub(crate) fn contents(&self) -> &str {
        &self.contents
    }

    pub(crate) const fn kind(&self) -> &'static str {
        self.kind
    }
}

#[tracing::instrument(name = "prepare_project_svd", skip_all)]
pub(crate) fn prepare_project_svd(
    paths: &crate::project::RegisterWorkspacePaths,
    publication_mmio: &BTreeSet<(u32, u8)>,
) -> crate::Result<PreparedPublication> {
    let output = paths
        .svd_output
        .clone()
        .ok_or("project SVD publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_publication_workspace(paths, publication_mmio, "publication SVD")?;
    let (contents, _) = workspace.render_svd()?;
    Ok(PreparedPublication {
        output,
        contents,
        kind: "SVD",
    })
}

#[tracing::instrument(name = "prepare_project_pac_raw", skip_all)]
pub(crate) fn prepare_project_pac_raw(
    paths: &crate::project::RegisterWorkspacePaths,
    publication_mmio: &BTreeSet<(u32, u8)>,
) -> crate::Result<PreparedPublication> {
    let configured = paths
        .pac_raw
        .as_ref()
        .ok_or("project raw PAC publication is not configured")
        .map_err(crate::Error::invalid)?;
    let target = PacTarget::parse(&configured.target)?;
    let edition = PacEdition::parse(&configured.edition)?;
    let api_pack = paths
        .api_pack
        .as_deref()
        .map(PacApiPack::load)
        .transpose()?;
    let workspace = load_publication_workspace(paths, publication_mmio, "PAC generation")?;
    let (svd, _) = workspace.render_svd()?;
    let contents = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "raw PAC",
    })
}

#[tracing::instrument(name = "prepare_project_pac_api", skip_all)]
pub(crate) fn prepare_project_pac_api(
    paths: &crate::project::RegisterWorkspacePaths,
) -> crate::Result<PreparedPublication> {
    let pack_path = paths
        .api_pack
        .as_deref()
        .ok_or("project closed PAC API pack is not configured")
        .map_err(crate::Error::invalid)?;
    let output = paths
        .api_output
        .clone()
        .ok_or("project closed PAC API output is not configured")
        .map_err(crate::Error::invalid)?;
    let pack = validate_pac_api(paths)?.ok_or_else(|| {
        crate::Error::invalid(format!(
            "project closed PAC API pack {} is not configured",
            pack_path.display()
        ))
    })?;
    let edition = paths
        .pac_raw
        .as_ref()
        .map_or(Ok(PacEdition::E2024), |pac| PacEdition::parse(&pac.edition))?;
    let contents = format_generated_rust(&pack.render_facade_rust()?, edition)?;
    Ok(PreparedPublication {
        output,
        contents,
        kind: "closed PAC API",
    })
}

pub(crate) fn prepare_project_bindings(
    paths: &crate::project::RegisterWorkspacePaths,
    publication_mmio: &BTreeSet<(u32, u8)>,
) -> crate::Result<PreparedPublication> {
    let configured = paths
        .bindings
        .as_ref()
        .ok_or("project PAC binding publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_publication_workspace(paths, publication_mmio, "PAC binding generation")?;
    let (svd, _) = workspace.render_svd()?;
    let contents =
        open_esp_radio_register_model::generate_pac_binding_index(&svd, &configured.crate_name)?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC binding index",
    })
}

fn load_publication_workspace(
    paths: &crate::project::RegisterWorkspacePaths,
    publication_mmio: &BTreeSet<(u32, u8)>,
    operation: &str,
) -> crate::Result<ProjectRegisterWorkspace> {
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let unreviewed = workspace.unreviewed_in_mmio_scope(publication_mmio)?;
    if unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "{operation} denied {} unreviewed MMIO observations",
            unreviewed
        )));
    }
    Ok(workspace)
}
