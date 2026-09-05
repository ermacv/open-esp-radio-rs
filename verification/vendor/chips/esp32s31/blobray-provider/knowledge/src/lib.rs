//! Declarative ESP32-S31 rev0 hardware and ABI facts.
//!
//! Executable C/ESP-IDF adapters and harness composition belong to the sibling
//! models crate. Selecting facts alone never installs executable summary hooks.

use open_radio_vendor_analysis_model::ReviewedCompressedPointerEncoding;
pub use open_radio_vendor_chip_contracts_esp32s31_rev0::{CONTRACTS, entry_contract};
use open_radio_vendor_semantics::*;

pub const RTC_XTAL_FREQUENCY_SEMANTIC: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp32s31-rtc-xtal-frequency-v1",
    source: "esp32s31-rev0-chip-addon",
    c_name: "rtc_clk_xtal_freq_get",
    argument_count: 0,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::Constant(40),
    semantic: ExternalSemanticSpec {
        operation: "clock.xtal-frequency.read",
        arguments: &[],
        return_type: "u32",
        replacement: Some("fixed ESP32-S31 40 MHz crystal contract"),
        event_dispatch: None,
    },
    evidence: "esp32s31-rev0-fixed-crystal-chip-contract",
};

pub const COMPRESSED_POINTER_ENCODINGS: &[ReviewedCompressedPointerEncoding] =
    &[ReviewedCompressedPointerEncoding::new(
        "esp32s31-controller-sram-low20-word-address-v1",
        0x2f00_0000,
        20,
        2,
    )];
