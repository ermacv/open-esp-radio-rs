//! The firmware of every supported chip, from its chip profile.
//!
//! The evaluator judges evidence of any chip: the workspaces, packages and
//! build files a HIL image reads come from `platform/<chip>/chip.toml` and
//! the conventions `oer-chip-profile` names, never from constants of one
//! chip.

use super::*;
use oer_chip_profile::Profile;

/// One chip's HIL image inputs, as repository-relative paths.
pub(crate) struct ChipFirmware {
    pub(crate) rust_target: String,
    /// `(workspace manifest, package)` of the HIL agent first, then of a
    /// staged boot's bootstrap.
    pub(crate) packages: Vec<(PathBuf, String)>,
    /// Workspace manifests, lock files and the policies the build reads.
    pub(crate) build_files: Vec<PathBuf>,
    /// The chip's HIL agent and platform directories.
    pub(crate) directories: Vec<PathBuf>,
}

/// The firmware of every chip with a profile under `root`.
pub(crate) fn all(root: &Path) -> Result<Vec<ChipFirmware>> {
    let relative = |path: PathBuf| -> Result<PathBuf> {
        Ok(path
            .strip_prefix(root)
            .map_err(|_| "profile path outside the repository")?
            .to_path_buf())
    };
    let mut chips = Vec::new();
    for profile in Profile::all(root).map_err(|error| error.to_string())? {
        let agent = relative(profile.hil_agent_workspace(root))?;
        let platform = relative(profile.platform_workspace(root))?;
        let mut packages = Vec::new();
        for (workspace, package) in profile.hil_image_packages(root) {
            packages.push((relative(workspace)?.join("Cargo.toml"), package));
        }
        let build_files = [
            agent.join("Cargo.toml"),
            agent.join("Cargo.lock"),
            agent.join("stack.toml"),
            platform.join("Cargo.toml"),
            platform.join("Cargo.lock"),
            platform.join("partitions/applications.csv"),
        ]
        .into_iter()
        .filter(|path| root.join(path).is_file())
        .collect();
        chips.push(ChipFirmware {
            rust_target: profile.rust_target,
            packages,
            build_files,
            directories: vec![agent, platform],
        });
    }
    Ok(chips)
}
