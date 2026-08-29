use blobray::{KnowledgeProviderDescriptor, ProviderRegistry};
use open_radio_vendor_chip_knowledge_esp32s31_rev0 as chip_knowledge;
use open_radio_vendor_knowledge_esp32s31 as esp32s31_knowledge;

const CHIP_PROVIDER_ID: &str = "esp32s31-rev0-chip-knowledge-v1";
const PROJECT_PROVIDER_ID: &str = "esp32s31-radio-knowledge-v1";

static KNOWLEDGE_PROVIDERS: &[KnowledgeProviderDescriptor] = &[
    KnowledgeProviderDescriptor {
        id: CHIP_PROVIDER_ID,
        extends: None,
        analysis_cache_revision: 1,
        contracts: &chip_knowledge::CONTRACTS,
        riscv: Some(&chip_knowledge::RISCV_HARNESS),
    },
    KnowledgeProviderDescriptor {
        id: PROJECT_PROVIDER_ID,
        extends: Some(CHIP_PROVIDER_ID),
        // Revision 8 adds caller-memory models for reviewed BLE crypto outputs.
        // The overlay harness is precomposed and its contracts are a checked
        // superset of the rev0 chip provider.
        analysis_cache_revision: 8,
        contracts: &esp32s31_knowledge::CONTRACTS,
        riscv: Some(&esp32s31_knowledge::RISCV_HARNESS),
    },
];

static PROVIDERS: ProviderRegistry = ProviderRegistry {
    knowledge: KNOWLEDGE_PROVIDERS,
};

fn main() -> std::process::ExitCode {
    blobray::main_entry_with_providers(&PROVIDERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_composes_one_explicit_chip_root_and_project_overlay() {
        PROVIDERS.validate().unwrap();
        assert_eq!(KNOWLEDGE_PROVIDERS.len(), 2);
        assert_eq!(KNOWLEDGE_PROVIDERS[0].id, CHIP_PROVIDER_ID);
        assert_eq!(KNOWLEDGE_PROVIDERS[0].extends, None);
        assert_eq!(KNOWLEDGE_PROVIDERS[1].id, PROJECT_PROVIDER_ID);
        assert_eq!(KNOWLEDGE_PROVIDERS[1].extends, Some(CHIP_PROVIDER_ID));

        assert!(
            chip_knowledge::CONTRACTS
                .entry_contract("esp32s31-phy-registered")
                .is_none()
        );
        assert!(
            esp32s31_knowledge::CONTRACTS
                .entry_contract("esp32s31-phy-registered")
                .is_some()
        );
    }

    #[test]
    fn exact_rom_identity_hook_remains_project_local_without_applicability_evidence() {
        let address = esp32s31_knowledge::wide_signed_divide_target_address();
        assert!(!(chip_knowledge::SUMMARIES.secondary_return_target)(
            address
        ));
        assert!((esp32s31_knowledge::RISCV_HARNESS
            .summaries
            .secondary_return_target)(address));

        let chip_crystal =
            (chip_knowledge::SUMMARIES.direct_external_semantic)("rtc_clk_xtal_freq_get").unwrap();
        let composed_crystal = (esp32s31_knowledge::RISCV_HARNESS
            .summaries
            .direct_external_semantic)("rtc_clk_xtal_freq_get")
        .unwrap();
        assert_eq!(chip_crystal, composed_crystal);
    }

    #[test]
    fn reviewed_manifests_select_the_matching_base_and_overlay_ids() {
        let chip: toml_edit::Document<String> =
            include_str!("../../../../chips/esp32s31/chip.toml")
                .parse()
                .unwrap();
        let project: toml_edit::Document<String> =
            include_str!("../../vendor-project.toml").parse().unwrap();
        assert_eq!(chip["knowledge-provider"].as_str(), Some(CHIP_PROVIDER_ID));
        assert_eq!(
            project["analysis-provider"].as_str(),
            Some(PROJECT_PROVIDER_ID)
        );
    }
}
