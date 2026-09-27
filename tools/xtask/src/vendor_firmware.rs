//! Build the tracked vendor firmware of a chip against its pinned ESP-IDF.
//!
//! The vendor firmware under `verification/<chip>/hil-vendor/<app>/` runs
//! vendor code on the board for hardware cross-checks. Every build uses the
//! ESP-IDF revision pinned by the chip's `artifacts.toml`, checked out in
//! `target/vendor-firmware/esp-idf/<revision>/`. Each IDF submodule whose
//! upstream repository is itself a pinned source is checked out at the
//! pinned revision instead and recorded in the tree's index, so the IDF
//! submodule check keeps it; every pinned artifact in such a submodule is
//! verified. After linking, every archive of the image named like a pinned
//! artifact must be that artifact. The IDF tools live in
//! `target/vendor-firmware/idf-tools`, apart from any user installation.
//! Outputs land in `target/vendor-firmware/<chip>/<app>/` with a
//! `build.json` recording the pins and the image digest.
use crate::vendor_fetch::{self, GitPin};
use crate::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build root, relative to the repository root.
const OUTPUT: &str = "target/vendor-firmware";
/// Pinned source id of the ESP-IDF tree.
const IDF_SOURCE: &str = "esp-idf";
/// IDF tool installation below [`OUTPUT`].
const TOOLS: &str = "idf-tools";
/// Marker of a completed tool installation for one IDF revision.
const TOOLS_MARKER: &str = ".oer-installed";
/// Vendor firmware projects of a chip, below `verification/<chip>/`.
const PROJECTS: &str = "hil-vendor";
/// Extension of the archives the image links.
const ARCHIVE_EXTENSION: &str = ".a";

fn git(directory: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} in {}: {}",
            args.join(" "),
            directory.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Make `directory` a checkout of `repository` at `revision`, fetching only
/// that commit. The checkout's `origin` is `repository`, so git resolves
/// relative submodule URLs against the upstream rather than the directory.
fn checkout(directory: &Path, repository: &str, revision: &str) -> Result<()> {
    if git(directory, &["rev-parse", "HEAD"]).is_ok_and(|head| head == revision)
        && directory.join(".git").exists()
    {
        if git(directory, &["remote", "get-url", "origin"]).is_err() {
            git(directory, &["remote", "add", "origin", repository])?;
            // Submodules initialized without an origin resolved against the
            // directory; take their URLs from `.gitmodules` again.
            if directory.join(".gitmodules").is_file() {
                git(directory, &["submodule", "sync", "--quiet"])?;
            }
        }
        return Ok(());
    }
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    std::fs::create_dir_all(directory)?;
    git(directory, &["init", "--quiet"])?;
    git(directory, &["remote", "add", "origin", repository])?;
    git(
        directory,
        &["fetch", "--quiet", "--depth", "1", "origin", revision],
    )?;
    git(directory, &["checkout", "--quiet", "FETCH_HEAD"])?;
    Ok(())
}

/// `url` of a submodule, resolved against the superproject `repository` as
/// git resolves relative submodule URLs, without a `.git` suffix.
fn resolve(repository: &str, url: &str) -> String {
    let mut base = repository.trim_end_matches('/').to_owned();
    let mut rest = url;
    loop {
        if let Some(next) = rest.strip_prefix("../") {
            if let Some(index) = base.rfind('/') {
                base.truncate(index);
            }
            rest = next;
        } else if let Some(next) = rest.strip_prefix("./") {
            rest = next;
        } else {
            break;
        }
    }
    if rest.len() == url.len() {
        normalize(url)
    } else {
        normalize(&format!("{base}/{rest}"))
    }
}

/// `repository` without a trailing slash or `.git` suffix.
fn normalize(repository: &str) -> String {
    repository
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_owned()
}

/// Submodule paths of `tree` by resolved upstream repository.
fn submodules(tree: &Path, repository: &str) -> Result<BTreeMap<String, String>> {
    let listing = git(
        tree,
        &[
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.(path|url)$",
        ],
    )?;
    let (mut paths, mut urls) = (BTreeMap::new(), BTreeMap::new());
    for line in listing.lines() {
        let (key, value) = line.split_once(' ').ok_or("malformed .gitmodules")?;
        if let Some(name) = key.strip_suffix(".path") {
            paths.insert(name.to_owned(), value.to_owned());
        } else if let Some(name) = key.strip_suffix(".url") {
            urls.insert(name.to_owned(), resolve(repository, value));
        }
    }
    Ok(urls
        .into_iter()
        .filter_map(|(name, url)| Some((url, paths.get(&name)?.clone())))
        .collect())
}

#[derive(Clone, Serialize, serde::Deserialize)]
struct Override {
    source: String,
    submodule: String,
    revision: String,
}

/// Check out the pinned ESP-IDF with its pinned submodules; returns the tree
/// and the submodules replaced.
fn prepare_tree(root: &Path, pins: &[GitPin]) -> Result<(PathBuf, Vec<Override>)> {
    let idf = pins
        .iter()
        .find(|p| p.id == IDF_SOURCE)
        .ok_or_else(|| format!("no `{IDF_SOURCE}` source pinned"))?;
    let tree = root.join(OUTPUT).join(IDF_SOURCE).join(&idf.revision);
    checkout(&tree, &idf.repository, &idf.revision)?;
    let modules = submodules(&tree, &idf.repository)?;
    let mut overrides = vec![];
    for pin in pins.iter().filter(|p| p.id != IDF_SOURCE) {
        let repository = normalize(&pin.repository);
        let Some(path) = modules.get(&repository) else {
            continue;
        };
        let directory = tree.join(path);
        checkout(&directory, &pin.repository, &pin.revision)?;
        git(
            &tree,
            &[
                "update-index",
                "--cacheinfo",
                &format!("160000,{},{path}", pin.revision),
            ],
        )?;
        git(&tree, &["submodule", "init", "--quiet", path])?;
        for (artifact, sha256) in &pin.artifacts {
            let actual = vendor_fetch::sha256(&directory.join(artifact))?;
            if &actual != sha256 {
                return Err(format!("{path}/{artifact}: sha256 {actual}, pinned {sha256}").into());
            }
        }
        overrides.push(Override {
            source: pin.id.clone(),
            submodule: path.clone(),
            revision: pin.revision.clone(),
        });
    }
    Ok((tree, overrides))
}

fn bash(script: &str, env: &[(&str, &Path)], log: &Path) -> Result<()> {
    let file = std::fs::File::create(log)?;
    let status = Command::new("bash")
        .args(["-ec", script])
        .envs(env.iter().map(|(k, v)| (*k, v.as_os_str())))
        .stdout(file.try_clone()?)
        .stderr(file)
        .status()?;
    if !status.success() {
        return Err(format!("command failed; see {}", log.display()).into());
    }
    Ok(())
}

/// Install the IDF tools of `tree` for `chip` once per revision.
fn install_tools(root: &Path, tree: &Path, chip: &str, revision: &str) -> Result<PathBuf> {
    let tools = root.join(OUTPUT).join(TOOLS);
    let marker = tools.join(format!("{TOOLS_MARKER}-{revision}-{chip}"));
    if !marker.is_file() {
        std::fs::create_dir_all(&tools)?;
        bash(
            r#""$IDF_PATH/install.sh" "$OER_CHIP""#,
            &[
                ("IDF_PATH", tree),
                ("IDF_TOOLS_PATH", &tools),
                ("OER_CHIP", Path::new(chip)),
            ],
            &tools.join(format!("install-{revision}.log")),
        )?;
        std::fs::write(&marker, "")?;
    }
    Ok(tools)
}

/// Archives the linker map `map` names below `tree` that a pinned artifact
/// describes but that differ from it. A linked archive is described by a pin
/// when its path ends with the pinned path relative to the pin's checkout:
/// `esp32s31/libphy.a` keeps another chip's `esp32c5/libphy.a` apart, and a
/// pin without a directory, such as `libble_app.a`, matches by file name.
fn unpinned_archives(map: &str, tree: &Path, pins: &[GitPin]) -> Result<Vec<String>> {
    let tree = tree.to_string_lossy();
    let mut linked = std::collections::BTreeSet::new();
    for token in map.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        if token.starts_with(tree.as_ref()) && token.ends_with(ARCHIVE_EXTENSION) {
            linked.insert(token.to_owned());
        }
    }
    let mut unpinned = vec![];
    for archive in linked {
        let digests = pins
            .iter()
            .flat_map(|pin| &pin.artifacts)
            .filter(|(path, _)| archive.ends_with(&format!("/{}", path.trim_start_matches('/'))))
            .map(|(_, sha256)| sha256.as_str())
            .collect::<Vec<_>>();
        if !digests.is_empty()
            && !digests.contains(&vendor_fetch::sha256(Path::new(&archive))?.as_str())
        {
            unpinned.push(archive);
        }
    }
    Ok(unpinned)
}

/// `build.json` of one built project.
#[derive(Serialize, serde::Deserialize)]
pub struct Build {
    pub chip: String,
    pub project: String,
    pub idf_revision: String,
    overrides: Vec<Override>,
    pub application: String,
    pub application_sha256: String,
    pub elf: String,
}

/// One ESP-IDF project to build: its directory and its target chip.
pub struct Project {
    pub name: String,
    pub source: PathBuf,
    pub chip: String,
}

/// Output directory of `project`, holding `build/`, `sdkconfig` and
/// `build.json`.
pub fn output(root: &Path, project: &Project) -> PathBuf {
    root.join(OUTPUT).join(&project.chip).join(&project.name)
}

/// The vendor firmware projects of `chip`, by name.
pub fn projects(ctx: &Context, chip: &str) -> Result<Vec<String>> {
    let directory = ctx.root.join("verification").join(chip).join(PROJECTS);
    let mut names = vec![];
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        if entry.path().join("CMakeLists.txt").is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Build the vendor firmware `project` of `chip`, or every project; prints
/// each application image.
pub fn run(ctx: &Context, chip: &str, project: Option<&str>) -> Result<()> {
    let available = projects(ctx, chip)?;
    let selected: Vec<&str> = match project {
        Some(name) if available.iter().any(|p| p == name) => vec![name],
        Some(name) => {
            return Err(format!(
                "no vendor firmware project {name}; available: {}",
                available.join(", ")
            )
            .into());
        }
        None => available.iter().map(String::as_str).collect(),
    };
    let projects = selected
        .into_iter()
        .map(|name| Project {
            name: name.to_owned(),
            source: ctx
                .root
                .join("verification")
                .join(chip)
                .join(PROJECTS)
                .join(name),
            chip: chip.to_owned(),
        })
        .collect::<Vec<_>>();
    for (project, build) in projects.iter().zip(build(ctx, chip, &projects)?) {
        println!("{:<16} {}", project.name, build.application);
    }
    Ok(())
}

/// Build `projects` against the ESP-IDF and vendor archives pinned by the
/// `artifacts.toml` of `pins`, each for its own target chip, and write their
/// `build.json`. An image linking an archive named like a pinned artifact but
/// differing from it is refused.
pub fn build(ctx: &Context, pins: &str, projects: &[Project]) -> Result<Vec<Build>> {
    let pins = vendor_fetch::git_pins(ctx, pins)?;
    let (tree, overrides) = prepare_tree(&ctx.root, &pins)?;
    let revision = git(&tree, &["rev-parse", "HEAD"])?;
    let mut builds = Vec::with_capacity(projects.len());
    for project in projects {
        let name = &project.name;
        let chip = &project.chip;
        let tools = install_tools(&ctx.root, &tree, chip, &revision)?;
        let output = output(&ctx.root, project);
        let build = output.join("build");
        std::fs::create_dir_all(&output)?;
        // `--preview`: the pinned IDF lists the chip as a preview target.
        bash(
            r#". "$IDF_PATH/export.sh" >/dev/null
idf.py --preview -C "$OER_PROJECT" -B "$OER_BUILD" -DIDF_TARGET="$OER_CHIP" -DSDKCONFIG="$OER_SDKCONFIG" build"#,
            &[
                ("IDF_PATH", &tree),
                ("IDF_TOOLS_PATH", &tools),
                ("OER_PROJECT", &project.source),
                ("OER_BUILD", &build),
                ("OER_CHIP", Path::new(chip)),
                ("OER_SDKCONFIG", &output.join("sdkconfig")),
            ],
            &output.join("build.log"),
        )?;
        let description: serde_json::Value =
            serde_json::from_slice(&std::fs::read(build.join("project_description.json"))?)?;
        let field = |key: &str| -> Result<String> {
            description[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("project description lacks `{key}`").into())
        };
        let application = build.join(field("app_bin")?);
        let elf = build.join(field("app_elf")?);
        let map = std::fs::read_to_string(elf.with_extension("map"))?;
        let unpinned = unpinned_archives(&map, &tree, &pins)?;
        if !unpinned.is_empty() {
            return Err(format!("{name} links unpinned archives: {}", unpinned.join(", ")).into());
        }
        let record = Build {
            chip: chip.to_owned(),
            project: name.to_owned(),
            idf_revision: revision.clone(),
            overrides: overrides.clone(),
            application_sha256: vendor_fetch::sha256(&application)?,
            application: application.display().to_string(),
            elf: elf.display().to_string(),
        };
        let mut bytes = serde_json::to_vec_pretty(&record)?;
        bytes.push(b'\n');
        std::fs::write(output.join("build.json"), bytes)?;
        builds.push(record);
    }
    Ok(builds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_resolves_relative_submodules_against_its_upstream() {
        let upstream = tempfile::tempdir().unwrap();
        let run = |directory: &Path, args: &[&str]| git(directory, args).unwrap();
        run(upstream.path(), &["init", "--quiet"]);
        run(
            upstream.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        run(upstream.path(), &["config", "user.name", "test"]);
        run(
            upstream.path(),
            &["config", "uploadpack.allowReachableSHA1InWant", "true"],
        );
        std::fs::write(upstream.path().join("file"), "x").unwrap();
        run(upstream.path(), &["add", "file"]);
        run(upstream.path(), &["commit", "--quiet", "-m", "one"]);
        let revision = run(upstream.path(), &["rev-parse", "HEAD"]);
        let repository = upstream.path().to_string_lossy().into_owned();
        let target = tempfile::tempdir().unwrap();
        let tree = target.path().join("tree");
        checkout(&tree, &repository, &revision).unwrap();
        assert_eq!(run(&tree, &["remote", "get-url", "origin"]), repository);
        // An earlier checkout without an origin gains one.
        run(&tree, &["remote", "remove", "origin"]);
        checkout(&tree, &repository, &revision).unwrap();
        assert_eq!(run(&tree, &["remote", "get-url", "origin"]), repository);
    }

    #[test]
    fn relative_submodule_urls_resolve_against_the_superproject() {
        let idf = "https://github.com/espressif/esp-idf";
        assert_eq!(
            resolve(idf, "../../espressif/esp-phy-lib.git"),
            "https://github.com/espressif/esp-phy-lib"
        );
        assert_eq!(
            resolve(idf, "https://github.com/espressif/esp-coex-lib.git"),
            "https://github.com/espressif/esp-coex-lib"
        );
        assert_eq!(
            normalize("https://github.com/espressif/esp-phy-lib/"),
            "https://github.com/espressif/esp-phy-lib"
        );
    }

    #[test]
    fn only_differing_archives_named_like_pins_are_unpinned() {
        let directory = tempfile::tempdir().unwrap();
        let tree = directory.path();
        for chip in ["esp32s31", "esp32c5"] {
            std::fs::create_dir_all(tree.join(chip)).unwrap();
        }
        std::fs::write(tree.join("esp32s31/libphy.a"), b"pinned").unwrap();
        std::fs::write(tree.join("esp32s31/libbtbb.a"), b"other").unwrap();
        std::fs::write(tree.join("esp32c5/libphy.a"), b"another chip").unwrap();
        std::fs::write(tree.join("libmain.a"), b"local").unwrap();
        std::fs::create_dir_all(tree.join("esp32s31-bt-lib")).unwrap();
        std::fs::write(tree.join("esp32s31-bt-lib/libble_app.a"), b"substituted").unwrap();
        let digest = |bytes: &[u8]| {
            let path = tree.join("digest");
            std::fs::write(&path, bytes).unwrap();
            vendor_fetch::sha256(&path).unwrap()
        };
        let pins = [GitPin {
            id: "esp-phy-lib".into(),
            repository: String::new(),
            revision: String::new(),
            artifacts: vec![
                ("esp32s31/libphy.a".into(), digest(b"pinned")),
                ("esp32s31/libbtbb.a".into(), digest(b"pinned")),
                // A pin without a directory matches by file name.
                ("libble_app.a".into(), digest(b"pinned")),
            ],
        }];
        let map = format!(
            "LOAD {0}/esp32s31/libphy.a\n {0}/esp32s31/libbtbb.a(x.o)\n{0}/esp32c5/libphy.a\n\
             {0}/libmain.a\n{0}/esp32s31-bt-lib/libble_app.a\n",
            tree.display()
        );
        let unpinned = unpinned_archives(&map, tree, &pins).unwrap();
        assert_eq!(
            unpinned,
            [
                format!("{}/esp32s31-bt-lib/libble_app.a", tree.display()),
                format!("{}/esp32s31/libbtbb.a", tree.display()),
            ]
        );
    }
}
