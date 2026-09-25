//! One read-query owner for captured slots, saved call paths and selected declarations.
use crate::*;
use blobray_analysis::paths::PathKey;
struct Binding<'a> {
    key: PathKey,
    entry: &'a KnowledgeEntry,
    slot: &'a InterfaceSlot,
    conditional: bool,
}
struct Bindings<'a> {
    values: AdmittedVec<'a, Binding<'a>>,
    _payloads: AdmittedVec<'a, MemoryReservation<'a>>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
impl<'a> Bindings<'a> {
    fn new(
        snapshot: &'a blobray_store::KnowledgeSnapshot<'_>,
        occurrence: &KnowledgeOccurrence,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut values = AdmittedVec::new(memory);
        let mut payloads = AdmittedVec::new(memory);
        for entry in snapshot.entries() {
            c.checkpoint(1)?;
            let p = &entry.proposal;
            if p.occurrence.revision != occurrence.revision
                || p.occurrence.source != occurrence.source
                || p.occurrence.object != occurrence.object
            {
                continue;
            }
            let KnowledgeClaim::Interface { contract } = &p.claim else {
                continue;
            };
            blobray_knowledge::validate_proposal(p)?;
            let conditional = !contract.index_domains.is_empty()
                || contract
                    .guards
                    .iter()
                    .any(|g| matches!(g, InterfaceGuard::RuntimeValue { .. }));
            for slot in &contract.slots {
                c.checkpoint(contract.path.len() as u64 + 1)?;
                let key = PathKey::new(&contract.root, &contract.path, slot.offset);
                payloads.push(
                    memory.reserve(key.allocated_bytes(), c.position())?,
                    c.position(),
                )?;
                values.push(
                    Binding {
                        key,
                        entry,
                        slot,
                        conditional,
                    },
                    c.position(),
                )?;
            }
        }
        c.checkpoint(values.len() as u64 * (values.len().max(1).ilog2() as u64 + 1))?;
        values.sort_unstable_by(|a, b| a.key.cmp(&b.key).then_with(|| a.entry.id.cmp(&b.entry.id)));
        Ok(Self {
            values,
            _payloads: payloads,
        })
    }
    fn emit(
        &self,
        mut observation: InterfaceObservation,
        summary: &mut InterfaceSummary,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(&InterfaceObservation, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        let mut matches = AdmittedVec::new(memory);
        for path in &observation.paths {
            c.checkpoint(path.path.len() as u64 + 1)?;
            let key = PathKey::new(&path.root, &path.path, path.slot);
            c.checkpoint(self.values.len().max(1).ilog2() as u64 + 1)?;
            let first = self.values.partition_point(|b| b.key < key);
            for (i, _) in self.values[first..]
                .iter()
                .enumerate()
                .take_while(|(_, b)| b.key == key)
            {
                c.checkpoint(1)?;
                matches.push(first + i, c.position())?;
            }
        }
        c.checkpoint(matches.len() as u64 * (matches.len().max(1).ilog2() as u64 + 1))?;
        matches.sort_unstable();
        let mut bytes = 0u64;
        let mut previous = None;
        let mut count = 0;
        for &i in matches.iter() {
            if previous == Some(i) {
                continue;
            }
            previous = Some(i);
            count += 1;
            let slot = self.values[i].slot;
            bytes = bytes
                .checked_add(
                    std::mem::size_of::<InterfaceBinding>() as u64
                        + 64
                        + slot.name.capacity() as u64
                        + slot.semantic.as_ref().map_or(0, SubjectId::allocated_bytes)
                        + slot
                            .signature
                            .as_ref()
                            .map_or(0, CallSignature::allocated_bytes),
                )
                .ok_or_else(|| invalid("interface binding capacity overflow"))?;
        }
        let _capacity = memory.reserve(bytes, c.position())?;
        observation.bindings.try_reserve_exact(count).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "interface binding allocation refused",
            )
        })?;
        previous = None;
        let mut accepted = 0;
        for &i in matches.iter() {
            c.checkpoint(1)?;
            if previous == Some(i) {
                continue;
            }
            previous = Some(i);
            let b = &self.values[i];
            if b.entry.state == AssertionState::Accepted {
                accepted += 1;
                summary.conditional += u64::from(b.conditional);
            }
            observation.bindings.push(InterfaceBinding {
                assertion: b.entry.id.clone(),
                state: b.entry.state,
                name: b.slot.name.clone(),
                semantic: b.slot.semantic.clone(),
                signature: b.slot.signature.clone(),
                conditions: if b.conditional {
                    InterfaceConditions::RuntimeConditionsUnverified
                } else {
                    InterfaceConditions::NoRuntimeConditions
                },
            });
        }
        summary.observations += 1;
        summary.unresolved_paths += u64::from(observation.paths.is_empty());
        summary.issues += u64::from(observation.issue.is_some());
        summary.matched_accepted += accepted;
        summary.ambiguous_bindings += u64::from(accepted > 1);
        emit(&observation, c)
    }
}
pub(crate) fn discover(
    project: &Project,
    request: &InterfaceQuery,
    decoder: Option<&dyn FunctionSemantics>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&InterfaceObservation, &mut dyn RunControl) -> Result<()>,
) -> Result<InterfaceSummary> {
    let _construction = memory.reserve(CONTROL_MESSAGE_BYTES as u64, c.position())?;
    let snapshot = project.knowledge_snapshot(request.knowledge.as_ref(), memory, c)?;
    match &request.input {
        InterfaceInput::Analysis { analysis, abi } => {
            let lease = project.analysis(analysis, c)?;
            let recipe = &lease.manifest.recipe;
            let occurrence = KnowledgeOccurrence {
                revision: recipe.revision.clone(),
                source: recipe.source.clone(),
                object: recipe.selector.object().clone(),
                symbol: recipe.selector.symbol().cloned(),
            };
            let index = Bindings::new(&snapshot, &occurrence, memory, c)?;
            let mut summary = InterfaceSummary {
                schema: 1,
                pointer_producer: None,
                captured_span: None,
                request: request.clone(),
                occurrence,
                payload: recipe.payload.clone(),
                observations: 0,
                unresolved_paths: 0,
                issues: 0,
                matched_accepted: 0,
                ambiguous_bindings: 0,
                conditional: 0,
            };
            let records = crate::research::load_records(&lease.records, memory, c)?;
            blobray_analysis::interfaces::discover(
                &records,
                recipe,
                *abi,
                memory,
                c,
                &mut |r, c| index.emit(r, &mut summary, memory, c, emit),
            )?;
            Ok(summary)
        }
        InterfaceInput::Data {
            occurrence,
            selector,
            layout,
        } => {
            let decoder = decoder.ok_or_else(|| {
                Error::new(
                    ErrorCode::Incompatible,
                    "interface pointer decoder unavailable",
                )
            })?;
            let producer = decoder.pointer_identity().ok_or_else(|| {
                Error::new(
                    ErrorCode::Incompatible,
                    "interface pointer profile unavailable",
                )
            })?;
            let index = Bindings::new(&snapshot, occurrence, memory, c)?;
            crate::data::with_object(project, occurrence, memory, c, |payload, object, c| {
                object.with_data(&occurrence.object, selector, c, |view, c| {
                    let root = match selector {
                        DataSelector::Symbol { symbol, .. } => AccessRoot::Symbol {
                            symbol: symbol.clone(),
                            addend: 0,
                        },
                        DataSelector::Image { address, .. } => AccessRoot::Address {
                            address: u32::try_from(*address)
                                .map_err(|_| invalid("interface address exceeds RV32"))?,
                        },
                        DataSelector::Section {
                            section, offset, ..
                        } => AccessRoot::Section {
                            section: *section,
                            offset: *offset,
                        },
                    };
                    let mut summary = InterfaceSummary {
                        schema: 1,
                        pointer_producer: Some(producer.into()),
                        captured_span: Some(view.span.clone()),
                        request: request.clone(),
                        occurrence: occurrence.as_ref().clone(),
                        payload: payload.clone(),
                        observations: 0,
                        unresolved_paths: 0,
                        issues: 0,
                        matched_accepted: 0,
                        ambiguous_bindings: 0,
                        conditional: 0,
                    };
                    let start = if view.address_space == CodeAddressSpace::Image {
                        view.section_address
                            .checked_add(view.span.section_range.start)
                            .ok_or_else(|| invalid("interface relocation address overflow"))?
                    } else {
                        view.span.section_range.start
                    };
                    blobray_analysis::pointers::analyze(
                        blobray_analysis::pointers::PointerInput {
                            bytes: view.bytes,
                            start,
                            image: view.address_space == CodeAddressSpace::Image,
                            unknown_write_extents: view.span.unknown_relocation_extents > 0,
                            max_write_bytes: view.max_relocation_width,
                            relocations: view.relocations,
                        },
                        layout,
                        decoder,
                        c,
                        &mut |r, c| {
                            let DataRecord::Pointer {
                                index: slot,
                                offset,
                                value,
                                ..
                            } = r
                            else {
                                return Ok(());
                            };
                            let (paths, issue) = match u32::try_from(*offset) {
                                Ok(slot) => (
                                    vec![InterfaceAccessPath {
                                        root: root.clone(),
                                        path: vec![],
                                        slot,
                                    }],
                                    matches!(value, PointerValue::Unresolved { .. })
                                        .then_some(AccessIssue::UnresolvedPointer),
                                ),
                                Err(_) => (vec![], Some(AccessIssue::OffsetOutOfRange)),
                            };
                            index.emit(
                                InterfaceObservation {
                                    ordinal: *slot,
                                    record: *slot,
                                    offset: *offset,
                                    paths,
                                    target: None,
                                    pointer: Some(value.clone()),
                                    issue,
                                    bindings: vec![],
                                },
                                &mut summary,
                                memory,
                                c,
                                emit,
                            )
                        },
                    )?;
                    Ok(summary)
                })
            })
        }
    }
}
