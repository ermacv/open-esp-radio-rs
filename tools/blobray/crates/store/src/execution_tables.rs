//! Structural lifecycle checks for explicitly reviewed runtime tables.
use super::*;
struct Live<'a> {
    declaration: &'a RuntimeTable,
    definition: ArtifactId,
    values: Vec<Option<u32>>,
    initialized: u32,
    pointers: u32,
    writes: u64,
    calls: u64,
    conditions: bool,
    issue: Option<RuntimeTableIssue>,
    seen: bool,
    closed: bool,
}
pub(super) struct Tables<'a> {
    live: Vec<Option<Live<'a>>>,
    targets: Vec<u32>,
    dirty: bool,
}
impl<'a> Tables<'a> {
    pub fn declaration(&self, id: &str) -> Option<&RuntimeTable> {
        self.live
            .iter()
            .flatten()
            .find(|v| v.declaration.id == id)
            .map(|v| v.declaration)
    }
    pub fn instance_id(&self, instance: u16) -> Option<&str> {
        self.live
            .get(instance as usize)
            .and_then(Option::as_ref)
            .map(|v| v.declaration.id.as_str())
    }
    pub fn new() -> Self {
        Self {
            live: Vec::new(),
            targets: Vec::new(),
            dirty: false,
        }
    }
    pub fn begin(
        &mut self,
        declarations: &'a [RuntimeTable],
        reset: SessionReset,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if reset == SessionReset::Cold && self.live.iter().any(Option::is_some) {
            return Err(integrity("cold reset has unclosed runtime tables"));
        }
        for i in self.live.iter_mut().flatten() {
            i.seen = false;
            if !blocked {
                i.conditions = false;
            }
        }
        if blocked {
            return Ok(());
        }
        self.dirty |= !declarations.is_empty();
        for d in declarations {
            c.checkpoint(self.live.len() as u64 + 1)?;
            if self.live.iter().flatten().any(|i| i.declaration.id == d.id) {
                return Err(integrity("runtime table id already live"));
            }
            let instance = Live {
                declaration: d,
                definition: d.identity(c)?,
                values: vec![None; d.slots.len()],
                initialized: 0,
                pointers: 0,
                writes: 0,
                calls: 0,
                conditions: false,
                issue: None,
                seen: false,
                closed: false,
            };
            if let Some(slot) = self.live.iter().position(Option::is_none) {
                self.live[slot] = Some(instance);
            } else {
                if self.live.len() == MAX_RUNTIME_TABLES {
                    return Err(integrity("too many live runtime tables"));
                }
                self.live.try_reserve(1).map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "runtime validation allocation refused",
                    )
                })?;
                self.live.push(Some(instance));
            }
        }
        Ok(())
    }
    pub fn event(
        &mut self,
        instance: u16,
        event: &RuntimeTableEvent,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        c.checkpoint(1)?;
        if let RuntimeTableEvent::IndirectTarget { target, .. } = event {
            if self.dirty {
                self.targets.clear();
                for i in self.live.iter().flatten() {
                    c.checkpoint(i.values.len() as u64)?;
                    self.targets.try_reserve(i.values.len()).map_err(|_| {
                        Error::new(
                            ErrorCode::ResourceLimited,
                            "runtime target validation allocation refused",
                        )
                    })?;
                    self.targets.extend(i.values.iter().flatten().copied());
                }
                c.checkpoint(
                    self.targets.len() as u64 * (self.targets.len().max(1).ilog2() as u64 + 1),
                )?;
                self.targets.sort_unstable();
                self.dirty = false;
            }
            c.checkpoint(2 * (self.targets.len().max(1).ilog2() as u64 + 1))?;
            let matches = self.targets.partition_point(|v| v <= target)
                - self.targets.partition_point(|v| v < target);
            if *target == 0 || matches != 1 {
                return Err(integrity(
                    "runtime target association is absent or ambiguous",
                ));
            }
        }
        let i = self
            .live
            .get_mut(usize::from(instance))
            .and_then(Option::as_mut)
            .ok_or_else(|| integrity("runtime event has no live instance"))?;
        c.checkpoint(i.declaration.slots.len() as u64 + 1)?;
        match *event {
            RuntimeTableEvent::Initialized { offset, target } => {
                let slot = i
                    .declaration
                    .slots
                    .get(i.initialized as usize)
                    .ok_or_else(|| integrity("extra slot initialization"))?;
                if slot.offset != offset || slot.target.address() != target {
                    return Err(integrity("runtime initialized slot differs"));
                }
                i.values[i.initialized as usize] = Some(target);
                i.initialized += 1;
                self.dirty = true;
            }
            RuntimeTableEvent::PointerInstalled { address, base } => {
                if i.declaration.pointer_cells.get(i.pointers as usize) != Some(&address)
                    || base != i.declaration.seed.address
                {
                    return Err(integrity("runtime pointer installation differs"));
                }
                i.pointers += 1;
            }
            RuntimeTableEvent::ConditionsChecked => {
                if i.initialized as usize != i.declaration.slots.len()
                    || i.pointers as usize != i.declaration.pointer_cells.len()
                {
                    return Err(integrity(
                        "runtime conditions checked before initialization",
                    ));
                }
                i.conditions = true;
            }
            RuntimeTableEvent::Written {
                offset,
                width,
                value,
                ..
            } => {
                if !matches!(width, 1 | 2 | 4)
                    || !offset.is_multiple_of(u32::from(width))
                    || offset
                        .checked_add(u32::from(width))
                        .is_none_or(|end| end > i.declaration.seed.length)
                    || (width < 4 && value >> (width * 8) != 0)
                {
                    return Err(integrity("runtime write geometry differs"));
                }
                for (slot, stored) in i.declaration.slots.iter().zip(&mut i.values) {
                    if offset < slot.offset + 4 && slot.offset < offset + u32::from(width) {
                        let old = stored.ok_or_else(|| {
                            integrity("runtime write precedes slot initialization")
                        })?;
                        let mut bytes = old.to_le_bytes();
                        let start = (offset - slot.offset) as usize;
                        bytes[start..start + usize::from(width)]
                            .copy_from_slice(&value.to_le_bytes()[..usize::from(width)]);
                        *stored = Some(u32::from_le_bytes(bytes));
                        self.dirty = true;
                    }
                }
                i.writes = i
                    .writes
                    .checked_add(1)
                    .ok_or_else(|| integrity("runtime write count overflow"))?;
            }
            RuntimeTableEvent::IndirectTarget { target, offset, .. } => {
                let slot = i
                    .declaration
                    .slots
                    .iter()
                    .position(|s| s.offset == offset)
                    .ok_or_else(|| integrity("runtime association has unknown slot"))?;
                if !i.conditions || i.values[slot] != Some(target) {
                    return Err(integrity(
                        "runtime association lacks conditions/current target",
                    ));
                }
                i.calls = i
                    .calls
                    .checked_add(1)
                    .ok_or_else(|| integrity("runtime call count overflow"))?;
            }
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        o: &RuntimeTableObservation,
        close: bool,
        blocked: bool,
    ) -> Result<()> {
        let i = self
            .live
            .get_mut(usize::from(o.instance))
            .and_then(Option::as_mut)
            .ok_or_else(|| integrity("runtime observation has no live instance"))?;
        let d = i.declaration;
        if i.seen
            || o.id != d.id
            || o.review != d.review
            || o.definition != i.definition
            || o.lifetime != d.lifetime
            || o.base != d.seed.address
            || o.length != d.seed.length
            || o.expected_slots as usize != d.slots.len()
            || o.expected_pointers as usize != d.pointer_cells.len()
            || o.initialized != i.initialized
            || o.pointer_installs != i.pointers
            || o.writes != i.writes
            || o.calls != i.calls
            || o.conditions_checked != i.conditions
            || o.closed != (close || d.lifetime == RegionLifetime::Phase)
            || o.status != o.expected_status()
            || (i.issue.is_some() && o.issue != i.issue)
            || (blocked && o.issue != i.issue)
        {
            return Err(integrity("runtime identity, lifecycle or closure differs"));
        }
        i.seen = true;
        i.closed = o.closed;
        i.issue = o.issue;
        Ok(())
    }
    pub fn finish_side(&mut self) -> Result<()> {
        if self.live.iter().flatten().any(|i| !i.seen) {
            return Err(integrity("missing runtime table observation"));
        }
        for i in &mut self.live {
            if i.as_ref().is_some_and(|i| i.closed) {
                *i = None;
                self.dirty = true;
            }
        }
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.live.iter().all(Option::is_none)
    }
}

impl Project {
    pub(super) fn validate_execution_interfaces(
        &self,
        request: &ExecutionRequest,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let mut selected = AdmittedVec::new(memory);
        for case in &request.cases {
            for (input, target) in std::iter::once((&case.vendor, &request.vendor))
                .chain(case.replacement.as_ref().zip(request.replacement.as_ref()))
            {
                for table in &input.tables {
                    c.checkpoint(1)?;
                    selected.push((table, target), c.position())?;
                }
            }
        }
        for (index, (table, _)) in selected.iter().enumerate() {
            c.checkpoint(index as u64 + 1)?;
            if selected[..index]
                .iter()
                .any(|(t, _)| t.review.knowledge == table.review.knowledge)
            {
                continue;
            }
            let snapshot = self.knowledge_snapshot(Some(&table.review.knowledge), memory, c)?;
            for (t, target) in &*selected {
                c.checkpoint(1)?;
                if t.review.knowledge != table.review.knowledge {
                    continue;
                }
                let entry = snapshot
                    .get(&t.review.assertion, c)?
                    .ok_or_else(|| integrity("execution interface assertion missing"))?;
                let KnowledgeClaim::Interface { contract } = &entry.proposal.claim else {
                    return Err(integrity("execution interface review has another claim"));
                };
                for slot in &t.slots {
                    c.checkpoint(contract.slots.len() as u64 + 1)?;
                    if matches!(
                        slot.target,
                        RuntimeSlotTarget::Model { .. } | RuntimeSlotTarget::Service { .. }
                    ) && !contract.slots.iter().any(|s| {
                        s.offset == slot.offset && s.semantic.is_some() && s.signature.is_some()
                    }) {
                        return Err(integrity(
                            "modeled table slot lacks reviewed semantic/signature",
                        ));
                    }
                }
                let occurrence = &entry.proposal.occurrence;
                c.checkpoint((contract.slots.len() * t.slots.len()) as u64)?;
                if entry.state != AssertionState::Accepted
                    || contract.layout_bytes != t.seed.length
                    || contract.slots.len() != t.slots.len()
                    || !contract
                        .slots
                        .iter()
                        .all(|s| t.slots.iter().any(|v| v.offset == s.offset))
                    || contract.pointer_bytes != 4
                    || contract.abi != target.abi
                    || occurrence.revision != target.revision
                    || occurrence.object.location != ObjectLocation::Standalone
                    || (occurrence.source != target.source
                        && !matches!(occurrence.source,FunctionSource::Input{input} if target.companions.contains(&input)))
                {
                    return Err(integrity(
                        "execution interface differs from selected accepted review/source",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn declaration() -> RuntimeTable {
        RuntimeTable {
            id: "callbacks".into(),
            review: InterfaceReview {
                knowledge: "1".repeat(64).parse().unwrap(),
                assertion: "2".repeat(64).parse().unwrap(),
            },
            lifetime: RegionLifetime::Phase,
            seed: MemorySeed {
                address: 0x3000,
                length: 16,
                fill: None,
                bytes: vec![],
            },
            slots: vec![RuntimeSlot {
                offset: 4,
                target: RuntimeSlotTarget::Code { address: 0x1000 },
            }],
            pointer_cells: vec![0x4000],
        }
    }
    fn initialize<'a>(d: &'a RuntimeTable) -> Tables<'a> {
        let mut t = Tables::new();
        let c = &mut || Ok(());
        t.begin(std::slice::from_ref(d), SessionReset::Cold, false, c)
            .unwrap();
        t.event(
            0,
            &RuntimeTableEvent::Initialized {
                offset: 4,
                target: 0x1000,
            },
            c,
        )
        .unwrap();
        t.event(
            0,
            &RuntimeTableEvent::PointerInstalled {
                address: 0x4000,
                base: 0x3000,
            },
            c,
        )
        .unwrap();
        t.event(0, &RuntimeTableEvent::ConditionsChecked, c)
            .unwrap();
        t
    }
    fn observation(d: &RuntimeTable) -> RuntimeTableObservation {
        RuntimeTableObservation {
            instance: 0,
            id: d.id.clone(),
            review: d.review.clone(),
            definition: d.identity(&mut || Ok(())).unwrap(),
            lifetime: d.lifetime,
            base: d.seed.address,
            length: d.seed.length,
            initialized: 1,
            expected_slots: 1,
            expected_pointers: 1,
            conditions_checked: true,
            pointer_installs: 1,
            writes: 0,
            calls: 0,
            closed: true,
            issue: None,
            status: ModelStatus::Complete,
        }
    }
    #[test]
    fn rejects_forged_identity_counts_conditions_and_closure() {
        let d = declaration();
        for variant in 0..8 {
            let mut t = initialize(&d);
            let mut o = observation(&d);
            match variant {
                0 => o.review.assertion = "3".repeat(64).parse().unwrap(),
                1 => o.calls = 1,
                2 => o.initialized = 0,
                3 => o.conditions_checked = false,
                4 => o.closed = false,
                5 => o.expected_slots = 0,
                6 => o.pointer_installs = 0,
                _ => o.definition = ArtifactId::of_bytes(b"forged"),
            }
            assert_eq!(
                t.observe(&o, true, false).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        let mut t = initialize(&d);
        assert!(t.finish_side().is_err());
        t.observe(&observation(&d), true, false).unwrap();
        t.finish_side().unwrap();
        assert!(t.closed());
        // Closed instance ids can be reused without growing a history-sized index.
        t.begin(
            std::slice::from_ref(&d),
            SessionReset::Warm,
            false,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(t.live.len(), 1);
    }
    #[test]
    fn current_target_index_rejects_old_values_and_aliases() {
        let d = declaration();
        let c = &mut || Ok(());
        let mut t = initialize(&d);
        let call = |target| RuntimeTableEvent::IndirectTarget {
            site: 0x800,
            target,
            offset: 4,
        };
        t.event(0, &call(0x1000), c).unwrap();
        t.event(
            0,
            &RuntimeTableEvent::Written {
                site: Some(0x804),
                offset: 4,
                width: 1,
                value: 4,
            },
            c,
        )
        .unwrap();
        assert!(t.event(0, &call(0x1000), c).is_err());
        t.event(0, &call(0x1004), c).unwrap();
        let mut alias = d.clone();
        alias.id = "alias".into();
        alias.seed.address = 0x5000;
        alias.slots[0].target = RuntimeSlotTarget::Code { address: 0x1004 };
        t.begin(std::slice::from_ref(&alias), SessionReset::Warm, false, c)
            .unwrap();
        t.event(0, &RuntimeTableEvent::ConditionsChecked, c)
            .unwrap();
        t.event(
            1,
            &RuntimeTableEvent::Initialized {
                offset: 4,
                target: 0x1004,
            },
            c,
        )
        .unwrap();
        assert!(t.event(0, &call(0x1004), c).is_err());
    }
    #[test]
    fn setup_cannot_skip_or_reorder_physical_initialization() {
        let d = declaration();
        let c = &mut || Ok(());
        let mut t = Tables::new();
        t.begin(std::slice::from_ref(&d), SessionReset::Cold, false, c)
            .unwrap();
        assert!(
            t.event(0, &RuntimeTableEvent::ConditionsChecked, c)
                .is_err()
        );
        assert!(
            t.event(
                0,
                &RuntimeTableEvent::Initialized {
                    offset: 8,
                    target: 0x1000
                },
                c
            )
            .is_err()
        );
        assert!(
            t.event(
                0,
                &RuntimeTableEvent::Written {
                    site: None,
                    offset: 4,
                    width: 4,
                    value: 0x1004
                },
                c
            )
            .is_err()
        );
        assert!(t.observe(&observation(&d), true, false).is_err());
    }
}
