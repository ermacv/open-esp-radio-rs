//! Espressif Bluetooth-controller platform boundaries.
//!
//! The controller archives provide weak `wr_btdm_osal_*` fallbacks which
//! deliberately assert until the platform registers its NPL and external
//! function tables.  Those fallbacks are not controller behavior and must not
//! become reachable analysis bodies.  Their exact ABI remains useful evidence,
//! so the ESP-IDF add-on exposes it as opaque semantic calls.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, ExternalArgumentDirection, ExternalArgumentSpec,
    ExternalReturnModel, ExternalSemanticSpec, SemanticFunctionBodyPolicy,
};

const fn input(name: &'static str, c_type: &'static str) -> ExternalArgumentSpec {
    ExternalArgumentSpec {
        name,
        c_type,
        direction: ExternalArgumentDirection::Input,
    }
}

const fn output(name: &'static str, c_type: &'static str) -> ExternalArgumentSpec {
    ExternalArgumentSpec {
        name,
        c_type,
        direction: ExternalArgumentDirection::Output,
    }
}

const fn input_output(name: &'static str, c_type: &'static str) -> ExternalArgumentSpec {
    ExternalArgumentSpec {
        name,
        c_type,
        direction: ExternalArgumentDirection::InputOutput,
    }
}

const NO_ARGUMENTS: &[ExternalArgumentSpec] = &[];
const EVENT_QUEUE_ARGUMENTS: &[ExternalArgumentSpec] =
    &[input_output("queue", "struct ble_npl_eventq *")];
const EVENT_QUEUE_TIMEOUT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("queue", "struct ble_npl_eventq *"),
    input("timeout", "ble_npl_time_t"),
];
const EVENT_QUEUE_EVENT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("queue", "struct ble_npl_eventq *"),
    input_output("event", "struct ble_npl_event *"),
];
const EVENT_ARGUMENTS: &[ExternalArgumentSpec] = &[input_output("event", "struct ble_npl_event *")];
const EVENT_INIT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("event", "struct ble_npl_event *"),
    input("callback", "ble_npl_event_fn *"),
    input("argument", "void *"),
];
const EVENT_SET_ARGUMENT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("event", "struct ble_npl_event *"),
    input("argument", "void *"),
];
const MUTEX_ARGUMENTS: &[ExternalArgumentSpec] = &[input_output("mutex", "struct ble_npl_mutex *")];
const MUTEX_TIMEOUT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("mutex", "struct ble_npl_mutex *"),
    input("timeout", "ble_npl_time_t"),
];
const CALLOUT_ARGUMENTS: &[ExternalArgumentSpec] =
    &[input_output("callout", "struct ble_npl_callout *")];
const CALLOUT_INIT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("callout", "struct ble_npl_callout *"),
    input_output("queue", "struct ble_npl_eventq *"),
    input("callback", "ble_npl_event_fn *"),
    input("argument", "void *"),
];
const CALLOUT_TIMEOUT_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input_output("callout", "struct ble_npl_callout *"),
    input("timeout", "ble_npl_time_t"),
];
const MILLISECONDS_ARGUMENTS: &[ExternalArgumentSpec] = &[input("milliseconds", "uint32_t")];
const CRITICAL_CONTEXT_ARGUMENTS: &[ExternalArgumentSpec] = &[input("context", "uint32_t")];
const TASK_CREATE_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input("entry", "void (*)(void *)"),
    input("name", "const char *"),
    input("stack_depth", "uint32_t"),
    input("argument", "void *"),
    input("priority", "uint32_t"),
    output("task_handle", "void **"),
    input("core_id", "uint32_t"),
];
const TASK_ARGUMENTS: &[ExternalArgumentSpec] = &[input("task", "void *")];
const INTERRUPT_ALLOC_ARGUMENTS: &[ExternalArgumentSpec] = &[
    input("source", "uint32_t"),
    input("flags", "uint32_t"),
    input("handler", "void (*)(void *)"),
    input("argument", "void *"),
    output("handle", "void **"),
];
const INTERRUPT_HANDLE_ARGUMENTS: &[ExternalArgumentSpec] = &[input_output("handle", "void **")];
const ALLOCATE_ARGUMENTS: &[ExternalArgumentSpec] = &[input("size", "size_t")];
const FREE_ARGUMENTS: &[ExternalArgumentSpec] = &[input("allocation", "void *")];
const EFUSE_MAC_ARGUMENTS: &[ExternalArgumentSpec] = &[output("mac", "uint8_t[6]")];

const EVIDENCE: &str = "exact Espressif Bluetooth-controller porting symbol and reviewed ESP-IDF/NimBLE NPL or external-function-table ABI; the weak fallback body is an unregistered-platform assert, not controller behavior";

macro_rules! opaque_boundary {
    (
        $static_name:ident,
        $id:literal,
        $symbol:literal,
        $operation:literal,
        $arguments:ident,
        $return_type:literal,
        $return_model:expr,
        $replacement:literal
    ) => {
        static $static_name: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
            id: $id,
            source: "esp-idf-addon",
            c_name: $symbol,
            argument_count: $arguments.len() as u8,
            body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
            return_model: $return_model,
            semantic: ExternalSemanticSpec {
                operation: $operation,
                arguments: $arguments,
                return_type: $return_type,
                replacement: Some($replacement),
                event_dispatch: None,
            },
            evidence: EVIDENCE,
        };
    };
}

opaque_boundary!(
    EVENTQ_INIT,
    "esp-idf.bluetooth-npl.eventq-init",
    "wr_btdm_osal_eventq_init",
    "rtos.event-queue.init",
    EVENT_QUEUE_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_DEINIT,
    "esp-idf.bluetooth-npl.eventq-deinit",
    "wr_btdm_osal_eventq_deinit",
    "rtos.event-queue.deinit",
    EVENT_QUEUE_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_GET,
    "esp-idf.bluetooth-npl.eventq-get",
    "wr_btdm_osal_eventq_get",
    "rtos.event-queue.receive",
    EVENT_QUEUE_TIMEOUT_ARGUMENTS,
    "struct ble_npl_event *",
    ExternalReturnModel::SymbolicU32,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_PUT,
    "esp-idf.bluetooth-npl.eventq-put",
    "wr_btdm_osal_eventq_put",
    "rtos.event-queue.send",
    EVENT_QUEUE_EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_PUT_TO_FRONT,
    "esp-idf.bluetooth-npl.eventq-put-to-front",
    "wr_btdm_osal_eventq_put_to_front",
    "rtos.event-queue.send-front",
    EVENT_QUEUE_EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_REMOVE,
    "esp-idf.bluetooth-npl.eventq-remove",
    "wr_btdm_osal_eventq_remove",
    "rtos.event-queue.remove",
    EVENT_QUEUE_EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENTQ_IS_EMPTY,
    "esp-idf.bluetooth-npl.eventq-is-empty",
    "wr_btdm_osal_eventq_is_empty",
    "rtos.event-queue.is-empty",
    EVENT_QUEUE_ARGUMENTS,
    "bool",
    ExternalReturnModel::SymbolicU32,
    "Rust-owned controller event queue"
);
opaque_boundary!(
    EVENT_RUN,
    "esp-idf.bluetooth-npl.event-run",
    "wr_btdm_osal_event_run",
    "rtos.event.run",
    EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust controller callback dispatch"
);
opaque_boundary!(
    EVENT_INIT,
    "esp-idf.bluetooth-npl.event-init",
    "wr_btdm_osal_event_init",
    "rtos.event.init",
    EVENT_INIT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event"
);
opaque_boundary!(
    EVENT_DEINIT,
    "esp-idf.bluetooth-npl.event-deinit",
    "wr_btdm_osal_event_deinit",
    "rtos.event.deinit",
    EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event"
);
opaque_boundary!(
    EVENT_RESET,
    "esp-idf.bluetooth-npl.event-reset",
    "wr_btdm_osal_event_reset",
    "rtos.event.reset",
    EVENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event"
);
opaque_boundary!(
    EVENT_IS_QUEUED,
    "esp-idf.bluetooth-npl.event-is-queued",
    "wr_btdm_osal_event_is_queued",
    "rtos.event.is-queued",
    EVENT_ARGUMENTS,
    "bool",
    ExternalReturnModel::SymbolicU32,
    "Rust-owned controller event"
);
opaque_boundary!(
    EVENT_GET_ARG,
    "esp-idf.bluetooth-npl.event-get-arg",
    "wr_btdm_osal_event_get_arg",
    "rtos.event.get-argument",
    EVENT_ARGUMENTS,
    "void *",
    ExternalReturnModel::SymbolicU32,
    "Rust-owned controller event"
);
opaque_boundary!(
    EVENT_SET_ARG,
    "esp-idf.bluetooth-npl.event-set-arg",
    "wr_btdm_osal_event_set_arg",
    "rtos.event.set-argument",
    EVENT_SET_ARGUMENT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller event"
);

opaque_boundary!(
    MUTEX_INIT,
    "esp-idf.bluetooth-npl.mutex-init",
    "wr_btdm_osal_mutex_init",
    "rtos.mutex.init",
    MUTEX_ARGUMENTS,
    "ble_npl_error_t",
    ExternalReturnModel::SymbolicU32,
    "Rust serialization owner"
);
opaque_boundary!(
    MUTEX_DEINIT,
    "esp-idf.bluetooth-npl.mutex-deinit",
    "wr_btdm_osal_mutex_deinit",
    "rtos.mutex.deinit",
    MUTEX_ARGUMENTS,
    "ble_npl_error_t",
    ExternalReturnModel::SymbolicU32,
    "Rust serialization owner"
);
opaque_boundary!(
    MUTEX_PEND,
    "esp-idf.bluetooth-npl.mutex-pend",
    "wr_btdm_osal_mutex_pend",
    "rtos.mutex.acquire",
    MUTEX_TIMEOUT_ARGUMENTS,
    "ble_npl_error_t",
    ExternalReturnModel::SymbolicU32,
    "Rust serialization owner"
);
opaque_boundary!(
    MUTEX_RELEASE,
    "esp-idf.bluetooth-npl.mutex-release",
    "wr_btdm_osal_mutex_release",
    "rtos.mutex.release",
    MUTEX_ARGUMENTS,
    "ble_npl_error_t",
    ExternalReturnModel::SymbolicU32,
    "Rust serialization owner"
);

opaque_boundary!(
    CALLOUT_INIT,
    "esp-idf.bluetooth-npl.callout-init",
    "wr_btdm_osal_callout_init",
    "time.callout.init",
    CALLOUT_INIT_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32,
    "Rust controller deadline timer"
);
opaque_boundary!(
    CALLOUT_RESET,
    "esp-idf.bluetooth-npl.callout-reset",
    "wr_btdm_osal_callout_reset",
    "time.callout.schedule",
    CALLOUT_TIMEOUT_ARGUMENTS,
    "ble_npl_error_t",
    ExternalReturnModel::SymbolicU32,
    "Rust controller deadline timer"
);
opaque_boundary!(
    CALLOUT_STOP,
    "esp-idf.bluetooth-npl.callout-stop",
    "wr_btdm_osal_callout_stop",
    "time.callout.stop",
    CALLOUT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust controller deadline timer"
);
opaque_boundary!(
    CALLOUT_DEINIT,
    "esp-idf.bluetooth-npl.callout-deinit",
    "wr_btdm_osal_callout_deinit",
    "time.callout.deinit",
    CALLOUT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust controller deadline timer"
);
opaque_boundary!(
    CALLOUT_MEM_RESET,
    "esp-idf.bluetooth-npl.callout-memory-reset",
    "wr_btdm_osal_callout_mem_reset",
    "time.callout.memory-reset",
    CALLOUT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust controller deadline timer"
);
opaque_boundary!(
    CALLOUT_IS_ACTIVE,
    "esp-idf.bluetooth-npl.callout-is-active",
    "wr_btdm_osal_callout_is_active",
    "time.callout.is-active",
    CALLOUT_ARGUMENTS,
    "bool",
    ExternalReturnModel::SymbolicU32,
    "Rust controller deadline timer"
);

opaque_boundary!(
    TIME_GET,
    "esp-idf.bluetooth-npl.time-get",
    "wr_btdm_osal_time_get",
    "time.monotonic-now",
    NO_ARGUMENTS,
    "ble_npl_time_t",
    ExternalReturnModel::SymbolicU32,
    "Rust monotonic clock"
);
opaque_boundary!(
    TIME_MS_TO_TICKS32,
    "esp-idf.bluetooth-npl.time-ms-to-ticks32",
    "wr_btdm_osal_time_ms_to_ticks32",
    "time.milliseconds-to-ticks",
    MILLISECONDS_ARGUMENTS,
    "ble_npl_time_t",
    ExternalReturnModel::SymbolicU32,
    "Rust duration conversion"
);
opaque_boundary!(
    ENTER_CRITICAL,
    "esp-idf.bluetooth-npl.enter-critical",
    "wr_btdm_osal_hw_enter_critical",
    "critical-section.enter",
    NO_ARGUMENTS,
    "uint32_t",
    ExternalReturnModel::SymbolicU32,
    "Rust critical-section owner"
);
opaque_boundary!(
    EXIT_CRITICAL,
    "esp-idf.bluetooth-npl.exit-critical",
    "wr_btdm_osal_hw_exit_critical",
    "critical-section.exit",
    CRITICAL_CONTEXT_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust critical-section owner"
);
opaque_boundary!(
    GET_TIME_FOREVER,
    "esp-idf.bluetooth-npl.get-time-forever",
    "wr_btdm_osal_get_time_forever",
    "time.forever-value",
    NO_ARGUMENTS,
    "ble_npl_time_t",
    ExternalReturnModel::SymbolicU32,
    "Rust controller wait policy"
);

opaque_boundary!(
    TASK_CREATE,
    "esp-idf.bluetooth-platform.task-create",
    "wr_btdm_osal_task_create",
    "rtos.task.create",
    TASK_CREATE_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32,
    "Rust-owned controller task"
);
opaque_boundary!(
    TASK_DELETE,
    "esp-idf.bluetooth-platform.task-delete",
    "wr_btdm_osal_task_delete",
    "rtos.task.delete",
    TASK_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned controller task"
);
opaque_boundary!(
    INTERRUPT_ALLOC,
    "esp-idf.bluetooth-platform.interrupt-allocate",
    "wr_btdm_osal_intr_alloc",
    "interrupt.allocate",
    INTERRUPT_ALLOC_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32,
    "Rust interrupt owner"
);
opaque_boundary!(
    INTERRUPT_FREE,
    "esp-idf.bluetooth-platform.interrupt-free",
    "wr_btdm_osal_intr_free",
    "interrupt.free",
    INTERRUPT_HANDLE_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32,
    "Rust interrupt owner"
);
opaque_boundary!(
    MALLOC,
    "esp-idf.bluetooth-platform.allocate",
    "wr_btdm_osal_malloc",
    "memory.allocate",
    ALLOCATE_ARGUMENTS,
    "void *",
    ExternalReturnModel::Allocated { size_argument: 0 },
    "Rust-owned bounded controller storage"
);
opaque_boundary!(
    FREE,
    "esp-idf.bluetooth-platform.free",
    "wr_btdm_osal_free",
    "memory.free",
    FREE_ARGUMENTS,
    "void",
    ExternalReturnModel::Void,
    "Rust-owned bounded controller storage"
);
opaque_boundary!(
    READ_EFUSE_MAC,
    "esp-idf.bluetooth-platform.read-efuse-mac",
    "wr_btdm_osal_read_efuse_mac",
    "device.read-efuse-mac",
    EFUSE_MAC_ARGUMENTS,
    "int",
    ExternalReturnModel::SymbolicU32,
    "Platform-provided controller identity"
);

pub(super) fn direct_external_semantic_function(
    name: &str,
) -> Option<&'static DirectSemanticFunctionSpec> {
    match name {
        "wr_btdm_osal_eventq_init" => Some(&EVENTQ_INIT),
        "wr_btdm_osal_eventq_deinit" => Some(&EVENTQ_DEINIT),
        "wr_btdm_osal_eventq_get" => Some(&EVENTQ_GET),
        "wr_btdm_osal_eventq_put" => Some(&EVENTQ_PUT),
        "wr_btdm_osal_eventq_put_to_front" => Some(&EVENTQ_PUT_TO_FRONT),
        "wr_btdm_osal_eventq_remove" => Some(&EVENTQ_REMOVE),
        "wr_btdm_osal_eventq_is_empty" => Some(&EVENTQ_IS_EMPTY),
        "wr_btdm_osal_event_run" => Some(&EVENT_RUN),
        "wr_btdm_osal_event_init" => Some(&EVENT_INIT),
        "wr_btdm_osal_event_deinit" => Some(&EVENT_DEINIT),
        "wr_btdm_osal_event_reset" => Some(&EVENT_RESET),
        "wr_btdm_osal_event_is_queued" => Some(&EVENT_IS_QUEUED),
        "wr_btdm_osal_event_get_arg" => Some(&EVENT_GET_ARG),
        "wr_btdm_osal_event_set_arg" => Some(&EVENT_SET_ARG),
        "wr_btdm_osal_mutex_init" => Some(&MUTEX_INIT),
        "wr_btdm_osal_mutex_deinit" => Some(&MUTEX_DEINIT),
        "wr_btdm_osal_mutex_pend" => Some(&MUTEX_PEND),
        "wr_btdm_osal_mutex_release" => Some(&MUTEX_RELEASE),
        "wr_btdm_osal_callout_init" => Some(&CALLOUT_INIT),
        "wr_btdm_osal_callout_reset" => Some(&CALLOUT_RESET),
        "wr_btdm_osal_callout_stop" => Some(&CALLOUT_STOP),
        "wr_btdm_osal_callout_deinit" => Some(&CALLOUT_DEINIT),
        "wr_btdm_osal_callout_mem_reset" => Some(&CALLOUT_MEM_RESET),
        "wr_btdm_osal_callout_is_active" => Some(&CALLOUT_IS_ACTIVE),
        "wr_btdm_osal_time_get" => Some(&TIME_GET),
        "wr_btdm_osal_time_ms_to_ticks32" => Some(&TIME_MS_TO_TICKS32),
        "wr_btdm_osal_hw_enter_critical" => Some(&ENTER_CRITICAL),
        "wr_btdm_osal_hw_exit_critical" => Some(&EXIT_CRITICAL),
        "wr_btdm_osal_get_time_forever" => Some(&GET_TIME_FOREVER),
        "wr_btdm_osal_task_create" => Some(&TASK_CREATE),
        "wr_btdm_osal_task_delete" => Some(&TASK_DELETE),
        "wr_btdm_osal_intr_alloc" => Some(&INTERRUPT_ALLOC),
        "wr_btdm_osal_intr_free" => Some(&INTERRUPT_FREE),
        "wr_btdm_osal_malloc" => Some(&MALLOC),
        "wr_btdm_osal_free" => Some(&FREE),
        "wr_btdm_osal_read_efuse_mac" => Some(&READ_EFUSE_MAC),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_controller_porting_fallbacks_are_exact_opaque_boundaries() {
        let expected = [
            "wr_btdm_osal_eventq_init",
            "wr_btdm_osal_eventq_get",
            "wr_btdm_osal_event_init",
            "wr_btdm_osal_mutex_pend",
            "wr_btdm_osal_callout_reset",
            "wr_btdm_osal_time_get",
            "wr_btdm_osal_hw_enter_critical",
            "wr_btdm_osal_task_create",
            "wr_btdm_osal_intr_alloc",
            "wr_btdm_osal_malloc",
            "wr_btdm_osal_read_efuse_mac",
        ];
        for symbol in expected {
            let contract = direct_external_semantic_function(symbol).unwrap();
            assert_eq!(contract.c_name, symbol);
            assert_eq!(
                contract.body_policy,
                SemanticFunctionBodyPolicy::OpaqueBoundary
            );
            assert_ne!(contract.return_model, ExternalReturnModel::Unmodeled);
        }
        assert!(direct_external_semantic_function("r_btdm_osal_malloc").is_none());
        assert!(direct_external_semantic_function("vendor_wr_btdm_osal_malloc").is_none());
        assert_eq!(
            direct_external_semantic_function("wr_btdm_osal_malloc")
                .unwrap()
                .return_model,
            ExternalReturnModel::Allocated { size_argument: 0 }
        );
    }

    #[test]
    fn controller_porting_abi_preserves_important_argument_directions() {
        let task = direct_external_semantic_function("wr_btdm_osal_task_create").unwrap();
        assert_eq!(task.argument_count, 7);
        assert_eq!(
            task.semantic.arguments[5].direction,
            ExternalArgumentDirection::Output
        );

        let event = direct_external_semantic_function("wr_btdm_osal_event_init").unwrap();
        assert_eq!(event.argument_count, 3);
        assert_eq!(
            event.semantic.arguments[0].direction,
            ExternalArgumentDirection::InputOutput
        );

        let interrupt = direct_external_semantic_function("wr_btdm_osal_intr_alloc").unwrap();
        assert_eq!(interrupt.argument_count, 5);
        assert_eq!(
            interrupt.semantic.arguments[4].direction,
            ExternalArgumentDirection::Output
        );
    }
}
