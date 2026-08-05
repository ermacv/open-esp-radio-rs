//! Versioned external ABI descriptions used by the structural reference tracer.
//!
//! These tables describe interfaces published by the platform. They do not
//! turn arbitrary indirect calls into trusted calls: a load must be rooted in
//! the named pointer cell and use an exact registered slot offset.

use open_radio_vendor_validator_core::{
    ExternalArgumentDirection, ExternalArgumentSpec, ExternalFunctionRef, ExternalFunctionSpec,
    ExternalReturnModel, ExternalSemanticSpec, ExternalTableRef, ExternalTableSpec,
};

const NO_ARGUMENTS: &[ExternalArgumentSpec] = &[];
const WIFI_INT_DISABLE_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "interrupt_mux",
    c_type: "*mut void",
    direction: ExternalArgumentDirection::InputOutput,
}];
const WIFI_INT_RESTORE_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "interrupt_mux",
        c_type: "*mut void",
        direction: ExternalArgumentDirection::InputOutput,
    },
    ExternalArgumentSpec {
        name: "restore_state",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
];
const COEX_PTI_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "pti_kind",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "out_pti",
        c_type: "*mut u8",
        direction: ExternalArgumentDirection::Output,
    },
];
const QUEUE_SEND_FROM_ISR_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "queue",
        c_type: "*mut void",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "item",
        c_type: "*const void",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "higher_priority_task_woken",
        c_type: "*mut bool",
        direction: ExternalArgumentDirection::Output,
    },
];
const TASK_DELAY_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "ticks",
    c_type: "u32",
    direction: ExternalArgumentDirection::Input,
}];
const EVENT_POST_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "event_base",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "event_id",
        c_type: "i32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "event_data",
        c_type: "*const void",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "event_data_size",
        c_type: "usize",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "ticks_to_wait",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
];
const TIMER_ARM_US_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "timer",
        c_type: "*mut timer",
        direction: ExternalArgumentDirection::InputOutput,
    },
    ExternalArgumentSpec {
        name: "micros",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "repeat",
        c_type: "bool",
        direction: ExternalArgumentDirection::Input,
    },
];
const NVS_OPEN_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "namespace",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "open_mode",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "out_handle",
        c_type: "*mut u32",
        direction: ExternalArgumentDirection::Output,
    },
];
const NVS_HANDLE_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "handle",
    c_type: "u32",
    direction: ExternalArgumentDirection::Input,
}];
const NVS_BLOB_SET_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "handle",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "key",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "value",
        c_type: "*const void",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "length",
        c_type: "usize",
        direction: ExternalArgumentDirection::Input,
    },
];
const NVS_BLOB_GET_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "handle",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "key",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "out_value",
        c_type: "*mut void",
        direction: ExternalArgumentDirection::Output,
    },
    ExternalArgumentSpec {
        name: "length",
        c_type: "*mut usize",
        direction: ExternalArgumentDirection::InputOutput,
    },
];
const NVS_ERASE_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "handle",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "key",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
];
const LOG_WRITEV_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "level",
        c_type: "u32",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "tag",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "format",
        c_type: "*const char",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "arguments",
        c_type: "va_list",
        direction: ExternalArgumentDirection::Input,
    },
];

const ESP32S31_WIFI_OSI_V9_FUNCTIONS: &[ExternalFunctionSpec] = &[
    ExternalFunctionSpec {
        id: "env-is-chip",
        offset: 0x004,
        c_name: "_env_is_chip",
        argument_count: 0,
        // The reference profile is explicitly the real ESP32-S31 target, not
        // the FPGA/emulation branch selected by a false callback result.
        return_model: ExternalReturnModel::Constant(1),
        semantic: ExternalSemanticSpec {
            operation: "target.is-real-chip",
            arguments: NO_ARGUMENTS,
            return_type: "bool (ABI u32)",
            replacement: Some("compile-time target capability"),
        },
    },
    ExternalFunctionSpec {
        id: "wifi-int-disable",
        offset: 0x028,
        c_name: "_wifi_int_disable",
        argument_count: 1,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "critical-section.enter",
            arguments: WIFI_INT_DISABLE_ARGUMENTS,
            return_type: "restore-state u32",
            replacement: Some("Rust interrupt critical-section guard"),
        },
    },
    ExternalFunctionSpec {
        id: "wifi-int-restore",
        offset: 0x02c,
        c_name: "_wifi_int_restore",
        argument_count: 2,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "critical-section.exit",
            arguments: WIFI_INT_RESTORE_ARGUMENTS,
            return_type: "void",
            replacement: Some("Rust interrupt critical-section guard release"),
        },
    },
    ExternalFunctionSpec {
        id: "rand",
        offset: 0x0bc,
        c_name: "_rand",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
        semantic: ExternalSemanticSpec {
            operation: "random.next-u32",
            arguments: NO_ARGUMENTS,
            return_type: "u32",
            replacement: Some("Rust RNG provider"),
        },
    },
    ExternalFunctionSpec {
        id: "random",
        offset: 0x144,
        c_name: "_random",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
        semantic: ExternalSemanticSpec {
            operation: "random.next-u32",
            arguments: NO_ARGUMENTS,
            return_type: "u32",
            replacement: Some("Rust RNG provider"),
        },
    },
    ExternalFunctionSpec {
        id: "slow-clock-calibration-get",
        offset: 0x148,
        c_name: "_slowclk_cal_get",
        argument_count: 0,
        return_model: ExternalReturnModel::SymbolicU32,
        semantic: ExternalSemanticSpec {
            operation: "clock.slow-calibration.get",
            arguments: NO_ARGUMENTS,
            return_type: "u32",
            replacement: Some("platform slow-clock calibration provider"),
        },
    },
    ExternalFunctionSpec {
        id: "coex-pti-get",
        offset: 0x1a8,
        c_name: "_coex_pti_get",
        argument_count: 2,
        return_model: ExternalReturnModel::PrivateStackOutputU8 {
            pointer_argument: 1,
        },
        semantic: ExternalSemanticSpec {
            operation: "coexistence.pti.query",
            arguments: COEX_PTI_ARGUMENTS,
            return_type: "status u32",
            replacement: Some("typed coexistence policy provider"),
        },
    },
    // The following slots have reviewed names, signatures and high-level
    // meaning, but not a validation-grade effect model. They remain explicit
    // reference blockers while exploratory IR can still show what happened.
    ExternalFunctionSpec {
        id: "queue-send-from-isr",
        offset: 0x068,
        c_name: "_queue_send_from_isr",
        argument_count: 3,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "rtos.queue.send-from-isr",
            arguments: QUEUE_SEND_FROM_ISR_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("async channel/event wakeup from ISR"),
        },
    },
    ExternalFunctionSpec {
        id: "task-delay",
        offset: 0x09c,
        c_name: "_task_delay",
        argument_count: 1,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "rtos.task.delay",
            arguments: TASK_DELAY_ARGUMENTS,
            return_type: "void",
            replacement: Some("Rust async timer"),
        },
    },
    ExternalFunctionSpec {
        id: "event-post",
        offset: 0x0b4,
        c_name: "_event_post",
        argument_count: 5,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "rtos.event.post",
            arguments: EVENT_POST_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust event dispatcher"),
        },
    },
    ExternalFunctionSpec {
        id: "timer-arm-us",
        offset: 0x0f0,
        c_name: "_timer_arm_us",
        argument_count: 3,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "timer.arm-micros",
            arguments: TIMER_ARM_US_ARGUMENTS,
            return_type: "void",
            replacement: Some("Rust async timer registration"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-open",
        offset: 0x124,
        c_name: "_nvs_open",
        argument_count: 3,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.open",
            arguments: NVS_OPEN_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-close",
        offset: 0x128,
        c_name: "_nvs_close",
        argument_count: 1,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.close",
            arguments: NVS_HANDLE_ARGUMENTS,
            return_type: "void",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-commit",
        offset: 0x12c,
        c_name: "_nvs_commit",
        argument_count: 1,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.commit",
            arguments: NVS_HANDLE_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-set-blob",
        offset: 0x130,
        c_name: "_nvs_set_blob",
        argument_count: 4,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.blob.write",
            arguments: NVS_BLOB_SET_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-get-blob",
        offset: 0x134,
        c_name: "_nvs_get_blob",
        argument_count: 4,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.blob.read",
            arguments: NVS_BLOB_GET_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "nvs-erase-key",
        offset: 0x138,
        c_name: "_nvs_erase_key",
        argument_count: 2,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "nvs.key.erase",
            arguments: NVS_ERASE_ARGUMENTS,
            return_type: "status i32",
            replacement: Some("typed Rust persistence provider"),
        },
    },
    ExternalFunctionSpec {
        id: "log-writev",
        offset: 0x150,
        c_name: "_log_writev",
        argument_count: 4,
        return_model: ExternalReturnModel::Unmodeled,
        semantic: ExternalSemanticSpec {
            operation: "logging.write-format",
            arguments: LOG_WRITEV_ARGUMENTS,
            return_type: "void",
            replacement: Some("Rust logging facade"),
        },
    },
];

const ESP32S31_WIFI_OSI_V9: ExternalTableSpec = ExternalTableSpec {
    id: "esp32s31-wifi-osi-v9",
    pointer_symbol: "g_osi_funcs_p",
    backing_symbol: "g_wifi_osi_funcs",
    version: 0x0000_0009,
    magic: 0xdead_beaf,
    size: 0x200,
    magic_offset: 0x1fc,
    functions: ESP32S31_WIFI_OSI_V9_FUNCTIONS,
};

pub const WIFI_OSI_V9: ExternalTableRef = ExternalTableRef::new(&ESP32S31_WIFI_OSI_V9);
pub const ENV_IS_CHIP: ExternalFunctionRef =
    ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[0]);
pub const RAND: ExternalFunctionRef = ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[3]);
pub const RANDOM: ExternalFunctionRef =
    ExternalFunctionRef::new(&ESP32S31_WIFI_OSI_V9_FUNCTIONS[4]);

#[cfg(test)]
pub fn slots(table: ExternalTableRef) -> impl Iterator<Item = ExternalFunctionRef> {
    table.spec().functions.iter().map(ExternalFunctionRef::new)
}

#[cfg(test)]
mod tests;
