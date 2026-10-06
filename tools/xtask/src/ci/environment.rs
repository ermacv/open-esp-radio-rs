//! Actual tool versions, flags and OS that qualify reusable CI coverage.
//! Weekly runner image versions do not enter that qualification.

use super::model::{CommonEnvironment, Environment};
use crate::Result;
use crate::registry::{Program, Workflow};
use std::{collections::BTreeMap, path::Path};

fn version(root: &Path, name: &std::ffi::OsStr, arguments: &[&str]) -> Result<String> {
    let output = oer_process::capture(
        std::process::Command::new(name)
            .current_dir(root)
            .args(arguments),
    )?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub(super) fn read(root: &Path, programs: &[Program]) -> Result<Environment> {
    let mut flags: BTreeMap<String, String> = [
        "CARGO",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_BOOTSTRAP",
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CC",
        "CFLAGS",
        "CXX",
        "CXXFLAGS",
        "AR",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), std::env::var(name).unwrap_or_default()))
    .collect();
    flags.extend(std::env::vars().filter(|(name, _)| {
        name.starts_with("CARGO_PROFILE_")
            || name.starts_with("CARGO_BUILD_")
            || name.starts_with("CARGO_TARGET_")
    }));
    Ok(Environment {
        common: CommonEnvironment {
            image: std::env::var("ImageOS").unwrap_or_else(|_| "local".to_owned()),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            rustc: version(
                root,
                &oer_toolchain::program(oer_toolchain::Tool::Rustc)?,
                &["-vV"],
            )?,
            cargo: version(
                root,
                &oer_toolchain::program(oer_toolchain::Tool::Cargo)?,
                &["-Vv"],
            )?,
            flags,
        },
        programs: programs
            .iter()
            .map(|program| {
                Ok((
                    program.id.to_owned(),
                    version(root, program.program.as_ref(), program.arguments)?,
                ))
            })
            .collect::<Result<_>>()?,
    })
}

pub(super) fn read_workflow(root: &Path, workflow: Workflow) -> Result<Environment> {
    read(root, &workflow.spec().programs()?)
}

pub fn write_environment(root: &Path, workflow: Workflow, output: &Path) -> Result<()> {
    oer_durable::atomic_json(output, &read_workflow(root, workflow)?)
}

pub(super) fn matches(expected: &Environment, actual: &Environment) -> bool {
    actual.common == expected.common
        && actual
            .programs
            .iter()
            .all(|(id, version)| expected.programs.get(id) == Some(version))
}

pub(super) fn check(root: &Path, expected: &Environment, programs: &[Program]) -> Result<bool> {
    Ok(matches(expected, &read(root, programs)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_and_program_changes_only_disqualify_reuse() {
        let root = oer_process::built_root();
        let original = read(&root, &[]).unwrap();
        let mut expected = original.clone();
        expected.common.rustc = "different compiler version".into();
        assert!(!check(&root, &expected, &[]).unwrap());
        for id in ["clang", "lld"] {
            let mut expected = original.clone();
            expected.programs.insert(id.into(), "old".into());
            let mut actual = original.clone();
            actual.programs.insert(id.into(), "new".into());
            assert!(!matches(&expected, &actual));
            actual.programs.insert(id.into(), "old".into());
            assert!(matches(&expected, &actual));
            // Other jobs do not need this declared conformance program.
            assert!(matches(&expected, &original));
        }
        assert!(check(&root, &original, &[]).unwrap());
    }
}
