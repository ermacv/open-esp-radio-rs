//! Pure navigation state for the project browser.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    ExecutionComparisonReport, FunctionDetailSummary, RegisterDetailSummary, WorkspaceSnapshot,
};

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
    detail_scroll: [u16; Section::ALL.len()],
    pub(super) search_query: String,
    pub(super) search_editing: bool,
    pub(super) busy: bool,
    pub(super) message: Option<String>,
    pub(super) comparisons: BTreeMap<String, Box<ExecutionComparisonReport>>,
    function_details: BTreeMap<String, Box<FunctionDetailSummary>>,
    requested_function_details: BTreeSet<String>,
    register_details: BTreeMap<u32, Box<RegisterDetailSummary>>,
    requested_register_details: BTreeSet<u32>,
}

impl BrowserState {
    pub(super) fn new(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            section: Section::Overview,
            selections: [0; Section::ALL.len()],
            detail_scroll: [0; Section::ALL.len()],
            search_query: String::new(),
            search_editing: false,
            busy: false,
            message: None,
            comparisons: BTreeMap::new(),
            function_details: BTreeMap::new(),
            requested_function_details: BTreeSet::new(),
            register_details: BTreeMap::new(),
            requested_register_details: BTreeSet::new(),
        }
    }

    pub(super) fn selected(&self) -> usize {
        self.filtered_indices()
            .get(self.selected_position())
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn selected_position(&self) -> usize {
        self.selections[self.section_index()]
    }

    pub(super) fn viewport_start(&self, rows: usize) -> usize {
        let selected = self.selected_position();
        selected.saturating_sub(rows.saturating_sub(1))
    }

    pub(super) fn detail_scroll(&self) -> u16 {
        self.detail_scroll[self.section_index()]
    }

    pub(super) fn is_visible(&self, index: usize) -> bool {
        self.item_matches(self.section, index)
    }

    pub(super) fn visible_count(&self) -> usize {
        self.item_count()
    }

    pub(super) fn select_next_section(&mut self) {
        let next = (self.section_index() + 1) % Section::ALL.len();
        self.section = Section::ALL[next];
        self.clamp_selection();
        self.reset_detail_scroll();
    }

    pub(super) fn select_previous_section(&mut self) {
        let current = self.section_index();
        let previous = current
            .checked_sub(1)
            .unwrap_or_else(|| Section::ALL.len() - 1);
        self.section = Section::ALL[previous];
        self.clamp_selection();
        self.reset_detail_scroll();
    }

    pub(super) fn select_next(&mut self) {
        let length = self.item_count();
        if length == 0 {
            return;
        }
        let section = self.section_index();
        self.selections[section] = (self.selections[section] + 1).min(length - 1);
        self.reset_detail_scroll();
    }

    pub(super) fn select_previous(&mut self) {
        let section = self.section_index();
        self.selections[section] = self.selections[section].saturating_sub(1);
        self.reset_detail_scroll();
    }

    pub(super) fn select_first(&mut self) {
        let section = self.section_index();
        self.selections[section] = 0;
        self.reset_detail_scroll();
    }

    pub(super) fn select_last(&mut self) {
        let section = self.section_index();
        self.selections[section] = self.item_count().saturating_sub(1);
        self.reset_detail_scroll();
    }

    pub(super) fn scroll_detail_down(&mut self, amount: u16) {
        let section = self.section_index();
        self.detail_scroll[section] = self.detail_scroll[section].saturating_add(amount);
    }

    pub(super) fn scroll_detail_up(&mut self, amount: u16) {
        let section = self.section_index();
        self.detail_scroll[section] = self.detail_scroll[section].saturating_sub(amount);
    }

    pub(super) fn begin_search(&mut self) {
        self.search_editing = true;
    }

    pub(super) fn finish_search(&mut self) {
        self.search_editing = false;
    }

    pub(super) fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_editing = false;
        self.reset_after_search();
    }

    pub(super) fn push_search(&mut self, character: char) {
        self.search_query.push(character);
        self.reset_after_search();
    }

    pub(super) fn pop_search(&mut self) {
        self.search_query.pop();
        self.reset_after_search();
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

    pub(super) fn request_function_detail(&mut self) -> Option<String> {
        if self.section != Section::Functions {
            return None;
        }
        let identity = self
            .snapshot
            .functions
            .get(self.selected())?
            .identity
            .clone();
        if self.requested_function_details.insert(identity.clone()) {
            Some(identity)
        } else {
            None
        }
    }

    pub(super) fn function_detail(&self, identity: &str) -> Option<&FunctionDetailSummary> {
        self.function_details.get(identity).map(Box::as_ref)
    }

    pub(super) fn function_detail_finished(
        &mut self,
        identity: String,
        detail: Option<FunctionDetailSummary>,
    ) {
        if let Some(detail) = detail {
            self.function_details.insert(identity, Box::new(detail));
        }
    }

    pub(super) fn request_register_detail(&mut self) -> Option<u32> {
        if self.section != Section::Registers {
            return None;
        }
        let address = self
            .snapshot
            .registers
            .registers
            .get(self.selected())?
            .address;
        self.requested_register_details
            .insert(address)
            .then_some(address)
    }

    pub(super) fn register_detail(&self, address: u32) -> Option<&RegisterDetailSummary> {
        self.register_details.get(&address).map(Box::as_ref)
    }

    pub(super) fn register_detail_finished(
        &mut self,
        address: u32,
        detail: Option<RegisterDetailSummary>,
    ) {
        if let Some(detail) = detail {
            self.register_details.insert(address, Box::new(detail));
        }
    }

    pub(super) fn activate(&mut self) -> Action {
        if self.section == Section::Comparisons {
            return self.begin_compare();
        }
        let target = match self.section {
            Section::Functions => {
                let function = self.snapshot.functions.get(self.selected());
                function.and_then(|function| {
                    self.snapshot
                        .registers
                        .registers
                        .iter()
                        .position(|register| function.registers.contains(&register.address))
                        .map(|index| (Section::Registers, index, "MMIO register"))
                        .or_else(|| {
                            self.snapshot
                                .interfaces
                                .slots
                                .iter()
                                .position(|slot| {
                                    slot.functions.iter().any(|name| {
                                        name == &function.identity || name == &function.symbol
                                    }) || slot.semantic.as_ref().is_some_and(|semantic| {
                                        function.semantic_operations.contains(semantic)
                                    })
                                })
                                .map(|index| {
                                    (Section::Interfaces, index, "interface/semantic call")
                                })
                        })
                })
            }
            Section::Registers => {
                let register = self.snapshot.registers.registers.get(self.selected());
                register
                    .and_then(|register| {
                        self.snapshot
                            .functions
                            .iter()
                            .position(|function| function.registers.contains(&register.address))
                    })
                    .map(|index| (Section::Functions, index, "register user"))
            }
            Section::Interfaces => {
                let slot = self.snapshot.interfaces.slots.get(self.selected());
                slot.and_then(|slot| {
                    self.snapshot
                        .functions
                        .iter()
                        .position(|function| {
                            slot.functions
                                .iter()
                                .any(|name| name == &function.identity || name == &function.symbol)
                        })
                        .map(|index| (Section::Functions, index, "calling function"))
                })
            }
            Section::Types => {
                let logical_type = self.snapshot.logical_types.get(self.selected());
                logical_type
                    .and_then(|logical_type| {
                        logical_type.bindings.iter().find_map(|binding| {
                            let function = binding
                                .object
                                .strip_prefix("argument:")?
                                .split(':')
                                .next()?;
                            self.snapshot.functions.iter().position(|item| {
                                item.identity == function || item.symbol == function
                            })
                        })
                    })
                    .map(|index| (Section::Functions, index, "bound function"))
            }
            _ => None,
        };
        if let Some((section, index, label)) = target {
            self.search_query.clear();
            self.search_editing = false;
            self.section = section;
            let section_index = self.section_index();
            self.selections[section_index] = index;
            self.detail_scroll[section_index] = 0;
            self.message = Some(format!("Navigated to {label}"));
        } else {
            self.message = Some("No reviewed cross-reference for this item".to_owned());
        }
        Action::Continue
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
        self.function_details.clear();
        self.requested_function_details.clear();
        self.register_details.clear();
        self.requested_register_details.clear();
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
        self.filtered_indices().len()
    }

    fn unfiltered_item_count(&self, section: Section) -> usize {
        match section {
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

    fn reset_detail_scroll(&mut self) {
        let section = self.section_index();
        self.detail_scroll[section] = 0;
    }

    fn reset_after_search(&mut self) {
        let section = self.section_index();
        self.selections[section] = 0;
        self.detail_scroll[section] = 0;
    }

    fn filtered_indices(&self) -> Vec<usize> {
        (0..self.unfiltered_item_count(self.section))
            .filter(|index| self.item_matches(self.section, *index))
            .collect()
    }

    fn item_matches(&self, section: Section, index: usize) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        let contains = |value: &str| value.to_ascii_lowercase().contains(&query);
        match section {
            Section::Overview => {
                self.snapshot
                    .project_status
                    .phases
                    .get(index)
                    .is_some_and(|phase| {
                        contains(&phase.name)
                            || phase.components.iter().any(|component| {
                                contains(&component.name)
                                    || component.diagnostic.as_deref().is_some_and(&contains)
                            })
                    })
            }
            Section::Functions => self.snapshot.functions.get(index).is_some_and(|function| {
                contains(&function.identity)
                    || contains(&function.symbol)
                    || function.reviewed_name.as_deref().is_some_and(&contains)
                    || function.role.as_deref().is_some_and(&contains)
                    || function
                        .semantic_operations
                        .iter()
                        .any(|value| contains(value))
            }),
            Section::Registers => {
                self.snapshot
                    .registers
                    .registers
                    .get(index)
                    .is_some_and(|register| {
                        contains(&register.name) || contains(&format!("{:#010x}", register.address))
                    })
            }
            Section::Interfaces => self
                .snapshot
                .interfaces
                .slots
                .get(index)
                .is_some_and(|slot| {
                    contains(&slot.id)
                        || contains(&slot.name)
                        || slot.semantic.as_deref().is_some_and(&contains)
                        || slot.functions.iter().any(|value| contains(value))
                }),
            Section::Comparisons => self.snapshot.comparisons.get(index).is_some_and(|profile| {
                contains(&profile.name)
                    || contains(&profile.vendor_symbol)
                    || contains(&profile.rust_symbol)
            }),
            Section::Diagnostics => self
                .snapshot
                .diagnostics
                .get(index)
                .is_some_and(|item| contains(&item.component) || contains(&item.message)),
            Section::Types => self
                .snapshot
                .logical_types
                .get(index)
                .is_some_and(|item| contains(&item.id) || contains(&item.name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DiagnosticRecord, DiagnosticSeverity, FunctionReviewState, FunctionSelection,
        FunctionSummary, InterfaceSlotSummary, InterfaceWorkspaceReport, ProjectStatusPhase,
        ProjectStatusReport, ProjectTargetIdentity, Readiness, RegisterSummary,
        RegisterWorkspaceReport,
    };

    fn snapshot(generation: u64, phases: usize, diagnostics: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            generation,
            project_status: ProjectStatusReport {
                project_id: "fixture".to_owned(),
                manifest: "vendor-project.toml".to_owned(),
                target: ProjectTargetIdentity {
                    id: "target".to_owned(),
                    architecture: "riscv32".to_owned(),
                    calling_convention: "ilp32".to_owned(),
                    harness: None,
                },
                overall: Readiness::Incomplete,
                phases: (0..phases)
                    .map(|index| ProjectStatusPhase {
                        name: format!("phase-{index}"),
                        status: Readiness::Ready,
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

    #[test]
    fn search_filters_navigation_and_detail_scroll_is_section_local() {
        let mut state = BrowserState::new(snapshot(1, 3, 3));
        state.section = Section::Diagnostics;
        state.begin_search();
        for character in "component-2".chars() {
            state.push_search(character);
        }
        state.finish_search();
        assert_eq!(state.visible_count(), 1);
        assert_eq!(state.selected(), 2);
        state.select_next();
        assert_eq!(state.selected(), 2);

        state.scroll_detail_down(16);
        assert_eq!(state.detail_scroll(), 16);
        state.select_previous_section();
        assert_eq!(state.detail_scroll(), 0);
        state.select_next_section();
        assert_eq!(state.detail_scroll(), 0);

        state.clear_search();
        assert_eq!(state.visible_count(), 3);
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn activation_follows_reviewed_function_interface_links_in_both_directions() {
        let mut workspace = snapshot(1, 0, 0);
        workspace.functions.push(FunctionSummary {
            profile: "radio".to_owned(),
            source: "rom".to_owned(),
            identity: "rom:init".to_owned(),
            symbol: "init".to_owned(),
            member: None,
            selection: FunctionSelection::SymbolPrefixRoot,
            review_status: FunctionReviewState::Reviewed,
            reviewed_name: None,
            role: None,
            summary: None,
            complete: true,
            blockers: Vec::new(),
            semantic_operations: Vec::new(),
            registers: Vec::new(),
            calls: 1,
        });
        workspace.interfaces.slots.push(InterfaceSlotSummary {
            id: "services.delay".to_owned(),
            contract: "services".to_owned(),
            offset: 4,
            width: 4,
            name: "delay".to_owned(),
            arguments: vec!["ticks".to_owned()],
            return_type: "void".to_owned(),
            variadic: false,
            semantic: Some("time.blocking-delay".to_owned()),
            effects: vec!["delay".to_owned()],
            replacement: None,
            execution_model: None,
            functions: vec!["rom:init".to_owned()],
            call_sites: vec![0x4000],
        });
        let mut state = BrowserState::new(workspace);
        state.section = Section::Functions;

        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Interfaces);
        assert_eq!(state.selected(), 0);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Functions);
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn activation_follows_function_register_usage_in_both_directions() {
        let mut workspace = snapshot(1, 0, 0);
        let function = FunctionSummary {
            profile: "radio".to_owned(),
            source: "rom".to_owned(),
            identity: "rom:init".to_owned(),
            symbol: "init".to_owned(),
            member: None,
            selection: FunctionSelection::SymbolPrefixRoot,
            review_status: FunctionReviewState::Reviewed,
            reviewed_name: None,
            role: None,
            summary: None,
            complete: true,
            blockers: Vec::new(),
            semantic_operations: Vec::new(),
            registers: vec![0x4000],
            calls: 0,
        };
        workspace.functions.push(function);
        workspace.registers.registers.push(RegisterSummary {
            address: 0x4000,
            name: "RADIO.STATUS".to_owned(),
        });
        let mut state = BrowserState::new(workspace);
        state.section = Section::Functions;

        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Registers);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Functions);
    }

    #[test]
    fn function_detail_is_requested_once_per_snapshot_generation() {
        let mut workspace = snapshot(1, 0, 0);
        workspace.functions.push(FunctionSummary {
            profile: "radio".to_owned(),
            source: "rom".to_owned(),
            identity: "rom:init".to_owned(),
            symbol: "init".to_owned(),
            member: None,
            selection: FunctionSelection::SymbolPrefixRoot,
            review_status: FunctionReviewState::Reviewed,
            reviewed_name: None,
            role: None,
            summary: None,
            complete: true,
            blockers: Vec::new(),
            semantic_operations: Vec::new(),
            registers: Vec::new(),
            calls: 0,
        });
        let mut state = BrowserState::new(workspace);
        state.section = Section::Functions;

        assert_eq!(state.request_function_detail().as_deref(), Some("rom:init"));
        assert_eq!(state.request_function_detail(), None);
        state.replace_snapshot(snapshot(2, 0, 0));
        assert!(state.requested_function_details.is_empty());
    }

    #[test]
    fn register_detail_is_requested_once_per_snapshot_generation() {
        let mut workspace = snapshot(1, 0, 0);
        workspace.registers.registers.push(RegisterSummary {
            address: 0x4000,
            name: "RADIO.STATUS".to_owned(),
        });
        let mut state = BrowserState::new(workspace);
        state.section = Section::Registers;

        assert_eq!(state.request_register_detail(), Some(0x4000));
        assert_eq!(state.request_register_detail(), None);
        state.replace_snapshot(snapshot(2, 0, 0));
        assert!(state.requested_register_details.is_empty());
    }
}
