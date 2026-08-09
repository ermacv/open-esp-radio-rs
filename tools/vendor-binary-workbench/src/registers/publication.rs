//! Prepared register publications and read-only freshness checks.

use std::path::{Path, PathBuf};

use super::*;

pub(crate) struct PreparedPublication {
    output: PathBuf,
    contents: String,
    kind: &'static str,
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

impl PreparedPublication {
    pub(crate) fn readiness(&self) -> crate::Result<PublicationReadiness> {
        match std::fs::read_to_string(&self.output) {
            Ok(existing) if existing == self.contents => Ok(PublicationReadiness::Current),
            Ok(_) => Ok(PublicationReadiness::Stale),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(PublicationReadiness::Missing)
            }
            Err(error) => Err(error.into()),
        }
    }

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
) -> crate::Result<PreparedPublication> {
    let output = paths
        .svd_output
        .clone()
        .ok_or("project SVD publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_release_workspace(paths, "release SVD")?;
    let (contents, _) = workspace.render_svd()?;
    Ok(PreparedPublication {
        output,
        contents,
        kind: "SVD",
    })
}

#[tracing::instrument(name = "prepare_project_pac", skip_all)]
pub(crate) fn prepare_project_pac(
    paths: &crate::project::RegisterWorkspacePaths,
) -> crate::Result<PreparedPublication> {
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
    let (svd, _) = workspace.render_svd()?;
    let contents = generate_pac_with_api(&svd, target, edition, api_pack.as_ref())?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC",
    })
}

pub(crate) fn prepare_project_bindings(
    paths: &crate::project::RegisterWorkspacePaths,
) -> crate::Result<PreparedPublication> {
    let configured = paths
        .bindings
        .as_ref()
        .ok_or("project PAC binding publication is not configured")
        .map_err(crate::Error::invalid)?;
    let workspace = load_release_workspace(paths, "PAC binding generation")?;
    let (svd, _) = workspace.render_svd()?;
    let contents =
        open_esp_radio_register_model::generate_pac_binding_index(&svd, &configured.crate_name)?;
    Ok(PreparedPublication {
        output: configured.output.clone(),
        contents,
        kind: "PAC binding index",
    })
}

fn load_release_workspace(
    paths: &crate::project::RegisterWorkspacePaths,
    operation: &str,
) -> crate::Result<ProjectRegisterWorkspace> {
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let summary = workspace.summary()?;
    if summary.unreviewed != 0 {
        return Err(crate::Error::invalid(format!(
            "{operation} denied {} unreviewed MMIO observations",
            summary.unreviewed
        )));
    }
    Ok(workspace)
}
