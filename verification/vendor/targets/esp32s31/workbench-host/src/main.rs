use std::path::Path;

use open_radio_vendor_binary_workbench::{
    DriverAdapterRequest, DriverAdapterVerification, KnowledgeProviderDescriptor, ProviderRegistry,
    ProviderResult, SemanticContractRequest, SemanticVerificationReport,
    VerificationProviderDescriptor,
};
use open_radio_vendor_knowledge_esp32s31 as esp32s31_knowledge;
use open_radio_vendor_verification_esp32s31 as esp32s31_verification;

fn verify_driver_adapter(
    request: &DriverAdapterRequest<'_>,
) -> ProviderResult<Option<DriverAdapterVerification>> {
    Ok(esp32s31_verification::verify_driver_adapter(request)?)
}

fn verify_semantic_contract(request: &SemanticContractRequest<'_>) -> ProviderResult<Option<bool>> {
    Ok(esp32s31_verification::verify_semantic_contract(request)?)
}

fn verify_named_contract(
    name: &str,
    svd: &open_radio_vendor_binary_workbench::MmioMap,
    vendor_artifact: &Path,
    vendor_companion: &Path,
) -> ProviderResult<SemanticVerificationReport> {
    use esp32s31_verification::verification;

    let report = match name {
        "channel" => verification::verify_esp32s31_channel(svd, vendor_artifact, vendor_companion)?,
        "rf-init" => verification::verify_esp32s31_rf_init(svd, vendor_artifact, vendor_companion)?,
        "bluetooth-tx-power" => verification::verify_esp32s31_bluetooth_tx_power(
            svd,
            vendor_artifact,
            vendor_companion,
        )?,
        "bluetooth-tx-gain-init" => verification::verify_esp32s31_bluetooth_tx_gain_init(
            svd,
            vendor_artifact,
            vendor_companion,
        )?,
        "baseband-init" => {
            verification::verify_esp32s31_baseband_init(svd, vendor_artifact, vendor_companion)?
        }
        "register-init" => {
            verification::verify_esp32s31_register_init(svd, vendor_artifact, vendor_companion)?
        }
        _ => return Err(format!("ESP32-S31 provider has no contract {name:?}").into()),
    };
    Ok(report)
}

static KNOWLEDGE_PROVIDERS: &[KnowledgeProviderDescriptor] = &[KnowledgeProviderDescriptor {
    id: "esp32s31-radio-knowledge-v1",
    contracts: &esp32s31_knowledge::CONTRACTS,
    riscv: Some(&esp32s31_knowledge::RISCV_HARNESS),
}];

static VERIFICATION_PROVIDERS: &[VerificationProviderDescriptor] =
    &[VerificationProviderDescriptor {
        id: "esp32s31-radio-verification-v1",
        verify_driver_adapter,
        verify_semantic_contract,
        driver_adapter_evidence_sources: esp32s31_verification::driver_adapter_evidence_sources,
        semantic_contract_evidence_sources:
            esp32s31_verification::semantic_contract_evidence_sources,
        verify_named_contract,
    }];

static PROVIDERS: ProviderRegistry = ProviderRegistry {
    knowledge: KNOWLEDGE_PROVIDERS,
    verification: VERIFICATION_PROVIDERS,
};

fn main() -> std::process::ExitCode {
    open_radio_vendor_binary_workbench::main_entry_with_providers(&PROVIDERS)
}
