use super::*;

#[test]
fn esp32s31_wifi_osi_layout_is_versioned_and_bounded() {
    let table = WIFI_OSI_V9.spec();
    assert_eq!(table.version, 9);
    assert_eq!(table.magic, 0xdead_beaf);
    assert_eq!(table.size, 0x200);
    assert_eq!(table.magic_offset, table.size - 4);
    assert!(slots(WIFI_OSI_V9).all(|slot| slot.spec().offset < table.magic_offset));
}

#[test]
fn rand_and_random_are_distinct_slots() {
    assert_eq!(WIFI_OSI_V9.function_at(0x0bc), Some(RAND));
    assert_eq!(WIFI_OSI_V9.function_at(0x144), Some(RANDOM));
    assert!(WIFI_OSI_V9.function_at(0x0c0).is_none());
}

#[test]
fn wifi_interrupt_critical_section_slots_are_typed_and_unmodeled() {
    for (offset, operation, arguments) in [
        (0x028, "critical-section.enter", 1),
        (0x02c, "critical-section.exit", 2),
    ] {
        let function = WIFI_OSI_V9.function_at(offset).unwrap().spec();
        assert_eq!(function.semantic.operation, operation);
        assert_eq!(function.argument_count, arguments);
        assert_eq!(function.return_model, ExternalReturnModel::Unmodeled);
    }
}

#[test]
fn every_external_slot_has_a_typed_semantic_contract() {
    for function in WIFI_OSI_V9.spec().functions {
        assert!(!function.semantic.operation.is_empty());
        assert_eq!(
            function.semantic.arguments.len(),
            usize::from(function.argument_count),
            "{} semantic arguments do not match its ABI arity",
            function.c_name
        );
        assert!(!function.semantic.return_type.is_empty());
        for argument in function.semantic.arguments {
            assert!(!argument.name.is_empty());
            assert!(!argument.c_type.is_empty());
        }
    }
}

#[test]
fn event_dispatch_metadata_references_the_reviewed_typed_signature() {
    let mut operations = Vec::new();
    for function in WIFI_OSI_V9.spec().functions {
        let Some(dispatch) = function.semantic.event_dispatch else {
            continue;
        };
        operations.push(function.semantic.operation);
        assert!(!dispatch.mechanism.is_empty());
        assert!(!dispatch.execution_context.is_empty());
        assert!(!dispatch.argument_roles.is_empty());
        let mut roles = Vec::new();
        let mut arguments = Vec::new();
        for binding in dispatch.argument_roles {
            assert!(!binding.role.is_empty());
            assert!(!roles.contains(&binding.role));
            roles.push(binding.role);
            assert!(!binding.argument.is_empty());
            assert!(!arguments.contains(&binding.argument));
            arguments.push(binding.argument);
            assert!(
                function
                    .semantic
                    .arguments
                    .iter()
                    .any(|argument| argument.name == binding.argument),
                "{} dispatch role {} references unknown argument {}",
                function.c_name,
                binding.role,
                binding.argument,
            );
        }
    }
    assert_eq!(operations, ["rtos.queue.send-from-isr", "rtos.event.post"]);
}

#[test]
fn replacement_boundaries_use_reviewed_v9_offsets() {
    for (offset, operation) in [
        (0x068, "rtos.queue.send-from-isr"),
        (0x09c, "time.blocking-delay"),
        (0x0b4, "rtos.event.post"),
        (0x0f0, "timer.arm-micros"),
        (0x124, "storage.nvs.open"),
        (0x130, "storage.nvs.blob.write"),
        (0x134, "storage.nvs.blob.read"),
        (0x150, "logging.write-format"),
    ] {
        let function = WIFI_OSI_V9.function_at(offset).unwrap();
        assert_eq!(function.spec().semantic.operation, operation);
        assert_eq!(function.spec().return_model, ExternalReturnModel::Unmodeled);
    }
}
