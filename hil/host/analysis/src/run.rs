//! One run as the analyses read it: its bundle and its suite.

use std::path::Path;

use oer_hil_run_bundle::{
    RunBundle, RunStore,
    run::{Outcome, RunState, ScenarioResult, SuiteResult},
};

use crate::Result;

/// A published run with its suite, when it has one.
#[derive(Clone, Debug)]
pub struct Run {
    pub bundle: RunBundle,
    pub suite: Option<SuiteResult>,
    /// Recorded as running, but its runner is gone.
    pub abandoned: bool,
}

impl Run {
    /// The run in `directory`; `None` when it has no manifest yet, or a
    /// manifest or suite outside this build's format.
    pub fn load(directory: &Path) -> Option<Self> {
        Self::of(RunBundle::open(directory).ok()??).ok()
    }

    /// The run `id` of `store`.
    pub fn open(store: &RunStore, id: &str) -> Result<Self> {
        Self::of(store.open(id)?)
    }

    /// Every readable run of `store`, oldest first.
    pub fn all(store: &RunStore) -> Result<Vec<Self>> {
        Ok(store
            .bundles()?
            .into_iter()
            .filter_map(|bundle| Self::of(bundle).ok())
            .collect())
    }

    pub fn of(bundle: RunBundle) -> Result<Self> {
        let suite = bundle.suite()?;
        let abandoned = bundle.abandoned();
        Ok(Self {
            bundle,
            suite,
            abandoned,
        })
    }

    pub fn id(&self) -> &str {
        self.bundle.id()
    }

    pub fn directory(&self) -> &Path {
        self.bundle.directory()
    }

    pub fn started_millis(&self) -> u64 {
        self.bundle.manifest().started_unix_millis
    }

    pub fn state(&self) -> RunState {
        self.bundle.manifest().state
    }

    /// A completed or interrupted bundle can no longer change.
    pub fn is_sealed(&self) -> bool {
        matches!(self.state(), RunState::Completed | RunState::Interrupted)
    }

    /// The suite outcome; `None` when the run has no suite.
    pub fn outcome(&self) -> Option<Outcome> {
        self.suite.as_ref().map(|suite| suite.outcome)
    }

    /// The run's status for display and selection: its suite outcome, or
    /// else `abandoned` or its state.
    pub fn status(&self) -> &'static str {
        match self.outcome() {
            Some(outcome) => outcome.id(),
            None if self.abandoned => "abandoned",
            None => self.state().id(),
        }
    }

    pub fn scenarios(&self) -> &[ScenarioResult] {
        self.suite
            .as_ref()
            .map_or(&[], |suite| suite.scenarios.as_slice())
    }

    pub fn scenario(&self, id: &str) -> Option<&ScenarioResult> {
        self.scenarios()
            .iter()
            .find(|scenario| scenario.scenario == id)
    }

    pub fn commit(&self) -> &str {
        &self.bundle.manifest().repository.commit
    }

    pub fn dirty(&self) -> bool {
        self.bundle.manifest().repository.dirty
    }

    /// The commit's first nine characters, `+` when the tree was dirty.
    pub fn short_commit(&self) -> String {
        let commit = self.commit();
        let commit = if commit.is_empty() {
            "unknown"
        } else {
            &commit[..commit.len().min(9)]
        };
        if self.dirty() {
            format!("{commit}+")
        } else {
            commit.to_owned()
        }
    }
}
