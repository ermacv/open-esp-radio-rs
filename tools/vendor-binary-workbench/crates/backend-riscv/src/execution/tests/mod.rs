//! Concrete execution regression tests and shared synthetic fixtures.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use rv_asm::AmoOp;

use super::image::UnresolvedRelocation;
use super::*;
use crate::MmioMap;

fn tiny_image(bytes: Vec<u8>, memory_size: u32) -> ExecutableImage {
    ExecutableImage {
        segments: vec![Segment {
            address: 0x1000,
            bytes,
            memory_size,
            writable: true,
        }],
        symbols_by_name: HashMap::from([("test".to_owned(), 0x1000)]),
        symbols_by_address: BTreeMap::from([(0x1000, "test".to_owned())]),
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::new(),
        unresolved_relocations_by_address: BTreeMap::new(),
        global_pointer: None,
    }
}

fn empty_svd() -> MmioMap {
    MmioMap {
        registers: Vec::new(),
        regions: Vec::new(),
    }
}

fn tail_relocation_image(target: Option<u32>) -> ExecutableImage {
    let mut symbols_by_name = HashMap::from([("wrapper".to_owned(), 0x1000)]);
    let mut symbols_by_address = BTreeMap::from([(0x1000, "wrapper".to_owned())]);
    let mut segments = vec![Segment {
        address: 0x1000,
        bytes: vec![
            0x17, 0x03, 0x00, 0x00, // auipc t1, 0
            0x67, 0x00, 0x03, 0x00, // jalr zero, 0(t1)
            0x63, 0x00, 0x00, 0x00, // beq zero, zero, 0 (must be unreachable)
        ],
        memory_size: 12,
        writable: true,
    }];
    if let Some(target) = target {
        symbols_by_name.insert("callee".to_owned(), target);
        symbols_by_address.insert(target, "callee".to_owned());
        segments.push(Segment {
            address: target,
            bytes: vec![0x67, 0x80, 0x00, 0x00], // ret
            memory_size: 4,
            writable: true,
        });
    }
    ExecutableImage {
        segments,
        symbols_by_name,
        symbols_by_address,
        symbol_sizes_by_address: BTreeMap::new(),
        local_text_symbols: BTreeSet::new(),
        call_trampoline_addresses: BTreeSet::new(),
        relocated_calls_by_address: BTreeMap::from([(
            0x1000,
            RelocatedCall {
                name: "callee".to_owned(),
                target: None,
            },
        )]),
        unresolved_relocations_by_address: BTreeMap::new(),
        global_pointer: None,
    }
}

mod calls;
mod devices;
mod image_and_control;
mod memory;
mod session;
