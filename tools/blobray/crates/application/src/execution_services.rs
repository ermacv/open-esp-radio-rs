//! Reviewed service dispatch: validate all inputs/outputs before queue mutation.
use super::*;
use crate::runtime_tables::Association;
impl Session<'_> {
    pub(super) fn validate_service_targets(
        &self,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for d in &input.services {
            for b in &d.bindings {
                c.checkpoint((input.tables.len() + MAX_RUNTIME_TABLES + 64) as u64)?;
                let table = input.tables.iter().find(|t| t.id == b.table).or_else(|| {
                    self.tables
                        .live()
                        .find(|(_, t)| t.declaration.id == b.table)
                        .map(|(_, t)| &t.declaration)
                });
                if table.is_none_or(|t| {
                    !t.slots.iter().any(|s| {
                        s.offset == b.slot
                            && s.target
                                == (RuntimeSlotTarget::Service {
                                    address: b.call.address,
                                })
                    })
                }) {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "FIFO binding differs from selected service slot",
                    ));
                }
            }
        }
        for (_, _, b) in self.services.bindings() {
            if self.calls.find(b.call.address, c)?.is_some() {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "FIFO and call model share a target",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn reviewed_service(
        &self,
        table: u16,
        offset: u32,
        service: u16,
        binding: u16,
        c: &mut dyn RunControl,
    ) -> Result<Option<RuntimeTableIssue>> {
        let b = self.services.binding(service, binding);
        if self.tables.get(table).declaration.id != b.table || offset != b.slot {
            return Ok(Some(RuntimeTableIssue::UnreviewedModel {
                target: b.call.address,
            }));
        }
        self.reviewed_model(table, offset, b.call.address, b.argument_words, c)
    }
    pub(super) fn prepare_services(
        &mut self,
        input: &Invocation,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for d in &input.services {
            for b in &d.bindings {
                c.checkpoint(MAX_RUNTIME_TABLES as u64)?;
                let Some((table, instance)) = self
                    .tables
                    .live()
                    .find(|(_, v)| v.declaration.id == b.table)
                else {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "FIFO binding requires a selected live interface",
                    ));
                };
                c.checkpoint(instance.declaration.slots.len() as u64)?;
                if !instance.declaration.slots.iter().any(|s| {
                    s.offset == b.slot
                        && s.target
                            == (RuntimeSlotTarget::Service {
                                address: b.call.address,
                            })
                }) || self
                    .reviewed_model(table, b.slot, b.call.address, b.argument_words, c)?
                    .is_some()
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "FIFO binding differs from reviewed service slot",
                    ));
                }
            }
        }
        if let ExecutionGoal::ObserveDequeue { service, value } = &input.goal {
            let instance = self.services.by_id(service, c)?.ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidRequest,
                    "dequeue goal requires a live selected service",
                )
            })?;
            if value.is_some_and(|v| !self.services.get(instance).declaration.fits(v)) {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "dequeue goal value exceeds item width",
                ));
            }
            self.service_goal = Some((instance, *value));
        }
        Ok(())
    }
    pub(super) fn service_call(
        &mut self,
        input: &CallInput,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallDispatch>> {
        let Some((instance, binding)) = self.services.find(input.target, c)? else {
            return Ok(None);
        };
        macro_rules! gap {
            ($issue:expr) => {
                return Ok(Some(self.services.fail(instance, $issue)))
            };
        }
        macro_rules! call_gap {
            ($issue:expr) => {
                gap!(FifoIssue::Call { issue: $issue })
            };
        }
        let b = self.services.binding(instance, binding);
        if !input.indirect
            || !matches!(self.tables.associate(input.target, c)?, Association::Slot { table, offset } if self.tables.get(table).declaration.id == b.table && offset == b.slot)
        {
            gap!(FifoIssue::UnreviewedBinding {
                target: input.target
            });
        }
        if input.tail && !b.call.allow_tail {
            call_gap!(CallIssue::TailNotAllowed);
        }
        let (words, handle_word, operation) = (b.argument_words, b.handle_word, b.operation);
        let mut arguments = [None; MAX_EXECUTION_ARGUMENT_WORDS];
        if let Some(issue) = self.read_call_words(input, words, &mut arguments, c)? {
            call_gap!(issue);
        }
        // Start/arguments/at most one output/result are admitted before queue/output mutation.
        if usize::from(words) + 4 > self.max_events - self.events.len() {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "FIFO call exceeds event capacity",
            ));
        }
        self.event(
            ExecutionEvent::ServiceCall {
                instance,
                binding,
                site: input.site,
                target: input.target,
                tail: input.tail,
            },
            c,
        )?;
        for word in 0..words {
            self.event(
                ExecutionEvent::ServiceArgument {
                    word,
                    value: arguments[word as usize],
                },
                c,
            )?;
        }
        macro_rules! argument {
            ($word:expr) => {
                match arguments[$word as usize] {
                    Some(v) => v,
                    None => call_gap!(CallIssue::UnknownArgument { word: $word }),
                }
            };
        }
        let handle = argument!(handle_word);
        let queue = self.services.get(instance);
        if handle != queue.declaration.handle {
            gap!(FifoIssue::Handle {
                expected: queue.declaration.handle,
                actual: handle
            });
        }
        let mut captured_input = None;
        let (transition, result, output) = match operation {
            FifoOperation::Length => (FifoTransition::Length, queue.depth, None),
            FifoOperation::Enqueue {
                input: source,
                success,
                full,
                wake,
            } => {
                let value = match source {
                    FifoInput::Argument { word, .. } => argument!(word),
                    FifoInput::PrivateStack { word, width } => {
                        let address = argument!(word);
                        c.checkpoint(self.regions.len() as u64 + 1)?;
                        if self
                            .region_index(address, width)
                            .is_none_or(|(i, _)| self.regions[i].kind != RegionKind::Stack)
                        {
                            gap!(FifoIssue::InputAccess { address });
                        }
                        let Some(v) = self.normal_value(address, width) else {
                            gap!(FifoIssue::InputAccess { address });
                        };
                        captured_input = Some((address, width, v));
                        v
                    }
                };
                if !queue.declaration.fits(value) {
                    gap!(FifoIssue::ItemWidth { value });
                }
                let full_queue = queue.depth == queue.declaration.capacity;
                let woke = !full_queue && queue.depth == 0;
                let transition = if full_queue {
                    FifoTransition::Full { value }
                } else {
                    FifoTransition::Enqueued { value, woke }
                };
                (
                    transition,
                    if full_queue { full } else { success },
                    wake.map(|o| (o, u32::from(woke))),
                )
            }
            FifoOperation::Dequeue {
                output,
                success,
                empty,
            } => match queue.front() {
                Some(value) => (
                    FifoTransition::Dequeued { value },
                    success,
                    Some((output, value)),
                ),
                None => (FifoTransition::Empty, empty, None),
            },
        };
        if let Some((address, width, value)) = captured_input {
            self.event(
                ExecutionEvent::ServiceInput {
                    address,
                    width,
                    value,
                },
                c,
            )?;
        }
        let destination = if let Some((o, value)) = output {
            let address = argument!(o.word);
            c.checkpoint(self.regions.len() as u64 + 1)?;
            let Some((i, offset)) =
                self.output_location(address, o.width, CallOutputScope::PrivateStack)
            else {
                call_gap!(CallIssue::OutputAccess {
                    address,
                    scope: CallOutputScope::PrivateStack
                });
            };
            Some((i, offset, address, o.width, value))
        } else {
            None
        };
        let depth = self.services.commit(instance, transition)?;
        if let Some((i, offset, address, width, value)) = destination {
            self.regions[i].bytes[offset..offset + width as usize]
                .copy_from_slice(&value.to_le_bytes()[..width as usize]);
            self.regions[i].known[offset..offset + width as usize].fill(1);
            self.invalidate_reservation(address, width);
            self.event(
                ExecutionEvent::ServiceOutput {
                    address,
                    width,
                    value,
                },
                c,
            )?;
        }
        let words = [Some(result), None];
        self.event(
            ExecutionEvent::ServiceResult {
                instance,
                transition,
                depth,
                words,
            },
            c,
        )?;
        if let FifoTransition::Dequeued { value } = transition
            && self.service_goal.is_some_and(|(selected, expected)| {
                selected == instance && expected.is_none_or(|v| v == value)
            })
        {
            return Ok(Some(CallDispatch::ObservedDequeue { instance, value }));
        }
        Ok(Some(CallDispatch::Returned { words }))
    }
}
