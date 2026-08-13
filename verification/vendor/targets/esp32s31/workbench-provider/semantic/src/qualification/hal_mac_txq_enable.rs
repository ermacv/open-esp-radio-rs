//! Verification of the owned ordinary-TX publication boundary.
//!
//! The vendor function mixes three responsibilities: it first publishes the
//! ordinary queue by setting CONTROL.ENABLE|VALID, then mutates a private
//! scheduler context for HE trigger-based traffic, and finally updates vendor
//! instrumentation. The adapter concretely replays the non-HE vendor path and
//! executes `TxSlot::submit_legacy`, including the real DMA typestate and final
//! `RadioRegisters` doorbell.  It therefore proves a reviewed refinement, not
//! whole-function equivalence.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};

use open_radio_vendor_backend_riscv::trace_binary_symbol;
use open_radio_vendor_semantics::{
    DraftReferenceEvent, DriverAdapterVerification, EffectDisposition, EffectPolicy,
    EffectSelector, ExpressionOperation, MemoryAccess, OmissionReason, PlatformOperation,
    SymbolicValue, evaluate_for_input,
};

use crate::{MmioMap, Result, StructuralPointerContext, artifact, execution, execution_model};

use super::{code_closure_sha256, inventory_symbol_sha256};

const SYMBOL: &str = "hal_mac_txq_enable";
const CONTROL: [u32; 4] = [0x2010_4d40, 0x2010_4d50, 0x2010_4d60, 0x2010_4d70];
const ENABLE_VALID: u32 = 0xc000_0000;

fn required_policy() -> BTreeMap<EffectSelector, EffectDisposition> {
    let mut rules = CONTROL
        .into_iter()
        .flat_map(|address| {
            [
                (
                    EffectSelector::MmioRead { width: 32, address },
                    EffectDisposition::Required,
                ),
                (
                    EffectSelector::MmioWrite { width: 32, address },
                    EffectDisposition::Required,
                ),
            ]
        })
        .collect::<BTreeMap<_, _>>();
    rules.insert(
        EffectSelector::PlatformProvidedService {
            service: "embassy-tx-queue-ownership".to_owned(),
        },
        EffectDisposition::Required,
    );
    rules.insert(
        EffectSelector::PlatformCall {
            operation: PlatformOperation::DebugDiagnostic,
        },
        EffectDisposition::AllowedOmission(OmissionReason::UnusedInstrumentation),
    );
    rules.insert(
        EffectSelector::InitializationPrerequisite {
            prerequisite: "he-trigger-based-tx-disabled".to_owned(),
        },
        EffectDisposition::Required,
    );
    rules
}

fn validate_policy(policy: &EffectPolicy) -> Result<()> {
    let actual = policy
        .rules()
        .map(|(selector, disposition)| (selector.clone(), disposition.clone()))
        .collect::<BTreeMap<_, _>>();
    let expected = required_policy();
    if actual != expected {
        return Err(format!(
        "hal_mac_txq_enable owned-publication policy differs from the closed adapter boundary:\nexpected {expected:#?}\nactual {actual:#?}"
        )
        .into());
    }
    Ok(())
}

fn validate_domain(
    address: &SymbolicValue,
    registers: &[open_radio_vendor_semantics::IndexedMmioRegister],
    guard: Option<&open_radio_vendor_semantics::IndexedMmioGuard>,
) -> Result<()> {
    let expected_registers = CONTROL.into_iter().collect::<BTreeSet<_>>();
    let actual_registers = registers
        .iter()
        .map(|register| register.address)
        .collect::<BTreeSet<_>>();
    if actual_registers != expected_registers {
        return Err(format!(
            "{SYMBOL} register slice addresses differ: expected {expected_registers:#x?}, got {actual_registers:#x?}"
        )
        .into());
    }
    let Some(guard) = guard else {
        return Err(format!("{SYMBOL} indexed register slice has no finite queue guard").into());
    };
    if guard.selector != SymbolicValue::input(0) || guard.maximum != 3 {
        return Err(format!("{SYMBOL} indexed queue guard differs: {guard:?}").into());
    }
    for queue in 0..4_u32 {
        let actual = evaluate_for_input(address, 0, queue)
            .ok_or_else(|| format!("{SYMBOL} queue {queue} address is not evaluable"))?;
        let expected = CONTROL[3 - queue as usize];
        if actual != expected {
            return Err(format!(
                "{SYMBOL} queue {queue} selects {actual:#010x} instead of {expected:#010x}"
            )
            .into());
        }
    }
    Ok(())
}

fn validate_vendor_register_slice(vendor_inventory: &Path, svd: &MmioMap) -> Result<String> {
    let symbols = artifact::load_code_symbols(
        vendor_inventory,
        SYMBOL,
        artifact::CodeSymbolSelection::Exported,
    )?;
    let symbol = symbols
        .iter()
        .find(|candidate| candidate.name == SYMBOL)
        .ok_or_else(|| format!("{SYMBOL} was not found in caller-owned vendor inventory"))?;
    let call_targets = symbol
        .relocations
        .iter()
        .filter(|relocation| {
            matches!(
                relocation.kind,
                artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
            )
        })
        .map(|relocation| relocation.symbol.as_str())
        .collect::<Vec<_>>();
    let expected_calls = [
        "GetAccess",
        "wifi_he_get_hetb_tid_bitmap",
        "is_use_muedca",
        "esp_test_tx_enab_statistics",
    ];
    if call_targets != expected_calls {
        return Err(format!(
            "{SYMBOL} suffix call boundary differs: expected {expected_calls:?}, got {call_targets:?}"
        )
        .into());
    }

    let trace = trace_binary_symbol(
        symbol,
        svd,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )?;
    let [read, write] = trace.reference_events.as_slice() else {
        return Err(format!(
            "{SYMBOL} must expose the indexed CONTROL transaction before GetAccess, got {:?}",
            trace.reference_events
        )
        .into());
    };
    let (
        DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            width: 32,
            address: read_address,
            registers: read_registers,
            guard: read_guard,
            value: None,
        },
        DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Write,
            width: 32,
            address: write_address,
            registers: write_registers,
            guard: write_guard,
            value: Some(write_value),
        },
    ) = (read, write)
    else {
        return Err(format!("{SYMBOL} register slice has a different event shape").into());
    };
    if read_address != write_address
        || read_registers != write_registers
        || read_guard != write_guard
    {
        return Err(format!("{SYMBOL} read/write indexed domains differ").into());
    }
    validate_domain(read_address, read_registers, read_guard.as_ref())?;
    let expected_value = SymbolicValue::IndexedRegisterImage {
        read_token: 0,
        and_mask: !ENABLE_VALID,
        or_mask: ENABLE_VALID,
    };
    if *write_value != expected_value {
        return Err(format!(
            "{SYMBOL} publishes {}, expected {}",
            write_value.canonical(),
            expected_value.canonical()
        )
        .into());
    }

    // `GetAccess(queue)` is an unresolved vendor-private ABI in the raw
    // archive. Treat its returned `a0` as the reviewed input of the suffix,
    // while retaining the relocation/call-list check above as the boundary
    // evidence. This avoids inventing a concrete address or pretending that
    // the generic backend knows the private scheduler context layout.
    const POST_GET_ACCESS: usize = 0x2a;
    let suffix = artifact::ArtifactSymbolDefinition {
        member: symbol.member.clone(),
        name: format!("{SYMBOL}::post-GetAccess"),
        address: symbol.address + POST_GET_ACCESS as u64,
        bytes: symbol.bytes[POST_GET_ACCESS..].to_vec(),
        addresses_resolved: symbol.addresses_resolved,
        memory_regions: symbol.memory_regions.clone(),
        relocations: symbol
            .relocations
            .iter()
            .filter(|relocation| relocation.address >= POST_GET_ACCESS as u32)
            .cloned()
            .collect(),
    };
    let suffix_trace = trace_binary_symbol(
        &suffix,
        svd,
        &BTreeMap::new(),
        &StructuralPointerContext::default(),
        None,
    )?;
    let [
        context_flags_read,
        context_owner_read,
        context_flags_write,
        scheduler_owner_read,
        scheduler_flags_read,
    ] = suffix_trace.reference_events.as_slice()
    else {
        return Err(format!(
            "{SYMBOL} post-GetAccess suffix must expose the five reviewed queue/scheduler-context effects, got {:?}",
            suffix_trace.reference_events
        )
        .into());
    };
    let context_flags_address = SymbolicValue::Expression {
        operation: ExpressionOperation::Add,
        left: std::sync::Arc::new(SymbolicValue::input(0)),
        right: std::sync::Arc::new(SymbolicValue::Constant(40)),
    };
    let expected_context_value = SymbolicValue::MemoryImage {
        read_token: 0,
        and_mask: 0xfd,
        or_mask: 0,
    };
    let scheduler_owner_address = SymbolicValue::Expression {
        operation: ExpressionOperation::Add,
        left: std::sync::Arc::new(SymbolicValue::MemoryImage {
            read_token: 1,
            and_mask: u32::MAX,
            or_mask: 0,
        }),
        right: std::sync::Arc::new(SymbolicValue::Constant(52)),
    };
    let scheduler_flags_address = SymbolicValue::Expression {
        operation: ExpressionOperation::Add,
        left: std::sync::Arc::new(SymbolicValue::MemoryImage {
            read_token: 2,
            and_mask: u32::MAX,
            or_mask: 0,
        }),
        right: std::sync::Arc::new(SymbolicValue::Constant(47)),
    };
    if !matches!(
        context_flags_read,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address,
            value: None,
            ..
        } if address == &context_flags_address
    ) || !matches!(
        context_owner_read,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address,
            value: None,
            ..
        } if address == &SymbolicValue::input(0)
    ) || !matches!(
        context_flags_write,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            address,
            value: Some(value),
            ..
        } if address == &context_flags_address && value == &expected_context_value
    ) || !matches!(
        scheduler_owner_read,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address,
            value: None,
            ..
        } if address == &scheduler_owner_address
    ) || !matches!(
        scheduler_flags_read,
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address,
            value: None,
            ..
        } if address == &scheduler_flags_address
    ) {
        return Err(format!(
            "{SYMBOL} vendor queue/scheduler-context suffix differs from the reviewed replacement boundary: {:?}",
            suffix_trace.reference_events
        )
        .into());
    }
    if suffix_trace.blockers.is_empty() && suffix_trace.reference_blockers.is_empty() {
        return Err(format!(
            "{SYMBOL} unexpectedly became a flat leaf; the register-slice boundary must be reviewed"
        )
        .into());
    }

    Ok(format!(
        "vendor-member {}\nqueue-map a0=0..3 -> CONTROL[3-a0]\nregister-transaction read32; write32 read0|{ENABLE_VALID:#010x}\nreplaced-context-effect access-flags &= 0xfd\nexcluded-he-scheduler-context owner=queue.owner scheduler=owner+0x34 flags=scheduler+0x2f\nsuffix-calls {}\n",
        symbol.member.as_deref().unwrap_or("<linked>"),
        expected_calls.join(",")
    ))
}

const VENDOR_CONTEXT: u32 = 0x3ffd_2000;
const VENDOR_OWNER: u32 = 0x3ffd_2100;
const VENDOR_SCHEDULER: u32 = 0x3ffd_2200;
const TX_STORAGE_SYMBOL: &str = "OPEN_LIBPP_TX_STORAGE";
const TX_STORAGE_BOUND: u32 = 128;
const EXPECTED_DESCRIPTOR_WORD0: u32 = 64 | (32 << 14) | 0xc000_0000;

fn control_events(result: &execution::ExecutionResult) -> Vec<(bool, u32, u32)> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            execution::ExecutionEvent::Read {
                width: 32,
                address,
                value,
                ..
            } if CONTROL.contains(address) => Some((false, *address, *value)),
            execution::ExecutionEvent::Write {
                width: 32,
                address,
                value,
                ..
            } if CONTROL.contains(address) => Some((true, *address, *value)),
            _ => None,
        })
        .collect()
}

fn final_ram_byte(result: &execution::ExecutionResult, address: u32) -> Option<u8> {
    let mut value = result.initial_memory.get(&address).copied();
    for event in &result.timeline {
        let execution::ExecutionTimelineEvent::RamWrite {
            width,
            address: write_address,
            value: written,
            ..
        } = event
        else {
            continue;
        };
        let bytes = u32::from(*width / 8);
        if address >= *write_address && address < write_address.saturating_add(bytes) {
            value = Some((written >> ((address - write_address) * 8)) as u8);
        }
    }
    value
}

fn vendor_scenario(queue: u32, initial: u32) -> execution::Scenario {
    let mut scenario = execution::Scenario {
        arguments: vec![queue],
        mmio_initial: BTreeMap::from([(CONTROL[3 - queue as usize], initial)]),
        call_responses: BTreeMap::from([
            (
                "GetAccess".to_owned(),
                VecDeque::from([execution::ModeledCallResponse::scalar(VENDOR_CONTEXT)]),
            ),
            (
                "esp_test_tx_enab_statistics".to_owned(),
                VecDeque::from([execution::ModeledCallResponse::scalar(0)]),
            ),
        ]),
        max_steps: 4_000,
        ..execution::Scenario::default()
    };
    crate::write_ram_word(&mut scenario, VENDOR_CONTEXT, VENDOR_OWNER);
    crate::write_ram_word(&mut scenario, VENDOR_CONTEXT + 0x28, 0xff);
    crate::write_ram_word(&mut scenario, VENDOR_OWNER + 0x34, VENDOR_SCHEDULER);
    // A zero scheduler mode takes the reviewed non-HE path and avoids both
    // trigger-bitmap and MU-EDCA behavior.
    scenario.memory_initial.insert(VENDOR_SCHEDULER + 0x2f, 0);
    scenario
}

fn vendor_contract_reason(
    result: &execution::ExecutionResult,
    queue: u32,
    initial: u32,
) -> Option<String> {
    let address = CONTROL[3 - queue as usize];
    let expected = [
        (false, address, initial),
        (true, address, initial | ENABLE_VALID),
    ];
    let actual = control_events(result);
    if actual != expected {
        return Some(format!(
            "vendor queue edge differs: expected {expected:#x?}, actual {actual:#x?}"
        ));
    }
    let calls = result
        .ordered_calls
        .iter()
        .map(|call| call.symbol.as_str())
        .collect::<Vec<_>>();
    if calls != ["GetAccess", "esp_test_tx_enab_statistics"] {
        return Some(format!("vendor non-HE call path differs: {calls:?}"));
    }
    if final_ram_byte(result, VENDOR_CONTEXT + 0x28) != Some(0xfd) {
        return Some("vendor replay did not clear the reviewed private access flag".to_owned());
    }
    None
}

fn production_scenario(svd: &MmioMap, queue: u32, active: bool) -> execution::Scenario {
    let control = CONTROL[3 - queue as usize];
    let initial_control = if active { ENABLE_VALID } else { 0 };
    let mmio_initial = svd
        .registers
        .iter()
        .filter(|register| register.address != control)
        .map(|register| (register.address, 0))
        .collect::<BTreeMap<_, _>>();
    execution::Scenario {
        arguments: vec![queue],
        mmio_initial,
        // `TxSlot::submit_legacy` prepares CONTROL, then deliberately reads
        // it back before publishing ENABLE|VALID.  An `mmio_initial` entry is
        // an immutable scripted read, so use the neutral stateful register
        // model for this one ordinary read/write register.  SelfClearing with
        // an all-bit store mask and no command bits is exactly a flat R/W
        // register and keeps the generic executor free of ESP32-S31 policy.
        device_models: vec![Arc::new(execution_model::DeviceModelSpec::SelfClearing {
            id: format!("txq{queue}-control"),
            address: control,
            width: 32,
            initial_value: initial_control,
            store_mask: u32::MAX,
            command_mask: 0,
        })],
        max_steps: 100_000,
        ..execution::Scenario::default()
    }
}

fn valid_owned_control_edge(controls: &[(bool, u32, u32)], address: u32) -> bool {
    let [
        (false, actual, initial),
        (true, written_address, prepared),
        (false, prepared_check_address, prepared_check),
        (false, reread_address, reread),
        (true, final_address, published),
    ] = controls
    else {
        return false;
    };
    *actual == address
        && *written_address == address
        && *prepared_check_address == address
        && *reread_address == address
        && *final_address == address
        && *initial == 0
        && *prepared_check == *prepared
        && *reread == *prepared
        && *prepared & ENABLE_VALID == 0
        && *published == *prepared | ENABLE_VALID
}

fn production_contract_reason(
    result: &execution::ExecutionResult,
    queue: u32,
    active: bool,
    storage: u32,
) -> Option<String> {
    let address = CONTROL[3 - queue as usize];
    let controls = control_events(result);
    if active {
        if result.return_value != 0
            || controls != [(false, address, ENABLE_VALID)]
            || result.events.iter().any(|event| {
                matches!(event, execution::ExecutionEvent::Write { address: actual, .. } if *actual == address)
            })
        {
            return Some(format!(
                "active queue was partially published: return={:#010x}, controls={controls:#x?}",
                result.return_value
            ));
        }
        return None;
    }
    if result.return_value != EXPECTED_DESCRIPTOR_WORD0 {
        return Some(format!(
            "production probe did not finish HardwareOwned with the production descriptor image: {:#010x}",
            result.return_value
        ));
    }
    if !valid_owned_control_edge(&controls, address) {
        return Some(format!(
            "production queue publication edge differs: {controls:#x?}"
        ));
    }
    let published = controls[4].2;
    let final_timeline_index = result.timeline.iter().rposition(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::Observable(execution::ExecutionEvent::Write {
                width: 32,
                address: actual,
                value,
                ..
            }) if *actual == address && *value == published
        )
    });
    let ownership_index = result.timeline.iter().rposition(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::RamWrite {
                address,
                value: 2,
                ..
            } if *address >= storage && *address < storage.saturating_add(TX_STORAGE_BOUND)
        )
    });
    if !matches!((ownership_index, final_timeline_index), (Some(ownership), Some(final_edge)) if ownership < final_edge)
    {
        return Some("HardwareOwned state was not recorded before the queue doorbell".to_owned());
    }
    None
}

#[allow(
    clippy::too_many_arguments,
    reason = "verification binds caller-owned vendor and Rust artifacts plus the closed policy"
)]
pub fn verify_esp32s31_hal_mac_txq_owned_publication(
    svd: &MmioMap,
    vendor_inventory: Option<&Path>,
    vendor_artifact: &Path,
    vendor_companion: Option<&Path>,
    vendor_replay_artifact: &Path,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_symbol: &str,
    policy: &EffectPolicy,
) -> Result<DriverAdapterVerification> {
    validate_policy(policy)?;
    let vendor_inventory = vendor_inventory
        .ok_or("hal_mac_txq_enable verification requires the caller-owned raw libpp inventory")?;
    let vendor_proof = validate_vendor_register_slice(vendor_inventory, svd)?;

    let mut vendor_image = execution::ExecutableImage::load(vendor_artifact)?;
    if let Some(companion) = vendor_companion {
        vendor_image.add_companion(companion)?;
    }
    let vendor_replay_image = execution::ExecutableImage::load(vendor_replay_artifact)?;
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }
    let vendor_inventory_digest = inventory_symbol_sha256(vendor_inventory, None, SYMBOL)?;
    let vendor_code_digest = code_closure_sha256(&vendor_image, SYMBOL)?;
    let vendor_replay_code_digest = code_closure_sha256(&vendor_replay_image, SYMBOL)?;
    let rust_code_digest = code_closure_sha256(&rust_image, rust_symbol)?;
    let storage = rust_image
        .symbol_address(TX_STORAGE_SYMBOL)
        .ok_or_else(|| format!("compiled Rust probe has no retained {TX_STORAGE_SYMBOL}"))?;
    let mut matched = true;
    let mut cases = Vec::new();
    let mut canonical = String::from("driver-adapter esp32s31-hal-mac-txq-owned-publication-v1\n");
    canonical.push_str(&format!(
        "vendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nvendor-replay-code-closure-sha256 {vendor_replay_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    ));
    canonical.push_str(&vendor_proof);
    canonical.push_str("platform-service embassy-tx-queue-ownership\n");
    canonical.push_str("prerequisite he-trigger-based-tx-disabled\n");
    canonical.push_str("omission vendor-tx-enable-statistics unused-instrumentation\n");

    for queue in 0..4_u32 {
        let initial = 0x0123_4567_u32 ^ queue.wrapping_mul(0x1111_1111);
        let address = CONTROL[3 - queue as usize];
        let vendor_result = super::execute_case(
            &vendor_replay_image,
            svd,
            SYMBOL,
            vendor_scenario(queue, initial),
            format!("vendor-queue-{queue}"),
            "concrete vendor replay",
        )?;
        let vendor_reason = vendor_contract_reason(&vendor_result, queue, initial);
        matched &= vendor_reason.is_none();
        cases.push(open_radio_vendor_semantics::DriverAdapterCase {
            name: format!("vendor-queue-{queue}"),
            matched: vendor_reason.is_none(),
            reason: vendor_reason,
        });

        let result = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            production_scenario(svd, queue, false),
            format!("production-queue-{queue}"),
            "production owned-TX execution",
        )?;
        let reason = production_contract_reason(&result, queue, false, storage);
        matched &= reason.is_none();
        cases.push(open_radio_vendor_semantics::DriverAdapterCase {
            name: format!("production-queue-{queue}"),
            matched: reason.is_none(),
            reason,
        });
        let active = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            production_scenario(svd, queue, true),
            format!("production-active-queue-{queue}"),
            "production fail-closed execution",
        )?;
        let active_reason = production_contract_reason(&active, queue, true, storage);
        matched &= active_reason.is_none();
        cases.push(open_radio_vendor_semantics::DriverAdapterCase {
            name: format!("production-active-queue-{queue}"),
            matched: active_reason.is_none(),
            reason: active_reason,
        });
        canonical.push_str(&format!(
            "case queue-{queue} address={address:#010x} vendor-initial={initial:#010x} vendor-write={:#010x} vendor-steps={} production-events={} production-return={:#010x} active-events={}\n",
            initial | ENABLE_VALID,
            vendor_result.steps,
            result.events.len(),
            result.return_value,
            active.events.len(),
        ));
    }
    Ok(DriverAdapterVerification::from_trust(
        crate::driver_adapter_trust("esp32s31-hal-mac-txq-owned-publication-v1")
            .expect("registered adapter has a trust boundary"),
        matched,
        canonical,
    )
    .with_cases(cases))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_names_the_replaced_and_omitted_suffix_responsibilities() {
        let rules = required_policy();
        assert_eq!(rules.len(), 11);
        assert_eq!(
            rules[&EffectSelector::PlatformProvidedService {
                service: "embassy-tx-queue-ownership".to_owned(),
            }],
            EffectDisposition::Required
        );
        assert_eq!(
            rules[&EffectSelector::InitializationPrerequisite {
                prerequisite: "he-trigger-based-tx-disabled".to_owned(),
            }],
            EffectDisposition::Required
        );
    }

    #[test]
    fn owned_publication_rejects_wrong_bank_order_and_enable_image() {
        let address = CONTROL[3];
        let prepared = 0x0123_4567;
        let expected = [
            (false, address, 0),
            (true, address, prepared),
            (false, address, prepared),
            (false, address, prepared),
            (true, address, prepared | ENABLE_VALID),
        ];
        assert!(valid_owned_control_edge(&expected, address));

        let mut wrong_bank = expected;
        wrong_bank[4].1 = CONTROL[2];
        assert!(!valid_owned_control_edge(&wrong_bank, address));
        let mut wrong_order = expected;
        wrong_order.swap(1, 2);
        assert!(!valid_owned_control_edge(&wrong_order, address));
        let mut missing_enable = expected;
        missing_enable[4].2 = prepared;
        assert!(!valid_owned_control_edge(&missing_enable, address));
    }
}
