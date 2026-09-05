//! Read-only views of checked-in project inputs, isolated from local bindings.

#[cfg(unix)]
pub struct ProjectFixture {
    root: std::path::PathBuf,
    pub manifest: std::path::PathBuf,
}

#[cfg(unix)]
impl ProjectFixture {
    pub fn checked_inputs(repository: &std::path::Path, label: &str, revisions: bool) -> Self {
        use std::{fs, os::unix::fs::symlink};

        let root = std::env::temp_dir().join(format!(
            "blobray-host-checked-{label}-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self {
            manifest: root.join("verification/vendor/projects/esp32s31/vendor-project.toml"),
            root,
        };
        // Preserve relative path relationships without copying or changing
        // checked-in evidence. No CLI operation in these tests writes outputs.
        for directory in ["driver", "registers", "tools", "qualification"] {
            symlink(repository.join(directory), fixture.root.join(directory)).unwrap();
        }
        let vendor = fixture.root.join("verification/vendor");
        fs::create_dir_all(vendor.join("projects/esp32s31")).unwrap();
        for directory in ["chips", "knowledge"] {
            symlink(
                repository.join("verification/vendor").join(directory),
                vendor.join(directory),
            )
            .unwrap();
        }
        let source = repository.join("verification/vendor/projects/esp32s31");
        let destination = fixture.manifest.parent().unwrap();
        for entry in fs::read_dir(&source).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if matches!(name.to_str(), Some("local.toml" | "generated"))
                || (!revisions && name == "revisions")
            {
                continue;
            }
            if name == "revisions" {
                let copied = destination.join("revisions");
                fs::create_dir_all(copied.join("snapshots")).unwrap();
                fs::copy(
                    entry.path().join("state.blobray"),
                    copied.join("state.blobray"),
                )
                .unwrap();
                for snapshot in fs::read_dir(entry.path().join("snapshots")).unwrap() {
                    let snapshot = snapshot.unwrap();
                    fs::copy(
                        snapshot.path(),
                        copied.join("snapshots").join(snapshot.file_name()),
                    )
                    .unwrap();
                }
            } else {
                symlink(entry.path(), destination.join(name)).unwrap();
            }
        }
        fixture
    }
}

#[cfg(unix)]
impl Drop for ProjectFixture {
    fn drop(&mut self) {
        // remove_dir_all removes these links themselves; it does not traverse
        // the repository directories to which they point.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
