//! Shared write/check policy for project-owned generated artifacts.

use std::{fs, path::Path};

use crate::Result;

pub(super) fn write_or_check(path: &Path, contents: &str, check: bool, kind: &str) -> Result<()> {
    if check {
        let existing = fs::read_to_string(path).map_err(|error| {
            format!("cannot check generated {kind} {}: {error}", path.display())
        })?;
        if existing != contents {
            return Err(format!(
                "generated {kind} differs from {}; rerun without --check",
                path.display()
            )
            .into());
        }
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_never_creates_or_updates_an_output() {
        let directory = std::env::temp_dir().join(format!(
            "vendor-workbench-generated-output-{}",
            std::process::id()
        ));
        let path = directory.join("nested/report.txt");
        let missing = write_or_check(&path, "expected\n", true, "fixture").unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("cannot check generated fixture")
        );
        assert!(!path.exists());

        write_or_check(&path, "original\n", false, "fixture").unwrap();
        let stale = write_or_check(&path, "changed\n", true, "fixture").unwrap_err();
        assert!(stale.to_string().contains("rerun without --check"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");

        write_or_check(&path, "original\n", true, "fixture").unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
