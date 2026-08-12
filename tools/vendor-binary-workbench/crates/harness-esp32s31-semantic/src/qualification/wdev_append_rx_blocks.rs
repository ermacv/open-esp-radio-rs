//! Verification of the Rust RX-ring transaction replacing
//! `wDev_AppendRxBlocks`.
//!
//! The vendor root owns a private intrusive list, an OS critical section,
//! diagnostic/statistics hooks and a bounded blocking reload loop. The open
//! driver instead owns descriptors and staging slots with Rust types and
//! crosses an Embassy scheduling edge between reload observations. This
//! adapter binds the immutable vendor structure to compiled production Rust
//! scenarios without reproducing `wDevCtrl` or `g_osi_funcs_p` in runtime.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};

use open_esp_radio_esp32s31_wifi_mac::rx::RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT;
use open_radio_vendor_backend_riscv::ReferenceResolver;
use open_radio_vendor_semantics::{
    DriverAdapterCase, DriverAdapterVerification, EffectDisposition, EffectPolicy, EffectSelector,
    OmissionReason, PlatformOperation, Timeout,
};

use crate::{MmioMap, Result, artifact, execution};

use super::{code_closure_sha256, inventory_symbol_sha256};

const SYMBOL: &str = "wDev_AppendRxBlocks";
const MEMBER: &str = "wdev.o";
const RX_CONTROL: u32 = 0x2010_4080;
const RX_BASE: u32 = 0x2010_4084;
const RX_NEXT: u32 = 0x2010_4088;
const RX_LAST: u32 = 0x2010_408c;
const RX_HIGH: u32 = 0x2010_4c70;
const DESCRIPTOR_BASE: u32 = 0x2f00_1000;
const DESCRIPTOR_BYTES: u32 = 12;
const WALKER_ENABLE: u32 = 0x8000_0000;
const RELOAD: u32 = 1;

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    scenario: u32,
    pending_samples: u32,
    repair_base: bool,
    timeout: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "immediate-settle",
        scenario: 0,
        pending_samples: 0,
        repair_base: false,
        timeout: false,
    },
    Case {
        name: "two-async-edges",
        scenario: 1,
        pending_samples: 2,
        repair_base: false,
        timeout: false,
    },
    Case {
        name: "terminal-frontier-base-repair",
        scenario: 2,
        pending_samples: 0,
        repair_base: true,
        timeout: false,
    },
    Case {
        name: "exact-attempt-timeout",
        scenario: 3,
        pending_samples: RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT,
        repair_base: false,
        timeout: true,
    },
];

fn required_policy() -> BTreeMap<EffectSelector, EffectDisposition> {
    [
        (
            EffectSelector::MmioRead {
                width: 32,
                address: RX_CONTROL,
            },
            EffectDisposition::ReplacedByAsync {
                condition: "rx-descriptor-reload-clear".to_owned(),
                timeout: Timeout::Attempts(RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT),
            },
        ),
        (
            EffectSelector::MmioWrite {
                width: 32,
                address: RX_CONTROL,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::MmioRead {
                width: 32,
                address: RX_NEXT,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::MmioRead {
                width: 32,
                address: RX_LAST,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::MmioWrite {
                width: 32,
                address: RX_BASE,
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 32,
                field: "rx-ring.old-tail-next".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 32,
                field: "rx-ring.pending-tail".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::StateWrite {
                width: 32,
                field: "rx-ring.accepted-tail".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::CriticalSection,
            },
            EffectDisposition::PlatformProvidedService {
                service: "embassy-rx-ring-ownership".to_owned(),
            },
        ),
        (
            EffectSelector::PlatformCall {
                operation: PlatformOperation::DebugDiagnostic,
            },
            EffectDisposition::AllowedOmission(OmissionReason::UnusedInstrumentation),
        ),
        (
            EffectSelector::PlatformProvidedService {
                service: "embassy-rx-ring-ownership".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "valid-zero-terminated-rx-chain".to_owned(),
            },
            EffectDisposition::Required,
        ),
        (
            EffectSelector::InitializationPrerequisite {
                prerequisite: "unique-rx-descriptor-storage".to_owned(),
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
            "{SYMBOL} composition policy differs from the closed RX ownership boundary:\nexpected {expected:#?}\nactual {actual:#?}"
        )
        .into());
    }
    Ok(())
}

fn validate_vendor_shape(vendor_inventory: &Path, svd: &MmioMap) -> Result<String> {
    let symbols = artifact::load_code_symbols(
        vendor_inventory,
        SYMBOL,
        artifact::CodeSymbolSelection::Exported,
    )?;
    let symbol = symbols
        .iter()
        .find(|candidate| candidate.name == SYMBOL && candidate.member.as_deref() == Some(MEMBER))
        .ok_or_else(|| format!("{MEMBER}::{SYMBOL} is absent from the caller-owned inventory"))?;
    if symbol.address != 0 || symbol.bytes.len() != 0x17a {
        return Err(format!(
            "{MEMBER}::{SYMBOL} boundary changed: address={:#x}, size={:#x}",
            symbol.address,
            symbol.bytes.len()
        )
        .into());
    }
    let calls = symbol
        .relocations
        .iter()
        .filter(|relocation| {
            matches!(
                relocation.kind,
                artifact::RelocationKind::Call | artifact::RelocationKind::CallPlt
            )
        })
        .map(|relocation| (relocation.address, relocation.symbol.as_str()))
        .collect::<Vec<_>>();
    let expected_calls = [
        (0x02c, "wifi_assert"),
        (0x084, "wdev_record_rx_linked_list"),
        (0x090, "hal_mac_rx_set_dscr_reload"),
        (0x09a, "wdev_record_rx_linked_list"),
        (0x0b6, "hal_mac_rx_is_dscr_reload"),
        (0x0ca, "wdev_dump_rx_linked_list"),
        (0x0d2, "hal_mac_rx_read_rxdscrnext"),
        (0x0dc, "hal_mac_rx_get_last_dscr"),
        (0x0f4, "hal_mac_rx_set_base"),
        (0x170, "hal_mac_rx_set_base"),
    ];
    if calls != expected_calls {
        return Err(format!(
            "{MEMBER}::{SYMBOL} call boundary differs: expected {expected_calls:?}, got {calls:?}"
        )
        .into());
    }
    if symbol.bytes.get(0x42..0x46) != Some(&[0xb7, 0xc5, 0xad, 0xde])
        || symbol.bytes.get(0x54..0x58) != Some(&[0x93, 0x85, 0xf5, 0xee])
    {
        return Err(
            format!("{MEMBER}::{SYMBOL} no longer constructs the 0xdeadbeef RX guards").into(),
        );
    }
    if symbol.bytes.get(0x8e..0x90) != Some(&[0x80, 0xc7]) {
        return Err(format!("{MEMBER}::{SYMBOL} old-tail next-link publication changed").into());
    }
    if symbol.bytes.get(0xae..0xb6) != Some(&[0xe1, 0x69, 0x93, 0x89, 0x19, 0x6a, 0x82, 0x97]) {
        return Err(format!(
            "{MEMBER}::{SYMBOL} no longer materializes the exact 0x186a1 reload bound before the OS unlock"
        )
        .into());
    }

    let trace = ReferenceResolver::load(vendor_inventory, &[], &crate::RISCV_HARNESS)?.trace(
        Some(MEMBER),
        SYMBOL,
        svd,
    )?;
    if trace.is_reference_eligible() {
        return Err(format!(
            "{MEMBER}::{SYMBOL} unexpectedly became a flat reference; review the composition boundary"
        )
        .into());
    }
    if trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("unresolved-call-relocation") && blocker.contains("wifi_assert")
    }) {
        return Err("wifi_assert regressed from a diagnostic boundary to an unknown ABI".into());
    }
    let bounded_private_loop = trace.reference_blockers.iter().any(|blocker| {
        blocker.contains("control-flow loop bounded unrolling exceeds") && blocker.contains("0x58")
    });
    if !bounded_private_loop {
        return Err(format!(
            "{MEMBER}::{SYMBOL} no longer exposes its reviewed private descriptor loop boundary: {:?}",
            trace.reference_blockers
        )
        .into());
    }

    Ok(format!(
        "vendor-member {MEMBER}\nvendor-size 0x17a\nvendor-reload-attempts {RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT}\nvendor-chain-guard 0xdeadbeef\nprivate-descriptor-loop bounded-at-0x58\nvendor-call-boundary {}\n",
        expected_calls
            .iter()
            .map(|(_, symbol)| *symbol)
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn read(address: u32, value: u32) -> execution::ExecutionEvent {
    execution::ExecutionEvent::Read {
        width: 32,
        address,
        region: "rx-dma".to_owned(),
        register: Some("RX_DMA".to_owned()),
        value,
    }
}

fn write(address: u32, value: u32) -> execution::ExecutionEvent {
    execution::ExecutionEvent::Write {
        width: 32,
        address,
        region: "rx-dma".to_owned(),
        register: Some("RX_DMA".to_owned()),
        value,
    }
}

fn full_fence() -> execution::ExecutionEvent {
    execution::ExecutionEvent::Fence {
        fm: 0,
        predecessor: 0x0f,
        successor: 0x0f,
    }
}

fn ownership_fence() -> execution::ExecutionEvent {
    execution::ExecutionEvent::Fence {
        fm: 0,
        predecessor: 0x02,
        successor: 0x03,
    }
}

fn expected_rust_events(case: Case) -> Vec<execution::ExecutionEvent> {
    let mut events = vec![
        // The source-owned cold-ring fixture now proves that the walker is
        // stopped before sampling RX_LAST. This is a lifecycle precondition
        // around the replacement, not a relaxed wDev_AppendRxBlocks effect.
        read(RX_CONTROL, 0),
        full_fence(),
        read(RX_LAST, 0),
        full_fence(),
        read(RX_HIGH, 0),
        write(RX_HIGH, 0x2f00_0000),
        write(RX_BASE, DESCRIPTOR_BASE),
        full_fence(),
        read(RX_CONTROL, 0),
        read(RX_CONTROL, 0),
        write(RX_CONTROL, WALKER_ENABLE),
        full_fence(),
        read(RX_CONTROL, WALKER_ENABLE),
        read(RX_CONTROL, WALKER_ENABLE),
        read(RX_CONTROL, WALKER_ENABLE),
        full_fence(),
        read(RX_CONTROL, WALKER_ENABLE),
        write(RX_CONTROL, WALKER_ENABLE | RELOAD),
        full_fence(),
    ];
    for sample in 0..case.pending_samples {
        events.push(read(RX_CONTROL, WALKER_ENABLE | RELOAD));
        if sample + 1 != RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT || !case.timeout {
            events.push(execution::ExecutionEvent::DelayMicros(1));
        }
    }
    if case.timeout {
        return events;
    }
    events.push(read(RX_CONTROL, WALKER_ENABLE));
    events.push(read(
        RX_NEXT,
        if case.repair_base {
            0
        } else {
            DESCRIPTOR_BASE + DESCRIPTOR_BYTES
        },
    ));
    if case.repair_base {
        events.push(read(RX_LAST, DESCRIPTOR_BASE + DESCRIPTOR_BYTES));
        events.push(write(RX_BASE, DESCRIPTOR_BASE));
        events.push(full_fence());
    }
    events.push(ownership_fence());
    events
}

fn atomic_ownership_matches(result: &execution::ExecutionResult, case: Case) -> bool {
    let atomics = result
        .timeline
        .iter()
        .filter_map(|event| match event {
            execution::ExecutionTimelineEvent::Atomic {
                operation,
                ordering,
                address,
                succeeded,
            } => Some((*operation, *ordering, *address, *succeeded)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let pair_count = if case.timeout { 2 } else { 4 };
    if atomics.len() != pair_count * 2 {
        return false;
    }
    let Some(address) = atomics.first().map(|atomic| atomic.2) else {
        return false;
    };
    address & 3 == 0
        && atomics.chunks_exact(2).all(|pair| {
            pair[0]
                == (
                    execution::AtomicOperation::LoadReserved,
                    execution::AtomicOrdering::Acquire,
                    address,
                    None,
                )
                && pair[1]
                    == (
                        execution::AtomicOperation::StoreConditional,
                        execution::AtomicOrdering::Release,
                        address,
                        Some(true),
                    )
        })
}

fn rust_scenario(case: Case, stack_fill: u8) -> execution::Scenario {
    let mut control = VecDeque::from([
        0,
        0,
        0,
        WALKER_ENABLE,
        WALKER_ENABLE,
        WALKER_ENABLE,
        WALKER_ENABLE,
    ]);
    control.extend(std::iter::repeat_n(
        WALKER_ENABLE | RELOAD,
        case.pending_samples as usize,
    ));
    if !case.timeout {
        control.push_back(WALKER_ENABLE);
    }
    let mut last = VecDeque::from([0]);
    if case.repair_base {
        last.push_back(DESCRIPTOR_BASE + DESCRIPTOR_BYTES);
    }
    let mut mmio_reads = BTreeMap::from([
        (RX_CONTROL, control),
        (RX_LAST, last),
        (RX_HIGH, VecDeque::from([0])),
    ]);
    if !case.timeout {
        mmio_reads.insert(
            RX_NEXT,
            VecDeque::from([if case.repair_base {
                0
            } else {
                DESCRIPTOR_BASE + DESCRIPTOR_BYTES
            }]),
        );
    }
    execution::Scenario {
        arguments: vec![case.scenario],
        mmio_reads,
        private_stack_fill: Some(stack_fill),
        max_steps: 6_000_000,
        ..execution::Scenario::default()
    }
}

fn expected_return(case: Case) -> u32 {
    if case.timeout {
        0x4000_0000 | (RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT - 1)
    } else {
        0x8000_0003 | case.pending_samples << 8
    }
}

fn event_matches(actual: &execution::ExecutionEvent, expected: &execution::ExecutionEvent) -> bool {
    match (actual, expected) {
        (
            execution::ExecutionEvent::Read {
                width: actual_width,
                address: actual_address,
                value: actual_value,
                ..
            },
            execution::ExecutionEvent::Read {
                width: expected_width,
                address: expected_address,
                value: expected_value,
                ..
            },
        )
        | (
            execution::ExecutionEvent::Write {
                width: actual_width,
                address: actual_address,
                value: actual_value,
                ..
            },
            execution::ExecutionEvent::Write {
                width: expected_width,
                address: expected_address,
                value: expected_value,
                ..
            },
        ) => {
            actual_width == expected_width
                && actual_address == expected_address
                && actual_value == expected_value
        }
        (
            execution::ExecutionEvent::DelayMicros(actual),
            execution::ExecutionEvent::DelayMicros(expected),
        ) => actual == expected,
        (
            execution::ExecutionEvent::Fence {
                fm: actual_fm,
                predecessor: actual_predecessor,
                successor: actual_successor,
            },
            execution::ExecutionEvent::Fence {
                fm: expected_fm,
                predecessor: expected_predecessor,
                successor: expected_successor,
            },
        ) => {
            actual_fm == expected_fm
                && actual_predecessor == expected_predecessor
                && actual_successor == expected_successor
        }
        _ => false,
    }
}

fn event_sequences_match(
    actual: &[execution::ExecutionEvent],
    expected: &[execution::ExecutionEvent],
) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| event_matches(actual, expected))
}

fn case_report(
    first: &execution::ExecutionResult,
    second: &execution::ExecutionResult,
    case: Case,
) -> DriverAdapterCase {
    let expected_return = expected_return(case);
    let expected_events = expected_rust_events(case);
    let padding_independent =
        first.events == second.events && first.return_value == second.return_value;
    let reason = if first.return_value != expected_return {
        Some(format!(
            "return differs: expected {expected_return:#010x}, actual {:#010x}",
            first.return_value
        ))
    } else if !event_sequences_match(&first.events, &expected_events) {
        let index = first
            .events
            .iter()
            .zip(&expected_events)
            .position(|(actual, expected)| !event_matches(actual, expected))
            .unwrap_or_else(|| first.events.len().min(expected_events.len()));
        let window_start = index.saturating_sub(2);
        let expected_end = (index + 3).min(expected_events.len());
        let actual_end = (index + 3).min(first.events.len());
        Some(format!(
            "observable event #{index} differs: expected-window={:?}, actual-window={:?} (expected-events={}, actual-events={})",
            &expected_events[window_start.min(expected_end)..expected_end],
            &first.events[window_start.min(actual_end)..actual_end],
            expected_events.len(),
            first.events.len(),
        ))
    } else if !atomic_ownership_matches(first, case) {
        Some("descriptor ownership atomic transition differs".to_owned())
    } else if !padding_independent {
        Some("result depends on private stack fill".to_owned())
    } else {
        None
    };
    DriverAdapterCase {
        name: case.name.to_owned(),
        matched: reason.is_none(),
        reason,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "verification binds caller-owned vendor/Rust artifacts and one closed effect policy"
)]
pub fn verify_esp32s31_wdev_append_rx_blocks(
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
        .ok_or("wDev_AppendRxBlocks verification requires the caller-owned raw libpp inventory")?;
    let vendor_proof = validate_vendor_shape(vendor_inventory, svd)?;

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
    let mut canonical = String::from("driver-adapter esp32s31-wdev-append-rx-blocks-v1\n");
    canonical.push_str(&format!(
        "vendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    ));
    canonical.push_str(&vendor_proof);
    canonical.push_str("scope rx-ring-ownership-and-reload-transaction\n");
    canonical.push_str("fixture-prefix cold-ring-publication\n");
    canonical.push_str("platform-service embassy-rx-ring-ownership\n");
    canonical.push_str("omission vendor-diagnostics-and-statistics unused-instrumentation\n");

    let mut matched = true;
    let mut case_reports = Vec::with_capacity(CASES.len());
    for case in CASES {
        let first = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            rust_scenario(*case, 0),
            case.name,
            "Rust execution with stack-fill=0x00",
        )?;
        let second = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            rust_scenario(*case, 0xa5),
            case.name,
            "Rust execution with stack-fill=0xa5",
        )?;
        let padding_independent =
            first.events == second.events && first.return_value == second.return_value;
        let report = case_report(&first, &second, *case);
        matched &= report.matched;
        canonical.push_str(&format!(
            "scenario {} pending-samples={} repair-base={} timeout={} return={:#010x} events={} atomic-pairs={} branches={} padding-independent={}\n",
            case.name,
            case.pending_samples,
            case.repair_base,
            case.timeout,
            first.return_value,
            first.events.len(),
            if case.timeout { 2 } else { 4 },
            first.ordered_branches.len(),
            padding_independent,
        ));
        case_reports.push(report);
    }
    Ok(
        DriverAdapterVerification::whole_function_equivalence(matched, canonical)
            .with_cases(case_reports),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_names_async_ownership_prerequisites_and_vendor_omissions() {
        let rules = required_policy();
        assert_eq!(rules.len(), 13);
        assert_eq!(
            rules[&EffectSelector::MmioRead {
                width: 32,
                address: RX_CONTROL,
            }],
            EffectDisposition::ReplacedByAsync {
                condition: "rx-descriptor-reload-clear".to_owned(),
                timeout: Timeout::Attempts(0x186a1),
            }
        );
        assert_eq!(RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT, 0x186a1);
    }

    #[test]
    fn composition_rejects_address_value_order_extra_access_and_return_mutations() {
        let case = CASES[0];
        let exact = expected_rust_events(case);
        let ownership_address = 0x3fce_0000;
        let timeline = (0..4)
            .flat_map(|_| {
                [
                    execution::ExecutionTimelineEvent::Atomic {
                        operation: execution::AtomicOperation::LoadReserved,
                        ordering: execution::AtomicOrdering::Acquire,
                        address: ownership_address,
                        succeeded: None,
                    },
                    execution::ExecutionTimelineEvent::Atomic {
                        operation: execution::AtomicOperation::StoreConditional,
                        ordering: execution::AtomicOrdering::Release,
                        address: ownership_address,
                        succeeded: Some(true),
                    },
                ]
            })
            .collect();
        let result = execution::ExecutionResult {
            events: exact.clone(),
            event_producers: Vec::new(),
            timeline,
            return_value: expected_return(case),
            completion: execution::ExecutionCompletion::Returned,
            steps: 0,
            branches: Default::default(),
            ordered_branches: Vec::new(),
            calls: Default::default(),
            ordered_calls: Vec::new(),
            indirect_calls: Default::default(),
            allocations: Vec::new(),
            table_lifecycle: Vec::new(),
            table_lifecycle_complete: true,
            fifo_lifecycle: Vec::new(),
            fifo_services: Vec::new(),
            device_model_coverage: Vec::new(),
            memory_changes: Vec::new(),
            initial_memory: Default::default(),
            persistent_memory: Default::default(),
        };
        assert!(case_report(&result, &result, case).matched);

        let mut wrong_address = result.clone();
        wrong_address.events[5] = write(RX_BASE + 4, DESCRIPTOR_BASE);
        assert!(!case_report(&wrong_address, &wrong_address, case).matched);
        let mut wrong_value = result.clone();
        wrong_value.events[17] = write(RX_CONTROL, WALKER_ENABLE);
        assert!(!case_report(&wrong_value, &wrong_value, case).matched);
        let mut wrong_order = result.clone();
        wrong_order.events.swap(17, 18);
        assert!(!case_report(&wrong_order, &wrong_order, case).matched);
        let mut extra = result.clone();
        extra.events.push(read(RX_CONTROL, 0));
        assert!(!case_report(&extra, &extra, case).matched);
        let mut wrong_return = result;
        wrong_return.return_value ^= 1;
        assert!(!case_report(&wrong_return, &wrong_return, case).matched);
    }
}
