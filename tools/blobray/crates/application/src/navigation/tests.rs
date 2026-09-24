use super::*;
fn node<'a>(memory: &'a WorkingMemory, name: &[u8], start: u64) -> Node<'a> {
    let object = ObjectId {
        artifact: ArtifactId::of_bytes(b"one image"),
        location: ObjectLocation::Standalone,
    };
    Node {
        function: NavigationFunction {
            analysis: ArtifactId::of_bytes(name).as_str().parse().unwrap(),
            location: FunctionLocation {
                source: FunctionSource::Input { input: 0 },
                selector: FunctionSelector::Range {
                    object,
                    section: 1,
                    extent: CodeRange { start, length: 4 },
                },
            },
        },
        extent: CodeRange { start, length: 4 },
        section: 1,
        space: CodeAddressSpace::Image,
        user_extent: true,
        _capacity: memory.reserve(256, RunPosition::default()).unwrap(),
    }
}
#[test]
fn cyclic_edges_and_partly_selected_alternatives_keep_exact_evidence() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let mut nodes = vec![
        node(&memory, b"a", 0x1000),
        node(&memory, b"b", 0x2000),
        node(&memory, b"b-other-analysis", 0x2000),
    ];
    nodes.sort_by(|a, b| a.function.analysis.cmp(&b.function.analysis));
    let a = nodes.iter().position(|n| n.extent.start == 0x1000).unwrap();
    let b = nodes.iter().position(|n| n.extent.start == 0x2000).unwrap();
    let mut pending = Vec::new();
    for (caller, record, target) in [
        (a, 11, AbstractValue::ImageAddress { address: 0x2000 }),
        (b, 22, AbstractValue::ImageAddress { address: 0x1000 }),
        (
            a,
            33,
            AbstractValue::Alternatives {
                values: ValueAlternatives::new(vec![
                    ValueAlternative::ImageAddress { address: 0x1000 },
                    ValueAlternative::ImageAddress { address: 0x3000 },
                ])
                .unwrap(),
            },
        ),
        (a, 44, AbstractValue::Unknown),
    ] {
        pending.push(Pending {
            caller,
            call: CallObservation {
                record,
                offset: nodes[caller].extent.start,
                call: true,
                target,
                saved_resolution: None,
            },
            _capacity: memory.reserve(256, RunPosition::default()).unwrap(),
        });
    }
    let mut output = Vec::new();
    calls::emit(
        &nodes,
        &pending,
        None,
        CallDirection::Callees,
        &memory,
        &mut || Ok(()),
        &mut |r, _| {
            output.push(r.clone());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(output.len(), 4); // Cycles are reported, never recursively traversed.
    for (row, count, issue) in [
        (0, 2, Some(NavigationIssue::AmbiguousTarget)),
        (1, 1, None),
        (2, 1, Some(NavigationIssue::AmbiguousTarget)),
        (3, 0, Some(NavigationIssue::UnresolvedTarget)),
    ] {
        let NavigationRecord::Call {
            candidates,
            issue: actual,
            record,
            ..
        } = &output[row]
        else {
            panic!()
        };
        assert_eq!(candidates.len(), count);
        assert_eq!(*actual, issue);
        assert_eq!(*record, (row as u64 + 1) * 11);
    }
    output.clear();
    calls::emit(
        &nodes,
        &pending,
        Some(&nodes[b].function.location),
        CallDirection::Callers,
        &memory,
        &mut || Ok(()),
        &mut |r, _| {
            output.push(r.clone());
            Ok(())
        },
    )
    .unwrap();
    assert!(output.iter().any(|r| matches!(
        r,
        NavigationRecord::Call {
            record: 44,
            focus_match: Some(false),
            ..
        }
    )));
    assert!(
        !output
            .iter()
            .any(|r| matches!(r, NavigationRecord::Call { record: 22, .. }))
    );
}
#[test]
fn composed_unqualified_addresses_never_inherit_the_callers_object() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let node = node(&memory, b"caller", 0x1000);
    let object = node.function.location.selector.object().clone();
    let occurrence = KnowledgeOccurrence {
        revision: ArtifactId::of_bytes(b"revision").as_str().parse().unwrap(),
        source: node.function.location.source.clone(),
        object: object.clone(),
        symbol: None,
    };
    let selector = DataSelector::Section {
        section: 2,
        offset: 0,
        length: 4,
    };
    let target = accesses::Target::Object {
        occurrence: &occurrence,
        location: DataLocation {
            selector,
            section: 2,
            section_range: CodeRange {
                start: 0,
                length: 4,
            },
            file_range: None,
            image_address: Some(0x2000),
            section_writable: true,
        },
        access: None,
    };
    let recipe = FunctionRecipe {
        schema: FUNCTION_SCHEMA,
        policy: FUNCTION_POLICY,
        project: ArtifactId::of_bytes(b"project").as_str().parse().unwrap(),
        revision: occurrence.revision.clone(),
        source: occurrence.source.clone(),
        selector: node.function.location.selector.clone(),
        payload: object.artifact.clone(),
        section: 1,
        extent: node.extent,
        user_extent: true,
        research: None,
        abi: RiscvAbi::Ilp32,
        address_space: CodeAddressSpace::Image,
        decoder: "test".into(),
        semantics: Some("test".into()),
    };
    let records: Vec<_> = [
        AbstractValue::Constant { value: 0x2000 },
        AbstractValue::ScopedAddress {
            source: occurrence.source.clone(),
            object,
            address: 0x2000,
        },
    ]
    .into_iter()
    .map(|address| FunctionRecord::CalleeEffect {
        callsite: 0x1000,
        analysis: ArtifactId::of_bytes(b"callee").as_str().parse().unwrap(),
        offset: 0x3000,
        access: MemoryKind::Store,
        width: 4,
        address,
        value: None,
    })
    .collect();
    let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
    let mut out = Vec::new();
    target
        .visit(
            &node,
            &recipe,
            &facts,
            &memory,
            &mut || Ok(()),
            &mut |r, _| {
                out.push(r.clone());
                Ok(())
            },
        )
        .unwrap();
    assert!(
        matches!(&out[0],NavigationRecord::Access {matches,issue:Some(NavigationIssue::ForeignOccurrence),..} if matches.is_empty())
    );
    assert!(matches!(&out[1],NavigationRecord::Access {matches,issue:None,..} if matches.len()==1));
}
