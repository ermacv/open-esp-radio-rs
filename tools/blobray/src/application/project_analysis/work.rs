//! Resolved work contracts retained from preparation through completion.

use super::pass_spec::{CachePass, CacheWork, PassKind};
use super::{ProjectAnalysisInput, ProjectAnalysisInputRequirement};
use crate::{
    Result,
    application::output_set::{OutputReceipt, OutputSet},
};
use std::path::PathBuf;

/// Immutable declaration. Configuration and I/O are not recomputed at completion.
pub(super) struct ResolvedWork {
    key: String,
    owner: PassKind,
    profile: Option<String>,
    configuration: String,
    inputs: Vec<ProjectAnalysisInput>,
    input_paths: Vec<PathBuf>,
    outputs: Vec<PathBuf>,
    check: bool,
    cacheable: bool,
}

pub(super) enum WorkDecision {
    Complete(crate::application::pipeline::StageRun),
    Execute(ExecutionWork),
}

/// A prepared operation owns its declaration until successful completion.
/// Consuming it once records exactly the inputs/outputs that were prepared.
pub(super) struct ExecutionWork {
    work: ResolvedWork,
    outputs: OutputSet,
}

impl ExecutionWork {
    pub(super) fn work(&self) -> &ResolvedWork {
        &self.work
    }
    pub(super) fn outputs(&self) -> &OutputSet {
        &self.outputs
    }
    pub(super) fn finish(self) -> Result<(ResolvedWork, Vec<OutputReceipt>)> {
        let receipts = self.outputs.receipts()?;
        Ok((self.work, receipts))
    }
}

impl ResolvedWork {
    pub(super) fn new(
        key: String,
        configuration: String,
        mut inputs: Vec<ProjectAnalysisInput>,
        outputs: Vec<PathBuf>,
        check: bool,
        provider_domain: Option<&str>,
    ) -> Result<Self> {
        let pass = CachePass::parse(&key)?;
        let owner = pass.spec.kind;
        let profile = match pass.work {
            CacheWork::Profile(id) => Some(id.to_owned()),
            _ => None,
        };
        let cacheable = pass.cacheable(provider_domain);
        inputs.sort_by(|a, b| a.path.cmp(&b.path).then(a.requirement.cmp(&b.requirement)));
        // Required sorts first: another optional use cannot weaken a requirement.
        inputs.dedup_by(|a, b| a.path == b.path);
        let input_paths = inputs.iter().map(|input| input.path.clone()).collect();
        Ok(Self {
            key,
            owner,
            profile,
            configuration,
            inputs,
            input_paths,
            outputs,
            check,
            cacheable,
        })
    }
    pub(super) fn key(&self) -> &str {
        &self.key
    }
    pub(super) fn owner(&self) -> PassKind {
        self.owner
    }
    pub(super) fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }
    pub(super) fn configuration(&self) -> &str {
        &self.configuration
    }
    pub(super) fn inputs(&self) -> &[ProjectAnalysisInput] {
        &self.inputs
    }
    pub(super) fn input_paths(&self) -> &[PathBuf] {
        &self.input_paths
    }
    pub(super) fn outputs(&self) -> &[PathBuf] {
        &self.outputs
    }
    pub(super) fn check(&self) -> bool {
        self.check
    }
    pub(super) fn cacheable(&self) -> bool {
        self.cacheable
    }
    pub(super) fn prepared(self) -> Result<ExecutionWork> {
        let outputs = OutputSet::new(&self.outputs, self.check)?;
        Ok(ExecutionWork {
            work: self,
            outputs,
        })
    }

    pub(super) fn validate_inputs(&self, deferred: &[PathBuf]) -> Result<()> {
        for input in &self.inputs {
            if deferred.contains(&input.path) {
                continue;
            }
            match std::fs::metadata(&input.path) {
                Ok(metadata) if metadata.is_file() || metadata.is_dir() => (),
                Ok(_) => {
                    return Err(crate::Error::invalid(format!(
                        "analysis input {} is neither a file nor a directory",
                        input.path.display()
                    )));
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && input.requirement == ProjectAnalysisInputRequirement::Optional => {}
                Err(error) => {
                    return Err(crate::Error::invalid(format!(
                        "analysis input {} is unavailable: {error}",
                        input.path.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(inputs: Vec<ProjectAnalysisInput>) -> ResolvedWork {
        ResolvedWork::new(
            "linked-ir:fixture".into(),
            "captured-configuration".into(),
            inputs,
            Vec::new(),
            false,
            Some("fixture-domain"),
        )
        .unwrap()
    }

    #[test]
    fn optional_use_cannot_hide_a_missing_required_input_or_expand_deferral() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing");
        for requirements in [
            [
                ProjectAnalysisInputRequirement::Optional,
                ProjectAnalysisInputRequirement::Required,
            ],
            [
                ProjectAnalysisInputRequirement::Required,
                ProjectAnalysisInputRequirement::Optional,
            ],
        ] {
            let work = work(
                requirements
                    .into_iter()
                    .map(|requirement| ProjectAnalysisInput {
                        path: path.clone(),
                        requirement,
                    })
                    .collect(),
            );
            assert!(work.validate_inputs(&[]).is_err());
            assert!(
                work.validate_inputs(&[directory.path().to_owned()])
                    .is_err()
            );
            assert!(work.validate_inputs(std::slice::from_ref(&path)).is_ok());
            assert_eq!(work.input_paths(), std::slice::from_ref(&path));
        }
    }

    #[test]
    fn optional_input_retains_identity_and_only_allows_absence() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file");
        std::fs::write(&file, "ordinary file").unwrap();
        let missing = directory.path().join("optional");
        let valid = work(vec![ProjectAnalysisInput {
            path: missing.clone(),
            requirement: ProjectAnalysisInputRequirement::Optional,
        }]);
        assert!(valid.validate_inputs(&[]).is_ok());
        assert_eq!(valid.input_paths(), &[missing]);
        let invalid = work(vec![ProjectAnalysisInput {
            path: file.join("child"),
            requirement: ProjectAnalysisInputRequirement::Optional,
        }]);
        assert!(invalid.validate_inputs(&[]).is_err());
    }

    #[test]
    fn execution_finish_requires_the_declared_emission_even_when_a_file_already_exists() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result");
        std::fs::write(&path, "old").unwrap();
        let declaration = || {
            ResolvedWork::new(
                "symbol-inventory".into(),
                "configuration".into(),
                vec![],
                vec![path.clone()],
                false,
                None,
            )
            .unwrap()
        };
        assert!(declaration().prepared().unwrap().finish().is_err());
        let execution = declaration().prepared().unwrap();
        execution
            .outputs()
            .file(0, "result")
            .unwrap()
            .text("new")
            .unwrap();
        let (completed, receipts) = execution.finish().unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(completed.outputs(), &[path]);
    }
}
