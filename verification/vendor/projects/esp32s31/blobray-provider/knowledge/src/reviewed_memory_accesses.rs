//! Sparse, artifact-local classification of unresolved controller RAM access.

use open_radio_vendor_analysis_model::{
    ReviewedMemoryAccessClassification, ReviewedMemoryAccessOccurrence,
    ReviewedMemoryAccessOperation as Operation, ReviewedMemoryAccessRole as Role,
};

const DTM_EVIDENCE: &str =
    "verification/vendor/projects/esp32s31/reference/bluetooth-direct-test-mode.md";
const DTM_ARTIFACT_SOURCE: &str = "ble-controller";
const DTM_ARTIFACT_SHA256: &str =
    "5dbd91c45d13a2afc99e5414732aaffe1230737b471ddcf0bbe7e17a9d98e225";

const fn classification(
    id: &'static str,
    function: &'static str,
    site: u32,
    operation: Operation,
    role: Role,
    object: &'static str,
) -> ReviewedMemoryAccessClassification {
    ReviewedMemoryAccessClassification::new(
        id,
        ReviewedMemoryAccessOccurrence::new(
            DTM_ARTIFACT_SOURCE,
            DTM_ARTIFACT_SHA256,
            function,
            site,
            operation,
        ),
        role,
        object,
        DTM_EVIDENCE,
    )
}

pub static CLASSIFICATIONS: &[ReviewedMemoryAccessClassification] = &[
    classification(
        "ble-controller-rx-pool-source-word-08",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a704,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-rx-buffer-pool-selection",
    ),
    classification(
        "ble-controller-rx-pool-source-word-0c",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a706,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-rx-buffer-pool-selection",
    ),
    classification(
        "ble-controller-rx-pool-source-word-04",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a70a,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-rx-buffer-pool-selection",
    ),
    classification(
        "ble-controller-rx-chain-bookkeeping-head",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a70c,
        Operation::Store,
        Role::SoftwareOnly,
        "ble-controller-rx-chain-bookkeeping",
    ),
    classification(
        "ble-controller-rx-chain-bookkeeping-tail",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a70e,
        Operation::Store,
        Role::SoftwareOnly,
        "ble-controller-rx-chain-bookkeeping",
    ),
    classification(
        "ble-controller-rx-chain-bookkeeping-swap-reserve",
        "ble-controller::r_sym_ble_D3G2s4EUhwQF8UMS2GBp",
        0x1005_a712,
        Operation::Store,
        Role::SoftwareOnly,
        "ble-controller-rx-chain-bookkeeping",
    ),
    classification(
        "ble-controller-pool-manager-count-load",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_5876,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-tail-load",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_587a,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-count-store",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_5888,
        Operation::Store,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-state-load",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_5890,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-head-load",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_5894,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-count-reload",
        "ble-controller::r_sym_ble_ESFPOPR2AmgbKTRUmQsQ",
        0x1004_589e,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
    classification(
        "ble-controller-pool-manager-list-count",
        "ble-controller::r_sym_ble_hRMbruvWmDLQOmL65xAj",
        0x1005_6dcc,
        Operation::Load,
        Role::SoftwareOnly,
        "ble-controller-buffer-pool-manager",
    ),
];
