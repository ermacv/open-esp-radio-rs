//! Where a chip's HIL images are built from: its agent's workspace and
//! package and, for a staged boot, the platform's bootstrap. The paths
//! follow the chip id by convention (`hil/targets/<id>`).

use std::path::{Path, PathBuf};

use oer_chip_profile::Profile;

/// The Cargo workspace of the chip's HIL agent firmware.
pub fn workspace(profile: &Profile, root: &Path) -> PathBuf {
    profile.directory(root, "hil/targets")
}

/// The package of the chip's HIL agent firmware in that workspace.
pub fn package(profile: &Profile) -> String {
    format!("oer-{}-hil-agent", profile.id)
}

/// The manifest of that package, which declares the features the agent's
/// images select from.
pub fn manifest(profile: &Profile, root: &Path) -> PathBuf {
    workspace(profile, root).join("agent/Cargo.toml")
}

/// Every `(workspace, package)` a HIL image of the chip is built from: the
/// HIL agent and, for a staged boot, the platform's bootstrap.
pub fn image_packages(profile: &Profile, root: &Path) -> Vec<(PathBuf, String)> {
    let mut packages = vec![(workspace(profile, root), package(profile))];
    if let Some(bootstrap) = profile.bootstrap_package() {
        packages.push((profile.platform_workspace(root), bootstrap));
    }
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    #[test]
    fn every_chip_s_hil_agent_workspace_declares_its_package() {
        let root = repository();
        for chip in Profile::all(&root).unwrap() {
            let workspace = workspace(&chip, &root);
            let package = package(&chip);
            let declared = std::fs::read_dir(&workspace)
                .unwrap()
                .filter_map(|entry| {
                    let manifest = entry.unwrap().path().join("Cargo.toml");
                    let text = std::fs::read_to_string(manifest).ok()?;
                    let manifest: toml::Table = toml::from_str(&text).ok()?;
                    Some(manifest.get("package")?.get("name")?.as_str()?.to_owned())
                })
                .any(|name| name == package);
            assert!(
                declared,
                "{} declares no package {package}",
                workspace.display()
            );
            assert!(manifest(&chip, &root).is_file());
        }
    }

    #[test]
    fn every_image_package_is_declared_in_its_workspace() {
        let root = repository();
        for chip in Profile::all(&root).unwrap() {
            for (workspace, package) in image_packages(&chip, &root) {
                let text = std::fs::read_to_string(workspace.join("Cargo.toml")).unwrap();
                let members: toml::Table = toml::from_str(&text).unwrap();
                let listed = members["workspace"]["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|member| {
                        let manifest = workspace.join(member.as_str()?).join("Cargo.toml");
                        let manifest: toml::Table =
                            toml::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
                        Some(manifest.get("package")?.get("name")?.as_str()?.to_owned())
                    })
                    .any(|name| name == package);
                assert!(listed, "{} has no member {package}", workspace.display());
            }
        }
    }
}
