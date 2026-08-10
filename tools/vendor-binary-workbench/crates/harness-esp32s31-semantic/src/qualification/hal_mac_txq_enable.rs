//! Verification of the register-owned prefix of `hal_mac_txq_enable`.
//!
//! The vendor function mixes three responsibilities: it first publishes the
//! ordinary queue by setting CONTROL.ENABLE|VALID, then mutates a private
//! scheduler context for HE trigger-based traffic, and finally updates vendor
//! instrumentation. The open driver keeps the first responsibility as a PAC
//! leaf, replaces scheduler ownership with its Embassy state machine, scopes
//! out HE trigger-based TX, and omits the instrumentation.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use open_radio_vendor_backend_riscv::trace_binary_symbol;
use open_radio_vendor_semantics::{
    DraftReferenceEvent, DriverAdapterVerification, EffectDisposition, EffectPolicy,
    EffectSelector, ExpressionOperation, MemoryAccess, OmissionReason, PlatformOperation,
    SymbolicValue, evaluate_for_input,
};

use crate::{MmioMap, Result, StructuralPointerContext, artifact, execution};

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
            "hal_mac_txq_enable register-slice policy differs from the closed adapter boundary:\nexpected {expected:#?}\nactual {actual:#?}"
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
            &suffix_trace.reference_events
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

fn expected_events(queue: u32, initial: u32) -> Vec<(bool, u32, u32)> {
    let address = CONTROL[3 - queue as usize];
    vec![
        (false, address, initial),
        (true, address, initial | ENABLE_VALID),
    ]
}

fn normalized_events(events: &[execution::ExecutionEvent]) -> Result<Vec<(bool, u32, u32)>> {
    events
        .iter()
        .map(|event| match event {
            execution::ExecutionEvent::Read {
                width: 32,
                address,
                value,
                ..
            } => Ok((false, *address, *value)),
            execution::ExecutionEvent::Write {
                width: 32,
                address,
                value,
                ..
            } => Ok((true, *address, *value)),
            other => Err(format!(
                "unexpected Rust {SYMBOL} register-slice event: {other:?}"
            )),
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn rust_register_slice_matches(
    events: &[execution::ExecutionEvent],
    return_value: u32,
    queue: u32,
    initial: u32,
) -> bool {
    return_value == 0
        && normalized_events(events).is_ok_and(|actual| actual == expected_events(queue, initial))
}

#[allow(
    clippy::too_many_arguments,
    reason = "verification binds caller-owned vendor and Rust artifacts plus the closed policy"
)]
pub fn verify_esp32s31_hal_mac_txq_enable_register_slice(
    svd: &MmioMap,
    vendor_inventory: Option<&Path>,
    vendor_artifact: &Path,
    vendor_companion: Option<&Path>,
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
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }
    let vendor_inventory_digest = inventory_symbol_sha256(vendor_inventory, None, SYMBOL)?;
    let vendor_code_digest = code_closure_sha256(&vendor_image, SYMBOL)?;
    let rust_code_digest = code_closure_sha256(&rust_image, rust_symbol)?;
    let mut covered = BTreeSet::new();
    let mut required = execution::CoverageInventory::default();
    let mut matched = true;
    let mut canonical =
        String::from("driver-adapter esp32s31-hal-mac-txq-enable-register-slice-v1\n");
    canonical.push_str(&format!(
        "vendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    ));
    canonical.push_str(&vendor_proof);
    canonical.push_str("platform-service embassy-tx-queue-ownership\n");
    canonical.push_str("prerequisite he-trigger-based-tx-disabled\n");
    canonical.push_str("omission vendor-tx-enable-statistics unused-instrumentation\n");

    for queue in 0..4_u32 {
        let initial = 0x0123_4567_u32 ^ queue.wrapping_mul(0x1111_1111);
        let address = CONTROL[3 - queue as usize];
        let result = execution::execute(
            &rust_image,
            svd,
            rust_symbol,
            execution::Scenario {
                arguments: vec![queue],
                mmio_initial: BTreeMap::from([(address, initial)]),
                max_steps: 500,
                ..execution::Scenario::default()
            },
        )?;
        let case_matched =
            rust_register_slice_matches(&result.events, result.return_value, queue, initial);
        matched &= case_matched;
        covered.extend(result.branches.iter().copied());

        let mut constraints = [None; 8];
        constraints[0] = Some(queue);
        let inventory =
            rust_image.coverage_inventory_with_argument_constraints(rust_symbol, &constraints)?;
        required.branch_sites.extend(inventory.branch_sites);
        required.branch_outcomes.extend(inventory.branch_outcomes);
        required.unresolved_edges.extend(inventory.unresolved_edges);
        canonical.push_str(&format!(
            "case queue-{queue} address={address:#010x} initial={initial:#010x} write={:#010x} steps={}\n",
            initial | ENABLE_VALID,
            result.steps
        ));
    }
    if !required.unresolved_edges.is_empty() {
        return Err(format!(
            "compiled Rust {SYMBOL} register slice has unresolved reachable control flow: {:?}",
            required.unresolved_edges
        )
        .into());
    }
    let uncovered = required
        .branch_outcomes
        .difference(&covered)
        .copied()
        .collect::<Vec<_>>();
    if !uncovered.is_empty() {
        return Err(format!(
            "compiled Rust {SYMBOL} register slice has uncovered admissible branch outcomes: {uncovered:?}"
        )
        .into());
    }
    canonical.push_str(&format!(
        "rust-branch-outcomes {} covered\n",
        required.branch_outcomes.len()
    ));
    Ok(DriverAdapterVerification { matched, canonical })
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

    fn read(address: u32, value: u32) -> execution::ExecutionEvent {
        execution::ExecutionEvent::Read {
            width: 32,
            address,
            region: "test".to_owned(),
            register: Some("TEST".to_owned()),
            value,
        }
    }

    fn write(address: u32, value: u32) -> execution::ExecutionEvent {
        execution::ExecutionEvent::Write {
            width: 32,
            address,
            region: "test".to_owned(),
            register: Some("TEST".to_owned()),
            value,
        }
    }

    #[test]
    fn register_slice_rejects_address_value_order_extra_access_and_return_mutations() {
        let initial = 0x0123_4567;
        let address = CONTROL[3];
        let expected = [
            read(address, initial),
            write(address, initial | ENABLE_VALID),
        ];
        assert!(rust_register_slice_matches(&expected, 0, 0, initial));

        let wrong_address = [
            read(CONTROL[2], initial),
            write(CONTROL[2], initial | ENABLE_VALID),
        ];
        assert!(!rust_register_slice_matches(&wrong_address, 0, 0, initial));
        let wrong_value = [read(address, initial), write(address, initial)];
        assert!(!rust_register_slice_matches(&wrong_value, 0, 0, initial));
        let wrong_order = [
            write(address, initial | ENABLE_VALID),
            read(address, initial),
        ];
        assert!(!rust_register_slice_matches(&wrong_order, 0, 0, initial));
        let extra = [
            read(address, initial),
            write(address, initial | ENABLE_VALID),
            read(address, initial | ENABLE_VALID),
        ];
        assert!(!rust_register_slice_matches(&extra, 0, 0, initial));
        assert!(!rust_register_slice_matches(&expected, 1, 0, initial));
    }
}
