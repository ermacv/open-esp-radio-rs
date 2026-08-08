//! Pure navigation state for the project browser.

use std::collections::BTreeMap;

use crate::{ExecutionComparisonReport, WorkspaceSnapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Section {
    Overview,
    Functions,
    Registers,
    Interfaces,
    Comparisons,
    Diagnostics,
    Types,
}

impl Section {
    pub(super) const ALL: [Self; 7] = [
        Self::Overview,
        Self::Functions,
        Self::Registers,
        Self::Interfaces,
        Self::Comparisons,
        Self::Diagnostics,
        Self::Types,
    ];

    pub(super) const fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Functions => "Functions",
            Self::Registers => "Registers",
            Self::Interfaces => "Interfaces",
            Self::Comparisons => "Comparisons",
            Self::Diagnostics => "Diagnostics",
            Self::Types => "Types",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Continue,
    Reload,
    Compare(String),
    Quit,
}

pub(super) struct BrowserState {
    pub(super) snapshot: WorkspaceSnapshot,
    pub(super) section: Section,
    selections: [usize; Section::ALL.len()],
    pub(super) busy: bool,
    pub(super) message: Option<String>,
    pub(super) comparisons: BTreeMap<String, Box<ExecutionComparisonReport>>,
}

impl BrowserState {
    pub(super) fn new(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            section: Section::Overview,
            selections: [0; Section::ALL.len()],
            busy: false,
            message: None,
            comparisons: BTreeMap::new(),
        }
    }

    pub(super) fn selected(&self) -> usize {
        self.selections[self.section_index()]
    }

    pub(super) fn select_next_section(&mut self) {
        let next = (self.section_index() + 1) % Section::ALL.len();
        self.section = Section::ALL[next];
        self.clamp_selection();
    }

    pub(super) fn select_previous_section(&mut self) {
        let current = self.section_index();
        let previous = current
            .checked_sub(1)
            .unwrap_or_else(|| Section::ALL.len() - 1);
        self.section = Section::ALL[previous];
        self.clamp_selection();
    }

    pub(super) fn select_next(&mut self) {
        let length = self.item_count();
        if length == 0 {
            return;
        }
        let section = self.section_index();
        self.selections[section] = (self.selections[section] + 1).min(length - 1);
    }

    pub(super) fn select_previous(&mut self) {
        let section = self.section_index();
        self.selections[section] = self.selections[section].saturating_sub(1);
    }

    pub(super) fn select_first(&mut self) {
        let section = self.section_index();
        self.selections[section] = 0;
    }

    pub(super) fn select_last(&mut self) {
        let section = self.section_index();
        self.selections[section] = self.item_count().saturating_sub(1);
    }

    pub(super) fn begin_reload(&mut self) -> Action {
        if self.busy {
            return Action::Continue;
        }
        self.busy = true;
        self.message = Some("Reloading project...".to_owned());
        Action::Reload
    }

    pub(super) fn begin_compare(&mut self) -> Action {
        if self.busy || self.section != Section::Comparisons {
            return Action::Continue;
        }
        let Some(profile) = self.snapshot.comparisons.get(self.selected()) else {
            return Action::Continue;
        };
        let name = profile.name.clone();
        self.busy = true;
        self.message = Some(format!("Comparing {name}..."));
        Action::Compare(name)
    }

    pub(super) fn comparison_finished(&mut self, name: String, report: ExecutionComparisonReport) {
        let verdict = report.verdict.label();
        self.comparisons.insert(name.clone(), Box::new(report));
        self.busy = false;
        self.message = Some(format!("Comparison {name}: {verdict}"));
    }

    pub(super) fn replace_snapshot(&mut self, snapshot: WorkspaceSnapshot) {
        let active = self.section;
        self.snapshot = snapshot;
        self.comparisons.clear();
        self.busy = false;
        self.message = Some(format!("Reloaded generation {}", self.snapshot.generation));
        for section in Section::ALL {
            self.section = section;
            self.clamp_selection();
        }
        self.section = active;
    }

    pub(super) fn operation_failed(&mut self, message: String) {
        self.busy = false;
        self.message = Some(format!("Operation failed: {message}"));
    }

    fn section_index(&self) -> usize {
        Section::ALL
            .iter()
            .position(|section| *section == self.section)
            .expect("active section belongs to the fixed section list")
    }

    fn item_count(&self) -> usize {
        match self.section {
            Section::Overview => self.snapshot.project_status.phases.len(),
            Section::Functions => self.snapshot.functions.len(),
            Section::Registers => self.snapshot.registers.registers.len(),
            Section::Interfaces => self.snapshot.interfaces.slots.len(),
            Section::Comparisons => self.snapshot.comparisons.len(),
            Section::Diagnostics => self.snapshot.diagnostics.len(),
            Section::Types => self.snapshot.logical_types.len(),
        }
    }

    fn clamp_selection(&mut self) {
        let section = self.section_index();
        self.selections[section] =
            self.selections[section].min(self.item_count().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DiagnosticRecord, DiagnosticSeverity, InterfaceWorkspaceReport, ProjectStatusSnapshot,
        RegisterWorkspaceReport, WorkspacePhaseSnapshot, WorkspaceReadiness,
    };

    fn snapshot(generation: u64, phases: usize, diagnostics: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            generation,
            project_status: ProjectStatusSnapshot {
                project_id: "fixture".to_owned(),
                manifest: "vendor-project.toml".to_owned(),
                target_id: "target".to_owned(),
                architecture: "riscv32".to_owned(),
                calling_convention: "ilp32".to_owned(),
                harness: None,
                overall: WorkspaceReadiness::Incomplete,
                phases: (0..phases)
                    .map(|index| WorkspacePhaseSnapshot {
                        name: format!("phase-{index}"),
                        status: WorkspaceReadiness::Ready,
                        components: Vec::new(),
                    })
                    .collect(),
            },
            functions: Vec::new(),
            logical_types: Vec::new(),
            registers: RegisterWorkspaceReport {
                configured: false,
                model: None,
                ranges: 0,
                observed: 0,
                reviewed: 0,
                manual: 0,
                unreviewed: 0,
                fields: 0,
                registers: Vec::new(),
            },
            interfaces: InterfaceWorkspaceReport {
                configured: false,
                facts: None,
                pack: None,
                observed_slots: 0,
                reviewed_slots: 0,
                unreviewed_slots: 0,
                contracts: Vec::new(),
                slots: Vec::new(),
            },
            comparisons: Vec::new(),
            diagnostics: (0..diagnostics)
                .map(|index| DiagnosticRecord {
                    severity: DiagnosticSeverity::Warning,
                    component: format!("component-{index}"),
                    message: "incomplete".to_owned(),
                    path: None,
                })
                .collect(),
        }
    }

    #[test]
    fn navigation_is_bounded_and_sections_wrap() {
        let mut state = BrowserState::new(snapshot(1, 2, 1));
        state.select_previous();
        assert_eq!(state.selected(), 0);
        state.select_next();
        state.select_next();
        assert_eq!(state.selected(), 1);

        state.select_previous_section();
        assert_eq!(state.section, Section::Types);
        state.select_next_section();
        assert_eq!(state.section, Section::Overview);
    }

    #[test]
    fn reload_replaces_generation_and_clamps_selection() {
        let mut state = BrowserState::new(snapshot(1, 3, 0));
        state.select_last();
        assert_eq!(state.selected(), 2);
        assert_eq!(state.begin_reload(), Action::Reload);
        assert_eq!(state.begin_reload(), Action::Continue);

        state.replace_snapshot(snapshot(2, 1, 0));
        assert_eq!(state.snapshot.generation, 2);
        assert_eq!(state.selected(), 0);
        assert!(!state.busy);
    }
}
