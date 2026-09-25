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
                    selected.push((table, target, selected.len()), c.position())?;
                }
            }
        }
        c.checkpoint(selected.len() as u64 * (selected.len().max(1).ilog2() as u64 + 1))?;
        // The ordinal preserves declaration order within each frozen review.
        selected.sort_unstable_by(|(a, _, i), (b, _, j)| {
            (&a.review.knowledge, i).cmp(&(&b.review.knowledge, j))
        });
        c.checkpoint(selected.len() as u64)?;
        for group in
            selected.chunk_by(|(a, _, _), (b, _, _)| a.review.knowledge == b.review.knowledge)
        {
            let snapshot =
                self.knowledge_snapshot(Some(&group[0].0.review.knowledge), memory, c)?;
            for (t, target, _) in group {
                c.checkpoint(1)?;
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
    struct Count {
        work: u64,
        histories: u64,
        stop: Option<u64>,
    }
    impl RunControl for Count {
        fn checkpoint(&mut self, work: u64) -> Result<()> {
            self.work += work;
            if self.stop.is_some_and(|stop| self.work >= stop) {
                return Err(Error::new(ErrorCode::Cancelled, "test cancellation"));
            }
            Ok(())
        }
        fn measure(&mut self, metric: WorkMetric, amount: u64) {
            if matches!(metric, WorkMetric::KnowledgeHistoryPasses) {
                self.histories += amount;
            }
        }
    }
    fn counter() -> Count {
        Count {
            work: 0,
            histories: 0,
            stop: None,
        }
    }

    fn reviewed(
        project: &Project,
    ) -> (
        RuntimeTable,
        ExecutionTarget,
        KnowledgeRevisionId,
        KnowledgeRevisionId,
    ) {
        let mut writer = project.writer().unwrap();
        let payload = ArtifactId::of_bytes(b"source");
        let mut table = declaration();
        let target = ExecutionTarget {
            revision: payload.as_str().parse().unwrap(),
            source: FunctionSource::Input { input: 0 },
            companions: vec![],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        };
        let proposal = KnowledgeProposal {
            subject: "fixture.interface".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: target.revision.clone(),
                source: target.source.clone(),
                object: ObjectId {
                    artifact: payload,
                    location: ObjectLocation::Standalone,
                },
                symbol: None,
            },
            claim: KnowledgeClaim::Interface {
                contract: Box::new(InterfaceContract {
                    root: AccessRoot::Address { address: 0x3000 },
                    path: vec![],
                    layout_version: "fixture/1".into(),
                    layout_bytes: 16,
                    pointer_bytes: 4,
                    abi: CallAbi::RiscvInteger,
                    index_domains: vec![],
                    guards: vec![],
                    slots: vec![InterfaceSlot {
                        offset: 4,
                        name: "callback".into(),
                        semantic: None,
                        signature: None,
                    }],
                    purpose: "fixture".into(),
                    applicability: "fixture".into(),
                }),
            },
            evidence: vec![],
            note: None,
        };
        let mut base = None;
        let mut first_accepted = None;
        for action in [
            KnowledgeAction::Propose { proposal },
            KnowledgeAction::Review {
                assertion: table.review.assertion.clone(),
                decision: ReviewDecision::Accept,
                supersedes: None,
            },
            KnowledgeAction::Review {
                assertion: table.review.assertion.clone(),
                decision: ReviewDecision::Accept,
                supersedes: None,
            },
            KnowledgeAction::Review {
                assertion: table.review.assertion.clone(),
                decision: ReviewDecision::Reject,
                supersedes: None,
            },
        ] {
            let accepted = matches!(
                action,
                KnowledgeAction::Review {
                    decision: ReviewDecision::Accept,
                    ..
                }
            );
            let change = KnowledgeChange {
                expected_base: base,
                actor: "fixture".into(),
                reason: "fixture".into(),
                action,
            };
            let (mut run, path) = writer
                .register_operation(
                    ResourceBudget::default(),
                    OwnerIdentity {
                        pid: 123,
                        start_ticks: 42,
                        boot_id: "test".into(),
                    },
                    RunOperation::Knowledge {
                        change: change.clone(),
                    },
                    |_| {},
                )
                .unwrap();
            let stage = Staging::open(&path).unwrap();
            let receipt = stage
                .knowledge_receipt(
                    &KnowledgeManifest {
                        schema: 2,
                        project: project.id().clone(),
                        change,
                        assertion: table.review.assertion.clone(),
                        evidence_roots: vec![],
                    },
                    &mut || Ok(()),
                )
                .unwrap();
            run.state = RunState::Running;
            writer.update_run(&run).unwrap();
            run.state = RunState::Validating;
            writer.update_run(&run).unwrap();
            let retained = writer
                .retain_knowledge(&run, &receipt, &mut || Ok(()))
                .unwrap();
            writer
                .publish_knowledge(&mut run, retained, &mut || Ok(()))
                .unwrap();
            if accepted {
                first_accepted.get_or_insert_with(|| receipt.revision.clone());
                table.review.knowledge = receipt.revision.clone();
            }
            base = Some(receipt.revision);
        }
        (table, target, first_accepted.unwrap(), base.unwrap())
    }
    fn interface_request(
        table: RuntimeTable,
        target: ExecutionTarget,
        count: usize,
    ) -> ExecutionRequest {
        let invocation = Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
            goal: ExecutionGoal::Return,
            entry: 0x1000,
            arguments: vec![],
            memory: vec![],
            models: vec![],
            calls: vec![],
            tables: vec![table],
            services: vec![],
        };
        ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target,
            replacement: None,
            binding: None,
            cases: (0..count)
                .map(|n| ExecutionCase {
                    relation: None,
                    stack_fill: None,
                    reset: if n == 0 {
                        SessionReset::Cold
                    } else {
                        SessionReset::Warm
                    },
                    name: format!("phase-{n}"),
                    vendor: invocation.clone(),
                    replacement: None,
                })
                .collect(),
            max_events: 16,
        }
    }
    #[test]
    fn retained_interfaces_scale_without_repeated_snapshot_scans() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path()).unwrap();
        let (table, target, _, _) = reviewed(&project);
        let memory = WorkingMemory::new(2 * 1024 * 1024).unwrap();
        let mut history = counter();
        drop(
            project
                .knowledge_snapshot(Some(&table.review.knowledge), &memory, &mut history)
                .unwrap(),
        );
        let mut previous = None;
        for count in [16, 32, 64] {
            let request = interface_request(table.clone(), target.clone(), count);
            let mut c = counter();
            project
                .validate_execution_interfaces(&request, &memory, &mut c)
                .unwrap();
            assert_eq!(c.histories, 1);
            assert_eq!(memory.used(), 0);
            // Remove the fixed history verification cost, so it cannot mask quadratic grouping.
            let work = c.work - history.work;
            if let Some(previous) = previous {
                assert!(work < previous * 3, "{count}: {work}/{previous}");
            }
            eprintln!("retained interfaces={count}, validation work excluding history={work}");
            previous = Some(work);
        }
    }
    #[test]
    fn retained_interfaces_check_late_declarations_frozen_reviews_and_release_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::create(dir.path()).unwrap();
        let (table, target, first, rejected) = reviewed(&project);
        let memory = WorkingMemory::new(2 * 1024 * 1024).unwrap();
        let request = interface_request(table, target, 16);
        let mut complete = counter();
        project
            .validate_execution_interfaces(&request, &memory, &mut complete)
            .unwrap();
        for variant in 0..4 {
            let mut bad = request.clone();
            let late = &mut bad.cases.last_mut().unwrap().vendor.tables[0];
            match variant {
                0 => late.seed.length += 4,
                1 => late.review.knowledge = rejected.clone(),
                2 => late.review.assertion = "3".repeat(64).parse().unwrap(),
                _ => bad.vendor.source = FunctionSource::Input { input: 1 },
            }
            assert_eq!(
                project
                    .validate_execution_interfaces(&bad, &memory, &mut counter())
                    .unwrap_err()
                    .code,
                ErrorCode::Integrity
            );
            assert_eq!(memory.used(), 0);
        }
        for stop in [1, 32, complete.work - 1] {
            let mut c = counter();
            c.stop = Some(stop);
            assert_eq!(
                project
                    .validate_execution_interfaces(&request, &memory, &mut c)
                    .unwrap_err()
                    .code,
                ErrorCode::Cancelled
            );
            assert_eq!(memory.used(), 0);
        }
        // Both sides and interleaved frozen snapshots remain independently checked,
        // even though the latest review rejected the assertion.
        let mut mixed = request.clone();
        mixed.replacement = Some(mixed.vendor.clone());
        for (n, case) in mixed.cases.iter_mut().enumerate() {
            case.replacement = Some(case.vendor.clone());
            if n % 2 == 0 {
                case.vendor.tables[0].review.knowledge = first.clone();
            } else {
                case.replacement.as_mut().unwrap().tables[0]
                    .review
                    .knowledge = first.clone();
            }
        }
        let mut c = counter();
        project
            .validate_execution_interfaces(&mixed, &memory, &mut c)
            .unwrap();
        assert_eq!(c.histories, 2);
        assert_eq!(memory.used(), 0);
        mixed.replacement.as_mut().unwrap().revision = "9".repeat(64).parse().unwrap();
        assert_eq!(
            project
                .validate_execution_interfaces(&mixed, &memory, &mut counter())
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(memory.used(), 0);
        let small = WorkingMemory::new(1024).unwrap();
        assert_eq!(
            project
                .validate_execution_interfaces(&request, &small, &mut counter())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(small.used(), 0);
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
