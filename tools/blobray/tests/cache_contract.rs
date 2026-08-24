use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn blobray() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_blobray-generic"));
    command.env_remove("RUST_LOG");
    command
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

struct TemporaryProject {
    root: PathBuf,
    manifest: PathBuf,
}

impl TemporaryProject {
    fn from_public_fixture(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "blobray-cache-contract-{label}-{}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("remove stale cache contract fixture");
        }
        std::fs::create_dir_all(&root).expect("create cache contract fixture");
        let source = repository_root().join("tools/blobray/tests/fixtures/generic-project");
        for name in [
            "vendor-project.toml",
            "target.toml",
            "chip.toml",
            "memory.toml",
        ] {
            std::fs::copy(source.join(name), root.join(name))
                .unwrap_or_else(|error| panic!("copy public fixture {name}: {error}"));
        }
        Self {
            manifest: root.join("vendor-project.toml"),
            root,
        }
    }

    fn stats(&self, format: Option<&str>) -> Output {
        let mut command = blobray();
        command
            .current_dir(repository_root())
            .args(["project", "cache", "stats", "--project"])
            .arg(&self.manifest)
            .args(["--color", "never", "--progress", "never"]);
        if let Some(format) = format {
            command.args(["--format", format]);
        }
        command.output().expect("run project cache stats")
    }
}

impl Drop for TemporaryProject {
    fn drop(&mut self) {
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root).expect("remove cache contract fixture");
        }
    }
}

#[test]
fn fresh_cache_stats_is_typed_and_does_not_create_or_modify_the_project_tree() {
    let project = TemporaryProject::from_public_fixture("fresh-json");
    let before = tree_snapshot(&project.root);

    let output = project.stats(Some("json"));

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cache stats stdout is one JSON document");
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["command"], "project cache stats");
    assert_eq!(report["present"], false);
    assert_eq!(
        report["cache_root"],
        project
            .root
            .join("generated/.blobray-cache")
            .display()
            .to_string()
    );
    assert_eq!(
        report["database_path"],
        project
            .root
            .join("generated/.blobray-cache/queries.sqlite3")
            .display()
            .to_string()
    );
    assert_eq!(report["schema"], serde_json::Value::Null);
    for field in [
        "root_bytes",
        "database_bytes",
        "pack_bytes",
        "query_results",
        "inline_bytes",
        "dependencies",
        "objects",
        "object_payload_bytes",
        "stage_bindings",
        "stage_outputs",
        "live_objects",
        "live_record_bytes",
        "reclaimable_pack_bytes",
    ] {
        assert_eq!(report[field], 0, "unexpected fresh-cache field {field}");
    }
    assert_eq!(report["query_kinds"], serde_json::json!([]));
    assert_eq!(report["compaction"]["eligible_on_next_write"], false);
    assert_eq!(report["compaction"]["minimum_reclaimable_percent"], 25);
    assert_eq!(tree_snapshot(&project.root), before);
    assert!(!project.root.join("generated/.blobray-cache").exists());
}

#[test]
fn fresh_cache_stats_human_output_is_bounded_and_explains_lazy_creation() {
    let project = TemporaryProject::from_public_fixture("fresh-human");

    let output = project.stats(None);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("human output is UTF-8");
    assert!(stdout.contains("Project cache"), "stdout: {stdout}");
    assert!(stdout.contains("NOT CREATED"), "stdout: {stdout}");
    assert!(stdout.contains("created lazily"), "stdout: {stdout}");
    assert!(stdout.lines().count() <= 6, "unbounded stdout: {stdout}");
    assert!(stdout.len() <= 1024, "unbounded stdout: {stdout}");
    assert!(!project.root.join("generated/.blobray-cache").exists());
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
    let mut entries = BTreeMap::new();
    collect_tree(root, root, &mut entries);
    entries
}

fn collect_tree(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, TreeEntry>) {
    let mut children = std::fs::read_dir(directory)
        .expect("read project fixture directory")
        .map(|entry| entry.expect("read project fixture entry").path())
        .collect::<Vec<_>>();
    children.sort();
    for path in children {
        let relative = path
            .strip_prefix(root)
            .expect("fixture entry belongs to root")
            .to_owned();
        let file_type = std::fs::symlink_metadata(&path)
            .expect("inspect project fixture entry")
            .file_type();
        if file_type.is_dir() {
            entries.insert(relative, TreeEntry::Directory);
            collect_tree(root, &path, entries);
        } else if file_type.is_file() {
            entries.insert(
                relative,
                TreeEntry::File(std::fs::read(&path).expect("read project fixture file")),
            );
        } else if file_type.is_symlink() {
            entries.insert(
                relative,
                TreeEntry::Symlink(
                    std::fs::read_link(&path)
                        .expect("read project fixture symlink")
                        .into_os_string(),
                ),
            );
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum TreeEntry {
    Directory,
    File(Vec<u8>),
    Symlink(std::ffi::OsString),
}
