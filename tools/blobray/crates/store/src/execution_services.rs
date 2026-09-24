//! Bounded structural FIFO transcript validation; never execute guest instructions.
use super::*;
struct Live<'a, 'm> {
    declaration: &'a FifoService,
    definition: ArtifactId,
    ring: ScratchBytes<'m>,
    head: u32,
    depth: u32,
    operations: u64,
    issue: Option<FifoIssue>,
    seen: bool,
    closed: bool,
}
impl Live<'_, '_> {
    fn front(&self) -> Option<u32> {
        if self.depth == 0 {
            None
        } else {
            let o = self.head as usize * 4;
            Some(u32::from_le_bytes(self.ring[o..o + 4].try_into().unwrap()))
        }
    }
}
struct Pending {
    instance: u16,
    binding: u16,
    arguments: [Option<u32>; MAX_EXECUTION_ARGUMENT_WORDS],
    next: u16,
    input: Option<u32>,
    output: Option<(u32, u8, u32)>,
}
pub(super) struct Services<'a, 'm> {
    memory: &'m WorkingMemory,
    live: AdmittedVec<'m, Option<Live<'a, 'm>>>,
    pending: Option<Pending>,
    association: Option<(u16, u32, u32, u32)>,
    goal: Option<(&'a str, Option<u32>)>,
    stack: Option<&'a MemorySeed>,
    matched: Option<(u16, u32)>,
}
impl<'a, 'm> Services<'a, 'm> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            live: AdmittedVec::new(memory),
            pending: None,
            association: None,
            goal: None,
            stack: None,
            matched: None,
        }
    }
    pub fn begin(
        &mut self,
        input: &'a Invocation,
        stack: &'a MemorySeed,
        reset: SessionReset,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if reset == SessionReset::Cold && self.live.iter().any(Option::is_some) {
            return Err(integrity("cold reset has live FIFO owners"));
        }
        self.matched = None;
        self.association = None;
        self.stack = Some(stack);
        self.goal = match &input.goal {
            ExecutionGoal::ObserveDequeue { service, value } => Some((service, *value)),
            _ => None,
        };
        for v in self.live.iter_mut().flatten() {
            v.seen = false;
        }
        if blocked {
            return Ok(());
        }
        for d in &input.services {
            c.checkpoint(self.live.len() as u64 + 1)?;
            if self
                .live
                .iter()
                .flatten()
                .any(|v| v.declaration.id == d.id || v.declaration.handle == d.handle)
            {
                return Err(integrity("duplicate live FIFO id/handle"));
            }
            let mut ring = self.memory.bytes(d.capacity as usize * 4, c.position())?;
            for (i, value) in d.items.iter().enumerate() {
                c.checkpoint(1)?;
                ring[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            let v = Live {
                declaration: d,
                definition: d.identity(c)?,
                ring,
                head: 0,
                depth: d.items.len() as u32,
                operations: 0,
                issue: None,
                seen: false,
                closed: false,
            };
            if let Some(i) = self.live.iter().position(Option::is_none) {
                self.live[i] = Some(v);
            } else {
                if self.live.len() == MAX_FIFO_SERVICES {
                    return Err(integrity("too many FIFO owners"));
                }
                self.live.push(Some(v), c.position())?;
            }
        }
        Ok(())
    }
    pub fn validate_bindings(
        &self,
        input: &Invocation,
        tables: &crate::execution_tables::Tables<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for d in &input.services {
            for b in &d.bindings {
                c.checkpoint((MAX_RUNTIME_TABLES + 64) as u64)?;
                let table = tables
                    .declaration(&b.table)
                    .ok_or_else(|| integrity("FIFO binding lacks selected live table"))?;
                if !table.slots.iter().any(|s| {
                    s.offset == b.slot
                        && s.target
                            == (RuntimeSlotTarget::Service {
                                address: b.call.address,
                            })
                }) {
                    return Err(integrity("FIFO binding differs from selected service slot"));
                }
            }
        }
        Ok(())
    }
    pub fn event(
        &mut self,
        event: &ExecutionEvent,
        tables: &crate::execution_tables::Tables<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(1)?;
        if self.matched.is_some() {
            return Err(integrity("event follows successful dequeue goal"));
        }
        let previous = self.association.take();
        if let ExecutionEvent::RuntimeTable {
            instance,
            event:
                RuntimeTableEvent::IndirectTarget {
                    site,
                    target,
                    offset,
                },
        } = event
        {
            self.association = Some((*instance, *site, *target, *offset));
        }
        if let ExecutionEvent::ServiceCall {
            instance,
            binding,
            site,
            target,
            tail,
        } = event
        {
            if self.pending.is_some() {
                return Err(integrity("nested FIFO response"));
            }
            let v = self
                .live
                .get(*instance as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| integrity("FIFO call has no instance"))?;
            let b = v
                .declaration
                .bindings
                .get(*binding as usize)
                .ok_or_else(|| integrity("FIFO call has no binding"))?;
            if b.call.address != *target
                || (*tail && !b.call.allow_tail)
                || !previous.is_some_and(|(table, s, t, o)| {
                    s == *site
                        && t == *target
                        && o == b.slot
                        && tables.instance_id(table) == Some(b.table.as_str())
                })
            {
                return Err(integrity("FIFO call lacks selected reviewed association"));
            }
            self.pending = Some(Pending {
                instance: *instance,
                binding: *binding,
                arguments: [None; MAX_EXECUTION_ARGUMENT_WORDS],
                next: 0,
                input: None,
                output: None,
            });
            return Ok(());
        }
        let service_event = matches!(
            event,
            ExecutionEvent::ServiceArgument { .. }
                | ExecutionEvent::ServiceInput { .. }
                | ExecutionEvent::ServiceOutput { .. }
                | ExecutionEvent::ServiceResult { .. }
        );
        if !service_event {
            if self.pending.is_some() {
                return Err(integrity("unrelated event interrupts FIFO response"));
            }
            return Ok(());
        }
        let p = self
            .pending
            .as_mut()
            .ok_or_else(|| integrity("FIFO effect lacks call"))?;
        let v = self.live[p.instance as usize].as_mut().unwrap();
        let b = &v.declaration.bindings[p.binding as usize];
        if let ExecutionEvent::ServiceArgument { word, value } = event {
            if *word != p.next
                || p.next >= b.argument_words
                || p.input.is_some()
                || p.output.is_some()
            {
                return Err(integrity("FIFO argument order differs"));
            }
            p.arguments[*word as usize] = *value;
            p.next += 1;
            return Ok(());
        }
        if p.next != b.argument_words
            || p.arguments[b.handle_word as usize] != Some(v.declaration.handle)
        {
            return Err(integrity("FIFO effect lacks arguments or correct handle"));
        }
        let stack = self.stack.unwrap();
        let in_stack = |address: u32, width: u8| {
            matches!(width, 1 | 2 | 4)
                && address.is_multiple_of(width as u32)
                && address >= stack.address
                && u64::from(address) + u64::from(width)
                    <= u64::from(stack.address) + u64::from(stack.length)
        };
        match event {
            ExecutionEvent::ServiceInput {
                address,
                width,
                value,
            } => {
                let FifoOperation::Enqueue {
                    input: FifoInput::PrivateStack { word, width: w },
                    ..
                } = b.operation
                else {
                    return Err(integrity("unexpected FIFO input read"));
                };
                if p.input.is_some()
                    || p.output.is_some()
                    || *width != w
                    || !in_stack(*address, w)
                    || p.arguments[word as usize] != Some(*address)
                    || !v.declaration.fits(*value)
                {
                    return Err(integrity("FIFO input read differs"));
                }
                p.input = Some(*value);
            }
            ExecutionEvent::ServiceOutput {
                address,
                width,
                value,
            } => {
                if p.output.is_some() || !in_stack(*address, *width) {
                    return Err(integrity("duplicate or non-stack FIFO output"));
                }
                p.output = Some((*address, *width, *value));
            }
            ExecutionEvent::ServiceResult {
                instance,
                transition,
                depth,
                words,
            } => {
                if *instance != p.instance {
                    return Err(integrity("FIFO result changes instance"));
                }
                let (expected, result, output) = match b.operation {
                    FifoOperation::Length => (FifoTransition::Length, v.depth, None),
                    FifoOperation::Enqueue {
                        input,
                        success,
                        full,
                        wake,
                    } => {
                        let value = match input {
                            FifoInput::Argument { word, .. } => p.arguments[word as usize],
                            FifoInput::PrivateStack { .. } => p.input,
                        }
                        .ok_or_else(|| integrity("FIFO enqueue lacks known input"))?;
                        if !v.declaration.fits(value) {
                            return Err(integrity("FIFO item exceeds width"));
                        }
                        let full_queue = v.depth == v.declaration.capacity;
                        let woke = !full_queue && v.depth == 0;
                        (
                            if full_queue {
                                FifoTransition::Full { value }
                            } else {
                                FifoTransition::Enqueued { value, woke }
                            },
                            if full_queue { full } else { success },
                            wake.map(|o| (o, u32::from(woke))),
                        )
                    }
                    FifoOperation::Dequeue {
                        output,
                        success,
                        empty,
                    } => match v.front() {
                        Some(value) => (
                            FifoTransition::Dequeued { value },
                            success,
                            Some((output, value)),
                        ),
                        None => (FifoTransition::Empty, empty, None),
                    },
                };
                let output =
                    output.map(|(o, value)| (p.arguments[o.word as usize], o.width, value));
                if transition != &expected
                    || *words != [Some(result), None]
                    || p.output.map(|(a, w, v)| (Some(a), w, v)) != output
                    || p.output
                        .is_some_and(|(a, w, _)| !a.is_multiple_of(w as u32))
                {
                    return Err(integrity("FIFO transition, output or return differs"));
                }
                match expected {
                    FifoTransition::Enqueued { value, .. } => {
                        let at = ((v.head + v.depth) % v.declaration.capacity) as usize * 4;
                        v.ring[at..at + 4].copy_from_slice(&value.to_le_bytes());
                        v.depth += 1;
                    }
                    FifoTransition::Dequeued { value } => {
                        v.head = (v.head + 1) % v.declaration.capacity;
                        v.depth -= 1;
                        if self.goal.is_some_and(|(id, expected)| {
                            id == v.declaration.id && expected.is_none_or(|e| e == value)
                        }) {
                            self.matched = Some((*instance, value));
                        }
                    }
                    _ => {}
                }
                if *depth != v.depth {
                    return Err(integrity("FIFO depth differs"));
                }
                v.operations = v
                    .operations
                    .checked_add(1)
                    .ok_or_else(|| integrity("FIFO count overflow"))?;
                self.pending = None;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
    pub fn observe(&mut self, o: &FifoObservation, close: bool, blocked: bool) -> Result<()> {
        let v = self
            .live
            .get_mut(o.instance as usize)
            .and_then(Option::as_mut)
            .ok_or_else(|| integrity("FIFO observation lacks live owner"))?;
        if v.seen
            || o.id != v.declaration.id
            || o.definition != v.definition
            || o.lifetime != v.declaration.lifetime
            || o.operations != v.operations
            || o.depth != v.depth
            || o.closed != (close || v.declaration.lifetime == RegionLifetime::Phase)
            || o.status != o.expected_status()
            || ((blocked || v.issue.is_some()) && o.issue != v.issue)
        {
            return Err(integrity("FIFO identity, state or closure differs"));
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.instance == o.instance)
        {
            if o.issue.is_none() {
                return Err(integrity("unfinished FIFO call has no issue"));
            }
            self.pending = None;
        }
        v.seen = true;
        v.closed = o.closed;
        v.issue = o.issue;
        Ok(())
    }
    pub fn goal_valid(&self, stop: &ExecutionStop) -> bool {
        match stop {
            ExecutionStop::ObservedDequeue { instance, value } => {
                self.matched == Some((*instance, *value))
            }
            _ => self.matched.is_none(),
        }
    }
    pub fn finish_side(&mut self) -> Result<()> {
        if self.pending.is_some() || self.live.iter().flatten().any(|v| !v.seen) {
            return Err(integrity("FIFO participation missing"));
        }
        for slot in &mut *self.live {
            if slot.as_ref().is_some_and(|v| v.closed) {
                *slot = None;
            }
        }
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.live.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Invocation {
        Invocation {
            observe_memory: vec![],
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![RuntimeTable {
                id: "callbacks".into(),
                review: InterfaceReview {
                    knowledge: "1".repeat(64).parse().unwrap(),
                    assertion: "2".repeat(64).parse().unwrap(),
                },
                lifetime: RegionLifetime::Phase,
                seed: MemorySeed {
                    address: 0x3000,
                    length: 8,
                    fill: None,
                    bytes: vec![],
                },
                slots: vec![RuntimeSlot {
                    offset: 4,
                    target: RuntimeSlotTarget::Service { address: 0x2000 },
                }],
                pointer_cells: vec![],
            }],
            services: vec![FifoService {
                id: "queue".into(),
                applicability: "fixture".into(),
                lifetime: RegionLifetime::Phase,
                handle: 0x55,
                item_width: 4,
                capacity: 2,
                items: vec![],
                bindings: vec![FifoBinding {
                    table: "callbacks".into(),
                    slot: 4,
                    call: CallBinding {
                        address: 0x2000,
                        boundary: CallBoundary::Unmapped,
                        allow_tail: false,
                    },
                    argument_words: 3,
                    handle_word: 0,
                    operation: FifoOperation::Enqueue {
                        input: FifoInput::Argument { word: 1, width: 4 },
                        success: 1,
                        full: 0,
                        wake: Some(FifoOutput { word: 2, width: 4 }),
                    },
                }],
            }],
        }
    }
    fn start<'a, 'm>(
        input: &'a Invocation,
        stack: &'a MemorySeed,
        memory: &'m WorkingMemory,
    ) -> (Services<'a, 'm>, crate::execution_tables::Tables<'a>) {
        let c = &mut || Ok(());
        let mut t = crate::execution_tables::Tables::new();
        t.begin(&input.tables, SessionReset::Cold, false, c)
            .unwrap();
        for e in [
            RuntimeTableEvent::Initialized {
                offset: 4,
                target: 0x2000,
            },
            RuntimeTableEvent::ConditionsChecked,
        ] {
            t.event(0, &e, c).unwrap();
        }
        let mut s = Services::new(memory);
        s.begin(input, stack, SessionReset::Cold, false, c).unwrap();
        let e = ExecutionEvent::RuntimeTable {
            instance: 0,
            event: RuntimeTableEvent::IndirectTarget {
                site: 0x1000,
                target: 0x2000,
                offset: 4,
            },
        };
        s.event(&e, &t, c).unwrap();
        s.event(
            &ExecutionEvent::ServiceCall {
                instance: 0,
                binding: 0,
                site: 0x1000,
                target: 0x2000,
                tail: false,
            },
            &t,
            c,
        )
        .unwrap();
        for (word, value) in [0x55, 42, 0x8000].into_iter().enumerate() {
            s.event(
                &ExecutionEvent::ServiceArgument {
                    word: word as u16,
                    value: Some(value),
                },
                &t,
                c,
            )
            .unwrap();
        }
        (s, t)
    }
    #[test]
    fn forged_fifo_effects_counts_and_goals_are_rejected() {
        let input = input();
        let stack = MemorySeed {
            address: 0x8000,
            length: 16,
            fill: None,
            bytes: vec![],
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let c = &mut || Ok(());
        for variant in 0..5 {
            let (mut s, t) = start(&input, &stack, &memory);
            s.event(
                &ExecutionEvent::ServiceOutput {
                    address: 0x8000,
                    width: 4,
                    value: if variant == 0 { 0 } else { 1 },
                },
                &t,
                c,
            )
            .unwrap();
            let event = ExecutionEvent::ServiceResult {
                instance: 0,
                transition: FifoTransition::Enqueued {
                    value: if variant == 1 { 43 } else { 42 },
                    woke: variant != 2,
                },
                depth: if variant == 3 { 2 } else { 1 },
                words: [Some(if variant == 4 { 9 } else { 1 }), None],
            };
            assert_eq!(
                s.event(&event, &t, c).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        let (mut s, t) = start(&input, &stack, &memory);
        assert!(!s.goal_valid(&ExecutionStop::ObservedDequeue {
            instance: 0,
            value: 42
        }));
        s.event(
            &ExecutionEvent::ServiceOutput {
                address: 0x8000,
                width: 4,
                value: 1,
            },
            &t,
            c,
        )
        .unwrap();
        s.event(
            &ExecutionEvent::ServiceResult {
                instance: 0,
                transition: FifoTransition::Enqueued {
                    value: 42,
                    woke: true,
                },
                depth: 1,
                words: [Some(1), None],
            },
            &t,
            c,
        )
        .unwrap();
        let mut o = FifoObservation {
            instance: 0,
            id: "queue".into(),
            definition: input.services[0].identity(c).unwrap(),
            lifetime: RegionLifetime::Phase,
            operations: 2,
            depth: 1,
            closed: true,
            issue: None,
            status: ModelStatus::Complete,
        };
        assert!(s.observe(&o, true, false).is_err());
        o.operations = 1;
        s.observe(&o, true, false).unwrap();
        s.finish_side().unwrap();
        assert!(s.closed());
    }
    #[test]
    fn fifo_effects_cannot_claim_normal_ram_as_private_stack() {
        let input = input();
        let stack = MemorySeed {
            address: 0x8000,
            length: 16,
            fill: None,
            bytes: vec![],
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        for (address, width) in [(0x3000, 4), (0x8000, 0), (0x800f, 4)] {
            let (mut s, t) = start(&input, &stack, &memory);
            assert!(
                s.event(
                    &ExecutionEvent::ServiceOutput {
                        address,
                        width,
                        value: 1
                    },
                    &t,
                    &mut || Ok(())
                )
                .is_err()
            );
        }
    }
}
