//! Tests for function-level trace classification.

use super::*;

use super::*;

#[test]
fn indexed_mmio_is_not_an_empty_exact_observable_trace() {
    let analysis = FunctionAnalysis {
        symbol: "indexed_queue_read".to_owned(),
        events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::input(0),
            registers: vec![
                IndexedMmioRegister {
                    address: 0x2010_4d40,
                    name: "WIFI_MAC_TX_QUEUE_CONTROL.CONTROL0".to_owned(),
                },
                IndexedMmioRegister {
                    address: 0x2010_4d50,
                    name: "WIFI_MAC_TX_QUEUE_CONTROL.CONTROL1".to_owned(),
                },
            ],
            guard: Some(IndexedMmioGuard {
                selector: SymbolicValue::input(0),
                maximum: 1,
            }),
            value: None,
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::indexed_register_read(0, 32, false),
        reference_flow: None,
        unresolved_branch: None,
    };

    assert!(analysis.is_reference_eligible());
    assert!(!analysis.is_exact());
}
