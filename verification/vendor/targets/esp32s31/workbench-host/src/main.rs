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
    _svd: &open_radio_vendor_binary_workbench::MmioMap,
    _vendor_artifact: &Path,
    _vendor_companion: &Path,
) -> ProviderResult<SemanticVerificationReport> {
    Err(format!("ESP32-S31 provider has no self-verdict semantic contract {name:?}").into())
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
