//! Git source inventory includes unstaged moves and excludes private/build inputs.

use crate::{Context, Result, process};
use std::{
    collections::BTreeSet,
    path::{Component, PathBuf},
};

pub fn source_files(context: &Context) -> Result<Vec<PathBuf>> {
    let output = process::capture(context.command("git").args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    let mut files = BTreeSet::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
    {
        #[cfg(unix)]
        let relative = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
        };
        #[cfg(not(unix))]
        let relative = PathBuf::from(std::str::from_utf8(bytes)?);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("Git source path is not contained: {}", relative.display()).into());
        }
        if relative.components().any(|part| matches!(part, Component::Normal(name) if name == "_oracles" || name == "target")) { continue; }
        let path = context.root.join(&relative);
        if !path.try_exists()? {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if !path.canonicalize()?.starts_with(&context.root) {
            return Err(format!("source path escaped repository: {}", path.display()).into());
        }
        files.insert(path);
    }
    Ok(files.into_iter().collect())
}

pub fn source_manifests(context: &Context) -> Result<Vec<PathBuf>> {
    Ok(source_files(context)?
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .collect())
}
