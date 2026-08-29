//! Executable models for reviewed ESP32-S31 external calls.
//!
//! The reviewed interface pack owns table roots, layout guards, slot offsets,
//! ABI types and semantic annotations.  This compiled harness supplies only
//! behavior that cannot be expressed as reviewed data.  A slot must opt in
//! through its explicit `execution-model` foreign key.

use open_radio_vendor_contracts::{
    ExternalCallModelRef, ExternalCallModelSetRef, ExternalCallModelSetSpec, ExternalCallModelSpec,
    ExternalOutputModel, ExternalReturnModel,
};

const COEX_PTI_OUTPUTS: &[ExternalOutputModel] = &[ExternalOutputModel::PrivateStack {
    pointer_argument: 1,
    width: 8,
}];
const QUEUE_SEND_FROM_ISR_OUTPUTS: &[ExternalOutputModel] = &[ExternalOutputModel::PrivateStack {
    pointer_argument: 2,
    width: 8,
}];
const QUEUE_RECEIVE_OUTPUTS: &[ExternalOutputModel] = &[ExternalOutputModel::PrivateStack {
    pointer_argument: 1,
    width: 32,
}];

const fn memory_word_outputs<const N: usize>(pointer_argument: u8) -> [ExternalOutputModel; N] {
    let mut outputs = [ExternalOutputModel::Memory {
        pointer_argument,
        byte_offset: 0,
        width: 32,
    }; N];
    let mut index = 0;
    while index < N {
        outputs[index] = ExternalOutputModel::Memory {
            pointer_argument,
            byte_offset: (index as u16) * 4,
            width: 32,
        };
        index += 1;
    }
    outputs
}

const fn p256_key_pair_outputs() -> [ExternalOutputModel; 24] {
    let mut outputs = [ExternalOutputModel::Memory {
        pointer_argument: 0,
        byte_offset: 0,
        width: 32,
    }; 24];
    let mut index = 0;
    while index < 16 {
        outputs[index] = ExternalOutputModel::Memory {
            pointer_argument: 0,
            byte_offset: (index as u16) * 4,
            width: 32,
        };
        index += 1;
    }
    while index < 24 {
        outputs[index] = ExternalOutputModel::Memory {
            pointer_argument: 1,
            byte_offset: ((index - 16) as u16) * 4,
            width: 32,
        };
        index += 1;
    }
    outputs
}

const P256_KEY_PAIR_OUTPUTS: &[ExternalOutputModel] = &p256_key_pair_outputs();
const P256_DH_OUTPUTS: &[ExternalOutputModel] = &memory_word_outputs::<8>(3);
const AES_CMAC_OUTPUTS: &[ExternalOutputModel] = &memory_word_outputs::<4>(3);

const ESP32S31_WIFI_OSI_MODELS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "env-is-chip",
        return_model: ExternalReturnModel::Constant(1),
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-int-disable",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-int-restore",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "rand",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "random",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-yield-from-isr",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "queue-send",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "queue-receive",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: QUEUE_RECEIVE_OUTPUTS,
    },
    ExternalCallModelSpec {
        id: "task-ms-to-tick",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "queue-msg-waiting",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "slow-clock-calibration-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-pti-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: COEX_PTI_OUTPUTS,
    },
    ExternalCallModelSpec {
        id: "queue-send-from-isr",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: QUEUE_SEND_FROM_ISR_OUTPUTS,
    },
    ExternalCallModelSpec {
        id: "task-delay",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "event-post",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "timer-arm-us",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "timer-disarm",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "timer-done",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-status-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-wifi-release",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-schedule-interval-set",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-open",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-close",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-commit",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-set-blob",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-get-blob",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "nvs-erase-key",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "log-writev",
        return_model: ExternalReturnModel::Unmodeled,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "esp-timer-get-time",
        return_model: ExternalReturnModel::SymbolicU64,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-zalloc",
        return_model: ExternalReturnModel::AllocatedZeroed { size_argument: 0 },
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-create-queue",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "semphr-create",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "semphr-delete",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-create-pinned",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-create",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-delete",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-get-current",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "task-get-max-priority",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "mutex-lock",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "mutex-unlock",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "free",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "malloc-internal",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-malloc",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-wifi-request",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "phy-enable",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-pm-sleep-lock-acquire",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-clock-enable",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-schedule-interval-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-schedule-current-period-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-schedule-flexible-period-get",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-schedule-phase-get",
        return_model: ExternalReturnModel::OpaquePointer,
        outputs: &[],
    },
];

const ESP32S31_WIFI_OSI_MODEL_SET: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "esp32s31-wifi-osi-v9",
    models: ESP32S31_WIFI_OSI_MODELS,
};

const ESP32S31_COEX_ADAPTER_MODELS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "coex-env-is-chip",
        return_model: ExternalReturnModel::Constant(1),
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "coex-xtal-frequency-get",
        // ESP-IDF exposes the crystal frequency in MHz through this ABI.
        // ESP32-S31 uses a 40 MHz crystal in the reviewed target pack.
        return_model: ExternalReturnModel::Constant(40),
        outputs: &[],
    },
];

const ESP32S31_COEX_ADAPTER_MODEL_SET: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "esp32s31-coex-adapter-v2",
    models: ESP32S31_COEX_ADAPTER_MODELS,
};

const ESP32S31_WIFI_RUNTIME_CALLBACKS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "wifi-rx-callback",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "wifi-gpio-debug-callback",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "netstack-buffer-ref",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "netstack-buffer-free",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "rate-to-schedule-index",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
];
const ESP32S31_WIFI_RUNTIME_CALLBACK_MODEL_SET: ExternalCallModelSetSpec =
    ExternalCallModelSetSpec {
        id: "esp32s31-wifi-runtime-callbacks-v1",
        models: ESP32S31_WIFI_RUNTIME_CALLBACKS,
    };

const ESP32S31_BLE_EXTERNAL_FUNCTION_MODELS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "allocate",
        return_model: ExternalReturnModel::Allocated { size_argument: 0 },
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "free",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "osi-assert",
        return_model: ExternalReturnModel::Void,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "random-u32",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: &[],
    },
    ExternalCallModelSpec {
        id: "p256-generate-key-pair",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: P256_KEY_PAIR_OUTPUTS,
    },
    ExternalCallModelSpec {
        id: "p256-diffie-hellman",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: P256_DH_OUTPUTS,
    },
    ExternalCallModelSpec {
        id: "aes-cmac",
        return_model: ExternalReturnModel::SymbolicU32,
        outputs: AES_CMAC_OUTPUTS,
    },
];

const ESP32S31_BLE_EXTERNAL_FUNCTION_MODEL_SET: ExternalCallModelSetSpec =
    ExternalCallModelSetSpec {
        id: "esp32s31-ble-external-functions-20250819",
        models: ESP32S31_BLE_EXTERNAL_FUNCTION_MODELS,
    };

pub const WIFI_OSI_MODELS_V9: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&ESP32S31_WIFI_OSI_MODEL_SET);
pub const COEX_ADAPTER_MODELS_V2: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&ESP32S31_COEX_ADAPTER_MODEL_SET);
pub const WIFI_RUNTIME_CALLBACKS_V1: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&ESP32S31_WIFI_RUNTIME_CALLBACK_MODEL_SET);
pub const BLE_EXTERNAL_FUNCTION_MODELS_20250819: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&ESP32S31_BLE_EXTERNAL_FUNCTION_MODEL_SET);
pub const ENV_IS_CHIP_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[0]);
pub const RAND_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[3]);
pub const RANDOM_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[4]);

#[cfg(test)]
mod tests;
