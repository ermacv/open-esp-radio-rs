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
