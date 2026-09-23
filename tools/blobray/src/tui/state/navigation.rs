//! Cross-section navigation over reviewed relationships.

use super::{Action, BrowserState, Section};

impl BrowserState {
    pub(in crate::tui) fn activate(&mut self) -> Action {
        if self.section == Section::Comparisons {
            return self.begin_compare();
        }
        if self.section == Section::Functions
            && let Some(function) = self.snapshot.functions.get(self.selected())
        {
            let ambiguous = function.registers.iter().find_map(|address| {
                let candidates = self
                    .snapshot
                    .registers
                    .registers()
                    .filter(|register| register.subject.address == u64::from(*address))
                    .map(|register| register.id.as_str())
                    .collect::<Vec<_>>();
                (candidates.len() > 1).then(|| candidates.join(", "))
            });
            if let Some(candidates) = ambiguous {
                self.operation_failed(format!("MMIO address matches several physical subjects; select one in Registers: {candidates}"));
                return Action::Continue;
            }
        }
        let target = match self.section {
            Section::Scopes => self
                .snapshot
                .review_scopes
                .get(self.selected())
                .and_then(|scope| {
                    scope.function_identities.iter().find_map(|identity| {
                        self.snapshot
                            .functions
                            .iter()
                            .position(|function| function.identity == *identity)
                    })
                })
                .map(|index| (Section::Functions, index, "scope function")),
            Section::Functions => {
                self.snapshot
                    .functions
                    .get(self.selected())
                    .and_then(|function| {
                        self.snapshot
                            .registers
                            .registers()
                            .position(|register| {
                                register.subject.route == "mmio"
                                    && register.subject.bank.is_none()
                                    && function.registers.iter().any(|address| {
                                        u64::from(*address) == register.subject.address
                                    })
                            })
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
                self.snapshot
                    .registers
                    .register_at(self.selected())
                    .and_then(|register| {
                        self.snapshot.functions.iter().position(|function| {
                            register.functions.contains(&function.identity)
                                || (register.subject.route == "mmio"
                                    && register.subject.bank.is_none()
                                    && self
                                        .snapshot
                                        .registers
                                        .registers()
                                        .filter(|other| {
                                            other.subject.address == register.subject.address
                                        })
                                        .count()
                                        == 1
                                    && function.registers.iter().any(|address| {
                                        u64::from(*address) == register.subject.address
                                    }))
                        })
                    })
                    .map(|index| (Section::Functions, index, "register user"))
            }
            Section::Blockers => self
                .snapshot
                .review_queue
                .get(self.selected())
                .and_then(|blocker| {
                    blocker.functions.iter().find_map(|name| {
                        self.snapshot.functions.iter().position(|function| {
                            function.identity == *name
                                || function.symbol == *name
                                || name.ends_with(&format!("::{}", function.symbol))
                        })
                    })
                })
                .map(|index| (Section::Functions, index, "blocked function")),
            Section::Interfaces => self
                .snapshot
                .interfaces
                .slots
                .get(self.selected())
                .and_then(|slot| {
                    self.snapshot.functions.iter().position(|function| {
                        slot.functions
                            .iter()
                            .any(|name| name == &function.identity || name == &function.symbol)
                    })
                })
                .map(|index| (Section::Functions, index, "calling function")),
            Section::Types => {
                self.snapshot
                    .logical_types
                    .get(self.selected())
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
}
