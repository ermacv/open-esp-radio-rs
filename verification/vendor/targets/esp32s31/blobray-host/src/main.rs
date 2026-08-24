use blobray::{KnowledgeProviderDescriptor, ProviderRegistry};
use open_radio_vendor_knowledge_esp32s31 as esp32s31_knowledge;

static KNOWLEDGE_PROVIDERS: &[KnowledgeProviderDescriptor] = &[KnowledgeProviderDescriptor {
    id: "esp32s31-radio-knowledge-v1",
    // Revision 2 adds the reviewed `ets_printf` diagnostic boundary used by
    // the BLE controller lifecycle image. This is deliberately independent
    // of the stable provider ID selected by the investigation project.
    // Revision 3 adds the reviewed ESP-IDF Bluetooth NPL/external-function
    // boundaries. The weak archive fallbacks assert only because the platform
    // callback tables are deliberately absent from the linked research image.
    // Revision 4 gives the reviewed controller allocator a fresh writable
    // symbolic object instead of an untyped integer return.
    analysis_cache_revision: 4,
    contracts: &esp32s31_knowledge::CONTRACTS,
    riscv: Some(&esp32s31_knowledge::RISCV_HARNESS),
}];

static PROVIDERS: ProviderRegistry = ProviderRegistry {
    knowledge: KNOWLEDGE_PROVIDERS,
};

fn main() -> std::process::ExitCode {
    blobray::main_entry_with_providers(&PROVIDERS)
}
