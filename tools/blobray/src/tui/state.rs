//! Pure navigation state for the project browser.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    ExecutionComparisonReport, FunctionDetailSummary, RegisterDetailSummary, WorkspaceSnapshot,
};

mod detail;
mod filter;
mod navigation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Section {
    Overview,
    Policy,
    Scopes,
    Code,
    Functions,
    Blockers,
    Registers,
    Interfaces,
    Comparisons,
    Diagnostics,
    Types,
}

impl Section {
    pub(super) const ALL: [Self; 11] = [
        Self::Overview,
        Self::Policy,
        Self::Scopes,
        Self::Code,
        Self::Functions,
        Self::Blockers,
        Self::Registers,
        Self::Interfaces,
        Self::Comparisons,
        Self::Diagnostics,
        Self::Types,
    ];

    pub(super) const fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Policy => "Policy",
            Self::Scopes => "Scopes",
            Self::Code => "Code",
            Self::Functions => "Functions",
            Self::Blockers => "Blockers",
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
    register_details: BTreeMap<String, Box<RegisterDetailSummary>>,
    requested_register_details: BTreeSet<String>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CodeWorkspaceReport, DiagnosticRecord, DiagnosticSeverity, FunctionReviewState,
        FunctionSelection, FunctionSummary, InterfaceSlotSummary, InterfaceWorkspaceReport,
        ProjectStatusPhase, ProjectStatusReport, ProjectTargetIdentity, Readiness,
        RegisterWorkspaceReport, ResearchCompleteness, ResearchProgress, ReviewScopeSummary,
    };

    fn snapshot(generation: u64, phases: usize, diagnostics: usize) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            generation,
            generated_analysis_epoch: None,
            project_status: ProjectStatusReport {
                project_id: "fixture".to_owned(),
                manifest: "vendor-project.toml".to_owned(),
                target: ProjectTargetIdentity {
                    id: "target".to_owned(),
                    architecture: "riscv32".to_owned(),
                    calling_convention: "ilp32".to_owned(),
                    knowledge_provider: None,
                },
                validation: crate::StatusValidation {
                    depth: crate::ValidationDepth::Shallow,
                    freshness: crate::EvidenceFreshness::Unknown,
                },
                research: ResearchProgress {
                    status: ResearchCompleteness::NotConfigured,
                    scopes: 0,
                    inventory_complete: 0,
                    inventory_open: 0,
                    root_causes: 0,
                    publication_coverage_gaps: 0,
                },
                verification: Readiness::NotConfigured,
                overall: Readiness::Incomplete,
                phases: (0..phases)
                    .map(|index| ProjectStatusPhase {
                        name: format!("phase-{index}"),
                        status: Readiness::Ready,
                        components: Vec::new(),
                    })
                    .collect(),
            },
            code: CodeWorkspaceReport {
                configured: false,
                facts: None,
                pack: None,
                review_output: None,
                observed_candidates: 0,
                accepted: 0,
                rejected: 0,
                unreviewed: 0,
                boundaries: Vec::new(),
            },
            functions: Vec::new(),
            logical_types: Vec::new(),
            registers: RegisterWorkspaceReport {
                configured: false,
                model: None,
                ranges: 0,
                observed: 0,
                publication: None,
                fields: 0,
                inventory: crate::tui::register_snapshot(Default::default()),
            },
            interfaces: InterfaceWorkspaceReport {
                observations: None,
                observation_state: crate::InterfaceObservationState::NotConfigured,
                configured: false,
                facts: None,
                pack: None,
                observed_slots: 0,
                reviewed_slots: 0,
                unreviewed_slots: 0,
                contracts: Vec::new(),
                slots: Vec::new(),
            },
            review_scopes: Vec::new(),
            analysis_surfaces: Vec::new(),
            verification_policy: Vec::new(),
            review_queue: Vec::new(),
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
    fn interface_observations_are_searchable_without_review_and_empty_search_hides_detail() {
        use crate::interface_discovery::{
            InterfaceAnalysisGap, InterfaceArgumentValue, InterfaceGapReason,
            InterfaceRegisterValue,
        };
        use ratatui::{Terminal, backend::TestBackend};

        let mut snapshot = snapshot(1, 0, 0);
        snapshot.interfaces.configured = true;
        snapshot.interfaces.observation_state = crate::InterfaceObservationState::Available;
        snapshot.interfaces.observations = Some(std::sync::Arc::new(crate::InterfaceFacts {
            artifacts: vec![crate::InterfaceFactArtifact {
                index: 0,
                sources: ["fixture".into()].into(),
                sha256: Some("a".repeat(64)),
            }],
            limits: Default::default(),
            tables: vec![],
            calls: vec![],
            assignments: vec![],
            decode_blockers: vec![],
            gaps: vec![crate::InterfaceGapFact {
                artifact: 0,
                evidence: InterfaceAnalysisGap {
                    owner: crate::artifact::ArtifactSymbolDefinition::synthetic_identity(
                        module_path!(),
                        &None,
                        "dispatch",
                        0,
                    ),
                    member: None,
                    function: "dispatch".into(),
                    site: 4,
                    reason: InterfaceGapReason::UnresolvedCallTarget,
                    registers: (0..32)
                        .map(|register| InterfaceRegisterValue {
                            register,
                            value: InterfaceArgumentValue::Unknown,
                        })
                        .collect(),
                },
            }],
            analysis_failures: vec![crate::InterfaceDecodeFailureFact {
                owner: crate::artifact::CodeIdentity::Synthetic {
                    namespace: module_path!().into(),
                    key: format!("fixture:{}", line!()),
                },
                artifact: 0,
                member: None,
                function: "unparsed".into(),
                error: "malformed instruction span".into(),
            }],
        }));
        let mut state = BrowserState::new(snapshot);
        state.section = Section::Interfaces;
        assert_eq!(state.visible_count(), 2);
        state.select_last();
        assert_eq!(state.selected(), 1);
        state.begin_search();
        for character in "unresolved-call-target".chars() {
            state.push_search(character);
        }
        state.finish_search();
        assert_eq!(state.visible_count(), 1);
        assert_eq!(state.selected(), 0);
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        let mut render = |state: &BrowserState| {
            terminal
                .draw(|frame| crate::tui::view::render(frame, state))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        };
        let rendered = render(&state);
        assert!(rendered.contains("Interface evidence (1/2)"));
        assert!(rendered.contains("gap: dispatch"));
        assert!(rendered.contains("unknown from this observation alone"));
        assert!(rendered.contains("unresolved-call-target"));
        assert!(rendered.contains("\"registers\""));
        state.search_query = "unknown".into();
        assert_eq!(state.visible_count(), 2);
        state.search_query = "no-such-observation".into();
        assert_eq!(state.visible_count(), 0);
        let rendered = render(&state);
        assert!(rendered.contains("No observations match the current view"));
        assert!(!rendered.contains("gap: dispatch"));
        assert!(!rendered.contains("\"registers\""));
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
            code_identity: crate::artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: format!("fixture:{}", line!()),
            },
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
            decode_blockers: 1,
            decode_blocker_classes: vec!["zero-fill-or-illegal-trap".to_owned()],
            decode_blocker_operations: vec!["illegal-zero".to_owned()],
            semantic_operations: Vec::new(),
            registers: Vec::new(),
            mmio_sites: Vec::new(),
            calls: 1,
        });
        workspace.interfaces.slots.push(InterfaceSlotSummary {
            id: "services.delay".to_owned(),
            contract: "services".to_owned(),
            offset: 4,
            width: 4,
            name: "delay".to_owned(),
            review_state: crate::InterfaceReviewState::Reviewed,
            selector: None,
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

        state.begin_search();
        for character in "illegal-trap".chars() {
            state.push_search(character);
        }
        state.finish_search();
        assert_eq!(state.visible_count(), 1);
        state.clear_search();

        state.begin_search();
        for character in "illegal-zero".chars() {
            state.push_search(character);
        }
        state.finish_search();
        assert_eq!(state.visible_count(), 1);
        state.clear_search();

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
            code_identity: crate::artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: format!("fixture:{}", line!()),
            },
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
            decode_blockers: 0,
            decode_blocker_classes: Vec::new(),
            decode_blocker_operations: Vec::new(),
            semantic_operations: Vec::new(),
            registers: vec![0x4000],
            mmio_sites: Vec::new(),
            calls: 0,
        };
        workspace.functions.push(function);
        crate::tui::append_register(
            &mut workspace.registers,
            crate::tui::register_fixture(0x4000, "RADIO.STATUS"),
        );
        let mut state = BrowserState::new(workspace);
        state.section = Section::Functions;

        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Registers);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Functions);
        let mut bank = state.snapshot.registers.register_at(0).unwrap().clone();
        bank.subject.bank = Some("second".into());
        bank.id = bank.subject.id();
        crate::tui::append_register(&mut state.snapshot.registers, bank);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Functions);
        assert!(
            state
                .message
                .as_deref()
                .unwrap()
                .contains("several physical subjects")
        );
        state.section = Section::Registers;
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(
            state.section,
            Section::Registers,
            "numeric ambiguity cannot establish a user"
        );
        let mut registers = state
            .snapshot
            .registers
            .registers()
            .cloned()
            .collect::<Vec<_>>();
        registers[0].functions.insert("rom:init".into());
        crate::tui::replace_registers(&mut state.snapshot.registers, registers);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(
            state.section,
            Section::Functions,
            "explicit subject relation remains navigable"
        );
        let remaining = state
            .snapshot
            .registers
            .registers()
            .filter(|register| register.subject.bank.is_some())
            .cloned()
            .collect();
        crate::tui::replace_registers(&mut state.snapshot.registers, remaining);
        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(
            state.section,
            Section::Functions,
            "a lone banked address is not an MMIO location"
        );
    }

    #[test]
    fn activation_opens_the_first_function_in_a_publication_scope() {
        let mut workspace = snapshot(1, 0, 0);
        workspace.functions.push(FunctionSummary {
            code_identity: crate::artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: format!("fixture:{}", line!()),
            },
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
            decode_blockers: 0,
            decode_blocker_classes: Vec::new(),
            decode_blocker_operations: Vec::new(),
            semantic_operations: Vec::new(),
            registers: Vec::new(),
            mmio_sites: Vec::new(),
            calls: 0,
        });
        workspace.review_scopes.push(ReviewScopeSummary {
            id: "radio-init".to_owned(),
            protocols: vec!["shared".to_owned()],
            publication: true,
            replacement_coverage: "complete".to_owned(),
            replacement_policy_excluded: 0,
            analysis_inventory_complete: true,
            profiles: vec!["radio".to_owned()],
            roots: 1,
            functions: 1,
            replacement_functions: 1,
            complete_functions: 1,
            mmio_registers: 0,
            table_calls: 0,
            context_fields: 0,
            memory_fields: 0,
            blockers: 0,
            decode_blockers: 0,
            unresolved_calls: 0,
            replacement_gaps: 0,
            function_identities: vec!["rom:init".to_owned()],
            mmio_addresses: Vec::new(),
        });
        let mut state = BrowserState::new(workspace);
        state.section = Section::Scopes;

        assert_eq!(state.activate(), Action::Continue);
        assert_eq!(state.section, Section::Functions);
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn function_detail_is_requested_once_per_snapshot_generation() {
        let mut workspace = snapshot(1, 0, 0);
        workspace.functions.push(FunctionSummary {
            code_identity: crate::artifact::CodeIdentity::Synthetic {
                namespace: module_path!().into(),
                key: format!("fixture:{}", line!()),
            },
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
            decode_blockers: 0,
            decode_blocker_classes: Vec::new(),
            decode_blocker_operations: Vec::new(),
            semantic_operations: Vec::new(),
            registers: Vec::new(),
            mmio_sites: Vec::new(),
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
        crate::tui::append_register(
            &mut workspace.registers,
            crate::tui::register_fixture(0x4000, "RADIO.STATUS"),
        );
        let mut state = BrowserState::new(workspace);
        state.section = Section::Registers;

        assert_eq!(
            state.request_register_detail(),
            Some(state.snapshot.registers.register_at(0).unwrap().id.clone())
        );
        assert_eq!(state.request_register_detail(), None);
        state.replace_snapshot(snapshot(2, 0, 0));
        assert!(state.requested_register_details.is_empty());
    }

    #[test]
    fn register_details_are_cached_by_subject_and_large_addresses_are_searchable() {
        use open_radio_vendor_contracts::register_inventory::KnowledgeProperty;
        let mut workspace = snapshot(1, 0, 0);
        let first = crate::tui::register_fixture(0x1_0000_4000, "FIRST");
        let mut second = first.clone();
        second.subject.bank = Some("second".into());
        second.id = second.subject.id();
        second.names = KnowledgeProperty::Unknown;
        second.fields.insert(
            "high".into(),
            crate::InventoryField {
                id: "high".into(),
                offset: 300,
                width: 64,
                mask: None,
                names: KnowledgeProperty::Unknown,
                kind: "declared".into(),
                semantics: KnowledgeProperty::Unknown,
                evidence: Default::default(),
            },
        );
        crate::tui::replace_registers(
            &mut workspace.registers,
            vec![first.clone(), second.clone()],
        );
        let mut state = BrowserState::new(workspace);
        state.section = Section::Registers;
        let section = state.section_index();
        state.selections[section] = state
            .snapshot
            .registers
            .registers()
            .position(|r| r.id == first.id)
            .unwrap();
        assert_eq!(state.request_register_detail(), Some(first.id.clone()));
        state.selections[section] = state
            .snapshot
            .registers
            .registers()
            .position(|r| r.id == second.id)
            .unwrap();
        assert_eq!(state.request_register_detail(), Some(second.id.clone()));
        assert_eq!(state.request_register_detail(), None);
        let project = crate::ProjectSpec::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/generic-project/vendor-project.toml"),
        )
        .unwrap();
        let inventory = crate::RegisterInventory {
            registers: [
                (first.id.clone(), first.clone()),
                (second.id.clone(), second.clone()),
            ]
            .into(),
            ..Default::default()
        };
        for register in [&first, &second] {
            let detail = crate::application::register_detail_from_inventory(
                &project,
                inventory.clone(),
                &crate::RegisterSelector::Subject(register.id.clone()),
            )
            .unwrap();
            state.register_detail_finished(register.id.clone(), state.snapshot.generation, detail);
        }
        assert_eq!(
            state.register_detail(&first.id).unwrap().subjects,
            vec![first.clone()]
        );
        assert_eq!(
            state.register_detail(&second.id).unwrap().subjects,
            vec![second.clone()]
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(280, 90)).unwrap();
        terminal
            .draw(|frame| crate::tui::view::render(frame, &state))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("0x100004000"));
        assert!(rendered.contains("bits 363..300 mask=unknown"));
        assert!(rendered.contains("field semantics=Unknown"));
        for (query, count) in [("100004000", 2), (second.id.as_str(), 1)] {
            state.clear_search();
            state.begin_search();
            for character in query.chars() {
                state.push_search(character);
            }
            state.finish_search();
            assert_eq!(state.visible_count(), count);
        }
        let stale = state.register_detail(&second.id).unwrap().clone();
        let mut replacement = state.snapshot.clone();
        replacement.generation += 1;
        let mut updated = second.clone();
        updated
            .names
            .insert("new evidence".into(), "new-source".into());
        crate::tui::replace_registers(&mut replacement.registers, vec![first.clone(), updated]);
        state.replace_snapshot(replacement);
        state.section = Section::Registers;
        state.clear_search();
        let section = state.section_index();
        state.selections[section] = state
            .snapshot
            .registers
            .registers()
            .position(|r| r.id == second.id)
            .unwrap();
        assert_eq!(state.request_register_detail(), Some(second.id.clone()));
        state.register_detail_finished(second.id.clone(), state.snapshot.generation, Some(stale));
        assert!(state.register_detail(&second.id).is_none());
        assert_eq!(
            state.request_register_detail(),
            Some(second.id.clone()),
            "stale completion must not suppress a fresh request"
        );
        let current_graph = state
            .snapshot
            .registers
            .inventory
            .snapshot()
            .unwrap()
            .inventory()
            .clone();
        let current_detail = crate::application::register_detail_from_inventory(
            &project,
            current_graph,
            &crate::RegisterSelector::Subject(second.id.clone()),
        )
        .unwrap();
        state.register_detail_finished(
            second.id.clone(),
            state.snapshot.generation - 1,
            current_detail,
        );
        assert!(
            state.register_detail(&second.id).is_none(),
            "matching graph content cannot authorize a stale review generation"
        );
        assert_eq!(state.request_register_detail(), Some(second.id.clone()));
        state.replace_snapshot(snapshot(3, 0, 0));
        assert!(state.register_detail(&first.id).is_none());
        assert!(state.register_detail(&second.id).is_none());
    }
    #[test]
    fn empty_register_view_exposes_sources_domains_gaps_and_load_failures() {
        use open_radio_vendor_contracts::register_inventory::{CoverageGap, SourceState};
        let mut graph = crate::RegisterInventory::default();
        graph.sources.push(crate::InventorySource {
            path: "missing.svd".into(),
            kind: "svd".into(),
            digest: None,
            evidence: Default::default(),
            state: SourceState::Missing,
        });
        graph.gaps.insert(CoverageGap {
            source: "capture".into(),
            scope: "indexed-access".into(),
            reason: "unknown upper bound".into(),
        });
        graph.address_domains.insert("domain:unbounded".into());
        graph.evidence.insert(
            "domain:unbounded".into(),
            crate::RegisterEvidence {
                id: "domain:unbounded".into(),
                kind: "indexed-mmio".into(),
                sources: Default::default(),
                identity: serde_json::json!("loop"),
                payload: serde_json::json!({"expression":"base + arg0 * 4","upper_bound":null}),
            },
        );
        graph.regions.push(crate::AddressCoverage {
            name: "peripheral".into(),
            address_space: "cpu".into(),
            alias_of: None,
            evidence: Default::default(),
            start: 0x1000,
            end_exclusive: 0x1100,
            boundary: "declared".into(),
            known_geometry: vec![],
            observed_extents: vec![],
            geometry_gaps: vec![(0x1000, 0x1100)],
            observation_gaps: vec![(0x1000, 0x1100)],
        });
        let mut workspace = snapshot(1, 0, 0);
        workspace.registers.inventory = crate::tui::register_snapshot(graph);
        let mut state = BrowserState::new(workspace);
        state.section = Section::Registers;
        assert_eq!(state.visible_count(), 0);
        assert_eq!(state.request_register_detail(), None);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(240, 70)).unwrap();
        terminal
            .draw(|frame| crate::tui::view::render(frame, &state))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        for expected in [
            "missing.svd",
            "Missing",
            "unknown upper bound",
            "domain:unbounded",
            "base + arg0 * 4",
            "unknown geometry",
            "unobserved-in-scope",
        ] {
            assert!(
                rendered.contains(expected),
                "{expected} absent from {rendered}"
            );
        }
        state.snapshot.registers.inventory = crate::RegisterInventoryState::Failed {
            reason: "explicit SVD capture failed".into(),
        };
        terminal
            .draw(|frame| crate::tui::view::render(frame, &state))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Inventory failed"));
        assert!(rendered.contains("explicit SVD capture failed"));
    }
}
