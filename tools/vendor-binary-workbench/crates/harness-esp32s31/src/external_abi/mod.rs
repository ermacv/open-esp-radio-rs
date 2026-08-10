//! Executable models for reviewed ESP32-S31 external calls.
//!
//! The reviewed interface pack owns table roots, layout guards, slot offsets,
//! ABI types and semantic annotations.  This compiled harness supplies only
//! behavior that cannot be expressed as reviewed data.  A slot must opt in
//! through its explicit `execution-model` foreign key.

use open_radio_vendor_contracts::{
    ExternalCallModelRef, ExternalCallModelSetRef, ExternalCallModelSetSpec, ExternalCallModelSpec,
    ExternalReturnModel,
};

const ESP32S31_WIFI_OSI_MODELS: &[ExternalCallModelSpec] = &[
    ExternalCallModelSpec {
        id: "env-is-chip",
        return_model: ExternalReturnModel::Constant(1),
    },
    ExternalCallModelSpec {
        id: "wifi-int-disable",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "wifi-int-restore",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "rand",
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalCallModelSpec {
        id: "random",
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalCallModelSpec {
        id: "slow-clock-calibration-get",
        return_model: ExternalReturnModel::SymbolicU32,
    },
    ExternalCallModelSpec {
        id: "coex-pti-get",
        return_model: ExternalReturnModel::PrivateStackOutputU8 {
            pointer_argument: 1,
        },
    },
    ExternalCallModelSpec {
        id: "queue-send-from-isr",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "task-delay",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "event-post",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "timer-arm-us",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-open",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-close",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-commit",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-set-blob",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-get-blob",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "nvs-erase-key",
        return_model: ExternalReturnModel::Unmodeled,
    },
    ExternalCallModelSpec {
        id: "log-writev",
        return_model: ExternalReturnModel::Unmodeled,
    },
];

const ESP32S31_WIFI_OSI_MODEL_SET: ExternalCallModelSetSpec = ExternalCallModelSetSpec {
    id: "esp32s31-wifi-osi-v9",
    models: ESP32S31_WIFI_OSI_MODELS,
};

pub const WIFI_OSI_MODELS_V9: ExternalCallModelSetRef =
    ExternalCallModelSetRef::new(&ESP32S31_WIFI_OSI_MODEL_SET);
pub const ENV_IS_CHIP_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[0]);
pub const RAND_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[3]);
pub const RANDOM_MODEL: ExternalCallModelRef =
    ExternalCallModelRef::new(&ESP32S31_WIFI_OSI_MODELS[4]);

#[cfg(test)]
mod tests;
