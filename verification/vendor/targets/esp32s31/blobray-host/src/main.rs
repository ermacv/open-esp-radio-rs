use blobray::{KnowledgeProviderDescriptor, ProviderRegistry};
use open_radio_vendor_knowledge_esp32s31 as esp32s31_knowledge;

static KNOWLEDGE_PROVIDERS: &[KnowledgeProviderDescriptor] = &[KnowledgeProviderDescriptor {
    id: "esp32s31-radio-knowledge-v1",
    contracts: &esp32s31_knowledge::CONTRACTS,
    riscv: Some(&esp32s31_knowledge::RISCV_HARNESS),
}];

static PROVIDERS: ProviderRegistry = ProviderRegistry {
    knowledge: KNOWLEDGE_PROVIDERS,
};

fn main() -> std::process::ExitCode {
    blobray::main_entry_with_providers(&PROVIDERS)
}
