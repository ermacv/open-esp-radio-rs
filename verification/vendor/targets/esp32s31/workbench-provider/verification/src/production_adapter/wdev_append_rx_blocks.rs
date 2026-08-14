//! Verification of the Rust RX-ring transaction replacing
//! `wDev_AppendRxBlocks`.
//!
//! The vendor root owns a private intrusive list, an OS critical section,
//! diagnostic/statistics hooks and a bounded blocking reload loop. The open
//! driver instead owns descriptors and staging slots with Rust types and
//! crosses an Embassy scheduling edge between reload observations. This
//! adapter executes the immutable vendor body with modeled `wDevCtrl` and OSI
//! state, then checks a compact refinement against compiled production Rust.
//! It does not implement a second RX-ring state transition.

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
const RX_STORAGE_SYMBOL: &str = "OPEN_LIBPP_RX_STORAGE";
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
    let minimum_pair_count = if case.timeout { 2 } else { 3 };
    if atomics.len() < minimum_pair_count * 2 || !atomics.len().is_multiple_of(2) {
        return false;
    }
    atomics.chunks_exact(2).all(|pair| {
        let address = pair[0].2;
        address & 3 == 0
            && pair[0]
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

fn rust_scenario(case: Case, stack_fill: u8, descriptor_base: u32) -> execution::Scenario {
    let mut control = VecDeque::from([0, 0, 0, WALKER_ENABLE, WALKER_ENABLE, WALKER_ENABLE]);
    control.extend(std::iter::repeat_n(
        WALKER_ENABLE | RELOAD,
        case.pending_samples as usize,
    ));
    if !case.timeout {
        control.push_back(WALKER_ENABLE);
    }
    let mut last = VecDeque::from([0, descriptor_base]);
    if !case.timeout {
        last.push_back(if case.repair_base {
            descriptor_base + DESCRIPTOR_BYTES
        } else {
            descriptor_base
        });
    }
    let mut mmio_reads = BTreeMap::from([
        (RX_CONTROL, control),
        (RX_LAST, last),
        (RX_HIGH, VecDeque::from([0])),
    ]);
    let mut next = VecDeque::from([0, descriptor_base + DESCRIPTOR_BYTES]);
    if !case.timeout {
        next.push_back(if case.repair_base {
            0
        } else {
            descriptor_base + DESCRIPTOR_BYTES
        });
    }
    mmio_reads.insert(RX_NEXT, next);
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

fn final_ram_word(result: &execution::ExecutionResult, address: u32) -> Option<u32> {
    let mut bytes = [None; 4];
    for (offset, slot) in bytes.iter_mut().enumerate() {
        *slot = result
            .initial_memory
            .get(&(address + offset as u32))
            .copied();
    }
    for event in &result.timeline {
        let execution::ExecutionTimelineEvent::RamWrite {
            width,
            address: write_address,
            value,
            ..
        } = event
        else {
            continue;
        };
        for offset in 0..u32::from(*width / 8) {
            let current = write_address.checked_add(offset)?;
            let Some(index) = current
                .checked_sub(address)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < bytes.len())
            else {
                continue;
            };
            bytes[index] = Some((value >> (offset * 8)) as u8);
        }
    }
    Some(u32::from_le_bytes([
        bytes[0]?, bytes[1]?, bytes[2]?, bytes[3]?,
    ]))
}

fn vendor_nonempty_append_case(
    image: &execution::ExecutableImage,
    svd: &MmioMap,
) -> Result<execution::ExecutionResult> {
    use crate::execution_model::{MemoryRange, TableInstance, TableInstanceSlot, TableSlotTarget};

    const OLD_TAIL: u32 = 0x3ffd_0000;
    const NEW_DESCRIPTOR: u32 = 0x3ffd_0100;
    const NEW_BUFFER: u32 = 0x3ffd_1000;
    const OSI_TABLE: u32 = 0x3ffe_0000;
    let wdev_control = image
        .symbol_address("wDevCtrl")
        .ok_or("libpp replay image has no wDevCtrl storage")?;
    let osi_pointer = image
        .symbol_address("g_osi_funcs_p")
        .ok_or("libpp replay image has no g_osi_funcs_p storage")?;
    let critical_lock = image
        .symbol_address("g_intr_lock_mux")
        .ok_or("libpp replay image has no g_intr_lock_mux storage")?;

    let mut scenario = execution::Scenario {
        arguments: vec![NEW_DESCRIPTOR, NEW_DESCRIPTOR],
        mmio_reads: BTreeMap::from([
            (RX_CONTROL, VecDeque::from([WALKER_ENABLE, WALKER_ENABLE])),
            (RX_NEXT, VecDeque::from([NEW_DESCRIPTOR])),
        ]),
        table_instances: vec![TableInstance {
            layout_id: "reviewed-platform-services-v9".to_owned(),
            base_address: OSI_TABLE,
            layout_size: 0x200,
            pointer_cells: vec![osi_pointer],
            pointer_cell_symbols: Vec::new(),
            slots: vec![
                TableInstanceSlot {
                    offset: 0x28,
                    target: TableSlotTarget::ModeledSymbol("wifi_int_disable".to_owned()),
                },
                TableInstanceSlot {
                    offset: 0x2c,
                    target: TableSlotTarget::ModeledSymbol("wifi_int_restore".to_owned()),
                },
            ],
        }],
        call_responses: BTreeMap::from([
            (
                "wifi_assert".to_owned(),
                VecDeque::from([execution::ModeledCallResponse::scalar(0)]),
            ),
            (
                "wifi_int_disable".to_owned(),
                VecDeque::from([execution::ModeledCallResponse::scalar(0x55aa)]),
            ),
            (
                "wifi_int_restore".to_owned(),
                VecDeque::from([execution::ModeledCallResponse::scalar(0)]),
            ),
            (
                "wdev_record_rx_linked_list".to_owned(),
                VecDeque::from([
                    execution::ModeledCallResponse::scalar(0),
                    execution::ModeledCallResponse::scalar(0),
                ]),
            ),
        ]),
        persistent_memory: vec![
            MemoryRange {
                start: OLD_TAIL,
                length: 12,
            },
            MemoryRange {
                start: NEW_DESCRIPTOR,
                length: 12,
            },
            MemoryRange {
                start: NEW_BUFFER,
                length: 32,
            },
        ],
        observed_memory: vec![
            MemoryRange {
                start: wdev_control,
                length: 8,
            },
            MemoryRange {
                start: OLD_TAIL,
                length: 12,
            },
            MemoryRange {
                start: NEW_DESCRIPTOR,
                length: 12,
            },
            MemoryRange {
                start: NEW_BUFFER,
                length: 32,
            },
        ],
        max_steps: 100_000,
        ..execution::Scenario::default()
    };
    crate::write_ram_word(&mut scenario, wdev_control, OLD_TAIL);
    crate::write_ram_word(&mut scenario, wdev_control + 4, OLD_TAIL);
    crate::write_ram_word(&mut scenario, critical_lock, 0x3fff_0000);
    crate::write_ram_word(&mut scenario, OLD_TAIL, 0);
    crate::write_ram_word(&mut scenario, OLD_TAIL + 4, 0);
    crate::write_ram_word(&mut scenario, OLD_TAIL + 8, 0);
    crate::write_ram_word(
        &mut scenario,
        NEW_DESCRIPTOR,
        16 | (4 << 14) | 0x4000_0000 | 0x8000_0000,
    );
    crate::write_ram_word(&mut scenario, NEW_DESCRIPTOR + 4, NEW_BUFFER);
    crate::write_ram_word(&mut scenario, NEW_DESCRIPTOR + 8, 0);
    for address in (NEW_BUFFER..NEW_BUFFER + 32).step_by(4) {
        crate::write_ram_word(&mut scenario, address, 0);
    }

    super::execute_case(
        image,
        svd,
        SYMBOL,
        scenario,
        "vendor-nonempty-append",
        "concrete vendor replay",
    )
}

fn vendor_contract_reason(
    result: &execution::ExecutionResult,
    wdev_control: u32,
) -> Option<String> {
    const OLD_TAIL: u32 = 0x3ffd_0000;
    const NEW_DESCRIPTOR: u32 = 0x3ffd_0100;
    if final_ram_word(result, wdev_control + 4) != Some(NEW_DESCRIPTOR) {
        return Some("vendor replay did not advance wDevCtrl.tail".to_owned());
    }
    if final_ram_word(result, OLD_TAIL + 8) != Some(NEW_DESCRIPTOR) {
        return Some("vendor replay did not publish old-tail.next".to_owned());
    }
    if final_ram_word(result, NEW_DESCRIPTOR) != Some(0x8004_0010)
        || final_ram_word(result, 0x3ffd_1000) != Some(0xdead_beef)
        || final_ram_word(result, 0x3ffd_1010) != Some(0xdead_beef)
    {
        return Some(
            "vendor replay did not rearm the descriptor and both buffer guards".to_owned(),
        );
    }
    let link_write = result.timeline.iter().position(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::RamWrite {
                width: 32,
                address,
                value: NEW_DESCRIPTOR,
                ..
            } if *address == OLD_TAIL + 8
        )
    });
    let reload_write = result.timeline.iter().position(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::Observable(execution::ExecutionEvent::Write {
                width: 32,
                address: RX_CONTROL,
                value,
                ..
            }) if *value == WALKER_ENABLE | RELOAD
        )
    });
    if !matches!((link_write, reload_write), (Some(link), Some(reload)) if link < reload) {
        return Some("vendor old-tail link was not ordered before the reload doorbell".to_owned());
    }
    let control_writes = result.events.iter().filter_map(|event| match event {
        execution::ExecutionEvent::Write {
            width: 32,
            address: RX_CONTROL,
            value,
            ..
        } => Some(*value),
        _ => None,
    });
    if control_writes.collect::<Vec<_>>() != [WALKER_ENABLE | RELOAD] {
        return Some("vendor replay did not publish exactly one reload doorbell".to_owned());
    }
    if result
        .events
        .iter()
        .any(|event| matches!(event, execution::ExecutionEvent::DelayMicros(_)))
    {
        return Some("vendor immediate-settle path unexpectedly delayed".to_owned());
    }
    None
}

fn production_contract_reason(
    result: &execution::ExecutionResult,
    case: Case,
    descriptor_base: u32,
) -> Option<String> {
    let write_values = |address| {
        result.events.iter().filter_map(move |event| match event {
            execution::ExecutionEvent::Write {
                width: 32,
                address: actual,
                value,
                ..
            } if *actual == address => Some(*value),
            _ => None,
        })
    };
    let delays = result
        .events
        .iter()
        .filter(|event| matches!(event, execution::ExecutionEvent::DelayMicros(1)))
        .count();
    let expected_delays = if case.timeout {
        case.pending_samples.saturating_sub(1)
    } else {
        case.pending_samples
    } as usize;
    let base_writes = write_values(RX_BASE).collect::<Vec<_>>();
    let expected_base_writes = if case.repair_base {
        vec![descriptor_base, descriptor_base]
    } else {
        vec![descriptor_base]
    };
    if write_values(RX_HIGH).collect::<Vec<_>>() != [descriptor_base & 0xfff0_0000] {
        return Some("production ring did not publish its validated DMA high window".to_owned());
    }
    if base_writes != expected_base_writes {
        return Some(format!(
            "production descriptor-base publications differ: {base_writes:#x?}"
        ));
    }
    let control_writes = write_values(RX_CONTROL).collect::<Vec<_>>();
    if control_writes != [WALKER_ENABLE, WALKER_ENABLE | RELOAD] {
        return Some(format!(
            "production walker/reload publications differ: {control_writes:#x?}"
        ));
    }
    let old_tail_next = descriptor_base + DESCRIPTOR_BYTES + 8;
    let link_write = result.timeline.iter().position(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::RamWrite {
                width: 32,
                address,
                value,
                ..
            } if *address == old_tail_next && *value == descriptor_base
        )
    });
    let reload_write = result.timeline.iter().position(|event| {
        matches!(
            event,
            execution::ExecutionTimelineEvent::Observable(execution::ExecutionEvent::Write {
                width: 32,
                address: RX_CONTROL,
                value,
                ..
            }) if *value == WALKER_ENABLE | RELOAD
        )
    });
    if !matches!((link_write, reload_write), (Some(link), Some(reload)) if link < reload) {
        return Some(
            "production old-tail link was not ordered before the reload doorbell".to_owned(),
        );
    }
    if delays != expected_delays {
        return Some(format!(
            "async edge count differs: expected {expected_delays}, actual {delays}"
        ));
    }
    None
}

fn case_report(
    first: &execution::ExecutionResult,
    second: &execution::ExecutionResult,
    case: Case,
    descriptor_base: u32,
) -> DriverAdapterCase {
    let expected_return = expected_return(case);
    let padding_independent =
        first.events == second.events && first.return_value == second.return_value;
    let reason = if first.return_value != expected_return {
        Some(format!(
            "return differs: expected {expected_return:#010x}, actual {:#010x}",
            first.return_value
        ))
    } else if let Some(reason) = production_contract_reason(first, case, descriptor_base) {
        Some(reason)
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
    vendor_replay_artifact: &Path,
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
    let vendor_replay_image = execution::ExecutableImage::load(vendor_replay_artifact)?;
    let mut rust_image = execution::ExecutableImage::load(rust_artifact)?;
    if let Some(companion) = rust_companion {
        rust_image.add_companion(companion)?;
    }
    let vendor_inventory_digest = inventory_symbol_sha256(vendor_inventory, Some(MEMBER), SYMBOL)?;
    let vendor_code_digest = code_closure_sha256(&vendor_image, SYMBOL)?;
    let vendor_replay_code_digest = code_closure_sha256(&vendor_replay_image, SYMBOL)?;
    let rust_code_digest = code_closure_sha256(&rust_image, rust_symbol)?;
    let descriptor_base = rust_image
        .symbol_address(RX_STORAGE_SYMBOL)
        .ok_or_else(|| {
            format!("compiled Rust probe has no retained {RX_STORAGE_SYMBOL} storage symbol")
        })?;
    let mut canonical = String::from("driver-adapter esp32s31-wdev-append-rx-blocks-v1\n");
    canonical.push_str(&format!(
        "vendor-inventory-symbol-sha256 {vendor_inventory_digest}\nvendor-linked-code-closure-sha256 {vendor_code_digest}\nvendor-replay-code-closure-sha256 {vendor_replay_code_digest}\nrust-code-closure-sha256 {rust_code_digest}\n"
    ));
    canonical.push_str(&vendor_proof);
    canonical.push_str("scope rx-ring-ownership-and-reload-transaction\n");
    canonical.push_str("fixture-prefix cold-ring-publication\n");
    canonical.push_str("platform-service embassy-rx-ring-ownership\n");
    canonical.push_str(
        "ordering-refinement ordered-cursor-before-descriptor-mutation-and-descriptor-publication-before-reload\n",
    );
    canonical.push_str(
        "settlement-refinement ordered-last-next-snapshot-before-terminal-frontier-repair\n",
    );
    canonical.push_str("omission vendor-diagnostics-and-statistics unused-instrumentation\n");

    let mut matched = true;
    let mut case_reports = Vec::with_capacity(CASES.len() + 1);
    let vendor_result = vendor_nonempty_append_case(&vendor_replay_image, svd)?;
    let wdev_control = vendor_replay_image
        .symbol_address("wDevCtrl")
        .expect("vendor replay precondition resolved wDevCtrl");
    let vendor_reason = vendor_contract_reason(&vendor_result, wdev_control);
    matched &= vendor_reason.is_none();
    canonical.push_str(&format!(
        "vendor-replay nonempty-append events={} calls={} branches={} matched={}\n",
        vendor_result.events.len(),
        vendor_result.ordered_calls.len(),
        vendor_result.ordered_branches.len(),
        vendor_reason.is_none(),
    ));
    case_reports.push(DriverAdapterCase {
        name: "vendor-nonempty-append".to_owned(),
        matched: vendor_reason.is_none(),
        reason: vendor_reason,
    });
    for case in CASES {
        let first = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            rust_scenario(*case, 0, descriptor_base),
            case.name,
            "Rust execution with stack-fill=0x00",
        )?;
        let second = super::execute_case(
            &rust_image,
            svd,
            rust_symbol,
            rust_scenario(*case, 0xa5, descriptor_base),
            case.name,
            "Rust execution with stack-fill=0xa5",
        )?;
        let padding_independent =
            first.events == second.events && first.return_value == second.return_value;
        let report = case_report(&first, &second, *case, descriptor_base);
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
    Ok(DriverAdapterVerification::from_trust(
        crate::driver_adapter_trust("esp32s31-wdev-append-rx-blocks-v1")
            .expect("registered adapter has a trust boundary"),
        matched,
        canonical,
    )
    .with_cases(case_reports))
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
    fn production_contract_rejects_wrong_publications_ownership_and_return() {
        let case = CASES[0];
        let descriptor_base = 0x2f00_1000;
        let ownership_address = 0x3fce_0000;
        let mut timeline = vec![
            execution::ExecutionTimelineEvent::RamWrite {
                site: 0,
                width: 32,
                address: descriptor_base + DESCRIPTOR_BYTES + 8,
                value: descriptor_base,
            },
            execution::ExecutionTimelineEvent::Observable(execution::ExecutionEvent::Write {
                width: 32,
                address: RX_CONTROL,
                region: "rx-dma".to_owned(),
                register: None,
                value: WALKER_ENABLE | RELOAD,
            }),
        ];
        timeline.extend((0..4).flat_map(|_| {
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
        }));
        let result = execution::ExecutionResult {
            events: vec![
                execution::ExecutionEvent::Write {
                    width: 32,
                    address: RX_HIGH,
                    region: "rx-dma".to_owned(),
                    register: None,
                    value: descriptor_base & 0xfff0_0000,
                },
                execution::ExecutionEvent::Write {
                    width: 32,
                    address: RX_BASE,
                    region: "rx-dma".to_owned(),
                    register: None,
                    value: descriptor_base,
                },
                execution::ExecutionEvent::Write {
                    width: 32,
                    address: RX_CONTROL,
                    region: "rx-dma".to_owned(),
                    register: None,
                    value: WALKER_ENABLE,
                },
                execution::ExecutionEvent::Write {
                    width: 32,
                    address: RX_CONTROL,
                    region: "rx-dma".to_owned(),
                    register: None,
                    value: WALKER_ENABLE | RELOAD,
                },
            ],
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
        assert!(case_report(&result, &result, case, descriptor_base).matched);

        let mut wrong_address = result.clone();
        if let execution::ExecutionEvent::Write { address, .. } = &mut wrong_address.events[1] {
            *address = RX_BASE + 4;
        }
        assert!(!case_report(&wrong_address, &wrong_address, case, descriptor_base).matched);
        let mut wrong_value = result.clone();
        if let execution::ExecutionEvent::Write { value, .. } = &mut wrong_value.events[3] {
            *value = WALKER_ENABLE;
        }
        assert!(!case_report(&wrong_value, &wrong_value, case, descriptor_base).matched);
        let mut wrong_ownership = result.clone();
        wrong_ownership.timeline.pop();
        assert!(!case_report(&wrong_ownership, &wrong_ownership, case, descriptor_base).matched);
        let mut wrong_return = result;
        wrong_return.return_value ^= 1;
        assert!(!case_report(&wrong_return, &wrong_return, case, descriptor_base).matched);
    }
}
