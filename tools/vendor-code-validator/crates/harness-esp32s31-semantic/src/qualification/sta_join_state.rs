//! Qualification of the Rust STA management-state replacement for the
//! infrastructure-only Authentication/Association slice of
//! `ieee80211_sta_new_state`.
//!
//! The vendor root also owns NVS/configuration lookups, power/coexistence,
//! mesh branches, logging, callback dispatch and the complete private
//! interface/node layout. None of those are imported into the open runtime.
//! This adapter pins the immutable vendor branch boundary and executes the
//! allocation-free Rust protocol owners as the declared replacement.

use std::{collections::BTreeMap, path::Path};

use open_esp_radio_ieee80211::station::{
    STA_AUTHENTICATION_ATTEMPT_LIMIT, STA_RESPONSE_TIMEOUT_MS, StaAssociationRetrySchedule,
};
use open_radio_vendor_validator_semantic::{
    DriverAdapterQualification, EffectDisposition, EffectPolicy, EffectSelector, OmissionReason,
    PlatformOperation,
};

use crate::{MmioRegisterMap, Result, artifact, artifact_sha256, execution};

use super::{code_closure_sha256, inventory_symbol_sha256};

const SYMBOL: &str = "ieee80211_sta_new_state";
const MEMBER: &str = "ieee80211_sta.o";
const RUST_ADAPTER_ID: &str = "esp32s31-sta-join-state-v1";
const VENDOR_SYMBOL_SIZE: usize = 0x0e7c;
const VENDOR_STATE_TIMEOUT_MS: u32 = STA_RESPONSE_TIMEOUT_MS;

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    scenario: u32,
    expected_return: u32,
}

const CASES: &[Case] = &[
    Case {
        name: "open-authentication-success",
        scenario: 0,
        expected_return: 0xa001_0124,
    },
    Case {
        name: "open-authentication-timeout-limit",
        scenario: 1,
        expected_return: 0xa103_0126,
    },
    Case {
        name: "association-success",
        scenario: 2,
        expected_return: 0xb123_0124,
    },
    Case {
        name: "association-exact-deadline",
        scenario: 3,
        expected_return: 0xb107_03e8,
    },
];

fn required_policy() -> BTreeMap<EffectSelector, EffectDisposition> {
    [
        (
            EffectSelector::StateWrite {
                width: 16,
                field: "sta-management.sequence".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 16,
                field: "sta-authentication.attempt".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 16,
                field: "sta-association.retry-ordinal".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 32,
                field: "sta-state.deadline-ms".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::RtosSchedulingAdapter,
            },
            EffectDisposition::PlatformProvidedService {
                service: "embassy-sta-state-deadline".to_owned(),
            },
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::DebugDiagnostic,
            },
            EffectDisposition::AllowedOmission(OmissionReason::DebugDiagnostic),
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::NvsCalibrationCache,
            },
            EffectDisposition::PlatformProvidedInput {
                input: "caller-owned-sta-configuration".to_owned(),
            },
        ),
        (
            EffectSelector::PlatformProvidedInput {
                input: "caller-owned-sta-configuration".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PlatformProvidedService {
                service: "embassy-sta-state-deadline".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "sta-authenticated".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PublishedEvent {
                event: "sta-associated".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "infrastructure-sta-only".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "mesh-and-power-state-transitions-disabled".to_owned(),
            },
            EffectDisposition::Required,
        ),
    ]
    .into_iter()
    .collect()
}

fn validate_policy(policy: &EffectPolicy) -> Result<()> {
    let actual = policy
        .rules()
        .map(|(selector, disposition)| (selector.clone(), disposition.clone()))
        .collect::<BTreeMap<_, _>>();
    let expected = required_policy();
    if actual != expected {
        return Err(format!(
            "{SYMBOL} STA-join policy differs from the closed architectural replacement:\nexpected {expected:#?}\nactual {actual:#?}"
        )
        .into());
    }
    Ok(())
}

fn require_relocation(
    symbol: &artifact::ArtifactSymbolDefinition,
    address: u32,
    kind: artifact::RelocationKind,
    target: &str,
) -> Result<()> {
    let actual = symbol
        .relocation(address, kind)
        .map(|relocation| relocation.symbol.as_str());
    if actual != Some(target) {
        return Err(format!(
            "{MEMBER}::{SYMBOL} relocation at {address:#05x} changed: expected {kind:?} {target}, got {actual:?}"
        )
        .into());
    }
    Ok(())
}

fn require_bytes(
    symbol: &artifact::ArtifactSymbolDefinition,
    offset: usize,
    bytes: &[u8],
    label: &str,
) -> Result<()> {
    if symbol.bytes.get(offset..offset + bytes.len()) != Some(bytes) {
        return Err(format!(
            "{MEMBER}::{SYMBOL} {label} instruction image changed at {offset:#05x}"
        )
        .into());
    }
    Ok(())
}

fn validate_vendor_shape(vendor_inventory: &Path) -> Result<String> {
    let symbols = artifact::load_symbols(vendor_inventory, SYMBOL)?;
    let symbol = symbols
        .iter()
        .find(|candidate| candidate.name == SYMBOL && candidate.member.as_deref() == Some(MEMBER))
        .ok_or_else(|| format!("{MEMBER}::{SYMBOL} is absent from the caller-owned inventory"))?;
    if symbol.address != 0 || symbol.bytes.len() != VENDOR_SYMBOL_SIZE {
        return Err(format!(
            "{MEMBER}::{SYMBOL} boundary changed: address={:#x}, size={:#x}",
            symbol.address,
            symbol.bytes.len()
        )
        .into());
    }

    // Ordinary Open Authentication has two non-mesh entry sites; ordinary
    // Association has one. Their subtype/argument materialization is checked
    // separately from the relocation so a call-site-preserving mutation
    // cannot silently select another management operation.
    require_relocation(
        symbol,
        0x0c62,
        artifact::RelocationKind::Call,
        "ieee80211_send_mgmt",
    )?;
    require_relocation(
        symbol,
        0x0c90,
        artifact::RelocationKind::Call,
        "ieee80211_send_mgmt",
    )?;
    require_relocation(
        symbol,
        0x0d4c,
        artifact::RelocationKind::Call,
        "ieee80211_send_mgmt",
    )?;
    require_bytes(
        symbol,
        0x0c5a,
        &[0x05, 0x46, 0x93, 0x05, 0x00, 0x0b],
        "Open Authentication subtype/sequence",
    )?;
    require_bytes(
        symbol,
        0x0d46,
        &[0x01, 0x46, 0x81, 0x45, 0x4e, 0x85],
        "Association request arguments",
    )?;

    // Both ordinary branches install a callback and reach the same exact
    // 1,000-ms OS timer arm. Mesh expiration remains outside this adapter.
    require_relocation(
        symbol,
        0x0a9a,
        artifact::RelocationKind::Hi20,
        "cnx_auth_timeout",
    )?;
    require_relocation(
        symbol,
        0x0d72,
        artifact::RelocationKind::Hi20,
        "cnx_assoc_timeout",
    )?;
    require_relocation(
        symbol,
        0x0d02,
        artifact::RelocationKind::Lo12I,
        "g_osi_funcs_p",
    )?;
    require_bytes(
        symbol,
        0x0d08,
        &[0x93, 0x05, 0x80, 0x3e],
        "one-second state deadline",
    )?;

    Ok(format!(
        "vendor-member {MEMBER}\nvendor-size {VENDOR_SYMBOL_SIZE:#x}\nvendor-auth-call-sites 0xc62,0xc90\nvendor-association-call-site 0xd4c\nvendor-state-deadline-ms {VENDOR_STATE_TIMEOUT_MS}\nvendor-timer-callbacks cnx_auth_timeout,cnx_assoc_timeout\n"
    ))
}

fn rust_scenario(case: Case, stack_fill: u8) -> execution::Scenario {
    execution::Scenario {
        arguments: vec![case.scenario],
        private_stack_fill: Some(stack_fill),
        max_steps: 2_000_000,
        ..execution::Scenario::default()
    }
}

fn case_matches(result: &execution::ExecutionResult, case: Case) -> bool {
    result.return_value == case.expected_return && result.events.is_empty()
}

#[allow(
    clippy::too_many_arguments,
    reason = "qualification binds caller-owned vendor/Rust artifacts and one closed effect policy"
)]
pub fn qualify_esp32s31_sta_join_state(
    svd: &MmioRegisterMap,
    vendor_inventory: Option<&Path>,
    vendor_artifact: &Path,
    vendor_companion: Option<&Path>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_symbol: &str,
    policy: &EffectPolicy,
    print_oracles: bool,
) -> Result<DriverAdapterQualification> {
    validate_policy(policy)?;
    let vendor_inventory = vendor_inventory
        .ok_or("STA join qualification requires the caller-owned raw libnet80211 inventory")?;
    if print_oracles {
        let vendor_inventory_digest = artifact_sha256(vendor_inventory)?;
        println!(
            "ORACLE\tlibnet80211\t{}\tsha256={vendor_inventory_digest}",
            vendor_inventory.display()
        );
    }
    let vendor_proof = validate_vendor_shape(vendor_inventory)?;

    let mut vendor_image = execution::ExecutableImage::load(vendor_artifact)?;
    if let Some(companion) = vendor_companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }
    let vendor_inventory_digest = inventory_symbol_sha256(vendor_inventory, Some(MEMBER), SYMBOL)?;
    let vendor_code_digest = code_closure_sha256(&vendor_image, SYMBOL)?;
    let rust_code_digest = code_closure_sha256(&rust_image, rust_symbol)?;
    let mut canonical = format!(
        "driver-adapter {RUST_ADAPTER_ID}\nvendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    );
    canonical.push_str(&vendor_proof);
    canonical.push_str("scope infrastructure-sta-authentication-and-association-state\n");
    canonical.push_str("executor production-sta-join-runner\n");
    canonical.push_str("platform-service embassy-sta-state-deadline\n");
    canonical.push_str("deadline-order absolute-monotonic-rx-before-timeout\n");
    canonical.push_str("association-success-rx-ownership live-handoff\n");
    canonical.push_str("platform-input caller-owned-sta-configuration\n");
    canonical.push_str("omission nvs,power,coex,mesh,logging,callbacks\n");
    canonical.push_str(&format!(
        "source-owned-auth-attempt-limit {STA_AUTHENTICATION_ATTEMPT_LIMIT}\nsource-owned-association-retry-ms {}\n",
        StaAssociationRetrySchedule::INTERVAL_MS,
    ));

    let mut matched = true;
    for case in CASES {
        let first = execution::execute(&rust_image, svd, rust_symbol, rust_scenario(*case, 0))?;
        let second = execution::execute(&rust_image, svd, rust_symbol, rust_scenario(*case, 0xa5))?;
        let padding_independent =
            first.return_value == second.return_value && first.events == second.events;
        let case_matched = case_matches(&first, *case) && padding_independent;
        matched &= case_matched;
        canonical.push_str(&format!(
            "scenario {} return={:#010x} events={} padding-independent={}\n",
            case.name,
            first.return_value,
            first.events.len(),
            padding_independent,
        ));
        println!(
            "STA-JOIN-CASE\t{}\t{}\treturn={:#010x}\tevents={}\tsteps={}\tpadding-independent={}",
            case.name,
            if case_matched { "MATCH" } else { "MISMATCH" },
            first.return_value,
            first.events.len(),
            first.steps,
            padding_independent,
        );
    }
    println!(
        "STA-JOIN-SUMMARY\t{SYMBOL}\t{}\tscenarios={}\tscope=infrastructure-sta-state",
        if matched { "MATCH" } else { "MISMATCH" },
        CASES.len(),
    );
    Ok(DriverAdapterQualification { matched, canonical })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_separates_vendor_deadline_from_source_owned_retry_policy() {
        assert_eq!(STA_RESPONSE_TIMEOUT_MS, VENDOR_STATE_TIMEOUT_MS);
        assert_eq!(STA_AUTHENTICATION_ATTEMPT_LIMIT, 3);
        assert_eq!(StaAssociationRetrySchedule::INTERVAL_MS, 160);
        assert_eq!(required_policy().len(), 13);
    }

    #[test]
    fn exact_return_and_observable_match_rejects_mutations() {
        let case = CASES[0];
        let exact = execution::ExecutionResult {
            events: Vec::new(),
            timeline: Vec::new(),
            return_value: case.expected_return,
            steps: 0,
            branches: Default::default(),
            ordered_branches: Vec::new(),
            calls: Default::default(),
            ordered_calls: Vec::new(),
            indirect_calls: Default::default(),
            memory_changes: Vec::new(),
            initial_memory: Default::default(),
            persistent_memory: Default::default(),
        };
        assert!(case_matches(&exact, case));

        let mut wrong_return = exact.clone();
        wrong_return.return_value ^= 1;
        assert!(!case_matches(&wrong_return, case));

        let mut extra_event = exact;
        extra_event
            .events
            .push(execution::ExecutionEvent::DelayMicros(1));
        assert!(!case_matches(&extra_event, case));
    }
}
