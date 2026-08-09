//! Section item filtering and search matching.

use super::{BrowserState, Section};

impl BrowserState {
    pub(super) fn filtered_indices(&self) -> Vec<usize> {
        (0..self.unfiltered_item_count(self.section))
            .filter(|index| self.item_matches(self.section, *index))
            .collect()
    }

    fn unfiltered_item_count(&self, section: Section) -> usize {
        match section {
            Section::Overview => self.snapshot.project_status.phases.len(),
            Section::Code => self.snapshot.code.boundaries.len(),
            Section::Functions => self.snapshot.functions.len(),
            Section::Registers => self.snapshot.registers.registers.len(),
            Section::Interfaces => self.snapshot.interfaces.slots.len(),
            Section::Comparisons => self.snapshot.comparisons.len(),
            Section::Diagnostics => self.snapshot.diagnostics.len(),
            Section::Types => self.snapshot.logical_types.len(),
        }
    }

    pub(super) fn item_matches(&self, section: Section, index: usize) -> bool {
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
            Section::Code => self
                .snapshot
                .code
                .boundaries
                .get(index)
                .is_some_and(|boundary| {
                    contains(&boundary.source)
                        || contains(&boundary.section)
                        || contains(&format!("{:#x}", boundary.address))
                        || boundary.name.as_deref().is_some_and(&contains)
                        || boundary.reason.as_deref().is_some_and(&contains)
                        || boundary.symbol_names.iter().any(|value| contains(value))
                        || boundary
                            .direct_control_flow
                            .iter()
                            .any(|edge| contains(&edge.caller))
                }),
            Section::Functions => self.snapshot.functions.get(index).is_some_and(|function| {
                contains(&function.identity)
                    || contains(&function.symbol)
                    || function.reviewed_name.as_deref().is_some_and(&contains)
                    || function.role.as_deref().is_some_and(&contains)
                    || function
                        .semantic_operations
                        .iter()
                        .any(|value| contains(value))
                    || function
                        .decode_blocker_classes
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
