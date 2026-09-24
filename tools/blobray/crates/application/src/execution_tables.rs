//! Runtime placement and condition checks over the session's current normal memory.
use super::*;
use crate::runtime_tables::Association;
impl Session<'_> {
    fn normal_value(&self, address: u32, width: u8) -> Option<u32> {
        if !matches!(width, 1 | 2 | 4) || !address.is_multiple_of(u32::from(width)) {
            return None;
        }
        let (i, o) = self.region_index(address, width)?;
        let r = &self.regions[i];
        let width = usize::from(width);
        if r.flags & 4 == 0 || r.known[o..o + width].contains(&0) {
            return None;
        }
        let mut bytes = [0; 4];
        bytes[..width].copy_from_slice(&r.bytes[o..o + width]);
        Some(u32::from_le_bytes(bytes))
    }
    fn executable(&self, address: u32) -> bool {
        address & 1 == 0
            && self.region_index(address, 2).is_some_and(|(i, o)| {
                self.regions[i].kind == RegionKind::Image
                    && self.regions[i].flags & 1 != 0
                    && !self.regions[i].known[o..o + 2].contains(&0)
            })
    }
    fn table_setup_word(&mut self, address: u32, value: u32, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(self.regions.len() as u64 + 1)?;
        let (i, o) = self
            .region_index(address, 4)
            .filter(|(i, _)| address.is_multiple_of(4) && self.regions[*i].flags & 2 != 0)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidRequest,
                    "table setup needs existing writable normal memory",
                )
            })?;
        self.regions[i].bytes[o..o + 4].copy_from_slice(&value.to_le_bytes());
        self.regions[i].known[o..o + 4].fill(1);
        self.invalidate_reservation(address, 4);
        Ok(())
    }
    pub(super) fn table_write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        site: Option<u32>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if let Some((instance, event)) = self.tables.note_write(address, width, value, site, c)? {
            self.event(ExecutionEvent::RuntimeTable { instance, event }, c)?;
        }
        Ok(())
    }
    fn table_condition(
        &self,
        instance: u16,
        c: &mut dyn RunControl,
    ) -> Result<Option<RuntimeTableIssue>> {
        let table = self.tables.get(instance);
        let contract = &table.contract;
        macro_rules! gap {
            ($issue:expr) => {
                return Ok(Some($issue))
            };
        }
        for domain in &contract.index_domains {
            c.checkpoint(1)?;
            if table.words[usize::from(domain.word)]
                .is_none_or(|v| v < domain.min || v > domain.max)
            {
                gap!(RuntimeTableIssue::IndexPrecondition { word: domain.word });
            }
        }
        let mut address = match table.root {
            RuntimeRoot::Address { address } => address,
            RuntimeRoot::EntryWord { word, .. } => match table.words[usize::from(word)] {
                Some(v) => v,
                None => gap!(RuntimeTableIssue::UnknownRoot),
            },
        };
        for step in &contract.path {
            c.checkpoint(self.regions.len() as u64 + 1)?;
            address = match *step {
                AccessStep::Offset { bytes } => match address.checked_add_signed(bytes) {
                    Some(v) => v,
                    None => gap!(RuntimeTableIssue::RootAddressOverflow),
                },
                AccessStep::Index { word, stride } => {
                    let Some(index) = table.words[usize::from(word)] else {
                        gap!(RuntimeTableIssue::IndexPrecondition { word });
                    };
                    let Some(value) = index
                        .checked_mul(stride)
                        .and_then(|n| address.checked_add(n))
                    else {
                        gap!(RuntimeTableIssue::RootAddressOverflow)
                    };
                    value
                }
                AccessStep::LoadPointer { offset } => {
                    let Some(pointer) = address.checked_add_signed(offset) else {
                        gap!(RuntimeTableIssue::RootAddressOverflow)
                    };
                    let Some(value) = self.normal_value(pointer, 4) else {
                        gap!(RuntimeTableIssue::UnknownPointer { address: pointer })
                    };
                    value
                }
            };
        }
        if address != table.declaration.seed.address {
            gap!(RuntimeTableIssue::RootMismatch {
                actual: address,
                expected: table.declaration.seed.address
            });
        }
        for guard in &contract.guards {
            let InterfaceGuard::RuntimeValue {
                offset,
                width,
                mask,
                value,
                ..
            } = *guard
            else {
                continue;
            };
            c.checkpoint(self.regions.len() as u64 + 1)?;
            let Some(location) = address.checked_add(offset) else {
                gap!(RuntimeTableIssue::RootAddressOverflow)
            };
            let Some(actual) = self.normal_value(location, width) else {
                gap!(RuntimeTableIssue::GuardUnknown { offset })
            };
            if actual & mask != value {
                gap!(RuntimeTableIssue::GuardMismatch { offset, actual });
            }
        }
        Ok(None)
    }
    fn reviewed_model(
        &self,
        table: u16,
        offset: u32,
        target: u32,
        call: usize,
        c: &mut dyn RunControl,
    ) -> Result<Option<RuntimeTableIssue>> {
        c.checkpoint(self.tables.get(table).contract.slots.len() as u64)?;
        let Some(reviewed) = self
            .tables
            .get(table)
            .contract
            .slots
            .iter()
            .find(|s| s.offset == offset)
        else {
            return Ok(Some(RuntimeTableIssue::UnreviewedModel { target }));
        };
        let Some(signature) = reviewed
            .signature
            .as_ref()
            .filter(|_| reviewed.semantic.is_some())
        else {
            return Ok(Some(RuntimeTableIssue::UnreviewedModel { target }));
        };
        // Only named fixed arguments impose a minimum; variadic words remain explicitly lowered.
        let mut count = 0u16;
        for argument in &signature.arguments {
            c.checkpoint(1)?;
            let width = match argument.value_type {
                AbiValueType::Integer { bits: 64, .. } => 2,
                AbiValueType::Integer {
                    bits: 8 | 16 | 32, ..
                }
                | AbiValueType::Pointer { .. } => 1,
                _ => return Ok(Some(RuntimeTableIssue::ModelAbi { target })),
            };
            if count >= 8 && width == 2 {
                count += count % 2;
            }
            count += width;
        }
        Ok((self.calls.declaration(call).argument_words < count)
            .then_some(RuntimeTableIssue::ModelAbi { target }))
    }
    pub(super) fn install_tables(
        &mut self,
        prepared: &[crate::execution_interfaces::PreparedTable<'_>],
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<Option<(u16, RuntimeTableIssue)>> {
        self.tables.begin_phase();
        let mut added = AdmittedVec::new(self.memory);
        for table in prepared {
            let seed = &table.declaration.seed;
            self.region(
                Mapping {
                    address: seed.address,
                    length: seed.length as usize,
                    flags: 6,
                    kind: RegionKind::Table(table.declaration.lifetime),
                },
                seed.fill,
                &seed.bytes,
                c,
            )?;
            added.push(self.tables.install(table, input, c)?, c.position())?;
        }
        for &instance in &*added {
            let count = self.tables.get(instance).declaration.slots.len();
            for index in 0..count {
                c.checkpoint(1)?;
                let slot = self.tables.get(instance).declaration.slots[index];
                let address = self.tables.get(instance).declaration.seed.address + slot.offset;
                let issue = match slot.target {
                    RuntimeSlotTarget::Null => None,
                    RuntimeSlotTarget::Code { address } => (!self.executable(address))
                        .then_some(RuntimeTableIssue::UnavailableTarget { target: address }),
                    RuntimeSlotTarget::Model { address } => match self.calls.find(address, c)? {
                        Some(call) => {
                            self.reviewed_model(instance, slot.offset, address, call, c)?
                        }
                        None => Some(RuntimeTableIssue::UnavailableTarget { target: address }),
                    },
                };
                if let Some(issue) = issue {
                    self.tables.fail(Some(instance), issue);
                    return Ok(Some((instance, issue)));
                }
                self.table_setup_word(address, slot.target.address(), c)?;
                self.tables.initialized(instance);
                self.event(
                    ExecutionEvent::RuntimeTable {
                        instance,
                        event: RuntimeTableEvent::Initialized {
                            offset: slot.offset,
                            target: slot.target.address(),
                        },
                    },
                    c,
                )?;
            }
        }
        for &instance in &*added {
            let count = self.tables.get(instance).declaration.pointer_cells.len();
            for index in 0..count {
                let table = self.tables.get(instance);
                let (address, base) = (
                    table.declaration.pointer_cells[index],
                    table.declaration.seed.address,
                );
                self.table_setup_word(address, base, c)?;
                self.table_write(address, 4, base, None, c)?;
                self.tables.pointer_installed(instance);
                self.event(
                    ExecutionEvent::RuntimeTable {
                        instance,
                        event: RuntimeTableEvent::PointerInstalled { address, base },
                    },
                    c,
                )?;
            }
        }
        let mut live = AdmittedVec::new(self.memory);
        for (instance, _) in self.tables.live() {
            c.checkpoint(1)?;
            live.push(instance, c.position())?;
        }
        for &instance in &*live {
            if let Some(issue) = self.table_condition(instance, c)? {
                self.tables.fail(Some(instance), issue);
                return Ok(Some((instance, issue)));
            }
            self.tables.checked(instance);
            self.event(
                ExecutionEvent::RuntimeTable {
                    instance,
                    event: RuntimeTableEvent::ConditionsChecked,
                },
                c,
            )?;
        }
        Ok(None)
    }
    pub(super) fn interface_call(
        &mut self,
        input: &CallInput,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallDispatch>> {
        if self.tables.live().next().is_none() {
            return Ok(None);
        }
        let (instance, offset, issue) = if input.target == 0 {
            (None, 0, Some(RuntimeTableIssue::NullTarget))
        } else {
            match self.tables.associate(input.target, c)? {
                Association::None => (
                    None,
                    0,
                    Some(RuntimeTableIssue::UnassociatedTarget {
                        target: input.target,
                    }),
                ),
                Association::Ambiguous(candidates) => (
                    None,
                    0,
                    Some(RuntimeTableIssue::AmbiguousTarget {
                        target: input.target,
                        candidates,
                    }),
                ),
                Association::Slot { table, offset } => {
                    let mut issue = self.table_condition(table, c)?;
                    if issue.is_none() {
                        issue = if let Some(call) = self.calls.find(input.target, c)? {
                            self.reviewed_model(table, offset, input.target, call, c)?
                        } else {
                            (!self.executable(input.target)).then_some(
                                RuntimeTableIssue::UnavailableTarget {
                                    target: input.target,
                                },
                            )
                        };
                    }
                    (Some(table), offset, issue)
                }
            }
        };
        if let Some(issue) = issue {
            self.tables.fail(instance, issue);
            return Ok(Some(CallDispatch::RuntimeInterface { instance, issue }));
        }
        let instance = instance.unwrap();
        self.tables.checked(instance);
        self.event(
            ExecutionEvent::RuntimeTable {
                instance,
                event: RuntimeTableEvent::ConditionsChecked,
            },
            c,
        )?;
        self.tables.associated(instance)?;
        self.event(
            ExecutionEvent::RuntimeTable {
                instance,
                event: RuntimeTableEvent::IndirectTarget {
                    site: input.site,
                    target: input.target,
                    offset,
                },
            },
            c,
        )?;
        Ok(None)
    }
}
