//! Typed, executable follow-up actions shared by every frontend.

use std::{ffi::OsStr, path::PathBuf};

use serde::Serialize;

use crate::Result;

/// Project resolution context carried by an executable follow-up action.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectContextRequirement {
    ProjectOnly,
    Target,
    RunSpec,
    Analysis,
}

impl ProjectContextRequirement {
    pub(super) const fn target_spec(self) -> bool {
        matches!(self, Self::Target | Self::RunSpec | Self::Analysis)
    }

    pub(super) const fn run_spec(self) -> bool {
        matches!(self, Self::RunSpec | Self::Analysis)
    }

    pub(super) const fn register_catalog(self) -> bool {
        matches!(self, Self::Analysis)
    }
}

/// One command that can be executed without reparsing a rendered shell line.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ExecutableAction {
    /// Complete argument vector, including logical `argv[0] == "blobray"`.
    pub argv: Vec<String>,
    /// Absolute directory from which the source invocation was resolved.
    pub working_directory: PathBuf,
    /// Resolution overrides included in `argv`.
    pub context: ProjectContextRequirement,
}

impl ExecutableAction {
    pub(crate) fn new(
        argv: Vec<String>,
        working_directory: PathBuf,
        context: ProjectContextRequirement,
    ) -> Result<Self> {
        if argv.first().map(String::as_str) != Some("blobray") {
            return Err(crate::Error::invalid(
                "executable action argv must start with logical argv[0] \"blobray\"",
            ));
        }
        if argv.iter().any(String::is_empty) {
            return Err(crate::Error::invalid(
                "executable action argv must not contain empty arguments",
            ));
        }
        if !working_directory.is_absolute() {
            return Err(crate::Error::invalid(format!(
                "executable action working directory must be absolute: {}",
                working_directory.display()
            )));
        }
        if working_directory.to_str().is_none() {
            return Err(crate::Error::invalid(
                "executable action working directory is not valid UTF-8 and cannot be serialized",
            ));
        }
        Ok(Self {
            argv,
            working_directory,
            context,
        })
    }

    /// Stable, unambiguous identity for action IDs and coalescing.
    pub(crate) fn canonical_execution_key(&self) -> String {
        fn append(key: &mut String, value: &str) {
            use std::fmt::Write as _;

            let _ = write!(key, "{}:", value.len());
            key.push_str(value);
        }

        let mut key = String::new();
        append(
            &mut key,
            self.working_directory
                .to_str()
                .expect("ExecutableAction validates a UTF-8 working directory"),
        );
        append(
            &mut key,
            match self.context {
                ProjectContextRequirement::ProjectOnly => "project-only",
                ProjectContextRequirement::Target => "target",
                ProjectContextRequirement::RunSpec => "run-spec",
                ProjectContextRequirement::Analysis => "analysis",
            },
        );
        for argument in &self.argv {
            append(&mut key, argument);
        }
        key
    }

    /// Copyable POSIX-shell presentation for human output only.
    pub(crate) fn render_posix(&self) -> String {
        self.argv
            .iter()
            .map(|argument| crate::shell::arg(OsStr::new(argument)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) fn inspect_label(&self) -> String {
        if self.argv.get(1).map(String::as_str) != Some("inspect") {
            return self.render_posix();
        }
        self.argv[2..]
            .iter()
            .take_while(|argument| argument.as_str() != "--project")
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_preserves_argument_boundaries() {
        let directory = std::env::current_dir().unwrap();
        let split = ExecutableAction::new(
            vec!["blobray".into(), "inspect".into(), "a".into(), "b".into()],
            directory.clone(),
            ProjectContextRequirement::Analysis,
        )
        .unwrap();
        let joined = ExecutableAction::new(
            vec!["blobray".into(), "inspect".into(), "a b".into()],
            directory,
            ProjectContextRequirement::Analysis,
        )
        .unwrap();

        assert_ne!(
            split.canonical_execution_key(),
            joined.canonical_execution_key()
        );
    }

    #[test]
    fn rendering_quotes_each_argument_without_changing_argv() {
        let action = ExecutableAction::new(
            vec![
                "blobray".into(),
                "inspect".into(),
                "function".into(),
                "archive:has space;$(touch nope)".into(),
            ],
            std::env::current_dir().unwrap(),
            ProjectContextRequirement::Analysis,
        )
        .unwrap();

        assert_eq!(
            action.render_posix(),
            "blobray inspect function 'archive:has space;$(touch nope)'"
        );
        assert_eq!(action.argv[3], "archive:has space;$(touch nope)");
    }

    #[test]
    fn relative_working_directory_is_rejected() {
        let error = ExecutableAction::new(
            vec!["blobray".into(), "project".into(), "status".into()],
            "relative".into(),
            ProjectContextRequirement::ProjectOnly,
        )
        .unwrap_err();

        assert!(error.to_string().contains("must be absolute"));
    }
}
