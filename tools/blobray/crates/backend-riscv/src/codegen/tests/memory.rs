//! Proven memory-transfer compaction and escape analysis.

use super::*;

fn unrolled_word_to_bytes_events(
    source: u32,
    destination: SymbolicValue,
    first_read_token: u32,
) -> Vec<DraftReferenceEvent> {
    let mut events = Vec::new();
    for byte in 0..4_u32 {
        let read_token = first_read_token + byte;
        events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::Constant(source),
            region: ".data".to_owned(),
            value: None,
        });
        events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            address: destination.clone().add_constant(byte),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: Some(
                SymbolicValue::memory_read(read_token, 32, false)
                    .shift_right(byte * 8)
                    .and(0xff),
            ),
        });
    }
    events
}

fn little_endian_loader_flow() -> DraftReferenceFlow {
    let events = [1_u32, 0, 2, 3]
        .into_iter()
        .map(|offset| DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address: SymbolicValue::input(0).add_constant(offset),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: None,
        })
        .collect();
    let value = SymbolicValue::memory_read(1, 8, false)
        .symbolic_bitor(SymbolicValue::memory_read(0, 8, false).shift_left(8))
        .symbolic_bitor(SymbolicValue::memory_read(2, 8, false).shift_left(16))
        .symbolic_bitor(SymbolicValue::memory_read(3, 8, false).shift_left(24));
    DraftReferenceFlow {
        events,
        terminator: DraftReferenceTerminator::Return(value),
    }
}

pub(super) fn bytes_to_word_events(
    source: SymbolicValue,
    destination: u32,
    token: u32,
) -> Vec<DraftReferenceEvent> {
    vec![
        DraftReferenceEvent::ComposedCall {
            token,
            site: 0x1000 + token * 4,
            symbol: "phy_byte_to_word".to_owned(),
            direct: true,
            tail: false,
            arguments: vec![source].into_boxed_slice(),
            flow: Box::new(little_endian_loader_flow()),
            result_modeled: true,
        },
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: SymbolicValue::Constant(destination),
            region: ".data".to_owned(),
            value: Some(SymbolicValue::CallResult(token)),
        },
    ]
}

#[test]
fn compacts_complete_word_to_bytes_groups_without_changing_the_access_shape() {
    let destination = SymbolicValue::input(0).add_constant(12);
    let mut reference_events = unrolled_word_to_bytes_events(0x1000_8000, destination.clone(), 0);
    reference_events.extend(unrolled_word_to_bytes_events(
        0x1000_8004,
        destination.add_constant(4),
        4,
    ));
    let trace = FunctionAnalysis {
        symbol: "word_to_bytes".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events,
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Linear { events, .. } = program.body else {
        panic!("linear trace resolved to a flow body");
    };
    assert!(matches!(
        events.as_slice(),
        [ResolvedReferenceEvent::WordToBytesMemoryLoop {
            source_region,
            destination_region,
            length: 8,
            ..
        }] if source_region == ".data"
            && destination_region == "caller-owned ABI argument RAM"
    ));
}

#[test]
fn does_not_compact_a_memory_read_token_that_escapes_the_loop() {
    let reference_events = unrolled_word_to_bytes_events(0x1000_8000, SymbolicValue::input(0), 0);
    let trace = FunctionAnalysis {
        symbol: "escaping_word".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events,
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::MemoryImage {
            read_token: 3,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Linear {
        events,
        return_value,
    } = program.body
    else {
        panic!("linear trace resolved to a flow body");
    };
    assert_eq!(events.len(), 8);
    assert!(
        events
            .iter()
            .all(|event| matches!(event, ResolvedReferenceEvent::Memory { .. }))
    );
    assert_eq!(return_value, trace.return_value);
}

#[test]
fn compacts_complete_little_endian_loader_groups() {
    let source = SymbolicValue::input(0).add_constant(12);
    let mut reference_events = bytes_to_word_events(source.clone(), 0x1000_8000, 0);
    reference_events.extend(bytes_to_word_events(source.add_constant(4), 0x1000_8004, 1));
    let trace = FunctionAnalysis {
        symbol: "bytes_to_word".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events,
        reference_dependencies: vec!["phy_byte_to_word".to_owned(); 2],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    assert_eq!(program.dependencies.len(), 1);
    let ResolvedReferenceBody::Linear { events, .. } = program.body else {
        panic!("linear trace resolved to a flow body");
    };
    assert!(matches!(
        events.as_slice(),
        [ResolvedReferenceEvent::BytesToWordMemoryLoop {
            source_region,
            destination_region,
            length: 8,
            ..
        }] if source_region == "caller-owned ABI argument RAM"
            && destination_region == ".data"
    ));
}
