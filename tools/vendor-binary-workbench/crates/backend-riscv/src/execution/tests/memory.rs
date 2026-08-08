//! Fail-closed memory, MMIO, atomic, stack and observation regressions.

use super::*;
use proptest::prelude::*;

#[test]
fn poison_memory_and_unseeded_mmio_fail_closed() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let empty_svd = empty_svd();
    let mut machine = Machine::new(&image, &empty_svd, 0x1000, Scenario::default());
    assert!(
        machine
            .read(0x4000_0000, 32)
            .unwrap_err()
            .to_string()
            .contains("poison/unmapped")
    );

    let mmio_svd = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "radio".to_owned(),
            start: 0x2010_0000,
            end: 0x2020_0000,
            readable: true,
            writable: true,
        }],
    };
    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
    assert!(
        machine
            .read(0x2010_0010, 32)
            .unwrap_err()
            .to_string()
            .contains("no explicit seed or response")
    );
}

#[test]
fn physical_mmio_without_a_register_name_is_an_observable_event() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let map = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "radio".to_owned(),
            start: 0x2010_0000,
            end: 0x2010_0100,
            readable: true,
            writable: true,
        }],
    };
    let address = 0x2010_0010;
    let mut scenario = Scenario::default();
    scenario.mmio_initial.insert(address, 0x1234_5678);
    let mut machine = Machine::new(&image, &map, 0x1000, scenario);

    assert_eq!(machine.read(address, 32).unwrap(), 0x1234_5678);
    assert_eq!(
        machine.events,
        [ExecutionEvent::Read {
            width: 32,
            address,
            region: "radio".to_owned(),
            register: None,
            value: 0x1234_5678,
        }]
    );
}

#[test]
fn mmio_accesses_fail_closed_at_region_and_permission_boundaries() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let read_only = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "status".to_owned(),
            start: 0x2010_0000,
            end: 0x2010_0004,
            readable: true,
            writable: false,
        }],
    };
    let mut machine = Machine::new(&image, &read_only, 0x1000, Scenario::default());
    assert!(
        machine
            .write(0x2010_0000, 32, 1)
            .unwrap_err()
            .to_string()
            .contains("not permitted")
    );
    assert!(
        machine
            .read(0x2010_0002, 32)
            .unwrap_err()
            .to_string()
            .contains("crosses the boundary")
    );
    assert!(
        machine
            .read(0x200f_fffe, 32)
            .unwrap_err()
            .to_string()
            .contains("crosses the boundary")
    );
}

#[test]
fn unreachable_unresolved_relocation_does_not_block_an_independent_function() {
    let mut image = tiny_image(
        vec![
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // unrelated relocated data
        ],
        8,
    );
    image.unresolved_relocations_by_address.insert(
        0x1004,
        UnresolvedRelocation {
            name: "unrelated_global".to_owned(),
            r_type: object::elf::R_RISCV_32,
            width: 4,
        },
    );

    assert!(image.coverage_inventory("test").is_ok());
    assert!(execute(&image, &empty_svd(), "test", Scenario::default()).is_ok());
    assert_eq!(image.loaded_byte(0x1004), None);
}

#[test]
fn instruction_fetch_through_unresolved_relocation_fails_closed() {
    let mut image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    image.unresolved_relocations_by_address.insert(
        0x1000,
        UnresolvedRelocation {
            name: "instruction_target".to_owned(),
            r_type: object::elf::R_RISCV_HI20,
            width: 4,
        },
    );

    let inventory_error = image.coverage_inventory("test").unwrap_err().to_string();
    assert!(inventory_error.contains("instruction fetch reached unresolved ELF relocation"));
    assert!(inventory_error.contains("instruction_target"));
    let execution_error = execute(&image, &empty_svd(), "test", Scenario::default())
        .unwrap_err()
        .to_string();
    assert!(execution_error.contains("instruction fetch reached unresolved ELF relocation"));
}

#[test]
fn ram_read_through_unresolved_relocated_word_requires_an_explicit_seed() {
    let mut image = tiny_image(
        vec![
            0x03, 0xa5, 0x05, 0x00, // lw a0, 0(a1)
            0x67, 0x80, 0x00, 0x00, // ret
            0, 0, 0, 0, // unresolved relocated word
        ],
        12,
    );
    image.unresolved_relocations_by_address.insert(
        0x1008,
        UnresolvedRelocation {
            name: "global_pointer".to_owned(),
            r_type: object::elf::R_RISCV_32,
            width: 4,
        },
    );
    let scenario = Scenario {
        arguments: vec![0, 0x1008],
        ..Scenario::default()
    };
    let error = execute(&image, &empty_svd(), "test", scenario)
        .unwrap_err()
        .to_string();
    assert!(error.contains("RAM read reached unresolved ELF relocation"));
    assert!(error.contains("global_pointer"));

    let mut seeded = Scenario {
        arguments: vec![0, 0x1008],
        ..Scenario::default()
    };
    seeded.memory_initial.extend(
        0x1234_5678_u32
            .to_le_bytes()
            .into_iter()
            .enumerate()
            .map(|(offset, byte)| (0x1008 + offset as u32, byte)),
    );
    assert_eq!(
        execute(&image, &empty_svd(), "test", seeded)
            .unwrap()
            .return_value,
        0x1234_5678
    );
}

#[test]
fn mmio_write_does_not_create_a_generic_readback_value() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let mmio_svd = MmioMap {
        registers: Vec::new(),
        regions: vec![crate::MmioRegion {
            name: "radio".to_owned(),
            start: 0x2010_0000,
            end: 0x2020_0000,
            readable: true,
            writable: true,
        }],
    };
    let address = 0x2010_0010;

    let mut seeded = Scenario::default();
    seeded.mmio_initial.insert(address, 0x1122_3344);
    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, seeded);
    machine.write(address, 32, 0xaabb_ccdd).unwrap();
    assert_eq!(machine.read(address, 32).unwrap(), 0x1122_3344);

    let mut machine = Machine::new(&image, &mmio_svd, 0x1000, Scenario::default());
    machine.write(address, 32, 0xaabb_ccdd).unwrap();
    assert!(machine.read(address, 32).is_err());
}

#[test]
fn bss_tail_is_known_zero() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
    assert_eq!(image.byte(0x1004), Some(0));
    assert_eq!(image.byte(0x1008), None);
}

#[test]
fn writes_to_read_only_elf_memory_fail_closed() {
    let mut image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 8);
    image.segments[0].writable = false;
    let svd = empty_svd();
    let mut machine = Machine::new(&image, &svd, 0x1000, Scenario::default());

    let error = machine.write(0x1004, 8, 0x5a).unwrap_err();
    assert!(error.to_string().contains("read-only ELF memory"));
    assert!(machine.persistent_memory().is_empty());
}

#[test]
fn execution_rejects_extra_arguments_and_unconsumed_mmio_reads() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let too_many = Scenario {
        arguments: vec![0; 9],
        ..Scenario::default()
    };
    assert!(
        execute(&image, &empty_svd(), "test", too_many)
            .unwrap_err()
            .to_string()
            .contains("stack arguments are not implemented")
    );

    let mut unconsumed = Scenario::default();
    unconsumed
        .mmio_reads
        .entry(0x2010_0010)
        .or_default()
        .push_back(1);
    assert!(
        execute(&image, &empty_svd(), "test", unconsumed)
            .unwrap_err()
            .to_string()
            .contains("unconsumed MMIO read responses")
    );
}

#[test]
fn fence_is_an_ordered_execution_event() {
    let image = tiny_image(
        vec![
            0x0f, 0x00, 0x30, 0x03, // fence rw, rw
            0x67, 0x80, 0x00, 0x00, // ret
        ],
        8,
    );
    let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(
        result.events,
        vec![ExecutionEvent::Fence {
            fm: 0,
            predecessor: 3,
            successor: 3,
        }]
    );
}

#[test]
fn atomic_word_operations_preserve_rv32_wrapping_and_comparison_semantics() {
    assert_eq!(atomic_word_result(AmoOp::Swap, 7, 11), 11);
    assert_eq!(atomic_word_result(AmoOp::Add, u32::MAX, 2), 1);
    assert_eq!(atomic_word_result(AmoOp::Xor, 0xaa, 0x0f), 0xa5);
    assert_eq!(atomic_word_result(AmoOp::And, 0xaa, 0x0f), 0x0a);
    assert_eq!(atomic_word_result(AmoOp::Or, 0xa0, 0x0f), 0xaf);
    assert_eq!(atomic_word_result(AmoOp::Min, u32::MAX, 1), u32::MAX);
    assert_eq!(atomic_word_result(AmoOp::Max, u32::MAX, 1), 1);
    assert_eq!(atomic_word_result(AmoOp::Minu, u32::MAX, 1), 1);
    assert_eq!(atomic_word_result(AmoOp::Maxu, u32::MAX, 1), u32::MAX);
}

proptest! {
    #[test]
    fn atomic_word_semantics_hold_for_arbitrary_rv32_words(current in any::<u32>(), source in any::<u32>()) {
        prop_assert_eq!(atomic_word_result(AmoOp::Swap, current, source), source);
        prop_assert_eq!(atomic_word_result(AmoOp::Add, current, source), current.wrapping_add(source));
        prop_assert_eq!(atomic_word_result(AmoOp::Xor, current, source), current ^ source);
        prop_assert_eq!(atomic_word_result(AmoOp::And, current, source), current & source);
        prop_assert_eq!(atomic_word_result(AmoOp::Or, current, source), current | source);
        prop_assert_eq!(atomic_word_result(AmoOp::Min, current, source), if (current as i32) < (source as i32) { current } else { source });
        prop_assert_eq!(atomic_word_result(AmoOp::Max, current, source), if (current as i32) > (source as i32) { current } else { source });
        prop_assert_eq!(atomic_word_result(AmoOp::Minu, current, source), current.min(source));
        prop_assert_eq!(atomic_word_result(AmoOp::Maxu, current, source), current.max(source));
    }
}

#[test]
fn executes_atomic_or_on_private_stack_memory() {
    let image = tiny_image(
        [
            0xffc1_0293_u32, // addi t0, sp, -4
            0x0002_a023,     // sw zero, 0(t0)
            0x0550_0313,     // addi t1, zero, 0x55
            0x4662_a52f,     // amoor.w.aqrl a0, t1, (t0)
            0x0002_a583,     // lw a1, 0(t0)
            0x00b5_6533,     // or a0, a0, a1
            0x0000_8067,     // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        28,
    );
    let result = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap();
    assert_eq!(result.return_value, 0x55);
    assert!(
        result
            .timeline
            .iter()
            .any(|event| matches!(event, ExecutionTimelineEvent::RamWrite { value: 0x55, .. }))
    );
}

#[test]
fn private_stack_fill_is_explicit_and_default_stack_remains_poison() {
    let image = tiny_image(
        [
            0xfff1_0293_u32, // addi t0, sp, -1
            0x0002_c503,     // lbu a0, 0(t0)
            0x0000_8067,     // ret
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect(),
        12,
    );
    let error = execute(&image, &empty_svd(), "test", Scenario::default()).unwrap_err();
    assert!(error.to_string().contains("poison/unmapped memory"));

    let scenario = Scenario {
        private_stack_fill: Some(0xa5),
        ..Scenario::default()
    };
    let result = execute(&image, &empty_svd(), "test", scenario).unwrap();
    assert_eq!(result.return_value, 0xa5);
}

#[test]
fn reports_only_observed_memory_mutations() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let address = 0x3fff_0000;
    let mut scenario = Scenario::default();
    scenario.memory_initial.insert(address, 0xaa);
    scenario.memory_initial.insert(address + 1, 0);
    scenario.observed_memory.push(MemoryRange {
        start: address,
        length: 2,
    });
    let mut machine = Machine::new(&image, &svd, 0, scenario);
    machine.write(address, 16, 0x55aa).unwrap();
    machine.write(address + 8, 8, 0xff).unwrap();
    assert_eq!(
        machine.memory_changes().unwrap(),
        vec![MemoryChange {
            address: address + 1,
            before: 0,
            after: 0x55,
        }]
    );
}

#[test]
fn observed_memory_alias_reports_normalized_addresses() {
    let image = tiny_image(vec![0x67, 0x80, 0x00, 0x00], 4);
    let svd = empty_svd();
    let actual = 0x3fff_0120;
    let mut scenario = Scenario::default();
    scenario.memory_initial.insert(actual, 0);
    scenario.memory_initial.insert(actual + 1, 0);
    scenario.memory_aliases.push(MemoryAlias {
        start: actual,
        length: 2,
        comparison_start: 0,
    });
    let mut machine = Machine::new(&image, &svd, 0, scenario);
    machine.write(actual + 1, 8, 0x5a).unwrap();
    assert_eq!(
        machine.memory_changes().unwrap(),
        vec![MemoryChange {
            address: 1,
            before: 0,
            after: 0x5a,
        }]
    );
}
